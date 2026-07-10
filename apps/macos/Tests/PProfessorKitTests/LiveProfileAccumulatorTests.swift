import Foundation
import Testing
@testable import PProfessorKit

@Suite("Live profile accumulator")
struct LiveProfileAccumulatorTests {
    private func profile(function: String, address: UInt64, count: Int64, prefix: [String] = []) -> DecodedProfile {
        var encoder = ProfileEncoder()
        _ = encoder.strings.intern(contentsOf: prefix)
        let samples = encoder.strings.intern("samples")
        let countUnit = encoder.strings.intern("count")
        let name = encoder.strings.intern(function)
        encoder.valueTypes = [ValueType(type: samples, unit: countUnit)]
        encoder.functions = [ProfFunction(id: 1, name: name, systemName: name, filename: 0, startLine: 0)]
        encoder.locations = [ProfLocation(id: 1, mappingID: 0, address: address, lines: [ProfLine(functionID: 1, line: 0)])]
        encoder.samples = [ProfSample(locationIDs: [1], values: [count])]
        encoder.durationNanos = 500_000_000
        return DecodedProfile.decode(from: encoder.encode())
    }

    @Test func mergesSameAddressAcrossDifferentStringTables() {
        var accumulator = LiveProfileAccumulator()
        accumulator.merge(delta: profile(function: "hot", address: 0x1234, count: 3))
        accumulator.merge(delta: profile(function: "hot", address: 0x1234, count: 4, prefix: ["unrelated", "strings"]))

        let result = accumulator.profile
        #expect(result.locations.count == 1)
        #expect(result.functions.count == 1)
        #expect(result.samples.count == 1)
        #expect(result.samples[0].values == [7])
    }

    @Test func keepsSameNamedFunctionsAtDifferentAddressesDistinct() {
        var accumulator = LiveProfileAccumulator()
        accumulator.merge(delta: profile(function: "hot", address: 0x1234, count: 3))
        accumulator.merge(delta: profile(function: "hot", address: 0x5678, count: 4))

        #expect(accumulator.profile.locations.count == 2)
        #expect(accumulator.profile.samples.count == 2)
    }

    @Test func checkpointReplacesPriorDeltas() {
        var accumulator = LiveProfileAccumulator()
        accumulator.merge(delta: profile(function: "old", address: 1, count: 2))
        accumulator.replace(with: profile(function: "new", address: 2, count: 9))

        let result = accumulator.profile
        #expect(result.samples.count == 1)
        #expect(result.samples[0].values == [9])
        let function = result.functions[0]
        #expect(result.string(at: function.name) == "new")
    }
}

private extension StringTable {
    mutating func intern(contentsOf values: [String]) -> [UInt64] {
        values.map { intern($0) }
    }
}
