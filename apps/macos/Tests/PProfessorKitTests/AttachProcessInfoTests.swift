import Foundation
import Testing
@testable import PProfessorKit

@Suite("Attach process metadata")
struct AttachProcessInfoTests {
    @Test func decodesProtectedProcessMetadata() throws {
        let data = Data(#"{"pid":42,"parent_pid":1,"uid":501,"name":"ReleaseApp","executable_path":"/Applications/ReleaseApp.app/Contents/MacOS/ReleaseApp","start_time_micros":123,"architecture":"arm64","attachable":false,"attachability_reason":"Protected by macOS"}"#.utf8)
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase

        let process = try decoder.decode(AttachProcessInfo.self, from: data)

        #expect(!process.canAttach)
        #expect(process.attachabilityReason == "Protected by macOS")
    }

    @Test func olderHelperResponseRemainsAttachable() throws {
        let data = Data(#"{"pid":42,"parent_pid":1,"uid":501,"name":"DebugApp","executable_path":null,"start_time_micros":123,"architecture":"arm64"}"#.utf8)
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase

        let process = try decoder.decode(AttachProcessInfo.self, from: data)

        #expect(process.canAttach)
        #expect(process.attachabilityReason == nil)
    }

    @Test func pickerTargetsExcludeProtectedAndCurrentProcesses() throws {
        let data = Data(#"[{"pid":42,"parent_pid":1,"uid":501,"name":"Protected","executable_path":null,"start_time_micros":1,"architecture":"arm64","attachable":false,"attachability_reason":"Protected by macOS"},{"pid":43,"parent_pid":1,"uid":501,"name":"Current","executable_path":null,"start_time_micros":2,"architecture":"arm64","attachable":true,"attachability_reason":null},{"pid":44,"parent_pid":1,"uid":501,"name":"Target","executable_path":null,"start_time_micros":3,"architecture":"arm64","attachable":true,"attachability_reason":null}]"#.utf8)
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let processes = try decoder.decode([AttachProcessInfo].self, from: data)

        let targets = AttachProcessInfo.pickerTargets(processes, excludingPID: 43)

        #expect(targets.map(\.pid) == [44])
    }
}
