import SwiftUI
import PProfessorKit

struct StackFrameRow: View {
    let stackNode: FlamegraphNode
    let isCurrentFrame: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 4) {
                Text(String(repeating: "  ", count: stackNode.depth))
                    .font(.system(.caption, design: .monospaced))

                Image(systemName: isCurrentFrame ? "arrow.right.circle.fill" : "arrow.turn.down.right")
                    .font(.caption2)
                    .foregroundColor(isCurrentFrame ? .accentColor : .secondary)

                Text(stackNode.functionName)
                    .font(.caption)
                    .foregroundColor(isCurrentFrame ? .primary : .secondary)

                Spacer()
            }

            if let filename = stackNode.filename {
                HStack(spacing: 4) {
                    Text(String(repeating: "  ", count: stackNode.depth + 1))
                        .font(.system(.caption, design: .monospaced))

                    let lineStr = stackNode.line.map { ":\($0)" } ?? ""
                    Text("\(filename)\(lineStr)")
                        .font(.caption2)
                        .foregroundColor(.blue)

                    Spacer()
                }
            }
        }
    }
}
