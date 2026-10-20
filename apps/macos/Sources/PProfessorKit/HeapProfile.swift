import Foundation

public enum ProfileValueSign: String, CaseIterable, Sendable, Hashable {
    case positive = "Increases"
    case negative = "Decreases"
}

extension DecodedProfile {
    public var preferredValueTypeIndex: Int {
        if defaultSampleType != 0,
           let index = sampleTypes.firstIndex(where: { string(at: $0.type) == string(at: defaultSampleType) }) {
            return index
        }
        return max(0, sampleTypes.count - 1)
    }
    public var isHeapProfile: Bool {
        sampleTypes.contains { ["alloc_space", "inuse_space", "alloc_objects", "inuse_objects"].contains(string(at: $0.type)) }
    }
    public var hasNegativeValues: Bool { samples.contains { $0.values.contains { $0 < 0 } } }
    public func heapDomain(for sample: ProfSample) -> String? {
        sample.labels.first(where: { string(at: $0.key) == "heap" }).map { string(at: $0.str) } ?? (isHeapProfile ? "[unspecified heap]" : nil)
    }
    public var heapDomains: [String] { Array(Set(samples.compactMap { heapDomain(for: $0) })).sorted() }

    /// Standard pprof protobuf; no application-only metadata or history envelope.
    public func encoded() -> Data {
        var encoder = ProfileEncoder()
        encoder.strings = StringTable(strings: stringTable)
        encoder.valueTypes = sampleTypes
        encoder.samples = samples
        encoder.locations = locations
        encoder.functions = functions
        encoder.mappings = mappings
        encoder.timeNanos = timeNanos
        encoder.durationNanos = durationNanos
        encoder.periodType = periodType ?? ValueType(type: 0, unit: 0)
        encoder.period = period
        encoder.defaultSampleType = defaultSampleType
        encoder.comments = comments
        encoder.dropFrames = dropFrames
        encoder.keepFrames = keepFrames
        encoder.docURL = docURL
        return encoder.encode()
    }
}

public func formatProfileValue(_ value: Int64, unit: String) -> String {
    let magnitude = abs(Double(value))
    let scales: [(Double, String)]
    switch unit {
    case "bytes", "byte": scales = [(1_073_741_824, "GiB"), (1_048_576, "MiB"), (1024, "KiB"), (1, "B")]
    case "nanoseconds", "ns": scales = [(1_000_000_000, "s"), (1_000_000, "ms"), (1000, "µs"), (1, "ns")]
    default: return unit == "count" || unit.isEmpty ? String(value) : "\(value) \(unit)"
    }
    let scale = scales.first { magnitude >= $0.0 } ?? scales.last!
    return scale.0 == 1 ? "\(value) \(scale.1)" : String(format: "%.2f %@", Double(value) / scale.0, scale.1)
}
