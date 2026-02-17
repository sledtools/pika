import SwiftUI

@MainActor
struct ActiveCallPill: View {
    let call: CallState
    let peerName: String
    let onTap: @MainActor () -> Void

    var body: some View {
        Button {
            onTap()
        } label: {
            HStack(spacing: 12) {
                Image(systemName: "phone.fill")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(.white)
                    .frame(width: 30, height: 30)
                    .background(Color.green, in: Circle())

                VStack(alignment: .leading, spacing: 2) {
                    Text("Return to call")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                    Text("\(peerName) · \(call.status.titleText)")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                }

                Spacer(minLength: 8)

                Image(systemName: "chevron.up")
                    .font(.footnote.weight(.bold))
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .background(.ultraThinMaterial, in: Capsule())
            .overlay(Capsule().strokeBorder(.quaternary, lineWidth: 0.5))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(TestIds.callReturnToCall)
    }
}
