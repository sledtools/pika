import Foundation
import SwiftUI

extension CallTimelineEvent: Identifiable {}

extension CallTimelineEvent {
    var date: Date {
        Date(timeIntervalSince1970: TimeInterval(timestamp))
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
