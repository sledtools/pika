package com.pika.app

import android.content.Intent
import android.net.Uri
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class NostrConnectIntentTest {
    private val scheme = AppManager.NOSTR_CONNECT_CALLBACK_SCHEME

    @Test
    fun withNostrConnectCallback_addsCallbackToNostrConnectUrls() {
        val raw =
            "nostrconnect://f8d6adf2627c4f3a8f182f95c6ccf5fd2ccf48f9aa94d7f9deaa0a5f88dbf9b6?relay=wss%3A%2F%2Frelay.primal.net&metadata=%7B%22name%22%3A%22Pika%22%7D"

        val out = AppManager.withNostrConnectCallback(raw)
        val parsed = Uri.parse(out)

        assertEquals("nostrconnect", parsed.scheme)
        assertEquals(AppManager.NOSTR_CONNECT_CALLBACK_URL, parsed.getQueryParameter("callback"))
    }

    @Test
    fun withNostrConnectCallback_isIdempotentWhenCallbackExists() {
        val encodedCallback = Uri.encode(AppManager.NOSTR_CONNECT_CALLBACK_URL)
        val raw =
            "nostrconnect://abc123?relay=wss%3A%2F%2Frelay.example.com&callback=$encodedCallback"

        val out = AppManager.withNostrConnectCallback(raw)

        assertEquals(raw, out)
        assertTrue(out.countOccurrences("callback=") == 1)
    }

    @Test
    fun withNostrConnectCallback_ignoresNonNostrConnectUrls() {
        val raw = "nostrsigner://request?type=get_public_key"

        val out = AppManager.withNostrConnectCallback(raw)

        assertEquals(raw, out)
    }

    @Test
    fun extractNostrConnectCallback_returnsCallbackUrlForMatchingIntent() {
        val intent =
            Intent(Intent.ACTION_VIEW).apply {
                data = Uri.parse("$scheme://nostrconnect-return?result=ok")
            }

        val callback = AppManager.extractNostrConnectCallback(intent)

        assertEquals("$scheme://nostrconnect-return?result=ok", callback)
    }

    @Test
    fun extractNostrConnectCallback_rejectsNonCallbackIntents() {
        val wrongHost =
            Intent(Intent.ACTION_VIEW).apply {
                data = Uri.parse("$scheme://other-host?result=ok")
            }
        val wrongAction =
            Intent(Intent.ACTION_MAIN).apply {
                data = Uri.parse("$scheme://nostrconnect-return?result=ok")
            }

        assertNull(AppManager.extractNostrConnectCallback(wrongHost))
        assertNull(AppManager.extractNostrConnectCallback(wrongAction))
    }

    // ── Chat deep link intent tests ──

    // A valid 64-char hex pubkey (always passes isValidPeerKey).
    private val validHexPubkey = "a".repeat(64)

    @Test
    fun extractChatDeepLinkNpub_returnsNpubForValidChatIntent() {
        val intent = Intent(Intent.ACTION_VIEW).apply {
            data = Uri.parse("$scheme://chat/$validHexPubkey")
        }
        assertEquals(validHexPubkey, AppManager.extractChatDeepLinkNpub(intent))
    }

    @Test
    fun extractChatDeepLinkNpub_returnsNullForWrongHost() {
        val intent = Intent(Intent.ACTION_VIEW).apply {
            data = Uri.parse("$scheme://nostrconnect-return/$validHexPubkey")
        }
        assertNull(AppManager.extractChatDeepLinkNpub(intent))
    }

    @Test
    fun extractChatDeepLinkNpub_returnsNullForInvalidNpub() {
        val intent = Intent(Intent.ACTION_VIEW).apply {
            data = Uri.parse("$scheme://chat/garbage")
        }
        assertNull(AppManager.extractChatDeepLinkNpub(intent))
    }

    @Test
    fun extractChatDeepLinkNpub_returnsNullForMissingPath() {
        val intent = Intent(Intent.ACTION_VIEW).apply {
            data = Uri.parse("$scheme://chat")
        }
        assertNull(AppManager.extractChatDeepLinkNpub(intent))
    }

    @Test
    fun extractChatDeepLinkNpub_returnsNullForWrongAction() {
        val intent = Intent(Intent.ACTION_MAIN).apply {
            data = Uri.parse("$scheme://chat/$validHexPubkey")
        }
        assertNull(AppManager.extractChatDeepLinkNpub(intent))
    }

    private fun String.countOccurrences(fragment: String): Int {
        if (fragment.isEmpty()) return 0
        var count = 0
        var index = 0
        while (true) {
            val next = indexOf(fragment, index)
            if (next < 0) return count
            count += 1
            index = next + fragment.length
        }
    }
}
