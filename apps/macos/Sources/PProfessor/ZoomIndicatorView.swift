import SwiftUI

struct ZoomIndicatorView: View {
    let frameName: String
    let onReset: () -> Void

    var body: some View {
        VStack(alignment: .trailing, spacing: 4) {
            HStack(spacing: 4) {
                Image(systemName: "magnifyingglass")
                    .font(.caption)
                Text("Zoomed to:")
                    .font(.caption)
            }
            .foregroundColor(.secondary)

            Text(frameName)
                .font(.caption)
                .fontWeight(.medium)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: 200)

            Button("Reset Zoom", action: onReset)
                .buttonStyle(.plain)
                .font(.caption)
                .foregroundColor(.accentColor)
        }
        .padding(8)
        .background(Color(NSColor.controlBackgroundColor).opacity(0.95))
        .cornerRadius(6)
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .stroke(Color.secondary.opacity(0.2), lineWidth: 1)
        )
    }
}
