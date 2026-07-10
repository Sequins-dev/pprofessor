import Testing
@testable import PProfessorKit

@Suite("Profile timeline")
struct ProfileTimelineTests {
    private func profile(timestamps: [Int64?], values: [Int64]) -> DecodedProfile {
        var profile = DecodedProfile()
        profile.stringTable = [
            "", "samples", "count", "pprofessor::timestamp", "nanoseconds",
            "work",
        ]
        profile.sampleTypes = [ValueType(type: 1, unit: 2)]
        profile.durationNanos = 30
        profile.functions = [
            ProfFunction(id: 1, name: 5, systemName: 0, filename: 0, startLine: 0),
        ]
        profile.locations = [ProfLocation(id: 1, lines: [ProfLine(functionID: 1, line: 0)])]
        profile.samples = zip(timestamps, values).map { timestamp, value in
            let labels = timestamp.map {
                [ProfLabel(key: 3, num: $0, numUnit: 4)]
            } ?? []
            return ProfSample(locationIDs: [1], values: [value], labels: labels)
        }
        return profile
    }

    @Test func completeTimedProfileBuildsNormalizedActivityBuckets() {
        let timeline = ProfileTimeline.build(
            from: profile(timestamps: [0, 10, 20], values: [1, 2, 3]),
            valueTypeIndex: 0,
            threadFilter: nil,
            bucketCount: 3
        )

        #expect(timeline?.durationNanos == 30)
        #expect(abs((timeline?.buckets[0] ?? 0) - (1.0 / 3.0)) < 0.001)
        #expect(abs((timeline?.buckets[1] ?? 0) - (2.0 / 3.0)) < 0.001)
        #expect(timeline?.buckets[2] == 1)
    }

    @Test func timelineIsUnavailableWhenAnySampleIsUntimed() {
        let timeline = ProfileTimeline.build(
            from: profile(timestamps: [0, nil], values: [1, 2]),
            valueTypeIndex: 0,
            threadFilter: nil,
            bucketCount: 10
        )

        #expect(timeline == nil)
    }

    @Test func rareStackScoresHigherThanCommonSteadyState() {
        var input = profile(
            timestamps: [0, 6, 12, 18, 24],
            values: [1, 1, 1, 1, 1]
        )
        input.stringTable.append("rare")
        input.functions.append(
            ProfFunction(id: 2, name: 6, systemName: 0, filename: 0, startLine: 0)
        )
        input.locations.append(
            ProfLocation(id: 2, lines: [ProfLine(functionID: 2, line: 0)])
        )
        input.samples[4].locationIDs = [2]

        let timeline = ProfileTimeline.build(
            from: input,
            valueTypeIndex: 0,
            threadFilter: nil,
            bucketCount: 5
        )

        #expect(timeline != nil)
        #expect(timeline!.buckets[4] > timeline!.buckets[0])
    }

    @Test func converterFiltersSamplesToSelectedTimeRange() {
        let result = convertProfile(
            profile(timestamps: [0, 10, 20], values: [1, 2, 3]),
            timeRangeNanos: 0...15
        )

        #expect(result.totalValue == 3)
    }

    @Test func liveSelectionFollowsUntilUserEditsAndResetRestoresFollowing() {
        var selection = TimelineSelectionState(durationNanos: 100)
        selection.updateDuration(200)
        #expect(selection.range == 0...200)
        #expect(selection.isFollowingLive)

        selection.select(40...80)
        selection.updateDuration(300)
        #expect(selection.range == 40...80)
        #expect(!selection.isFollowingLive)

        selection.reset(durationNanos: 300)
        #expect(selection.range == 0...300)
        #expect(selection.isFollowingLive)
    }
}
