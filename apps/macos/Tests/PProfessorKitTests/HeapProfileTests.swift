import Foundation
import Testing
@testable import PProfessorKit

@Suite("Heap profiles")
struct HeapProfileTests {
    @Test func defaultMetricAndMetadataSurviveExportAndCheckpointReplacement() {
        var encoder = ProfileEncoder()
        let count = encoder.strings.intern("count")
        let bytes = encoder.strings.intern("bytes")
        let objects = encoder.strings.intern("alloc_objects")
        let live = encoder.strings.intern("inuse_space")
        encoder.valueTypes = [ValueType(type: objects, unit: count), ValueType(type: live, unit: bytes)]
        encoder.defaultSampleType = live
        encoder.comments = [encoder.strings.intern("Exact native allocation history")]
        encoder.samples = [ProfSample(locationIDs: [], values: [10, 1_048_576])]
        let profile = DecodedProfile.decode(from: encoder.encode())
        #expect(profile.preferredValueTypeIndex == 1)
        var accumulator = LiveProfileAccumulator()
        accumulator.replace(with: profile)
        accumulator.replace(with: profile)
        #expect(accumulator.profile.samples[0].values == [10, 1_048_576])
        #expect(accumulator.profile.preferredValueTypeIndex == 1)
        let exported = DecodedProfile.decode(from: accumulator.profile.encoded())
        #expect(exported.string(at: exported.defaultSampleType) == "inuse_space")
        #expect(exported.comments.map { exported.string(at: $0) } == ["Exact native allocation history"])
    }
    @Test func metricsFormatByUnitRatherThanMagnitude() {
        #expect(formatProfileValue(1_048_576, unit: "bytes") == "1.00 MiB")
        #expect(formatProfileValue(2_000_000, unit: "count") == "2000000")
        #expect(formatProfileValue(2_000_000, unit: "nanoseconds") == "2.00 ms")
        #expect(formatProfileValue(-1_048_576, unit: "bytes") == "-1.00 MiB")
    }
    @Test func defaultWithoutExplicitPreferenceIsLastMetric() {
        var p = DecodedProfile()
        p.sampleTypes = [ValueType(type: 0, unit: 0), ValueType(type: 0, unit: 0)]
        #expect(p.preferredValueTypeIndex == 1)
    }
    @Test func negativeDifferencesRemainVisibleInDecreaseView() {
        var e = ProfileEncoder()
        e.valueTypes = [ValueType(type: e.strings.intern("inuse_space"), unit: e.strings.intern("bytes"))]
        e.functions = [ProfFunction(id: 1, name: e.strings.intern("released"), systemName: 0, filename: 0, startLine: 0)]
        e.locations = [ProfLocation(id: 1, lines: [ProfLine(functionID: 1, line: 0)])]
        e.samples = [ProfSample(locationIDs: [1], values: [-64])]
        let p = DecodedProfile.decode(from: e.encode())
        let view = convertProfile(p, valueSign: .negative)
        #expect(view.totalValue == 64)
        #expect(view.nodes.contains { $0.functionName == "released" })
        #expect(DecodedProfile.decode(from: p.encoded()).samples[0].values == [-64])
    }
}

@Suite("Heap domains")
struct HeapDomainTests {
    @Test func overlappingDomainsAreNeverSummedByDefault() {
        var e = ProfileEncoder()
        e.valueTypes = [ValueType(type: e.strings.intern("inuse_space"), unit: e.strings.intern("bytes"))]
        e.functions = [ProfFunction(id: 1, name: e.strings.intern("allocate"), systemName: 0, filename: 0, startLine: 0)]
        e.locations = [ProfLocation(id: 1, lines: [ProfLine(functionID: 1, line: 0)])]
        let heap = e.strings.intern("heap")
        e.samples = [ProfSample(locationIDs: [1], values: [64], labels: [ProfLabel(key: heap, str: e.strings.intern("native"))]), ProfSample(locationIDs: [1], values: [40], labels: [ProfLabel(key: heap, str: e.strings.intern("v8"))])]
        let p = DecodedProfile.decode(from: e.encode())
        #expect(convertProfile(p).totalValue == 64)
        #expect(convertProfile(p, heapDomain: "v8").totalValue == 40)
    }
}

@Suite("Heap pprof export")
struct HeapExportTests {
    @Test func standardSourceMetadataSurvivesCheckpointExport() {
        var e = ProfileEncoder()
        let live = e.strings.intern("inuse_space")
        e.valueTypes = [ValueType(type: live, unit: e.strings.intern("bytes"))]
        e.functions = [ProfFunction(id: 1, name: e.strings.intern("allocate"), systemName: 0, filename: 0, startLine: 1)]
        var line = ProfLine(functionID: 1, line: 12)
        line.column = 7
        var location = ProfLocation(id: 1, mappingID: 1, address: 0x100, lines: [line])
        location.isFolded = true
        e.locations = [location]
        var mapping = ProfMapping(id: 1, memoryStart: 0x100, memoryLimit: 0x200, fileOffset: 0, filename: e.strings.intern("app"), buildID: e.strings.intern("build-1"))
        mapping.hasFunctions = true
        mapping.hasFilenames = true
        mapping.hasLineNumbers = true
        mapping.hasInlineFrames = true
        e.mappings = [mapping]
        e.samples = [ProfSample(locationIDs: [1], values: [64])]
        var accumulator = LiveProfileAccumulator()
        accumulator.replace(with: DecodedProfile.decode(from: e.encode()))
        let p = DecodedProfile.decode(from: accumulator.profile.encoded())
        #expect(p.locations[0].isFolded)
        #expect(p.locations[0].lines[0].column == 7)
        #expect(p.mappings[0].hasFunctions)
        #expect(p.mappings[0].hasFilenames)
        #expect(p.mappings[0].hasLineNumbers)
        #expect(p.mappings[0].hasInlineFrames)
    }
}
