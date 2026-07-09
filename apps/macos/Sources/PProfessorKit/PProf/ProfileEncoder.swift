import Foundation

public struct ValueType: Sendable {
    public var type: UInt64
    public var unit: UInt64

    public init(type: UInt64, unit: UInt64) {
        self.type = type
        self.unit = unit
    }

    func encode(into buf: inout Data) {
        encodeVarintField(field: 1, value: type, into: &buf)
        encodeVarintField(field: 2, value: unit, into: &buf)
    }
}

public struct ProfLine: Sendable {
    public var functionID: UInt64
    public var line: Int64

    public init(functionID: UInt64, line: Int64) {
        self.functionID = functionID
        self.line = line
    }

    func encode(into buf: inout Data) {
        encodeVarintField(field: 1, value: functionID, into: &buf)
        encodeSInt64Field(field: 2, value: line, into: &buf)
    }
}

public struct ProfLocation: Sendable {
    public var id: UInt64
    public var lines: [ProfLine]

    public init(id: UInt64, lines: [ProfLine]) {
        self.id = id
        self.lines = lines
    }

    func encode(into buf: inout Data) {
        encodeVarintField(field: 1, value: id, into: &buf)
        for line in lines {
            var inner = Data()
            line.encode(into: &inner)
            encodeLengthDelimited(field: 4, data: inner, into: &buf)
        }
    }
}

public struct ProfFunction: Sendable {
    public var id: UInt64
    public var name: UInt64
    public var systemName: UInt64
    public var filename: UInt64
    public var startLine: Int64

    public init(id: UInt64, name: UInt64, systemName: UInt64, filename: UInt64, startLine: Int64) {
        self.id = id
        self.name = name
        self.systemName = systemName
        self.filename = filename
        self.startLine = startLine
    }

    func encode(into buf: inout Data) {
        encodeVarintField(field: 1, value: id, into: &buf)
        encodeVarintField(field: 2, value: name, into: &buf)
        encodeVarintField(field: 3, value: systemName, into: &buf)
        encodeVarintField(field: 4, value: filename, into: &buf)
        encodeSInt64Field(field: 5, value: startLine, into: &buf)
    }
}

public struct ProfLabel: Sendable {
    public var key: UInt64     // field 1, string table index
    public var str: UInt64     // field 2, string table index (string value)
    public var num: Int64      // field 3, numeric value
    public var numUnit: UInt64 // field 4, string table index (unit for num)

    public init(key: UInt64, str: UInt64 = 0, num: Int64 = 0, numUnit: UInt64 = 0) {
        self.key = key
        self.str = str
        self.num = num
        self.numUnit = numUnit
    }

    func encode(into buf: inout Data) {
        encodeVarintField(field: 1, value: key, into: &buf)
        encodeVarintField(field: 2, value: str, into: &buf)
        encodeSInt64Field(field: 3, value: num, into: &buf)
        encodeVarintField(field: 4, value: numUnit, into: &buf)
    }
}

public struct ProfSample: Sendable {
    public var locationIDs: [UInt64]
    public var values: [Int64]
    public var labels: [ProfLabel]

    public init(locationIDs: [UInt64], values: [Int64], labels: [ProfLabel] = []) {
        self.locationIDs = locationIDs
        self.values = values
        self.labels = labels
    }

    func encode(into buf: inout Data) {
        encodePackedUInt64(field: 1, values: locationIDs, into: &buf)
        encodePackedInt64(field: 2, values: values, into: &buf)
        for label in labels {
            var inner = Data()
            label.encode(into: &inner)
            encodeLengthDelimited(field: 3, data: inner, into: &buf)
        }
    }
}

public struct ProfileEncoder {
    public var strings: StringTable
    public var valueTypes: [ValueType]
    public var samples: [ProfSample]
    public var locations: [ProfLocation]
    public var functions: [ProfFunction]
    public var timeNanos: Int64
    public var durationNanos: Int64
    public var periodType: ValueType
    public var period: Int64

    public init() {
        strings = StringTable()
        valueTypes = []
        samples = []
        locations = []
        functions = []
        timeNanos = 0
        durationNanos = 0
        periodType = ValueType(type: 0, unit: 0)
        period = 0
    }

    public func encode() -> Data {
        var buf = Data()

        for vt in valueTypes {
            var inner = Data(); vt.encode(into: &inner)
            encodeLengthDelimited(field: 1, data: inner, into: &buf)
        }
        for s in samples {
            var inner = Data(); s.encode(into: &inner)
            encodeLengthDelimited(field: 2, data: inner, into: &buf)
        }
        for loc in locations {
            var inner = Data(); loc.encode(into: &inner)
            encodeLengthDelimited(field: 4, data: inner, into: &buf)
        }
        for f in functions {
            var inner = Data(); f.encode(into: &inner)
            encodeLengthDelimited(field: 5, data: inner, into: &buf)
        }
        for s in strings.strings {
            encodeStringField(field: 6, string: s, into: &buf)
        }
        encodeSInt64Field(field: 9, value: timeNanos, into: &buf)
        encodeSInt64Field(field: 10, value: durationNanos, into: &buf)
        var ptBuf = Data(); periodType.encode(into: &ptBuf)
        encodeLengthDelimited(field: 11, data: ptBuf, into: &buf)
        encodeSInt64Field(field: 12, value: period, into: &buf)

        return buf
    }
}
