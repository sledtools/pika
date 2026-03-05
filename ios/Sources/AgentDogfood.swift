import Foundation

func isDogfoodAgentEligible(npub: String?, auth: AuthState) -> Bool {
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

func makeDogfoodAgentButtonState(isBusy: Bool) -> DogfoodAgentButtonState {
    if isBusy {
        return DogfoodAgentButtonState(title: "Starting Personal Agent...", isBusy: true)
    }
    return DogfoodAgentButtonState(title: "Start Personal Agent", isBusy: false)
}
