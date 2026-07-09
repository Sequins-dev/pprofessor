import Foundation

// MARK: - DecodedProfile

/// A fully decoded pprof profile.
public struct DecodedProfile: Sendable {
    public var sampleTypes: [ValueType] = []
    public var samples: [ProfSample] = []
    public var locations: [ProfLocation] = []
    public var functions: [ProfFunction] = []
    public var stringTable: [String] = []
    public var timeNanos: Int64 = 0
    public var durationNanos: Int64 = 0
    public var periodType: ValueType?
    public var period: Int64 = 0

    public init() {}

    /// Decode a raw (uncompressed) pprof protobuf buffer.
    public static func decode(from data: Data) -> DecodedProfile {
        var profile = DecodedProfile()
        decodeProtobuf(from: data) { field, wireType, value in
            switch field {
            case 1 where wireType == .lengthDelimited:
                profile.sampleTypes.append(ValueType.decode(from: value))
            case 2 where wireType == .lengthDelimited:
                profile.samples.append(ProfSample.decode(from: value))
            case 4 where wireType == .lengthDelimited:
                profile.locations.append(ProfLocation.decode(from: value))
            case 5 where wireType == .lengthDelimited:
                profile.functions.append(ProfFunction.decode(from: value))
            case 6 where wireType == .lengthDelimited:
                profile.stringTable.append(String(data: value, encoding: .utf8) ?? "")
            case 9 where wireType == .varint:
                let (v, _) = decodeVarint(from: value, at: 0)
                profile.timeNanos = Int64(bitPattern: v)
            case 10 where wireType == .varint:
                let (v, _) = decodeVarint(from: value, at: 0)
                profile.durationNanos = Int64(bitPattern: v)
            case 11 where wireType == .lengthDelimited:
                profile.periodType = ValueType.decode(from: value)
            case 12 where wireType == .varint:
                let (v, _) = decodeVarint(from: value, at: 0)
                profile.period = Int64(bitPattern: v)
            default:
                break
            }
        }
        return profile
    }

    /// Resolve a string table index to a string, returning "" for out-of-range indices.
    public func string(at index: UInt64) -> String {
        let i = Int(index)
        guard i < stringTable.count else { return "" }
        return stringTable[i]
    }
}

// MARK: - Decode extensions on existing model structs

extension ValueType {
    static func decode(from data: Data) -> ValueType {
        var type_: UInt64 = 0
        var unit: UInt64 = 0
        decodeProtobuf(from: data) { field, wireType, value in
            guard wireType == .varint else { return }
            let (v, _) = decodeVarint(from: value, at: 0)
            switch field {
            case 1: type_ = v
            case 2: unit = v
            default: break
            }
        }
        return ValueType(type: type_, unit: unit)
    }
}

extension ProfLine {
    static func decode(from data: Data) -> ProfLine {
        var functionID: UInt64 = 0
        var line: Int64 = 0
        decodeProtobuf(from: data) { field, wireType, value in
            guard wireType == .varint else { return }
            let (v, _) = decodeVarint(from: value, at: 0)
            switch field {
            case 1: functionID = v
            case 2: line = Int64(bitPattern: v)
            default: break
            }
        }
        return ProfLine(functionID: functionID, line: line)
    }
}

extension ProfLocation {
    static func decode(from data: Data) -> ProfLocation {
        var id: UInt64 = 0
        var lines: [ProfLine] = []
        decodeProtobuf(from: data) { field, wireType, value in
            switch field {
            case 1 where wireType == .varint:
                let (v, _) = decodeVarint(from: value, at: 0)
                id = v
            case 4 where wireType == .lengthDelimited:
                lines.append(ProfLine.decode(from: value))
            default:
                break
            }
        }
        return ProfLocation(id: id, lines: lines)
    }
}

extension ProfFunction {
    static func decode(from data: Data) -> ProfFunction {
        var id: UInt64 = 0
        var name: UInt64 = 0
        var systemName: UInt64 = 0
        var filename: UInt64 = 0
        var startLine: Int64 = 0
        decodeProtobuf(from: data) { field, wireType, value in
            guard wireType == .varint else { return }
            let (v, _) = decodeVarint(from: value, at: 0)
            switch field {
            case 1: id = v
            case 2: name = v
            case 3: systemName = v
            case 4: filename = v
            case 5: startLine = Int64(bitPattern: v)
            default: break
            }
        }
        return ProfFunction(id: id, name: name, systemName: systemName, filename: filename, startLine: startLine)
    }
}

extension ProfLabel {
    static func decode(from data: Data) -> ProfLabel {
        var key: UInt64 = 0
        var str: UInt64 = 0
        var num: Int64 = 0
        var numUnit: UInt64 = 0
        decodeProtobuf(from: data) { field, wireType, value in
            guard wireType == .varint else { return }
            let (v, _) = decodeVarint(from: value, at: 0)
            switch field {
            case 1: key = v
            case 2: str = v
            case 3: num = Int64(bitPattern: v)
            case 4: numUnit = v
            default: break
            }
        }
        return ProfLabel(key: key, str: str, num: num, numUnit: numUnit)
    }
}

extension ProfSample {
    static func decode(from data: Data) -> ProfSample {
        var locationIDs: [UInt64] = []
        var values: [Int64] = []
        var labels: [ProfLabel] = []
        decodeProtobuf(from: data) { field, wireType, value in
            guard wireType == .lengthDelimited else { return }
            switch field {
            case 1: locationIDs = decodePackedUInt64s(from: value)
            case 2: values = decodePackedInt64s(from: value)
            case 3: labels.append(ProfLabel.decode(from: value))
            default: break
            }
        }
        return ProfSample(locationIDs: locationIDs, values: values, labels: labels)
    }
}
