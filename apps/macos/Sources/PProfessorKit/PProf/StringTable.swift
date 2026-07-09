/// String interning table for pprof protobuf output.
/// Index 0 is always the empty string (required by the pprof spec).
public struct StringTable {
    public private(set) var strings: [String] = []
    private var indices: [String: UInt64] = [:]

    public init() {
        _ = intern("")
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
