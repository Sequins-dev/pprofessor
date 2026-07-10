import Foundation

public let pprofessorTimestampLabel = "pprofessor::timestamp"

public struct ProfileTimeline: Sendable, Equatable {
    public let durationNanos: Int64
    public let buckets: [Double]

    public static func build(
        from profile: DecodedProfile,
        valueTypeIndex: Int,
        threadFilter: String?,
        bucketCount: Int
    ) -> ProfileTimeline? {
        guard !profile.samples.isEmpty, bucketCount > 0 else { return nil }

        let timedSamples: [(sample: ProfSample, timestamp: Int64)] = profile.samples.compactMap { sample in
            profileTimestampNanos(profile: profile, sample: sample).map { (sample, $0) }
        }
        guard timedSamples.count == profile.samples.count else { return nil }

        let lastTimestamp = timedSamples.map(\.timestamp).max() ?? 0
        let duration = max(1, profile.durationNanos, lastTimestamp + max(1, profile.period))
        let eligibleSamples = timedSamples.compactMap { sample, timestamp -> (ProfSample, Int64, Double)? in
            guard valueTypeIndex >= 0, valueTypeIndex < sample.values.count else { return nil }
            if let threadFilter,
               profileThreadLabel(profile: profile, sample: sample) != threadFilter {
                return nil
            }
            let value = Double(sample.values[valueTypeIndex])
            guard value > 0 else { return nil }
            return (sample, timestamp, value)
        }
        let effectiveBucketCount = min(bucketCount, max(16, eligibleSamples.count / 3))
        var stackWeights: [[UInt64]: Double] = [:]
        var totalWeight = 0.0
        for (sample, _, value) in eligibleSamples {
            stackWeights[sample.locationIDs, default: 0] += value
            totalWeight += value
        }

        var rawBuckets = [Double](repeating: 0, count: effectiveBucketCount)

        for (sample, timestamp, value) in eligibleSamples {
            let clampedTimestamp = min(max(0, timestamp), duration - 1)
            let index = min(
                effectiveBucketCount - 1,
                Int(clampedTimestamp * Int64(effectiveBucketCount) / duration)
            )
            let stackWeight = stackWeights[sample.locationIDs] ?? value
            let surprise = totalWeight > 0 ? log2(totalWeight / stackWeight) : 0
            rawBuckets[index] += value * (1 + surprise)
        }

        let nonzero = rawBuckets.filter { $0 > 0 }
        let maximum = nonzero.max() ?? 0
        let minimum = nonzero.min() ?? 0
        let buckets: [Double]
        if maximum > 0, maximum - minimum > 0.000_001 {
            buckets = rawBuckets.map { $0 / maximum }
        } else {
            buckets = rawBuckets.map { $0 > 0 ? 0.25 : 0 }
        }
        return ProfileTimeline(durationNanos: duration, buckets: buckets)
    }
}

public struct TimelineSelectionState: Sendable, Equatable {
    public private(set) var range: ClosedRange<Int64>
    public private(set) var isFollowingLive: Bool

    public init(durationNanos: Int64) {
        let duration = max(1, durationNanos)
        range = 0...duration
        isFollowingLive = true
    }

    public mutating func updateDuration(_ durationNanos: Int64) {
        let duration = max(1, durationNanos)
        if isFollowingLive {
            range = 0...duration
        } else {
            let lower = min(max(0, range.lowerBound), duration)
            let upper = min(max(lower, range.upperBound), duration)
            range = lower...upper
        }
    }

    public mutating func select(_ newRange: ClosedRange<Int64>) {
        range = max(0, newRange.lowerBound)...max(newRange.lowerBound, newRange.upperBound)
        isFollowingLive = false
    }

    public mutating func reset(durationNanos: Int64) {
        self = TimelineSelectionState(durationNanos: durationNanos)
    }
}

func profileTimestampNanos(profile: DecodedProfile, sample: ProfSample) -> Int64? {
    for label in sample.labels where profile.string(at: label.key) == pprofessorTimestampLabel {
        let unit = label.numUnit == 0 ? "" : profile.string(at: label.numUnit)
        guard unit.isEmpty || unit == "nanoseconds" else { return nil }
        return label.num
    }
    return nil
}

func profileThreadLabel(profile: DecodedProfile, sample: ProfSample) -> String? {
    let threadKeys: Set<String> = [
        "thread", "tid", "thread_id", "thread_name", "threadName", "thread.id", "thread.name",
    ]
    for label in sample.labels {
        guard threadKeys.contains(profile.string(at: label.key)) else { continue }
        if label.str != 0 { return profile.string(at: label.str) }
        if label.num != 0 { return "\(label.num)" }
    }
    return nil
}
