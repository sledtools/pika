import Foundation

func isAgentEligible(npub: String?, auth: AuthState) -> Bool {
    guard let normalized = npub?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased(),
          !normalized.isEmpty else {
        return false
    }

    guard case .loggedIn(_, _, let mode) = auth else {
        return false
    }
    switch mode {
    case .localNsec:
        return true
    case .externalSigner, .bunkerSigner:
        return false
    }
}

func makeAgentButtonState(isBusy: Bool) -> AgentButtonState {
    if isBusy {
        return AgentButtonState(title: "Starting Agent...", isBusy: true)
    }
    return AgentButtonState(title: "Start Agent", isBusy: false)
}
