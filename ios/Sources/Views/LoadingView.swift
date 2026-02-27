import SwiftUI

struct LoadingView: View {
    @State private var visibleLines: [String] = []
    @State private var currentIndex = 0

    private static let statusLines = [
        "Initializing MLS state...",
        "Loading identity keys...",
        "Connecting to message relays...",
        "Syncing relay subscriptions...",
        "Restoring session...",
        "Fetching key packages...",
        "Decrypting message history...",
        "Syncing group state...",
    ]

    var body: some View {
        VStack(spacing: 0) {
            Spacer()

            Image("PikaLogo")
                .resizable()
                .scaledToFit()
                .frame(width: 120, height: 120)
                .clipShape(RoundedRectangle(cornerRadius: 24))

            Text("Pika")
                .font(.largeTitle.weight(.bold))
                .padding(.top, 12)

            Spacer()
                .frame(height: 48)

            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(visibleLines.enumerated()), id: \.offset) { index, line in
                    Text(line)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(index == visibleLines.count - 1 ? .secondary : .quaternary)
                        .transition(.opacity.combined(with: .move(edge: .bottom)))
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .frame(height: 160, alignment: .bottom)
            .clipped()
            .padding(.horizontal, 28)

            Spacer()
        }
        .onAppear {
            startAnimating()
        }
    }

    private func startAnimating() {
        guard currentIndex < Self.statusLines.count else { return }
        let delay = currentIndex == 0 ? 0.4 : Double.random(in: 0.3...0.8)
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(delay))
            withAnimation(.easeOut(duration: 0.2)) {
                visibleLines.append(Self.statusLines[currentIndex])
                currentIndex += 1
            }
            startAnimating()
        }
    }
}

#if DEBUG
#Preview("Loading") {
    LoadingView()
}
#endif
