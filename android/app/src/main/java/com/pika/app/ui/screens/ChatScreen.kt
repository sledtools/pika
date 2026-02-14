package com.pika.app.ui.screens

import android.Manifest
import android.content.pm.PackageManager
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
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
import com.pika.app.ui.theme.PikaBlue
import com.pika.app.ui.TestTags

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

    val title = chat.peerName ?: chat.peerNpub

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
            )
        },
    ) { inner ->
        Column(
            modifier = Modifier.fillMaxSize().padding(inner),
        ) {
            CallControls(
                manager = manager,
                chatId = chat.chatId,
            )

            LazyColumn(
                modifier = Modifier.weight(1f).fillMaxWidth().testTag(TestTags.CHAT_MESSAGE_LIST),
                reverseLayout = true,
                contentPadding = PaddingValues(horizontal = 12.dp, vertical = 10.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                val reversed = chat.messages.asReversed()
                items(reversed, key = { it.id }) { msg ->
                    MessageBubble(message = msg)
                }
            }

            Row(
                modifier = Modifier.fillMaxWidth().padding(10.dp),
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

@Composable
private fun MessageBubble(message: ChatMessage) {
    val isMine = message.isMine
    val bubbleColor = if (isMine) PikaBlue else MaterialTheme.colorScheme.surfaceVariant
    val textColor = if (isMine) Color.White else MaterialTheme.colorScheme.onSurfaceVariant
    val align = if (isMine) Alignment.End else Alignment.Start

    Column(modifier = Modifier.fillMaxWidth(), horizontalAlignment = align) {
        Row(verticalAlignment = Alignment.Bottom) {
            Box(
                modifier =
                    Modifier
                        .clip(RoundedCornerShape(18.dp))
                        .background(bubbleColor)
                        .padding(horizontal = 12.dp, vertical = 9.dp)
                        .widthIn(max = 280.dp),
            ) {
                Text(message.content, color = textColor, style = MaterialTheme.typography.bodyLarge)
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
        Spacer(Modifier.height(2.dp))
    }
}
