import SwiftUI

struct AgentProvisioningView: View {
    let state: AgentProvisioningState?
    let onRetry: @MainActor () -> Void

    var body: some View {
        VStack {
            Spacer()

            if state?.phase == .error {
                Image(systemName: "exclamationmark.triangle")
                    .font(.system(size: 40))
                    .foregroundStyle(.secondary)
                    .padding(.bottom, 16)

                Text(state?.statusMessage ?? "Something went wrong")
                    .font(.headline)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 32)

                Button("Try Again") {
                    onRetry()
                }
                .buttonStyle(.borderedProminent)
                .padding(.top, 16)
            } else {
                ProgressView()
                    .scaleEffect(1.5)
                    .padding(.bottom, 16)

                Text(state?.statusMessage ?? "Starting agent...")
                    .font(.headline)
                    .foregroundStyle(.secondary)

                if let elapsed = state?.elapsedSecs, elapsed > 0 {
                    Text("\(elapsed)s elapsed")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
            }

            if let npub = state?.agentNpub {
                Text(String(npub.prefix(20)) + "...")
                    .font(.caption2)
                    .monospaced()
                    .foregroundStyle(.tertiary)
                    .padding(.top, 8)
            }

            Spacer()

            HStack {
                Text("Message")
                    .foregroundStyle(.quaternary)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color(.tertiarySystemFill), in: RoundedRectangle(cornerRadius: 20))
            }
            .padding()
            .disabled(true)
        }
        .navigationTitle(state?.agentNpub.map { String($0.prefix(12)) + "..." } ?? "New Agent")
        .navigationBarTitleDisplayMode(.inline)
    }
}
