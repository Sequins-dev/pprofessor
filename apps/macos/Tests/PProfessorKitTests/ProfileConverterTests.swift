import Testing
import Foundation
@testable import PProfessorKit

@Suite("ProfileConverter")
struct ProfileConverterTests {

    /// Build a minimal DecodedProfile with a single string table and functions/locations/samples.
    private func makeProfile(stacks: [([String], Int64)]) -> DecodedProfile {
        var profile = DecodedProfile()

        // String table: index 0 = ""
        profile.stringTable = ["", "cpu", "nanoseconds"]
        profile.sampleTypes = [ValueType(type: 1, unit: 2)]

        var functionID: UInt64 = 1
        var locationID: UInt64 = 1
        var functionsByName: [String: UInt64] = [:]

        for (stack, _) in stacks {
            for funcName in stack {
                if functionsByName[funcName] == nil {
                    let nameIdx = UInt64(profile.stringTable.count)
                    profile.stringTable.append(funcName)
                    profile.functions.append(
                        ProfFunction(id: functionID, name: nameIdx, systemName: 0, filename: 0, startLine: 0)
                    )
                    functionsByName[funcName] = functionID
                    functionID += 1
                }
            }
        }

        for (stack, value) in stacks {
            // pprof stacks are leaf-first, so we reverse the input (which is root-first)
            let reversedStack = stack.reversed()
            var locIDs: [UInt64] = []
            for funcName in reversedStack {
                let fid = functionsByName[funcName]!
                profile.locations.append(
                    ProfLocation(id: locationID, lines: [ProfLine(functionID: fid, line: 0)])
                )
                locIDs.append(locationID)
                locationID += 1
            }
            profile.samples.append(ProfSample(locationIDs: locIDs, values: [value]))
        }

        return profile
    }

    /// Build a profile where each stack has an associated thread label.
    private func makeProfileWithThreads(stacks: [([String], Int64, String)]) -> DecodedProfile {
        var profile = DecodedProfile()
        profile.stringTable = ["", "cpu", "nanoseconds", "thread"]
        profile.sampleTypes = [ValueType(type: 1, unit: 2)]
        let threadKeyIdx: UInt64 = 3

        var functionID: UInt64 = 1
        var locationID: UInt64 = 1
        var functionsByName: [String: UInt64] = [:]
        var threadNameIdx: [String: UInt64] = [:]

        for (stack, _, threadName) in stacks {
            for funcName in stack {
                if functionsByName[funcName] == nil {
                    let nameIdx = UInt64(profile.stringTable.count)
                    profile.stringTable.append(funcName)
                    profile.functions.append(ProfFunction(id: functionID, name: nameIdx, systemName: 0, filename: 0, startLine: 0))
                    functionsByName[funcName] = functionID
                    functionID += 1
                }
            }
            if threadNameIdx[threadName] == nil {
                threadNameIdx[threadName] = UInt64(profile.stringTable.count)
                profile.stringTable.append(threadName)
            }
        }

        for (stack, value, threadName) in stacks {
            let reversedStack = stack.reversed()
            var locIDs: [UInt64] = []
            for funcName in reversedStack {
                let fid = functionsByName[funcName]!
                profile.locations.append(ProfLocation(id: locationID, lines: [ProfLine(functionID: fid, line: 0)]))
                locIDs.append(locationID)
                locationID += 1
            }
            let label = ProfLabel(key: threadKeyIdx, str: threadNameIdx[threadName]!)
            profile.samples.append(ProfSample(locationIDs: locIDs, values: [value], labels: [label]))
        }
        return profile
    }

    @Test func noThreadLabels() {
        let profile = makeProfile(stacks: [
            (["main", "foo"], 100)
        ])
        let result = convertProfile(profile)
        #expect(result.availableThreads.isEmpty)
    }

    @Test func threadLabelsExtracted() {
        let profile = makeProfileWithThreads(stacks: [
            (["main", "foo"], 100, "main"),
            (["main", "bar"], 50, "worker"),
        ])
        let result = convertProfile(profile)
        #expect(result.availableThreads == ["main", "worker"])
    }

    @Test func threadFilterIncludesOnlyMatchingSamples() {
        let profile = makeProfileWithThreads(stacks: [
            (["main", "foo"], 100, "main"),
            (["main", "bar"], 50, "worker"),
        ])
        let result = convertProfile(profile, threadFilter: "main")
        #expect(result.totalValue == 100)
        // "bar" should not appear in any node
        #expect(!result.nodes.contains(where: { $0.functionName == "bar" }))
        #expect(result.nodes.contains(where: { $0.functionName == "foo" }))
    }

    @Test func threadFilterAllThreadsWhenNil() {
        let profile = makeProfileWithThreads(stacks: [
            (["main", "foo"], 100, "main"),
            (["main", "bar"], 50, "worker"),
        ])
        let result = convertProfile(profile, threadFilter: nil)
        #expect(result.totalValue == 150)
        #expect(result.nodes.contains(where: { $0.functionName == "foo" }))
        #expect(result.nodes.contains(where: { $0.functionName == "bar" }))
    }

    @Test func emptyProfile() {
        let profile = DecodedProfile()
        let result = convertProfile(profile)
        #expect(result.nodes.isEmpty)
        #expect(result.rootNodeId == nil)
        #expect(result.totalValue == 0)
    }

    @Test func singleSampleThreeDeep() {
        let profile = makeProfile(stacks: [
            (["main", "foo", "bar"], 100)
        ])
        let result = convertProfile(profile)

        // root + 3 frames = 4 nodes
        #expect(result.nodes.count == 4)
        #expect(result.rootNodeId == "root")
        #expect(result.totalValue == 100)

        let root = result.nodes[result.nodeIndex["root"]!]
        #expect(root.totalValue == 100)
        #expect(root.selfValue == 0)
        #expect(root.childIds.count == 1)

        let mainId = root.childIds[0]
        let main = result.nodes[result.nodeIndex[mainId]!]
        #expect(main.functionName == "main")
        #expect(main.totalValue == 100)
        #expect(main.selfValue == 0)
        #expect(main.depth == 1)

        let fooId = main.childIds[0]
        let foo = result.nodes[result.nodeIndex[fooId]!]
        #expect(foo.functionName == "foo")
        #expect(foo.depth == 2)

        let barId = foo.childIds[0]
        let bar = result.nodes[result.nodeIndex[barId]!]
        #expect(bar.functionName == "bar")
        #expect(bar.selfValue == 100)  // leaf gets all self value
        #expect(bar.totalValue == 100)
        #expect(bar.depth == 3)
    }

    @Test func twoSamplesSharedPrefix() {
        let profile = makeProfile(stacks: [
            (["main", "foo", "bar"], 60),
            (["main", "foo", "baz"], 40),
        ])
        let result = convertProfile(profile)

        #expect(result.totalValue == 100)

        let rootIdx = result.nodeIndex["root"]!
        let root = result.nodes[rootIdx]
        #expect(root.totalValue == 100)

        let mainId = root.childIds[0]
        let main = result.nodes[result.nodeIndex[mainId]!]
        #expect(main.totalValue == 100)
        #expect(main.selfValue == 0)

        let fooId = main.childIds[0]
        let foo = result.nodes[result.nodeIndex[fooId]!]
        #expect(foo.totalValue == 100)
        #expect(foo.selfValue == 0)
        #expect(foo.childIds.count == 2)

        let barId = foo.childIds[0]
        let bar = result.nodes[result.nodeIndex[barId]!]
        #expect(bar.functionName == "bar")
        #expect(bar.selfValue == 60)
        #expect(bar.totalValue == 60)

        let bazId = foo.childIds[1]
        let baz = result.nodes[result.nodeIndex[bazId]!]
        #expect(baz.functionName == "baz")
        #expect(baz.selfValue == 40)
        #expect(baz.totalValue == 40)
    }

    @Test func percentagesCorrect() {
        let profile = makeProfile(stacks: [
            (["main", "a"], 75),
            (["main", "b"], 25),
        ])
        let result = convertProfile(profile)
        #expect(result.totalValue == 100)

        let rootIdx = result.nodeIndex["root"]!
        let root = result.nodes[rootIdx]
        #expect(abs(root.totalPercentage - 100.0) < 0.001)

        let mainId = root.childIds[0]
        let main = result.nodes[result.nodeIndex[mainId]!]
        #expect(abs(main.totalPercentage - 100.0) < 0.001)

        let aId = main.childIds[0]
        let a = result.nodes[result.nodeIndex[aId]!]
        #expect(abs(a.selfPercentage - 75.0) < 0.001)
        #expect(abs(a.totalPercentage - 75.0) < 0.001)

        let bId = main.childIds[1]
        let b = result.nodes[result.nodeIndex[bId]!]
        #expect(abs(b.selfPercentage - 25.0) < 0.001)
    }
}
