import Foundation
import SwiftUI

struct CallTimelineEvent: Identifiable, Equatable, Hashable {
    let id: String
    let chatId: String
    let text: String
    let timestamp: Date

    init(id: String, chatId: String, text: String, timestamp: Date = .now) {
        self.id = id
        self.chatId = chatId
        self.text = text
        self.timestamp = timestamp
    }
}

struct CallTimelineEventRow: View {
    let event: CallTimelineEvent

    var body: some View {
        HStack {
            Spacer()
            Label(event.text, systemImage: "phone.badge.clock")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .background(.ultraThinMaterial, in: Capsule())
                .accessibilityIdentifier(TestIds.callTimelineEvent)
            Spacer()
        }
        .padding(.vertical, 4)
    }
}
