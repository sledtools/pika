import Foundation

extension CallStatus {
    var titleText: String {
        switch self {
        case .offering:
            return "Calling..."
        case .ringing:
            return "Incoming call"
        case .connecting:
            return "Connecting..."
        case .active:
            return "Call active"
        case .ended:
            return "Call ended"
        }
    }
}

func callReasonText(_ reason: String) -> String {
    switch reason {
    case "user_hangup":
        return "User hangup"
    case "declined":
        return "Declined"
    case "busy":
        return "Busy"
    case "auth_failed":
        return "Auth failed"
    case "runtime_error":
        return "Runtime error"
    case "publish_failed":
        return "Publish failed"
    case "serialize_failed":
        return "Serialize failed"
    case "unsupported_group":
        return "Unsupported group"
    default:
        return reason.replacingOccurrences(of: "_", with: " ").capitalized
    }
}

func formattedCallDuration(seconds: Int64) -> String {
    let total = max(0, seconds)
    let hours = total / 3600
    let minutes = (total % 3600) / 60
    let secs = total % 60

    func twoDigit(_ value: Int64) -> String {
        value < 10 ? "0\(value)" : "\(value)"
    }

    if hours > 0 {
        return "\(hours):\(twoDigit(minutes)):\(twoDigit(secs))"
    }
    return "\(twoDigit(minutes)):\(twoDigit(secs))"
}

func callDurationText(startedAt: Int64?, nowSeconds: Int64 = Int64(Date().timeIntervalSince1970)) -> String? {
    guard let startedAt, startedAt > 0 else { return nil }
    return formattedCallDuration(seconds: nowSeconds - startedAt)
}

func formattedCallDebugStats(_ debug: CallDebugStats) -> String {
    "tx \(debug.txFrames)  rx \(debug.rxFrames)  drop \(debug.rxDropped)"
}

