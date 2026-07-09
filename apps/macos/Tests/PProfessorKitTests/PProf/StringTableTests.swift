import Testing
@testable import PProfessorKit

@Suite("StringTable")
struct StringTableTests {
    @Test func emptyAtZero() {
        let st = StringTable()
        #expect(st.strings[0] == "")
    }

    @Test func internDeduplication() {
        var st = StringTable()
        let a = st.intern("hello")
        let b = st.intern("hello")
        #expect(a == b)
    }

    @Test func sequentialIndexing() {
        var st = StringTable()
        let a = st.intern("foo")
        let b = st.intern("bar")
        #expect(b == a + 1)
    }

    @Test func emptyStringIndex() {
        var st = StringTable()
        let idx = st.intern("")
        #expect(idx == 0)
    }
}
