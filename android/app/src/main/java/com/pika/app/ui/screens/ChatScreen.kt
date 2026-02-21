package com.pika.app.ui.screens

import android.Manifest
import android.content.pm.PackageManager
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.rust.CallState
import com.pika.app.rust.CallStatus
import com.pika.app.rust.ChatMessage
import com.pika.app.rust.MessageDeliveryState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material.icons.filled.Info
import com.pika.app.rust.Screen
import com.pika.app.ui.theme.PikaBlue
import com.pika.app.ui.TestTags
import dev.jeziellago.compose.markdowntext.MarkdownText
import org.json.JSONObject

@Composable
@OptIn(ExperimentalMaterial3Api::class)
fun ChatScreen(manager: AppManager, chatId: String, padding: PaddingValues) {
    val chat = manager.state.currentChat
    if (chat == null || chat.chatId != chatId) {
        Box(modifier = Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
            Text("Loading chat…")
        }
        return
    }

    var draft by remember { mutableStateOf("") }
    val listState = rememberLazyListState()
    val newestMessageId = chat.messages.lastOrNull()?.id
    var shouldStickToBottom by remember(chat.chatId) { mutableStateOf(true) }
    var programmaticScrollInFlight by remember { mutableStateOf(false) }
    val isAtBottom by remember(listState) {
        derivedStateOf { listState.isNearBottomForReverseLayout() }
    }

    LaunchedEffect(chat.chatId) {
        shouldStickToBottom = true
        if (chat.messages.isNotEmpty()) {
            listState.scrollToItem(0)
        }
    }

    LaunchedEffect(isAtBottom, listState.isScrollInProgress, programmaticScrollInFlight) {
        if (isAtBottom) {
            shouldStickToBottom = true
        } else if (listState.isScrollInProgress && !programmaticScrollInFlight) {
            shouldStickToBottom = false
        }
    }

    LaunchedEffect(newestMessageId) {
        if (newestMessageId == null || !shouldStickToBottom) return@LaunchedEffect
        programmaticScrollInFlight = true
        try {
            listState.animateScrollToItem(0)
        } finally {
            programmaticScrollInFlight = false
        }
    }

    val myPubkey =
        when (val a = manager.state.auth) {
            is com.pika.app.rust.AuthState.LoggedIn -> a.pubkey
            else -> null
        }
    val title = chatTitle(chat, myPubkey)

    Scaffold(
        modifier = Modifier.padding(padding),
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        text = title,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                },
                navigationIcon = {
                    IconButton(
                        onClick = {
                            val stack = manager.state.router.screenStack
                            manager.dispatch(AppAction.UpdateScreenStack(stack.dropLast(1)))
                        },
                    ) {
                        Icon(Icons.Default.ArrowBack, contentDescription = "Back")
                    }
                },
                actions = {
                    if (chat.isGroup) {
                        IconButton(
                            onClick = {
                                manager.dispatch(AppAction.PushScreen(Screen.GroupInfo(chat.chatId)))
                            },
                        ) {
                            Icon(Icons.Default.Info, contentDescription = "Group info")
                        }
                    }
                },
            )
        },
    ) { inner ->
        Column(
            modifier =
                Modifier
                    .fillMaxSize()
                    .padding(inner)
                    .padding(top = 8.dp),
        ) {
            CallControls(
                manager = manager,
                chatId = chat.chatId,
            )

            LazyColumn(
                state = listState,
                modifier = Modifier.weight(1f).fillMaxWidth().testTag(TestTags.CHAT_MESSAGE_LIST),
                reverseLayout = true,
                contentPadding = PaddingValues(horizontal = 12.dp, vertical = 10.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                val reversed = chat.messages.asReversed()
                items(reversed, key = { it.id }) { msg ->
                    MessageBubble(
                        message = msg,
                        onSendMessage = { text ->
                            manager.dispatch(AppAction.SendMessage(chat.chatId, text))
                        },
                    )
                }
            }

            Row(
                modifier =
                    Modifier
                        .fillMaxWidth()
                        .navigationBarsPadding()
                        .imePadding()
                        .padding(start = 12.dp, top = 8.dp, end = 12.dp, bottom = 16.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                OutlinedTextField(
                    value = draft,
                    onValueChange = { draft = it },
                    modifier = Modifier.weight(1f).testTag(TestTags.CHAT_MESSAGE_INPUT),
                    placeholder = { Text("Message") },
                    singleLine = false,
                    maxLines = 4,
                )
                Spacer(Modifier.width(10.dp))
                Button(
                    onClick = {
                        val text = draft
                        draft = ""
                        manager.dispatch(AppAction.SendMessage(chat.chatId, text))
                    },
                    modifier = Modifier.testTag(TestTags.CHAT_SEND),
                ) {
                    Text("Send")
                }
            }
        }
    }
}

private fun LazyListState.isNearBottomForReverseLayout(tolerancePx: Int = 12): Boolean {
    if (firstVisibleItemIndex != 0) return false
    return firstVisibleItemScrollOffset <= tolerancePx
}

private fun chatTitle(chat: com.pika.app.rust.ChatViewState, selfPubkey: String?): String {
    if (chat.isGroup) {
        return chat.groupName?.trim().takeIf { !it.isNullOrBlank() } ?: "Group chat"
    }
    val peer =
        chat.members.firstOrNull { selfPubkey == null || it.pubkey != selfPubkey }
            ?: chat.members.firstOrNull()
    return peer?.name?.trim().takeIf { !it.isNullOrBlank() } ?: peer?.npub ?: "Chat"
}

// Parsed segment of a message: either markdown text or a pika-* custom block.
private sealed class MessageSegment {
    data class Markdown(val text: String) : MessageSegment()
    data class PikaPrompt(val title: String, val options: List<String>) : MessageSegment()
    data class PikaHtml(val html: String) : MessageSegment()
}

private fun parseMessageSegments(content: String): List<MessageSegment> {
    val segments = mutableListOf<MessageSegment>()
    val pattern = Regex("```pika-([\\w-]+)(?:[ \\t]+(\\S+))?\\n([\\s\\S]*?)```")
    var lastEnd = 0

    for (match in pattern.findAll(content)) {
        val before = content.substring(lastEnd, match.range.first)
        if (before.isNotBlank()) segments.add(MessageSegment.Markdown(before))

        val blockType = match.groupValues[1]
        val blockBody = match.groupValues[3].trim()

        when (blockType) {
            "prompt" -> {
                try {
                    val json = JSONObject(blockBody)
                    val title = json.getString("title")
                    val optionsArray = json.getJSONArray("options")
                    val options = (0 until optionsArray.length()).map { optionsArray.getString(it) }
                    segments.add(MessageSegment.PikaPrompt(title, options))
                } catch (_: Exception) {
                    segments.add(MessageSegment.Markdown("```$blockType\n$blockBody\n```"))
                }
            }
            "html" -> {
                segments.add(MessageSegment.PikaHtml(blockBody))
            }
            "html-update", "prompt-response" -> {
                // Consumed by Rust core; silently drop if one slips through.
            }
            else -> {
                segments.add(MessageSegment.Markdown("```$blockType\n$blockBody\n```"))
            }
        }

        lastEnd = match.range.last + 1
    }

    val tail = content.substring(lastEnd)
    if (tail.isNotBlank()) segments.add(MessageSegment.Markdown(tail))

    return segments
}

@Composable
private fun CallControls(manager: AppManager, chatId: String) {
    val ctx = LocalContext.current
    val activeCall = manager.state.activeCall
    val callForChat = if (activeCall?.chatId == chatId) activeCall else null
    val hasLiveCallElsewhere = activeCall?.let { it.chatId != chatId && isLiveCallStatus(it.status) } ?: false
    var pendingMicAction by remember { mutableStateOf<PendingMicAction?>(null) }
    val micPermissionLauncher =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            val action = pendingMicAction
            pendingMicAction = null
            if (granted && action != null) {
                dispatchMicAction(manager, chatId, action)
            } else if (!granted) {
                Toast.makeText(ctx, "Microphone permission is required for calls.", Toast.LENGTH_SHORT).show()
            }
        }

    val dispatchWithMicPermission: (PendingMicAction) -> Unit = { action ->
        val hasMic =
            ContextCompat.checkSelfPermission(ctx, Manifest.permission.RECORD_AUDIO) ==
                PackageManager.PERMISSION_GRANTED
        if (hasMic) {
            dispatchMicAction(manager, chatId, action)
        } else {
            pendingMicAction = action
            micPermissionLauncher.launch(Manifest.permission.RECORD_AUDIO)
        }
    }

    Column(modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp)) {
        if (callForChat != null) {
            Text(
                text = callStatusText(callForChat),
                style = MaterialTheme.typography.labelLarge,
            )
            callForChat.debug?.let { debug ->
                Text(
                    text = "tx ${debug.txFrames}  rx ${debug.rxFrames}  drop ${debug.rxDropped}",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Spacer(Modifier.height(6.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                when (callForChat.status) {
                    is CallStatus.Ringing -> {
                        Button(
                            onClick = { dispatchWithMicPermission(PendingMicAction.Accept) },
                            modifier = Modifier.testTag(TestTags.CHAT_CALL_ACCEPT),
                        ) {
                            Text("Accept")
                        }
                        Button(
                            onClick = { manager.dispatch(AppAction.RejectCall(chatId)) },
                            modifier = Modifier.testTag(TestTags.CHAT_CALL_REJECT),
                        ) {
                            Text("Reject")
                        }
                    }
                    is CallStatus.Offering, is CallStatus.Connecting, is CallStatus.Active -> {
                        Button(
                            onClick = { manager.dispatch(AppAction.ToggleMute) },
                            modifier = Modifier.testTag(TestTags.CHAT_CALL_MUTE),
                        ) {
                            Text(if (callForChat.isMuted) "Unmute" else "Mute")
                        }
                        Button(
                            onClick = { manager.dispatch(AppAction.EndCall) },
                            modifier = Modifier.testTag(TestTags.CHAT_CALL_END),
                        ) {
                            Text("End")
                        }
                    }
                    is CallStatus.Ended -> {
                        Button(
                            onClick = { dispatchWithMicPermission(PendingMicAction.Start) },
                            modifier = Modifier.testTag(TestTags.CHAT_CALL_START),
                        ) {
                            Text("Start Again")
                        }
                    }
                }
            }
        } else {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Button(
                    onClick = { dispatchWithMicPermission(PendingMicAction.Start) },
                    enabled = !hasLiveCallElsewhere,
                    modifier = Modifier.testTag(TestTags.CHAT_CALL_START),
                ) {
                    Text("Start Call")
                }
                if (hasLiveCallElsewhere) {
                    Text(
                        text = "Another call is active",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

private enum class PendingMicAction {
    Start,
    Accept,
}

private fun dispatchMicAction(manager: AppManager, chatId: String, action: PendingMicAction) {
    when (action) {
        PendingMicAction.Start -> manager.dispatch(AppAction.StartCall(chatId))
        PendingMicAction.Accept -> manager.dispatch(AppAction.AcceptCall(chatId))
    }
}

private fun callStatusText(call: CallState): String =
    when (val status = call.status) {
        is CallStatus.Offering -> "Calling…"
        is CallStatus.Ringing -> "Incoming call"
        is CallStatus.Connecting -> "Connecting…"
        is CallStatus.Active -> "Call active"
        is CallStatus.Ended -> "Call ended: ${status.reason}"
    }

private fun isLiveCallStatus(status: CallStatus): Boolean =
    when (status) {
        is CallStatus.Offering,
        is CallStatus.Ringing,
        is CallStatus.Connecting,
        is CallStatus.Active,
        -> true
        is CallStatus.Ended -> false
    }

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun MessageBubble(message: ChatMessage, onSendMessage: (String) -> Unit) {
    val isMine = message.isMine
    val bubbleColor = if (isMine) PikaBlue else MaterialTheme.colorScheme.surfaceVariant
    val textColor = if (isMine) Color.White else MaterialTheme.colorScheme.onSurfaceVariant
    val align = if (isMine) Alignment.End else Alignment.Start
    val segments = remember(message.displayContent) { parseMessageSegments(message.displayContent) }
    val clipboard = LocalClipboardManager.current
    val ctx = LocalContext.current

    Column(modifier = Modifier.fillMaxWidth(), horizontalAlignment = align) {
        for (segment in segments) {
            when (segment) {
                is MessageSegment.Markdown -> {
                    Row(verticalAlignment = Alignment.Bottom) {
                        Box(
                            modifier =
                                Modifier
                                    .clip(RoundedCornerShape(18.dp))
                                    .background(bubbleColor)
                                    .combinedClickable(
                                        onClick = {},
                                        onLongClick = {
                                            clipboard.setText(AnnotatedString(message.content))
                                            Toast.makeText(ctx, "Copied", Toast.LENGTH_SHORT).show()
                                        },
                                    )
                                    .padding(horizontal = 12.dp, vertical = 9.dp)
                                    .widthIn(max = 280.dp),
                        ) {
                            MarkdownText(
                                markdown = segment.text.trim(),
                                style = MaterialTheme.typography.bodyLarge.copy(color = textColor),
                                enableSoftBreakAddsNewLine = true,
                                afterSetMarkdown = { textView ->
                                    textView.includeFontPadding = false
                                },
                            )
                        }
                        if (isMine) {
                            Spacer(Modifier.width(6.dp))
                            Text(
                                text =
                                    when (message.delivery) {
                                        is MessageDeliveryState.Pending -> "…"
                                        is MessageDeliveryState.Sent -> "✓"
                                        is MessageDeliveryState.Failed -> "!"
                                    },
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                style = MaterialTheme.typography.labelSmall,
                            )
                        }
                    }
                }
                is MessageSegment.PikaPrompt -> {
                    PikaPromptCard(
                        title = segment.title,
                        options = segment.options,
                        message = message,
                        onSelect = onSendMessage,
                    )
                }
                is MessageSegment.PikaHtml -> {
                    Box(
                        modifier =
                            Modifier
                                .clip(RoundedCornerShape(16.dp))
                                .background(MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.5f))
                                .padding(12.dp)
                                .widthIn(max = 280.dp),
                    ) {
                        MarkdownText(
                            markdown = segment.html,
                            style = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurfaceVariant),
                            enableSoftBreakAddsNewLine = true,
                            afterSetMarkdown = { textView ->
                                textView.includeFontPadding = false
                            },
                        )
                    }
                }
            }
        }
        Spacer(Modifier.height(2.dp))
    }
}

@Composable
private fun PikaPromptCard(title: String, options: List<String>, message: ChatMessage, onSelect: (String) -> Unit) {
    val hasVoted = message.myPollVote != null
    Column(
        modifier =
            Modifier
                .clip(RoundedCornerShape(16.dp))
                .background(MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.5f))
                .padding(12.dp)
                .widthIn(max = 280.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text(
            text = title,
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.onSurface,
        )
        for (option in options) {
            val tally = message.pollTally.firstOrNull { it.option == option }
            val isMyVote = message.myPollVote == option
            TextButton(
                onClick = {
                    val response = "```pika-prompt-response\n{\"prompt_id\":\"${message.id}\",\"selected\":\"$option\"}\n```"
                    onSelect(response)
                },
                enabled = !hasVoted,
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(10.dp),
                colors =
                    ButtonDefaults.textButtonColors(
                        containerColor = if (isMyVote) PikaBlue.copy(alpha = 0.25f) else PikaBlue.copy(alpha = 0.1f),
                        contentColor = PikaBlue,
                        disabledContainerColor = if (isMyVote) PikaBlue.copy(alpha = 0.25f) else PikaBlue.copy(alpha = 0.1f),
                        disabledContentColor = PikaBlue.copy(alpha = 0.7f),
                    ),
            ) {
                Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    Text(option)
                    if (tally != null) {
                        Text("${tally.count}", style = MaterialTheme.typography.titleSmall)
                    }
                }
            }
            if (tally != null && tally.voterNames.isNotEmpty()) {
                Text(
                    text = tally.voterNames.joinToString(", "),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(start = 12.dp),
                )
            }
        }
    }
}
