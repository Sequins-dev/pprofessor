/// String interning table for pprof protobuf output.
/// Index 0 is always the empty string (required by the pprof spec).
public struct StringTable {
    public private(set) var strings: [String] = []
    private var indices: [String: UInt64] = [:]

    public init() {
        _ = intern("")
    }

    /// Preserve original indexes, including duplicate strings in imported data.
    public init(strings: [String]) {
        self.strings = strings
        for (index, string) in strings.enumerated() where indices[string] == nil {
            indices[string] = UInt64(index)
        }
    }

    @discardableResult
    public mutating func intern(_ s: String) -> UInt64 {
        if let existing = indices[s] { return existing }
        let idx = UInt64(strings.count)
        strings.append(s)
        indices[s] = idx
        return idx
    }
}
