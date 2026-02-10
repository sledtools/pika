package com.pika.app.ui

object PeerKeyNormalizer {
    fun normalize(input: String): String {
        var s = input.trim()
        s = s.lowercase()
        if (s.startsWith("nostr:")) {
            s = s.removePrefix("nostr:")
        }
        return s
    }
}

