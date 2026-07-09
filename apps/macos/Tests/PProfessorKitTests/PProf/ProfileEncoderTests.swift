import Testing
@testable import PProfessorKit

@Suite("ProfileEncoder")
struct ProfileEncoderTests {
    @Test func encodeNonEmpty() {
        var enc = ProfileEncoder()
        let sSamples = enc.strings.intern("samples")
        let sCount = enc.strings.intern("count")
        enc.valueTypes.append(ValueType(type: sSamples, unit: sCount))
        enc.periodType = ValueType(type: sSamples, unit: sCount)
        enc.period = 10_000_000
        enc.timeNanos = 1_000_000_000
        enc.durationNanos = 2_000_000_000
        let data = enc.encode()
        #expect(!data.isEmpty)
        #expect(data[0] == 0x0a)
    }
}
