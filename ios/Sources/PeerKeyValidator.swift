import Foundation

enum PeerKeyValidator {
    static func normalize(_ input: String) -> String {
        var s = input.trimmingCharacters(in: .whitespacesAndNewlines)
        s = s.lowercased()
        if s.hasPrefix("nostr:") {
            s.removeFirst("nostr:".count)
        }
        return s
    }

    // Minimal UX validation. Real parsing happens in Rust, but this prevents common operator errors
    // like pasting the bech32 payload without the "npub1" prefix.
    static func isValidPeer(_ s: String) -> Bool {
        if isValidHexPubkey(s) { return true }
        if isValidNpub(s) { return true }
        return false
    }

    private static func isValidHexPubkey(_ s: String) -> Bool {
        guard s.count == 64 else { return false }
        return s.unicodeScalars.allSatisfy { scalar in
            switch scalar.value {
            case 48...57, 65...70, 97...102: // 0-9 A-F a-f
                return true
            default:
                return false
            }
        }
    }

    private static func isValidNpub(_ raw: String) -> Bool {
        let s = raw.lowercased()
        guard s.hasPrefix("npub1") else { return false }

        // bech32 charset: qpzry9x8gf2tvdw0s3jn54khce6mua7l
        let allowed = Set("qpzry9x8gf2tvdw0s3jn54khce6mua7l")
        let payload = s.dropFirst(5)
        guard payload.count >= 10 else { return false }
        return payload.allSatisfy { allowed.contains($0) }
    }
}
