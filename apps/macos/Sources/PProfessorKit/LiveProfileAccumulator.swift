import Foundation

public struct LiveProfileAccumulator: Sendable {
    public private(set) var profile: DecodedProfile

    private var stringIDs: [String: UInt64]
    private var mappingIDs: [MappingKey: UInt64]
    private var functionIDs: [FunctionKey: UInt64]
    private var locationIDs: [LocationKey: UInt64]
    private var sampleIndexes: [SampleKey: Int]

    public init() {
        var profile = DecodedProfile()
        profile.stringTable = [""]
        self.profile = profile
        self.stringIDs = ["": 0]
        self.mappingIDs = [:]
        self.functionIDs = [:]
        self.locationIDs = [:]
        self.sampleIndexes = [:]
    }

    public mutating func replace(with checkpoint: DecodedProfile) {
        self = LiveProfileAccumulator()
        merge(delta: checkpoint)
    }

    public mutating func merge(delta: DecodedProfile) {
        if profile.sampleTypes.isEmpty {
            profile.sampleTypes = delta.sampleTypes.map {
                ValueType(type: intern(delta.string(at: $0.type)), unit: intern(delta.string(at: $0.unit)))
            }
            if let periodType = delta.periodType {
                profile.periodType = ValueType(
                    type: intern(delta.string(at: periodType.type)),
                    unit: intern(delta.string(at: periodType.unit))
                )
            }
            profile.period = delta.period
            profile.timeNanos = delta.timeNanos
        }
        profile.durationNanos = max(profile.durationNanos, delta.durationNanos)

        var incomingMappingIDs: [UInt64: UInt64] = [:]
        for mapping in delta.mappings {
            let key = MappingKey(
                buildID: delta.string(at: mapping.buildID),
                filename: delta.string(at: mapping.filename),
                memoryStart: mapping.memoryStart,
                memoryLimit: mapping.memoryLimit,
                fileOffset: mapping.fileOffset
            )
            let globalID: UInt64
            if let existing = mappingIDs[key] {
                globalID = existing
            } else {
                globalID = UInt64(profile.mappings.count + 1)
                mappingIDs[key] = globalID
                profile.mappings.append(ProfMapping(
                    id: globalID,
                    memoryStart: mapping.memoryStart,
                    memoryLimit: mapping.memoryLimit,
                    fileOffset: mapping.fileOffset,
                    filename: intern(key.filename),
                    buildID: intern(key.buildID)
                ))
            }
            incomingMappingIDs[mapping.id] = globalID
        }

        let incomingFunctions = Dictionary(delta.functions.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        var incomingLocationIDs: [UInt64: UInt64] = [:]
        for location in delta.locations {
            let globalMappingID = incomingMappingIDs[location.mappingID] ?? 0
            var globalLines: [ProfLine] = []
            var lineKeys: [LineKey] = []
            for line in location.lines {
                guard let function = incomingFunctions[line.functionID] else { continue }
                let functionKey = FunctionKey(
                    name: delta.string(at: function.name),
                    systemName: delta.string(at: function.systemName),
                    filename: delta.string(at: function.filename),
                    startLine: function.startLine
                )
                let globalFunctionID: UInt64
                if let existing = functionIDs[functionKey] {
                    globalFunctionID = existing
                } else {
                    globalFunctionID = UInt64(profile.functions.count + 1)
                    functionIDs[functionKey] = globalFunctionID
                    profile.functions.append(ProfFunction(
                        id: globalFunctionID,
                        name: intern(functionKey.name),
                        systemName: intern(functionKey.systemName),
                        filename: intern(functionKey.filename),
                        startLine: functionKey.startLine
                    ))
                }
                globalLines.append(ProfLine(functionID: globalFunctionID, line: line.line))
                lineKeys.append(LineKey(functionID: globalFunctionID, line: line.line))
            }

            let key = LocationKey(mappingID: globalMappingID, address: location.address, lines: lineKeys)
            let globalLocationID: UInt64
            if let existing = locationIDs[key] {
                globalLocationID = existing
            } else {
                globalLocationID = UInt64(profile.locations.count + 1)
                locationIDs[key] = globalLocationID
                profile.locations.append(ProfLocation(
                    id: globalLocationID,
                    mappingID: globalMappingID,
                    address: location.address,
                    lines: globalLines
                ))
            }
            incomingLocationIDs[location.id] = globalLocationID
        }

        for sample in delta.samples {
            let stack = sample.locationIDs.compactMap { incomingLocationIDs[$0] }
            guard !stack.isEmpty else { continue }
            let labels = sample.labels.map { label in
                ProfLabel(
                    key: intern(delta.string(at: label.key)),
                    str: label.str == 0 ? 0 : intern(delta.string(at: label.str)),
                    num: label.num,
                    numUnit: label.numUnit == 0 ? 0 : intern(delta.string(at: label.numUnit))
                )
            }
            let labelKeys = labels.map { LabelKey(key: $0.key, str: $0.str, num: $0.num, numUnit: $0.numUnit) }
            let key = SampleKey(stack: stack, labels: labelKeys)
            if let index = sampleIndexes[key] {
                if profile.samples[index].values.count < sample.values.count {
                    profile.samples[index].values.append(contentsOf: repeatElement(0, count: sample.values.count - profile.samples[index].values.count))
                }
                for valueIndex in sample.values.indices {
                    profile.samples[index].values[valueIndex] += sample.values[valueIndex]
                }
            } else {
                sampleIndexes[key] = profile.samples.count
                profile.samples.append(ProfSample(locationIDs: stack, values: sample.values, labels: labels))
            }
        }
    }

    private mutating func intern(_ value: String) -> UInt64 {
        if let existing = stringIDs[value] { return existing }
        let id = UInt64(profile.stringTable.count)
        profile.stringTable.append(value)
        stringIDs[value] = id
        return id
    }
}

private struct MappingKey: Hashable, Sendable {
    let buildID: String
    let filename: String
    let memoryStart: UInt64
    let memoryLimit: UInt64
    let fileOffset: UInt64
}

private struct FunctionKey: Hashable, Sendable {
    let name: String
    let systemName: String
    let filename: String
    let startLine: Int64
}

private struct LineKey: Hashable, Sendable {
    let functionID: UInt64
    let line: Int64
}

private struct LocationKey: Hashable, Sendable {
    let mappingID: UInt64
    let address: UInt64
    let lines: [LineKey]
}

private struct LabelKey: Hashable, Sendable {
    let key: UInt64
    let str: UInt64
    let num: Int64
    let numUnit: UInt64
}

private struct SampleKey: Hashable, Sendable {
    let stack: [UInt64]
    let labels: [LabelKey]
}
