package com.pika.app

import android.Manifest
import android.util.Log
import androidx.compose.ui.test.ComposeTimeoutException
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.test.rule.GrantPermissionRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.pika.app.rust.AppAction
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assume
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class PikaE2eUiTest {
    @get:Rule
    val compose = createAndroidComposeRule<MainActivity>()

    @get:Rule
    val micPermission: GrantPermissionRule = GrantPermissionRule.grant(Manifest.permission.RECORD_AUDIO)

    private val uiReadyTimeoutMs = 90_000L

    @Test
    fun e2e_deployedRustBot_pingPong() {
        val args = InstrumentationRegistry.getArguments()
        Assume.assumeTrue(
            "Set -Pandroid.testInstrumentationRunnerArguments.pika_e2e=1 to enable public-relay E2E UI test.",
            args.getString("pika_e2e") == "1",
        )

        val ctx = InstrumentationRegistry.getInstrumentation().targetContext
        val testNsec = args.getString("pika_nsec") ?: args.getString("pika_test_nsec") ?: ""
        val botNpub = args.getString("pika_peer_npub") ?: ""

        // Public-relay E2E should be explicit. Defaults hide misconfiguration and cause flaky hangs.
        check(botNpub.isNotBlank()) { "Missing instrumentation arg: pika_peer_npub" }
        check(testNsec.isNotBlank()) { "Missing instrumentation arg: pika_nsec" }

        // Start from a known auth state, but avoid UI-driven login: Compose semantics are flaky on
        // physical devices and we're primarily validating the full Kotlin <-> Rust stack.
        runOnMain { AppManager.getInstance(ctx).logout() }
        waitUntilState(uiReadyTimeoutMs, ctx, "logged out") { it.auth is com.pika.app.rust.AuthState.LoggedOut }

        runOnMain { AppManager.getInstance(ctx).dispatch(AppAction.Login(testNsec)) }
        waitUntilState(120_000, ctx, "logged in") { it.auth is com.pika.app.rust.AuthState.LoggedIn }

        Log.d("PikaE2eUiTest", "botNpub=$botNpub")

        // Create + open chat via actions to avoid UI flakes.
        // 3 attempts x 60s (same total budget as old single 180s wait).
        runOnMain { AppManager.getInstance(ctx).dispatch(AppAction.CreateChat(botNpub)) }
        retryOnTimeout(
            maxAttempts = 3,
            beforeRetry = { runOnMain { AppManager.getInstance(ctx).dispatch(AppAction.CreateChat(botNpub)) } },
        ) {
            waitUntilState(60_000, ctx, "chat created") { st ->
                st.chatList.any { !it.isGroup && it.members.any { m -> m.npub == botNpub } }
            }
        }
        val chatId =
            runOnMain {
                AppManager.getInstance(ctx).state.chatList.first { !it.isGroup && it.members.any { m -> m.npub == botNpub } }.chatId
            }
        runOnMain { AppManager.getInstance(ctx).dispatch(AppAction.OpenChat(chatId)) }
        waitUntilState(60_000, ctx, "chat opened") { it.currentChat?.chatId == chatId }

        // Send deterministic probe. Use a fresh nonce per retry to avoid duplicate suppression.
        val expectedPongs = linkedSetOf<String>()
        fun sendProbe(): String {
            val nonce = java.util.UUID.randomUUID().toString().replace("-", "").lowercase()
            val probe = "ping:$nonce"
            val expect = "pong:$nonce"
            expectedPongs.add(expect)
            runOnMain {
                val app = AppManager.getInstance(ctx)
                app.dispatch(AppAction.OpenChat(chatId))
                app.dispatch(AppAction.SendMessage(chatId, probe, null, null))
            }
            return probe
        }
        var lastProbe = sendProbe()

        dumpState("after probe", ctx)

        // Ensure the outbound message actually made it into Rust-owned state (guards against UI flake).
        waitUntilState(30_000, ctx, "probe persisted") { st ->
            hasChatMessage(st, chatId, lastProbe)
        }

        // Wait for deterministic ack from the bot. 4 attempts x 90s to absorb relay jitter in nightly.
        retryOnTimeout(
            maxAttempts = 4,
            beforeRetry = {
                lastProbe = sendProbe()
                dumpState("retry ping", ctx)
            },
        ) {
            waitUntilState(90_000, ctx, "pong received") { st ->
                expectedPongs.any { expect -> hasChatMessage(st, chatId, expect) }
            }
        }
    }

    @Test
    fun e2e_deployedRustBot_callAudio() {
        val args = InstrumentationRegistry.getArguments()
        Assume.assumeTrue(
            "Set -Pandroid.testInstrumentationRunnerArguments.pika_e2e=1 to enable public-relay E2E UI test.",
            args.getString("pika_e2e") == "1",
        )

        val ctx = InstrumentationRegistry.getInstrumentation().targetContext
        val testNsec = args.getString("pika_nsec") ?: args.getString("pika_test_nsec") ?: ""
        val botNpub = args.getString("pika_peer_npub") ?: ""

        check(botNpub.isNotBlank()) { "Missing instrumentation arg: pika_peer_npub" }
        check(testNsec.isNotBlank()) { "Missing instrumentation arg: pika_nsec" }

        // Start from a known auth state.
        runOnMain { AppManager.getInstance(ctx).logout() }
        waitUntilState(uiReadyTimeoutMs, ctx, "logged out") { it.auth is com.pika.app.rust.AuthState.LoggedOut }

        runOnMain { AppManager.getInstance(ctx).dispatch(AppAction.Login(testNsec)) }
        waitUntilState(120_000, ctx, "logged in") { it.auth is com.pika.app.rust.AuthState.LoggedIn }

        // Create + open chat via actions to avoid UI flakes.
        // 3 attempts x 60s (same total budget as old single 180s wait).
        runOnMain { AppManager.getInstance(ctx).dispatch(AppAction.CreateChat(botNpub)) }
        retryOnTimeout(
            maxAttempts = 3,
            beforeRetry = { runOnMain { AppManager.getInstance(ctx).dispatch(AppAction.CreateChat(botNpub)) } },
        ) {
            waitUntilState(60_000, ctx, "chat created") { st ->
                st.chatList.any { !it.isGroup && it.members.any { m -> m.npub == botNpub } }
            }
        }
        val chatId =
            runOnMain {
                AppManager.getInstance(ctx).state.chatList.first { !it.isGroup && it.members.any { m -> m.npub == botNpub } }.chatId
            }
        runOnMain { AppManager.getInstance(ctx).dispatch(AppAction.OpenChat(chatId)) }
        waitUntilState(60_000, ctx, "chat opened") { it.currentChat?.chatId == chatId }

        // Preflight: confirm bot is responsive before starting a call (reduces flakes on real devices).
        run {
            val nonce = java.util.UUID.randomUUID().toString().replace("-", "").lowercase()
            val probe = "ping:$nonce"
            val expect = "pong:$nonce"
            runOnMain { AppManager.getInstance(ctx).dispatch(AppAction.SendMessage(chatId, probe, null, null)) }
            compose.waitUntil(90_000) {
                runOnMain {
                    AppManager.getInstance(ctx).state.currentChat?.messages?.any {
                        it.content.trim() == expect
                    }
                        ?: false
                }
            }
        }

        // Start call.
        fun waitForActiveMedia(timeoutMs: Long) {
            val minTx = 100UL
            compose.waitUntil(timeoutMs) {
                runOnMain {
                    val call = AppManager.getInstance(ctx).state.activeCall
                    val active = call?.status is com.pika.app.rust.CallStatus.Active
                    val tx = call?.debug?.txFrames ?: 0UL
                    val rx = call?.debug?.rxFrames ?: 0UL
                    active && tx >= minTx && rx >= 1UL
                }
            }
        }

        // Public relays + deployed bot are nondeterministic. Allow a retry if the bot
        // doesn't accept quickly (common when relays are flaky).
        runOnMain { AppManager.getInstance(ctx).dispatch(AppAction.StartCall(chatId)) }
        retryOnTimeout(
            maxAttempts = 2,
            beforeRetry = {
                runOnMain { AppManager.getInstance(ctx).dispatch(AppAction.EndCall) }
                Thread.sleep(1000)
                runOnMain { AppManager.getInstance(ctx).dispatch(AppAction.StartCall(chatId)) }
            },
        ) {
            waitForActiveMedia(180_000)
        }

        // End call.
        runOnMain { AppManager.getInstance(ctx).dispatch(AppAction.EndCall) }

        // Wait for call to be gone or ended.
        compose.waitUntil(60_000) {
            runOnMain {
                val call = AppManager.getInstance(ctx).state.activeCall
                call == null || call.status is com.pika.app.rust.CallStatus.Ended
            }
        }
    }

    private fun retryOnTimeout(maxAttempts: Int, beforeRetry: (() -> Unit)? = null, block: () -> Unit) {
        var attempt = 0
        while (true) {
            attempt++
            try {
                block()
                return
            } catch (e: Throwable) {
                if (e !is ComposeTimeoutException && e !is AssertionError) throw e
                if (attempt >= maxAttempts) throw e
                Log.d("PikaE2eUiTest", "attempt $attempt timed out; retrying")
                beforeRetry?.invoke()
                Thread.sleep(2000)
            }
        }
    }

    private fun hasChatMessage(st: com.pika.app.rust.AppState, chatId: String, message: String): Boolean {
        val expected = message.trim()
        val currentHas =
            st.currentChat
                ?.takeIf { it.chatId == chatId }
                ?.messages
                ?.any { it.content.trim() == expected }
                ?: false
        if (currentHas) return true
        val summary = st.chatList.firstOrNull { it.chatId == chatId } ?: return false
        return summary.lastMessage?.trim() == expected || summary.lastMessagePreview.trim() == expected
    }

    private fun waitUntilState(
        timeoutMs: Long,
        ctx: android.content.Context,
        desc: String,
        predicate: (com.pika.app.rust.AppState) -> Boolean,
    ) {
        try {
            compose.waitUntil(timeoutMs) {
                runOnMain {
                    predicate(AppManager.getInstance(ctx).state)
                }
            }
        } catch (e: ComposeTimeoutException) {
            dumpState("timeout: $desc", ctx)
            throw AssertionError("timeout waiting for state condition: $desc", e)
        }
    }

    private fun dumpState(phase: String, ctx: android.content.Context) {
        runCatching {
            val st = AppManager.getInstance(ctx).state
            val msgCount = st.currentChat?.messages?.size ?: 0
            val lastMsg = st.currentChat?.messages?.lastOrNull()?.content
            Log.d(
                "PikaE2eUiTest",
                "phase=$phase rev=${st.rev} auth=${st.auth} default=${st.router.defaultScreen} stack=${st.router.screenStack} chats=${st.chatList.size} current=${st.currentChat?.chatId} msgCount=$msgCount lastMsg=${lastMsg ?: ""}",
            )
        }
    }

    private fun <T> runOnMain(block: () -> T): T {
        val ref = AtomicReference<T>()
        InstrumentationRegistry.getInstrumentation().runOnMainSync { ref.set(block()) }
        @Suppress("UNCHECKED_CAST")
        return ref.get() as T
    }
}
