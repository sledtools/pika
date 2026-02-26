package com.pika.app

import android.content.ClipboardManager
import android.content.Context
import android.util.Log
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performImeAction
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.performScrollToNode
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.pressKey
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.input.key.Key
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.pika.app.ui.TestTags
import java.util.concurrent.atomic.AtomicReference
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.rules.RuleChain
import org.junit.rules.TestRule
import org.junit.runner.Description
import org.junit.runners.model.Statement
import org.json.JSONObject

@RunWith(AndroidJUnit4::class)
class PikaUiTest {
    private val disableNetworkRule =
        TestRule { base: Statement, _: Description ->
            object : Statement() {
                override fun evaluate() {
                    val ctx = InstrumentationRegistry.getInstrumentation().targetContext
                    val path = ctx.filesDir.resolve("pika_config.json")
                    val cfg =
                        JSONObject().apply {
                            put("disable_network", true)
                            put("call_moq_url", "https://us-east.moq.logos.surf/anon")
                            put("call_broadcast_prefix", "pika/calls")
                        }
                    path.writeText(cfg.toString())
                    base.evaluate()
                }
            }
        }

    private val composeRule = createAndroidComposeRule<MainActivity>()

    @get:Rule
    val rules: TestRule = RuleChain.outerRule(disableNetworkRule).around(composeRule)

    private val compose get() = composeRule

    private fun hardResetForeground() {
        // Avoid doing anything that would background the current Activity: Compose tests depend on
        // the Activity being in foreground so the semantics tree exists.
        //
        // If external tools (agent-device/uiautomator) are running concurrently, they can still
        // interfere; run UI tests in isolation.
    }

    @Test
    fun loginNsecField_isPasswordSemantics() {
        hardResetForeground()
        val ctx = InstrumentationRegistry.getInstrumentation().targetContext
        runOnMain { AppManager.getInstance(ctx).logout() }

        compose.onNodeWithTag(TestTags.LOGIN_NSEC)
            .assertIsDisplayed()
            .assert(SemanticsMatcher.keyIsDefined(SemanticsProperties.Password))
    }

    @Test
    @OptIn(ExperimentalTestApi::class)
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
        compose.onNodeWithTag(TestTags.CHATLIST_MY_PROFILE).performClick()
        compose.waitUntil(30_000) {
            runCatching { compose.onNodeWithText("Profile").assertIsDisplayed() }.isSuccess &&
                runCatching {
                    compose.onNodeWithTag(TestTags.MYPROFILE_COPY_NPUB).assertIsDisplayed()
                }.isSuccess
        }

        // Use the in-app "Copy" action and read from the system clipboard (no semantics scraping).
        compose.onNodeWithTag(TestTags.MYPROFILE_COPY_NPUB).performClick()
        val myNpub = waitForClipboardMatching(Regex("^npub1[0-9a-z]+$"))
        Log.d("PikaUiTest", "myNpub=$myNpub")

        compose.onNodeWithContentDescription("New Chat").performClick()
        // Material3 TopAppBar title semantics can be present but not "displayed" per test
        // semantics; the peer input field is a better screen-ready signal.
        compose.waitUntil(30_000) {
            runCatching {
                compose.onAllNodesWithTag(TestTags.NEWCHAT_PEER_NPUB).fetchSemanticsNodes().isNotEmpty()
            }.getOrDefault(false)
        }

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

        val imeMsg = "hello from ui test ime"
        compose.onNodeWithTag(TestTags.CHAT_MESSAGE_INPUT).performTextInput(imeMsg)
        compose.onNodeWithTag(TestTags.CHAT_MESSAGE_INPUT).assertTextContains(imeMsg)
        compose.waitForIdle()
        compose.onNodeWithTag(TestTags.CHAT_MESSAGE_INPUT).performImeAction()

        // Message should appear optimistically even if publishing fails.
        dumpState("after IME send", ctx)
        compose.waitUntil(30_000) {
            runOnMain {
                AppManager.getInstance(ctx).state.currentChat?.messages?.any { it.content == imeMsg }
                    ?: false
            } && runCatching {
                compose.onNodeWithTag(TestTags.CHAT_MESSAGE_LIST).assertIsDisplayed()
            }.isSuccess
        }
        check(
            runOnMain {
                AppManager.getInstance(ctx).state.currentChat?.messages?.any { it.content == imeMsg }
                    ?: false
            },
        )

        val hardwareEnterMsg = "hello from ui test enter"
        compose.onNodeWithTag(TestTags.CHAT_MESSAGE_INPUT).performTextInput(hardwareEnterMsg)
        compose.onNodeWithTag(TestTags.CHAT_MESSAGE_INPUT).assertTextContains(hardwareEnterMsg)
        compose.waitForIdle()
        compose.onNodeWithTag(TestTags.CHAT_MESSAGE_INPUT).performKeyInput {
            pressKey(Key.Enter)
        }

        dumpState("after hardware enter send", ctx)
        compose.waitUntil(30_000) {
            runOnMain {
                AppManager.getInstance(ctx).state.currentChat?.messages?.any { it.content == hardwareEnterMsg }
                    ?: false
            }
        }
        check(
            runOnMain {
                AppManager.getInstance(ctx).state.currentChat?.messages?.any { it.content == hardwareEnterMsg }
                    ?: false
            },
        )

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
        compose.onNodeWithTag(TestTags.CHATLIST_MY_PROFILE).performClick()
        compose.waitUntil(30_000) {
            runCatching { compose.onNodeWithText("Profile").assertIsDisplayed() }.isSuccess
        }
        compose.onNodeWithTag(TestTags.MYPROFILE_SHEET_LIST)
            .performScrollToNode(hasTestTag(TestTags.MYPROFILE_LOGOUT))
        compose.onNodeWithTag(TestTags.MYPROFILE_LOGOUT).performClick()
        compose.waitUntil(30_000) {
            runCatching {
                compose.onNodeWithTag(TestTags.MYPROFILE_LOGOUT_CONFIRM).assertIsDisplayed()
            }.isSuccess
        }
        compose.onNodeWithTag(TestTags.MYPROFILE_LOGOUT_CONFIRM).performClick()
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
