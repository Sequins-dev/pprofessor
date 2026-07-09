import Testing
import Foundation
@testable import PProfessorKit

@Suite("ProfileDecoder")
struct ProfileDecoderTests {

    @Test func decodeEmptyData() {
        let profile = DecodedProfile.decode(from: Data())
        #expect(profile.samples.isEmpty)
        #expect(profile.functions.isEmpty)
        #expect(profile.locations.isEmpty)
        #expect(profile.stringTable.isEmpty)
    }

    @Test func roundTrip() throws {
        // Build a profile with the existing encoder
        var encoder = ProfileEncoder()

        let cpuIdx = encoder.strings.intern("cpu")
        let nsIdx = encoder.strings.intern("nanoseconds")
        let funcName = encoder.strings.intern("myFunction")
        let sysName = encoder.strings.intern("_myFunction")
        let filename = encoder.strings.intern("main.swift")

        encoder.valueTypes = [ValueType(type: cpuIdx, unit: nsIdx)]
        encoder.periodType = ValueType(type: cpuIdx, unit: nsIdx)
        encoder.period = 1_000_000
        encoder.timeNanos = 1_700_000_000_000_000_000
        encoder.durationNanos = 5_000_000_000

        encoder.functions = [
            ProfFunction(id: 1, name: funcName, systemName: sysName, filename: filename, startLine: 10)
        ]
        encoder.locations = [
            ProfLocation(id: 1, lines: [ProfLine(functionID: 1, line: 42)])
        ]
        encoder.samples = [
            ProfSample(locationIDs: [1], values: [1_000_000])
        ]

        let encoded = encoder.encode()
        let decoded = DecodedProfile.decode(from: encoded)

        // String table
        #expect(decoded.stringTable[0] == "")  // always empty at index 0
        #expect(decoded.string(at: cpuIdx) == "cpu")
        #expect(decoded.string(at: nsIdx) == "nanoseconds")
        #expect(decoded.string(at: funcName) == "myFunction")

        // Value types
        #expect(decoded.sampleTypes.count == 1)
        #expect(decoded.sampleTypes[0].type == cpuIdx)
        #expect(decoded.sampleTypes[0].unit == nsIdx)

        // Period
        #expect(decoded.period == 1_000_000)
        #expect(decoded.periodType?.type == cpuIdx)

        // Timestamps
        #expect(decoded.timeNanos == 1_700_000_000_000_000_000)
        #expect(decoded.durationNanos == 5_000_000_000)

        // Functions
        #expect(decoded.functions.count == 1)
        #expect(decoded.functions[0].id == 1)
        #expect(decoded.functions[0].name == funcName)
        #expect(decoded.functions[0].filename == filename)
        #expect(decoded.functions[0].startLine == 10)

        // Locations
        #expect(decoded.locations.count == 1)
        #expect(decoded.locations[0].id == 1)
        #expect(decoded.locations[0].lines.count == 1)
        #expect(decoded.locations[0].lines[0].functionID == 1)
        #expect(decoded.locations[0].lines[0].line == 42)

        // Samples
        #expect(decoded.samples.count == 1)
        #expect(decoded.samples[0].locationIDs == [1])
        #expect(decoded.samples[0].values == [1_000_000])
    }

    @Test func roundTripLabels() {
        var encoder = ProfileEncoder()
        let cpuIdx = encoder.strings.intern("cpu")
        let nsIdx = encoder.strings.intern("ns")
        let threadKey = encoder.strings.intern("thread")
        let threadName = encoder.strings.intern("main")

        encoder.valueTypes = [ValueType(type: cpuIdx, unit: nsIdx)]
        encoder.functions = [ProfFunction(id: 1, name: cpuIdx, systemName: 0, filename: 0, startLine: 0)]
        encoder.locations = [ProfLocation(id: 1, lines: [ProfLine(functionID: 1, line: 0)])]
        encoder.samples = [
            ProfSample(
                locationIDs: [1],
                values: [100],
                labels: [ProfLabel(key: threadKey, str: threadName)]
            )
        ]

        let decoded = DecodedProfile.decode(from: encoder.encode())
        #expect(decoded.samples.count == 1)
        #expect(decoded.samples[0].labels.count == 1)
        #expect(decoded.samples[0].labels[0].key == threadKey)
        #expect(decoded.samples[0].labels[0].str == threadName)
    }

    @Test func roundTripMultipleSamples() {
        var encoder = ProfileEncoder()
        let cpuIdx = encoder.strings.intern("cpu")
        let nsIdx = encoder.strings.intern("ns")
        let f1 = encoder.strings.intern("funcA")
        let f2 = encoder.strings.intern("funcB")

        encoder.valueTypes = [ValueType(type: cpuIdx, unit: nsIdx)]
        encoder.functions = [
            ProfFunction(id: 1, name: f1, systemName: 0, filename: 0, startLine: 0),
            ProfFunction(id: 2, name: f2, systemName: 0, filename: 0, startLine: 0),
        ]
        encoder.locations = [
            ProfLocation(id: 1, lines: [ProfLine(functionID: 1, line: 1)]),
            ProfLocation(id: 2, lines: [ProfLine(functionID: 2, line: 2)]),
        ]
        encoder.samples = [
            ProfSample(locationIDs: [1], values: [100]),
            ProfSample(locationIDs: [2, 1], values: [200]),
        ]

        let decoded = DecodedProfile.decode(from: encoder.encode())
        #expect(decoded.samples.count == 2)
        #expect(decoded.samples[0].values == [100])
        #expect(decoded.samples[1].values == [200])
        #expect(decoded.samples[1].locationIDs == [2, 1])
    }
}
