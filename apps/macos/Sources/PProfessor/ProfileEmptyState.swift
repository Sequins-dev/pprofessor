import SwiftUI

struct ProfileEmptyState: View {
    let isLoading: Bool
    let onOpenFile: (() -> Void)?

    var body: some View {
        VStack(spacing: 20) {
            Spacer()

            if isLoading {
                ProgressView("Loading profile...")
            } else {
                Image(systemName: "flame")
                    .font(.system(size: 48))
                    .foregroundColor(.secondary)

                Text("No Profile Data")
                    .font(.title2)
                    .foregroundColor(.secondary)

                Text("Open a .pb.gz pprof file to visualize it.")
                    .font(.body)
                    .foregroundStyle(.tertiary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 300)

                if let onOpenFile {
                    Button("Open File...", action: onOpenFile)
                        .buttonStyle(.borderedProminent)
                }
            }

            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
