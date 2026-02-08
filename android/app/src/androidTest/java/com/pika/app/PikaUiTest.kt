package com.pika.app

import android.content.ClipboardManager
import android.content.Context
import android.util.Log
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.pika.app.ui.TestTags
import java.util.concurrent.atomic.AtomicReference
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class PikaUiTest {
    @get:Rule
    val compose = createAndroidComposeRule<MainActivity>()

    private fun hardResetForeground() {
        // Avoid doing anything that would background the current Activity: Compose tests depend on
        // the Activity being in foreground so the semantics tree exists.
        //
        // If external tools (agent-device/uiautomator) are running concurrently, they can still
        // interfere; run UI tests in isolation.
    }

    @Test
    fun createAccount_noteToSelf_sendMessage_and_logout() {
        hardResetForeground()
        val ctx = InstrumentationRegistry.getInstrumentation().targetContext

        // Ensure we start from a known state (tests run on a shared emulator).
        runOnMain { AppManager.getInstance(ctx).logout() }

        // Create an account deterministically.
        compose.onNodeWithTag(TestTags.LOGIN_CREATE_ACCOUNT).performClick()

        compose.waitUntil(30_000) {
            runCatching {
                compose.onNodeWithText("Chats").assertIsDisplayed()
            }.isSuccess
        }
        compose.onNodeWithContentDescription("My npub").performClick()
        compose.onNodeWithText("My npub").assertIsDisplayed()

        // Use the in-app "Copy" action and read from the system clipboard (no semantics scraping).
        compose.onNodeWithText("Copy").performClick()
        val myNpub = waitForClipboardMatching(Regex("^npub1[0-9a-z]+$"))
        Log.d("PikaUiTest", "myNpub=$myNpub")

        compose.onNodeWithContentDescription("New Chat").performClick()
        compose.onNodeWithText("New chat").assertIsDisplayed()

        compose.onNodeWithTag(TestTags.NEWCHAT_PEER_NPUB).performTextInput(myNpub)
        compose.waitForIdle()
        compose.onNodeWithTag(TestTags.NEWCHAT_START).performClick()

        // Note-to-self is the deterministic offline flow; we don't depend on a specific title,
        // just that a chat screen opens with a message composer.
        dumpState("after Start chat click", ctx)
        compose.waitUntil(60_000) {
            runCatching {
                compose.onNodeWithTag(TestTags.CHAT_MESSAGE_INPUT).assertIsDisplayed()
            }.isSuccess
        }
        compose.onNodeWithTag(TestTags.CHAT_MESSAGE_INPUT).assertIsDisplayed()

        val msg = "hello from ui test"
        compose.onNodeWithTag(TestTags.CHAT_MESSAGE_INPUT).performTextInput(msg)
        compose.onNodeWithTag(TestTags.CHAT_MESSAGE_INPUT).assertTextContains(msg)
        // Ensure Compose state is updated before tapping Send (avoid dispatching an empty draft).
        compose.waitForIdle()
        compose.onNodeWithTag(TestTags.CHAT_SEND).performClick()

        // Message should appear optimistically even if publishing fails.
        dumpState("after Send click", ctx)
        compose.waitUntil(30_000) {
            val hasInState =
                runOnMain {
                    AppManager.getInstance(ctx).state.currentChat?.messages?.any { it.content == msg }
                        ?: false
                }
            if (!hasInState) return@waitUntil false
            runCatching { compose.onAllNodesWithText(msg).fetchSemanticsNodes().isNotEmpty() }
                .getOrDefault(false)
        }
        // Avoid flaking on "displayed" if the software keyboard overlaps the message list.
        // Existence in the semantics tree means Compose rendered the Rust-owned state.
        check(compose.onAllNodesWithText(msg).fetchSemanticsNodes().isNotEmpty())

        // Back to chat list then logout.
        compose.onNodeWithContentDescription("Back").performClick()
        // Depending on router stack behavior, we may land back on "New chat" first (then need one more back).
        compose.waitUntil(10_000) {
            runCatching { compose.onNodeWithText("Chats").assertIsDisplayed() }.isSuccess ||
                runCatching { compose.onNodeWithText("New chat").assertIsDisplayed() }.isSuccess
        }
        if (runCatching { compose.onNodeWithText("New chat").assertIsDisplayed() }.isSuccess) {
            compose.onNodeWithContentDescription("Back").performClick()
        }
        compose.onNodeWithText("Chats").assertIsDisplayed()
        compose.onNodeWithContentDescription("Logout").performClick()
        compose.onNodeWithText("Pika").assertIsDisplayed()
    }

    private fun waitForClipboardMatching(re: Regex): String {
        val ctx = InstrumentationRegistry.getInstrumentation().targetContext
        val clipboard = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        var out: String? = null
        compose.waitUntil(15_000) {
            val clip = clipboard.primaryClip
            val item = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)
            val text = item?.coerceToText(ctx)?.toString()?.trim()
            out = text?.takeIf { re.matches(it) }
            out != null
        }
        return requireNotNull(out)
    }

    private fun dumpState(phase: String, ctx: Context) {
        // Best-effort: this is a black-box UI test, but dumping the Rust-derived state helps
        // diagnose flakes on emulators.
        runCatching {
            val st = AppManager.getInstance(ctx).state
            val msgCount = st.currentChat?.messages?.size ?: 0
            val lastMsg = st.currentChat?.messages?.lastOrNull()?.content
            Log.d(
                "PikaUiTest",
                "phase=$phase rev=${st.rev} default=${st.router.defaultScreen} stack=${st.router.screenStack} chats=${st.chatList.size} current=${st.currentChat?.chatId} msgCount=$msgCount lastMsg=${lastMsg ?: ""}",
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
