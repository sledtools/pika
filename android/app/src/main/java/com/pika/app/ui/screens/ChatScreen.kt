package com.pika.app.ui.screens

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Base64
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.calculateEndPadding
import androidx.compose.foundation.layout.calculateStartPadding
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Badge
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SmallFloatingActionButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.rust.ChatMediaAttachment
import com.pika.app.rust.ChatMediaKind
import com.pika.app.rust.ChatMessage
import com.pika.app.rust.MessageDeliveryState
import com.pika.app.rust.MessageSegment
import com.pika.app.rust.ReactionSummary
import com.pika.app.rust.TypingMember
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Reply
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.ArrowDownward
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.AttachFile
import androidx.compose.material.icons.filled.Call
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Done
import androidx.compose.material.icons.filled.InsertDriveFile
import androidx.compose.material.icons.filled.ErrorOutline
import androidx.compose.material.icons.filled.Group
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.PhotoLibrary
import androidx.compose.material.icons.filled.Schedule
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.core.content.ContextCompat
import kotlin.math.max
import kotlin.math.roundToInt
import com.pika.app.rust.Screen
import com.pika.app.rust.VoiceRecordingPhase
import com.pika.app.ui.Avatar
import com.pika.app.ui.TestTags
import dev.jeziellago.compose.markdowntext.MarkdownText
import coil.compose.AsyncImage
import kotlinx.coroutines.delay
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

// Represents an item in the chat timeline: either a message or the unread divider.
private sealed class ChatListItem {
    data class Message(val message: ChatMessage) : ChatListItem()
    object NewMessagesDivider : ChatListItem()
}

private enum class GroupedBubblePosition {
    Single,
    Top,
    Middle,
    Bottom,
}

private val QUICK_REACTIONS = listOf("❤️", "👍", "👎", "😂", "😮", "😢")

private data class MediaUploadPayload(
    val bytes: ByteArray,
    val mimeType: String,
    val filename: String,
)

private fun readMediaUploadPayload(ctx: Context, uri: Uri): MediaUploadPayload? {
    val bytes = ctx.contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: return null
    if (bytes.isEmpty()) return null
    val mimeType = ctx.contentResolver.getType(uri).orEmpty()
    val filename =
        ctx.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { cursor ->
                val idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (idx >= 0 && cursor.moveToFirst()) cursor.getString(idx) else null
            }
            ?.trim()
            .takeUnless { it.isNullOrEmpty() }
            ?: "attachment.bin"
    return MediaUploadPayload(bytes = bytes, mimeType = mimeType, filename = filename)
}

@Composable
@OptIn(ExperimentalMaterial3Api::class)
fun ChatScreen(
    manager: AppManager,
    chatId: String,
    padding: PaddingValues,
    onOpenCallSurface: (String) -> Unit,
) {
    val chat = manager.state.currentChat
    if (chat == null || chat.chatId != chatId) {
        Box(modifier = Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
            Text("Loading chat…")
        }
        return
    }

    val ctx = LocalContext.current
    var draft by remember { mutableStateOf("") }
    var replyDraft by remember(chat.chatId) { mutableStateOf<ChatMessage?>(null) }
    var showAttachmentSheet by remember(chat.chatId) { mutableStateOf(false) }
    var fullscreenImageAttachment by remember(chat.chatId) { mutableStateOf<ChatMediaAttachment?>(null) }
    val listState = rememberLazyListState()
    val coroutineScope = rememberCoroutineScope()
    val newestMessageId = chat.messages.lastOrNull()?.id
    var shouldStickToBottom by remember(chat.chatId) { mutableStateOf(true) }
    var programmaticScrollInFlight by remember { mutableStateOf(false) }
    val isAtBottom by remember(listState) {
        derivedStateOf { listState.isNearBottomForReverseLayout() }
    }

    val firstUnreadMessageId = chat.firstUnreadMessageId
    val oldestMessageId = chat.messages.firstOrNull()?.id
    var loadOlderInFlight by remember(chat.chatId) { mutableStateOf(false) }
    var requestedLoadOlderBeforeId by remember(chat.chatId) { mutableStateOf<String?>(null) }
    var lastTypingStartedAtMs by remember(chat.chatId) { mutableStateOf(0L) }

    // Track new messages arriving while the user is scrolled up.
    var newMessageCount by remember(chat.chatId) { mutableIntStateOf(0) }
    var prevMessageCount by remember(chat.chatId) { mutableIntStateOf(chat.messages.size) }
    var composerHeightPx by remember(chat.chatId) { mutableIntStateOf(0) }
    val density = LocalDensity.current
    val keyboardOrNavBottomInset =
        with(density) {
            max(
                WindowInsets.navigationBars.getBottom(this),
                WindowInsets.ime.getBottom(this),
            ).toDp()
        }
    val floatingComposerInset =
        with(density) { composerHeightPx.toDp() }
            .coerceAtLeast(52.dp) + keyboardOrNavBottomInset + 8.dp

    val myPubkey =
        when (val a = manager.state.auth) {
            is com.pika.app.rust.AuthState.LoggedIn -> a.pubkey
            else -> null
        }
    val typingMembers =
        remember(chat.typingMembers, myPubkey) {
            chat.typingMembers.filter { it.pubkey != myPubkey }
        }
    val activeVoiceRecording = manager.state.voiceRecording
    val title = chatTitle(chat, myPubkey)
    val peer =
        if (!chat.isGroup) {
            chat.members.firstOrNull { it.pubkey != myPubkey } ?: chat.members.firstOrNull()
        } else {
            null
        }
    val activeCall = manager.state.activeCall
    val callForChat = activeCall?.takeIf { it.chatId == chat.chatId }
    val hasLiveCallElsewhere = activeCall?.let { it.chatId != chat.chatId && it.isLive } ?: false
    val isCallActionDisabled = callForChat == null && hasLiveCallElsewhere
    val messagesById = remember(chat.messages) { chat.messages.associateBy { it.id } }
    val reversed = remember(chat.messages) { chat.messages.asReversed() }
    val reversedIndexById =
        remember(reversed) { reversed.mapIndexed { index, message -> message.id to index }.toMap() }
    val unreadDividerIndex = remember(reversed, firstUnreadMessageId) {
        val firstUnreadIndex =
            firstUnreadMessageId?.let { id ->
                reversed.indexOfFirst { it.id == id }.takeIf { it >= 0 }
            } ?: return@remember null
        if (firstUnreadIndex in 0 until reversed.lastIndex) firstUnreadIndex + 1 else null
    }
    val bubblePositionByMessageId = remember(chat.messages, firstUnreadMessageId) {
        buildBubblePositions(chat.messages, firstUnreadMessageId)
    }

    fun sendDraftMessage() {
        val text = draft.trim()
        if (text.isBlank()) return
        draft = ""
        manager.dispatch(
            AppAction.SendMessage(chat.chatId, text, null, replyDraft?.id),
        )
        replyDraft = null
    }

    fun sendMediaFromUri(uri: Uri?) {
        if (uri == null) return
        coroutineScope.launch {
            val payload = withContext(Dispatchers.IO) { readMediaUploadPayload(ctx, uri) }
            if (payload == null) {
                Toast.makeText(ctx, "Unable to read selected file", Toast.LENGTH_SHORT).show()
                return@launch
            }
            val base64 = Base64.encodeToString(payload.bytes, Base64.NO_WRAP)
            val caption = draft.trim()
            manager.dispatch(
                AppAction.SendChatMedia(
                    chatId = chat.chatId,
                    dataBase64 = base64,
                    mimeType = payload.mimeType,
                    filename = payload.filename,
                    caption = caption,
                ),
            )
            if (caption.isNotEmpty()) {
                draft = ""
            }
            replyDraft = null
        }
    }

    val pickPhotoOrVideoLauncher =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            sendMediaFromUri(uri)
        }

    val pickFileLauncher =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            sendMediaFromUri(uri)
        }

    val voiceRecorder = remember(chat.chatId) { AndroidVoiceRecorder(ctx.applicationContext) }
    val onVoiceLevel: (Float) -> Unit = { level ->
        manager.dispatch(AppAction.VoiceRecordingAudioLevel(level))
    }
    val onVoiceTranscript: (String) -> Unit = { transcript ->
        manager.dispatch(AppAction.VoiceRecordingTranscript(transcript))
    }
    DisposableEffect(chat.chatId) {
        onDispose {
            if (manager.state.voiceRecording != null) {
                manager.dispatch(AppAction.VoiceRecordingCancel)
            }
            voiceRecorder.release()
        }
    }

    fun startVoiceRecordingInternal() {
        val started = voiceRecorder.start(onLevel = onVoiceLevel, onTranscript = onVoiceTranscript)
        if (started) {
            manager.dispatch(AppAction.VoiceRecordingStart)
        } else {
            Toast.makeText(ctx, "Unable to start voice recording", Toast.LENGTH_SHORT).show()
        }
    }

    val micPermissionLauncher =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            if (granted) {
                startVoiceRecordingInternal()
            } else {
                Toast.makeText(ctx, "Microphone permission is required", Toast.LENGTH_SHORT).show()
            }
        }

    fun startVoiceRecording() {
        if (activeVoiceRecording != null) return
        val hasMicPermission =
            ContextCompat.checkSelfPermission(ctx, Manifest.permission.RECORD_AUDIO) ==
                PackageManager.PERMISSION_GRANTED
        if (hasMicPermission) {
            startVoiceRecordingInternal()
        } else {
            micPermissionLauncher.launch(Manifest.permission.RECORD_AUDIO)
        }
    }

    fun cancelVoiceRecording() {
        voiceRecorder.cancel()
        manager.dispatch(AppAction.VoiceRecordingCancel)
    }

    fun toggleVoiceRecordingPause(phase: VoiceRecordingPhase) {
        when (phase) {
            VoiceRecordingPhase.PAUSED -> {
                val resumed =
                    voiceRecorder.resume(
                        onLevel = onVoiceLevel,
                        onTranscript = onVoiceTranscript,
                    )
                if (resumed) {
                    manager.dispatch(AppAction.VoiceRecordingResume)
                }
            }
            VoiceRecordingPhase.RECORDING -> {
                if (voiceRecorder.pause()) {
                    manager.dispatch(AppAction.VoiceRecordingPause)
                }
            }
            VoiceRecordingPhase.IDLE, VoiceRecordingPhase.DONE -> Unit
        }
    }

    fun sendVoiceRecording(recording: com.pika.app.rust.VoiceRecordingState) {
        manager.dispatch(AppAction.VoiceRecordingStop)
        coroutineScope.launch {
            val file = withContext(Dispatchers.IO) { voiceRecorder.stop() }
            if (file == null) {
                manager.dispatch(AppAction.VoiceRecordingCancel)
                Toast.makeText(ctx, "Unable to finalize voice recording", Toast.LENGTH_SHORT).show()
                return@launch
            }
            val payload = withContext(Dispatchers.IO) { runCatching { file.readBytes() }.getOrNull() }
            withContext(Dispatchers.IO) { file.delete() }
            if (payload == null || payload.isEmpty()) {
                manager.dispatch(AppAction.VoiceRecordingCancel)
                Toast.makeText(ctx, "Voice recording was empty", Toast.LENGTH_SHORT).show()
                return@launch
            }
            val timestamp = System.currentTimeMillis() / 1000L
            val transcript = recording.transcript.trim()
            val caption = if (transcript.isEmpty()) "" else "*$transcript*"
            manager.dispatch(
                AppAction.SendChatMedia(
                    chatId = chat.chatId,
                    dataBase64 = Base64.encodeToString(payload, Base64.NO_WRAP),
                    mimeType = "audio/mp4",
                    filename = "voice_${timestamp}.m4a",
                    caption = caption,
                ),
            )
            manager.dispatch(AppAction.VoiceRecordingCancel)
            replyDraft = null
        }
    }

    fun maybeNotifyTyping(text: String) {
        if (text.isBlank()) return
        val now = System.currentTimeMillis()
        if (now - lastTypingStartedAtMs < 3_000L) return
        lastTypingStartedAtMs = now
        manager.dispatch(AppAction.TypingStarted(chat.chatId))
    }

    // Build the display list, inserting the "NEW MESSAGES" divider between read and unread.
    val listItems: List<ChatListItem> = remember(reversed, unreadDividerIndex) {
        buildList {
            for ((i, msg) in reversed.withIndex()) {
                if (unreadDividerIndex != null && i == unreadDividerIndex) {
                    add(ChatListItem.NewMessagesDivider)
                }
                add(ChatListItem.Message(msg))
            }
        }
    }

    // On chat open: if there are unreads, scroll to the divider; otherwise scroll to newest.
    LaunchedEffect(chat.chatId) {
        if (unreadDividerIndex != null && listItems.isNotEmpty()) {
            shouldStickToBottom = false
            listState.scrollToItem(unreadDividerIndex)
        } else if (chat.messages.isNotEmpty()) {
            listState.scrollToItem(0)
        }
        replyDraft = null
    }

    LaunchedEffect(isAtBottom, listState.isScrollInProgress, programmaticScrollInFlight) {
        if (isAtBottom) {
            shouldStickToBottom = true
            newMessageCount = 0
        } else if (listState.isScrollInProgress && !programmaticScrollInFlight) {
            shouldStickToBottom = false
        }
    }

    // Scroll to newest when a new message arrives and we're stuck to the bottom.
    LaunchedEffect(newestMessageId) {
        if (newestMessageId == null || !shouldStickToBottom) return@LaunchedEffect
        programmaticScrollInFlight = true
        try {
            listState.animateScrollToItem(0)
        } finally {
            programmaticScrollInFlight = false
        }
    }

    // Track new messages arriving while scrolled up, for the badge.
    LaunchedEffect(reversed.size) {
        val current = reversed.size
        if (current > prevMessageCount && !shouldStickToBottom) {
            newMessageCount += current - prevMessageCount
        }
        prevMessageCount = current
    }

    LaunchedEffect(chat.chatId, oldestMessageId, chat.canLoadOlder, loadOlderInFlight) {
        if (!loadOlderInFlight) return@LaunchedEffect
        if (!chat.canLoadOlder || oldestMessageId != requestedLoadOlderBeforeId) {
            loadOlderInFlight = false
        }
    }

    LaunchedEffect(chat.chatId, loadOlderInFlight, oldestMessageId, requestedLoadOlderBeforeId) {
        if (!loadOlderInFlight) return@LaunchedEffect
        delay(4_000)
        if (loadOlderInFlight && oldestMessageId == requestedLoadOlderBeforeId) {
            loadOlderInFlight = false
        }
    }

    // When the keyboard appears, scroll to the newest message so it stays visible above the input.
    val imeBottom = WindowInsets.ime.getBottom(density)
    val composerBottomPadding = if (imeBottom > 0) 6.dp else 4.dp
    LaunchedEffect(imeBottom) {
        if (imeBottom > 0 && shouldStickToBottom) {
            coroutineScope.launch {
                listState.animateScrollToItem(0)
            }
        }
    }

    val layoutDirection = LocalLayoutDirection.current
    val chatScaffoldPadding =
        PaddingValues(
            start = padding.calculateStartPadding(layoutDirection),
            top = padding.calculateTopPadding(),
            end = padding.calculateEndPadding(layoutDirection),
            bottom = 0.dp,
        )

    Scaffold(
        modifier = Modifier.padding(chatScaffoldPadding),
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = {
            TopAppBar(
                windowInsets = WindowInsets(0, 0, 0, 0),
                colors =
                    TopAppBarDefaults.topAppBarColors(
                        containerColor = MaterialTheme.colorScheme.surface,
                    ),
                title = {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        if (chat.isGroup || peer == null) {
                            Box(
                                modifier =
                                    Modifier
                                        .size(30.dp)
                                        .clip(MaterialTheme.shapes.small)
                                        .background(MaterialTheme.colorScheme.secondaryContainer),
                                contentAlignment = Alignment.Center,
                            ) {
                                Icon(
                                    imageVector = Icons.Default.Group,
                                    contentDescription = null,
                                    tint = MaterialTheme.colorScheme.onSecondaryContainer,
                                    modifier = Modifier.size(18.dp),
                                )
                            }
                        } else {
                            Avatar(
                                name = peer.name,
                                npub = peer.npub,
                                pictureUrl = peer.pictureUrl,
                                size = 30.dp,
                            )
                        }
                        Text(
                            text = title,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                },
                navigationIcon = {
                    IconButton(
                        onClick = {
                            val stack = manager.state.router.screenStack
                            manager.dispatch(AppAction.UpdateScreenStack(stack.dropLast(1)))
                        },
                    ) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                actions = {
                    IconButton(
                        onClick = { onOpenCallSurface(chat.chatId) },
                        enabled = !isCallActionDisabled,
                        modifier =
                            Modifier.testTag(
                                if (callForChat?.isLive == true) {
                                    TestTags.CHAT_CALL_OPEN
                                } else {
                                    TestTags.CHAT_CALL_START
                                },
                            ),
                    ) {
                        Icon(Icons.Default.Call, contentDescription = "Call")
                    }

                    if (chat.isGroup) {
                        IconButton(
                            onClick = {
                                manager.dispatch(AppAction.PushScreen(Screen.GroupInfo(chat.chatId)))
                            },
                        ) {
                            Icon(Icons.Default.Info, contentDescription = "Group info")
                        }
                    } else {
                        if (peer != null) {
                            IconButton(
                                onClick = {
                                    manager.dispatch(AppAction.OpenPeerProfile(peer.pubkey))
                                },
                            ) {
                                Icon(Icons.Default.Info, contentDescription = "Contact info")
                            }
                        }
                    }
                },
            )
        },
    ) { inner ->
        Box(
            modifier =
                Modifier
                    .fillMaxSize()
                    .padding(inner),
        ) {
            LazyColumn(
                state = listState,
                modifier = Modifier.fillMaxSize().testTag(TestTags.CHAT_MESSAGE_LIST),
                reverseLayout = true,
                contentPadding =
                    PaddingValues(
                        start = 12.dp,
                        top = 10.dp,
                        end = 12.dp,
                        bottom = floatingComposerInset,
                    ),
                verticalArrangement = Arrangement.spacedBy(3.dp),
            ) {
                items(
                    items = listItems,
                    key = { item ->
                        when (item) {
                            is ChatListItem.Message -> item.message.id
                            is ChatListItem.NewMessagesDivider -> "new-messages-divider"
                        }
                    },
                ) { item ->
                    when (item) {
                        is ChatListItem.Message -> {
                            val msg = item.message
                            MessageBubble(
                                message = msg,
                                position =
                                    bubblePositionByMessageId[msg.id]
                                        ?: GroupedBubblePosition.Single,
                                showSender = chat.isGroup,
                                messagesById = messagesById,
                                onReplyTo = { replyMessage ->
                                    replyDraft = replyMessage
                                },
                                onJumpToMessage = { targetId ->
                                    val index = reversedIndexById[targetId] ?: return@MessageBubble
                                    coroutineScope.launch {
                                        listState.animateScrollToItem(index)
                                    }
                                },
                                onRetryMessage = { messageId ->
                                    manager.dispatch(AppAction.RetryMessage(chat.chatId, messageId))
                                },
                                onReact = { messageId, emoji ->
                                    manager.dispatch(AppAction.ReactToMessage(chat.chatId, messageId, emoji))
                                },
                                onDownloadMedia = { messageId, originalHashHex ->
                                    manager.dispatch(
                                        AppAction.DownloadChatMedia(
                                            chatId = chat.chatId,
                                            messageId = messageId,
                                            originalHashHex = originalHashHex,
                                        ),
                                    )
                                },
                                onOpenImage = { attachment ->
                                    fullscreenImageAttachment = attachment
                                },
                                onHypernoteAction = { messageId, actionName, form ->
                                    manager.dispatch(
                                        AppAction.HypernoteAction(
                                            chatId = chat.chatId,
                                            messageId = messageId,
                                            actionName = actionName,
                                            form = form,
                                        ),
                                    )
                                },
                            )
                        }
                        is ChatListItem.NewMessagesDivider -> {
                            NewMessagesDividerRow()
                        }
                    }
                }

                item(key = "load-older-trigger") {
                    if (loadOlderInFlight) {
                        Row(
                            modifier = Modifier.fillMaxWidth().padding(vertical = 12.dp),
                            horizontalArrangement = Arrangement.Center,
                        ) {
                            CircularProgressIndicator(
                                modifier = Modifier.size(18.dp),
                                strokeWidth = 2.dp,
                            )
                        }
                    }
                    LaunchedEffect(chat.chatId, chat.canLoadOlder, oldestMessageId, loadOlderInFlight) {
                        if (!chat.canLoadOlder || loadOlderInFlight) return@LaunchedEffect
                        val beforeMessageId = oldestMessageId ?: return@LaunchedEffect
                        if (requestedLoadOlderBeforeId == beforeMessageId) return@LaunchedEffect
                        requestedLoadOlderBeforeId = beforeMessageId
                        loadOlderInFlight = true
                        manager.dispatch(
                            AppAction.LoadOlderMessages(
                                chatId = chat.chatId,
                                beforeMessageId = beforeMessageId,
                                limit = 30u,
                            ),
                        )
                    }
                }
            }

            // Scroll-to-bottom button with new message count badge.
            if (!isAtBottom) {
                Column(
                    modifier =
                        Modifier
                            .align(Alignment.BottomEnd)
                            .padding(end = 16.dp, bottom = floatingComposerInset + 4.dp),
                    horizontalAlignment = Alignment.End,
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    if (newMessageCount > 0) {
                        Badge(
                            containerColor = MaterialTheme.colorScheme.primary,
                        ) {
                            Text(
                                text = "$newMessageCount new",
                                style = MaterialTheme.typography.labelSmall,
                            )
                        }
                    }
                    SmallFloatingActionButton(
                        onClick = {
                            shouldStickToBottom = true
                            newMessageCount = 0
                            coroutineScope.launch { listState.animateScrollToItem(0) }
                        },
                        containerColor = MaterialTheme.colorScheme.surfaceVariant,
                        contentColor = MaterialTheme.colorScheme.onSurfaceVariant,
                    ) {
                        Icon(Icons.Default.ArrowDownward, contentDescription = "Scroll to bottom")
                    }
                }
            }

            Column(
                modifier =
                    Modifier
                        .align(Alignment.BottomCenter)
                        .fillMaxWidth()
                        .navigationBarsPadding()
                        .imePadding()
                        .padding(
                            start = 10.dp,
                            top = 6.dp,
                            end = 10.dp,
                            bottom = composerBottomPadding,
                        )
                        .onSizeChanged { composerHeightPx = it.height },
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                if (typingMembers.isNotEmpty()) {
                    TypingIndicatorRow(
                        typingMembers = typingMembers,
                        modifier = Modifier.padding(horizontal = 6.dp),
                    )
                }

                if (activeVoiceRecording == null) {
                    replyDraft?.let { replying ->
                        ReplyComposerPreview(
                            message = replying,
                            onClear = { replyDraft = null },
                        )
                    }

                    Surface(
                        shape = MaterialTheme.shapes.large,
                        color = MaterialTheme.colorScheme.surfaceContainerHigh,
                        tonalElevation = 1.dp,
                        shadowElevation = 6.dp,
                    ) {
                        Row(
                            modifier =
                                Modifier
                                    .fillMaxWidth()
                                    .padding(horizontal = 12.dp, vertical = 8.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Box {
                                IconButton(
                                    onClick = { showAttachmentSheet = true },
                                    modifier = Modifier.size(40.dp),
                                ) {
                                    Icon(
                                        imageVector = Icons.Default.Add,
                                        contentDescription = "Attach",
                                    )
                                }
                            }
                            Spacer(Modifier.width(4.dp))
                            BasicTextField(
                                value = draft,
                                onValueChange = {
                                    draft = it
                                    maybeNotifyTyping(it.trim())
                                },
                                modifier =
                                    Modifier
                                        .weight(1f)
                                        .testTag(TestTags.CHAT_MESSAGE_INPUT)
                                        .onPreviewKeyEvent { keyEvent ->
                                            if (keyEvent.type == KeyEventType.KeyUp &&
                                                (keyEvent.key == Key.Enter || keyEvent.key == Key.NumPadEnter)
                                            ) {
                                                sendDraftMessage()
                                                true
                                            } else {
                                                false
                                            }
                                        },
                                textStyle =
                                    MaterialTheme.typography.bodyLarge.copy(
                                        color = MaterialTheme.colorScheme.onSurface,
                                    ),
                                cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                                singleLine = true,
                                keyboardOptions =
                                    KeyboardOptions(
                                        keyboardType = KeyboardType.Text,
                                        imeAction = ImeAction.Send,
                                    ),
                                keyboardActions =
                                    KeyboardActions(
                                        onSend = { sendDraftMessage() },
                                    ),
                                decorationBox = { innerTextField ->
                                    if (draft.isBlank()) {
                                        Text(
                                            text = "Message",
                                            style = MaterialTheme.typography.bodyLarge,
                                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                                        )
                                    }
                                    innerTextField()
                                },
                            )
                            Spacer(Modifier.width(8.dp))
                            if (draft.trim().isNotBlank()) {
                                FilledIconButton(
                                    onClick = { sendDraftMessage() },
                                    enabled = draft.isNotBlank(),
                                    modifier = Modifier.size(40.dp).testTag(TestTags.CHAT_SEND),
                                ) {
                                    Icon(
                                        imageVector = Icons.AutoMirrored.Filled.Send,
                                        contentDescription = "Send",
                                    )
                                }
                            } else {
                                IconButton(
                                    onClick = { startVoiceRecording() },
                                    modifier = Modifier.size(40.dp),
                                ) {
                                    Icon(
                                        imageVector = Icons.Default.Mic,
                                        contentDescription = "Record voice message",
                                    )
                                }
                            }
                        }
                    }
                } else {
                    Surface(
                        shape = MaterialTheme.shapes.large,
                        color = MaterialTheme.colorScheme.surfaceContainerHigh,
                        tonalElevation = 1.dp,
                        shadowElevation = 6.dp,
                    ) {
                        VoiceRecordingComposer(
                            recording = activeVoiceRecording,
                            onSend = { sendVoiceRecording(activeVoiceRecording) },
                            onCancel = { cancelVoiceRecording() },
                            onTogglePause = { toggleVoiceRecordingPause(activeVoiceRecording.phase) },
                        )
                    }
                }
            }
        }
    }

    fullscreenImageAttachment?.let { attachment ->
        FullscreenImageViewer(
            attachment = attachment,
            onDismiss = { fullscreenImageAttachment = null },
        )
    }

    if (showAttachmentSheet) {
        val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
        ModalBottomSheet(
            onDismissRequest = { showAttachmentSheet = false },
            sheetState = sheetState,
        ) {
            ListItem(
                headlineContent = { Text("Photos & Videos") },
                leadingContent = {
                    Icon(
                        imageVector = Icons.Default.PhotoLibrary,
                        contentDescription = null,
                    )
                },
                modifier = Modifier.clickable {
                    showAttachmentSheet = false
                    pickPhotoOrVideoLauncher.launch(arrayOf("image/*", "video/*"))
                },
            )
            ListItem(
                headlineContent = { Text("File") },
                leadingContent = {
                    Icon(
                        imageVector = Icons.Default.AttachFile,
                        contentDescription = null,
                    )
                },
                modifier = Modifier
                    .clickable {
                        showAttachmentSheet = false
                        pickFileLauncher.launch(arrayOf("*/*"))
                    }
                    .padding(bottom = 16.dp),
            )
        }
    }
}

private fun LazyListState.isNearBottomForReverseLayout(tolerancePx: Int = 100): Boolean {
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

private fun buildBubblePositions(
    messages: List<ChatMessage>,
    firstUnreadMessageId: String?,
): Map<String, GroupedBubblePosition> {
    if (messages.isEmpty()) return emptyMap()

    val positions = HashMap<String, GroupedBubblePosition>(messages.size)
    for (index in messages.indices) {
        val current = messages[index]
        val previous = messages.getOrNull(index - 1)
        val next = messages.getOrNull(index + 1)

        val dividerBeforeCurrent = firstUnreadMessageId != null && current.id == firstUnreadMessageId
        val dividerBeforeNext = firstUnreadMessageId != null && next?.id == firstUnreadMessageId

        val samePreviousSender =
            !dividerBeforeCurrent &&
                previous != null &&
                previous.senderPubkey == current.senderPubkey &&
                previous.isMine == current.isMine
        val sameNextSender =
            !dividerBeforeNext &&
                next != null &&
                next.senderPubkey == current.senderPubkey &&
                next.isMine == current.isMine

        positions[current.id] =
            when {
                !samePreviousSender && !sameNextSender -> GroupedBubblePosition.Single
                !samePreviousSender && sameNextSender -> GroupedBubblePosition.Top
                samePreviousSender && sameNextSender -> GroupedBubblePosition.Middle
                else -> GroupedBubblePosition.Bottom
            }
    }

    return positions
}

private fun messageBubbleShape(position: GroupedBubblePosition, isMine: Boolean): RoundedCornerShape {
    val large = 18.dp
    val grouped = 6.dp
    return when (position) {
        GroupedBubblePosition.Single -> RoundedCornerShape(large)
        GroupedBubblePosition.Top ->
            if (isMine) {
                RoundedCornerShape(topStart = large, topEnd = large, bottomStart = large, bottomEnd = grouped)
            } else {
                RoundedCornerShape(topStart = large, topEnd = large, bottomStart = grouped, bottomEnd = large)
            }
        GroupedBubblePosition.Middle ->
            if (isMine) {
                RoundedCornerShape(topStart = large, topEnd = grouped, bottomStart = large, bottomEnd = grouped)
            } else {
                RoundedCornerShape(topStart = grouped, topEnd = large, bottomStart = grouped, bottomEnd = large)
            }
        GroupedBubblePosition.Bottom ->
            if (isMine) {
                RoundedCornerShape(topStart = large, topEnd = grouped, bottomStart = large, bottomEnd = large)
            } else {
                RoundedCornerShape(topStart = grouped, topEnd = large, bottomStart = large, bottomEnd = large)
            }
    }
}

@Composable
private fun DeliveryStateIcon(
    delivery: MessageDeliveryState,
    tint: androidx.compose.ui.graphics.Color,
    onClick: (() -> Unit)? = null,
) {
    val iconModifier =
        Modifier
            .size(13.dp)
            .let { modifier ->
                if (onClick != null) {
                    modifier.clickable { onClick() }
                } else {
                    modifier
                }
            }
    when (delivery) {
        is MessageDeliveryState.Pending ->
            Icon(
                imageVector = Icons.Default.Schedule,
                contentDescription = "Pending",
                tint = tint,
                modifier = iconModifier,
            )
        is MessageDeliveryState.Sent ->
            Icon(
                imageVector = Icons.Default.Done,
                contentDescription = "Sent",
                tint = tint,
                modifier = iconModifier,
            )
        is MessageDeliveryState.Failed ->
            Icon(
                imageVector = Icons.Default.ErrorOutline,
                contentDescription = "Failed",
                tint = MaterialTheme.colorScheme.error,
                modifier = iconModifier,
            )
    }
}

@Composable
private fun NewMessagesDividerRow() {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 10.dp, horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        HorizontalDivider(
            modifier = Modifier.weight(1f),
            color = MaterialTheme.colorScheme.primary.copy(alpha = 0.35f),
        )
        Text(
            text = "NEW MESSAGES",
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.primary.copy(alpha = 0.8f),
        )
        HorizontalDivider(
            modifier = Modifier.weight(1f),
            color = MaterialTheme.colorScheme.primary.copy(alpha = 0.35f),
        )
    }
}

@Composable
private fun TypingIndicatorRow(
    typingMembers: List<TypingMember>,
    modifier: Modifier = Modifier,
) {
    val dotProgress =
        rememberInfiniteTransition(label = "typingDots").animateFloat(
            initialValue = 0f,
            targetValue = 1f,
            animationSpec =
                infiniteRepeatable(
                    animation = tween(durationMillis = 900, easing = LinearEasing),
                    repeatMode = RepeatMode.Restart,
                ),
            label = "typingDotsProgress",
        )
    val dotCount = 1 + ((dotProgress.value * 3f).toInt() % 3)
    val dots = ".".repeat(dotCount)
    val label = typingLabel(typingMembers)

    Surface(
        modifier = modifier,
        shape = MaterialTheme.shapes.medium,
        color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.72f),
        tonalElevation = 1.dp,
    ) {
        Text(
            text = "$label$dots",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(horizontal = 10.dp, vertical = 6.dp),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

private fun typingLabel(typingMembers: List<TypingMember>): String {
    val names =
        typingMembers.mapNotNull { member ->
            member.name?.trim()?.takeIf { it.isNotEmpty() } ?: member.pubkey.take(8)
        }
    if (names.isEmpty()) return "Someone is typing"
    return when (names.size) {
        1 -> "${names[0]} is typing"
        2 -> "${names[0]} and ${names[1]} are typing"
        else -> "${names[0]} and ${names.size - 1} others are typing"
    }
}

@Composable
private fun ReactionChipsRow(
    isMine: Boolean,
    reactions: List<ReactionSummary>,
    onToggleReaction: (String) -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(top = 1.dp, bottom = 3.dp),
        horizontalArrangement = if (isMine) Arrangement.End else Arrangement.Start,
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            for (reaction in reactions) {
                val chipColor =
                    if (reaction.reactedByMe) {
                        MaterialTheme.colorScheme.primary
                    } else {
                        MaterialTheme.colorScheme.surfaceContainerHigh
                    }
                val contentColor =
                    if (reaction.reactedByMe) {
                        MaterialTheme.colorScheme.onPrimary
                    } else {
                        MaterialTheme.colorScheme.onSurface
                    }
                Row(
                    modifier =
                        Modifier
                            .clip(MaterialTheme.shapes.small)
                            .background(chipColor)
                            .clickable { onToggleReaction(reaction.emoji) }
                            .padding(horizontal = 8.dp, vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    Text(
                        text = reaction.emoji,
                        style = MaterialTheme.typography.labelMedium,
                    )
                    if (reaction.count > 1u) {
                        Text(
                            text = reaction.count.toString(),
                            style = MaterialTheme.typography.labelSmall,
                            color = contentColor,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun MediaAttachmentContent(
    attachment: ChatMediaAttachment,
    isMine: Boolean,
    onDownload: () -> Unit,
    onOpenImage: () -> Unit,
) {
    val hasLocalFile = attachment.localPath?.let { File(it).exists() } == true
    val isImage = attachment.kind == ChatMediaKind.IMAGE
    val isAudio = attachment.kind == ChatMediaKind.VOICE_NOTE
    val containerColor =
        when {
            isAudio && isMine -> MaterialTheme.colorScheme.primary
            isAudio -> MaterialTheme.colorScheme.surfaceContainerHighest
            isMine -> MaterialTheme.colorScheme.primary.copy(alpha = 0.25f)
            else -> MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.7f)
        }
    val primaryContentColor =
        if (isAudio && isMine) {
            MaterialTheme.colorScheme.onPrimary
        } else {
            MaterialTheme.colorScheme.onSurface
        }
    val secondaryContentColor =
        if (isAudio && isMine) {
            MaterialTheme.colorScheme.onPrimary.copy(alpha = 0.72f)
        } else {
            MaterialTheme.colorScheme.onSurfaceVariant
        }

    if (isImage) {
        Surface(
            modifier = Modifier.widthIn(max = 280.dp),
            shape = MaterialTheme.shapes.medium,
            color = containerColor,
        ) {
            if (hasLocalFile) {
                val imageModel = attachment.localPath?.let { File(it) }
                Box(
                    modifier =
                        Modifier
                            .fillMaxWidth()
                            .then(
                                if (attachment.width != null && attachment.height != null &&
                                    attachment.width!! > 0u && attachment.height!! > 0u
                                ) {
                                    Modifier.aspectRatio(attachment.width!!.toFloat() / attachment.height!!.toFloat())
                                } else {
                                    Modifier.heightIn(min = 120.dp, max = 260.dp)
                                },
                            )
                            .clickable { onOpenImage() },
                ) {
                    AsyncImage(
                        model = imageModel,
                        contentDescription = attachment.filename,
                        modifier = Modifier.fillMaxSize(),
                        contentScale = ContentScale.Crop,
                    )
                }
            } else {
                Row(
                    modifier =
                        Modifier
                            .fillMaxWidth()
                            .heightIn(min = 84.dp)
                            .padding(horizontal = 12.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Icon(
                        imageVector = Icons.Default.Download,
                        contentDescription = null,
                        tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Text(
                        text = attachment.filename.ifBlank { "Image" },
                        modifier = Modifier.weight(1f),
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    TextButton(onClick = onDownload) {
                        Text("Download")
                    }
                }
            }
        }
        return
    }

    if (isAudio) {
        Surface(
            modifier = Modifier.widthIn(max = 280.dp),
            shape = MaterialTheme.shapes.medium,
            color = containerColor,
        ) {
            if (hasLocalFile) {
                VoiceAttachmentPlayerRow(
                    localPath = attachment.localPath ?: "",
                    isMine = isMine,
                    modifier = Modifier.padding(horizontal = 10.dp, vertical = 8.dp),
                )
            } else {
                Row(
                    modifier =
                        Modifier
                            .fillMaxWidth()
                            .padding(horizontal = 12.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Icon(
                        imageVector = Icons.Default.Mic,
                        contentDescription = null,
                        tint = secondaryContentColor,
                    )
                    Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(
                            text = "Voice message",
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            style = MaterialTheme.typography.bodyMedium,
                            color = primaryContentColor,
                        )
                        Text(
                            text = attachment.mimeType.ifBlank { "audio/mp4" },
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            style = MaterialTheme.typography.bodySmall,
                            color = secondaryContentColor,
                        )
                    }
                    TextButton(
                        onClick = onDownload,
                        colors = ButtonDefaults.textButtonColors(contentColor = primaryContentColor),
                    ) {
                        Text("Download")
                    }
                }
            }
        }
        return
    }

    Surface(
        modifier = Modifier.widthIn(max = 280.dp),
        shape = MaterialTheme.shapes.medium,
        color = containerColor,
    ) {
        Row(
            modifier =
                Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Icon(
                imageVector = Icons.Default.InsertDriveFile,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    text = attachment.filename.ifBlank { "Attachment" },
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurface,
                )
                if (attachment.mimeType.isNotBlank()) {
                    Text(
                        text = attachment.mimeType,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            if (hasLocalFile) {
                Text(
                    text = "Saved",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                TextButton(onClick = onDownload) {
                    Text("Download")
                }
            }
        }
    }
}

@Composable
private fun FullscreenImageViewer(
    attachment: ChatMediaAttachment,
    onDismiss: () -> Unit,
) {
    val imageModel =
        attachment.localPath
            ?.takeIf { File(it).exists() }
            ?.let { File(it) }
            ?: attachment.url.takeIf { it.isNotBlank() }

    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false),
    ) {
        Box(
            modifier =
                Modifier
                    .fillMaxSize()
                    .background(Color.Black.copy(alpha = 0.95f)),
        ) {
            IconButton(
                onClick = onDismiss,
                modifier = Modifier.align(Alignment.TopEnd).padding(12.dp),
            ) {
                Icon(
                    imageVector = Icons.Default.Close,
                    contentDescription = "Close image",
                    tint = Color.White,
                )
            }
            if (imageModel != null) {
                AsyncImage(
                    model = imageModel,
                    contentDescription = attachment.filename,
                    modifier = Modifier.fillMaxSize().padding(16.dp),
                    contentScale = ContentScale.Fit,
                )
            } else {
                Text(
                    text = "Image unavailable",
                    modifier = Modifier.align(Alignment.Center),
                    color = Color.White,
                )
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun MessageBubble(
    message: ChatMessage,
    position: GroupedBubblePosition,
    showSender: Boolean,
    messagesById: Map<String, ChatMessage>,
    onReplyTo: (ChatMessage) -> Unit,
    onJumpToMessage: (String) -> Unit,
    onRetryMessage: (String) -> Unit,
    onReact: (String, String) -> Unit,
    onDownloadMedia: (String, String) -> Unit,
    onOpenImage: (ChatMediaAttachment) -> Unit,
    onHypernoteAction: (messageId: String, actionName: String, form: Map<String, String>) -> Unit,
) {
    val isMine = message.isMine
    val bubbleColor =
        if (isMine) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surfaceVariant
    val textColor =
        if (isMine) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurfaceVariant
    val align = if (isMine) Alignment.End else Alignment.Start
    val segments = remember(message.segments, message.displayContent) {
        if (message.segments.isNotEmpty()) {
            message.segments
        } else if (message.displayContent.isBlank()) {
            emptyList()
        } else {
            listOf(MessageSegment.Markdown(text = message.displayContent))
        }
    }
    val ctx = LocalContext.current
    val clipboardManager = LocalClipboardManager.current
    val haptic = LocalHapticFeedback.current
    val coroutineScope = rememberCoroutineScope()
    val replyTarget = remember(message.replyToMessageId, messagesById) {
        message.replyToMessageId?.let { messagesById[it] }
    }
    val formattedTime = message.displayTimestamp
    val showFooter = position == GroupedBubblePosition.Bottom || position == GroupedBubblePosition.Single
    // Swipe-to-reply state
    val swipeOffset = remember { Animatable(0f) }
    val swipeThreshold = 80f
    var replyTriggered by remember { mutableStateOf(false) }

    Column(modifier = Modifier.fillMaxWidth(), horizontalAlignment = align) {
        if (!isMine && showSender && (position == GroupedBubblePosition.Top || position == GroupedBubblePosition.Single)) {
            val senderName =
                message.senderName?.trim().takeUnless { it.isNullOrBlank() } ?: message.senderPubkey.take(8)
            Text(
                text = senderName,
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(start = 8.dp, bottom = 2.dp),
            )
        }

        message.replyToMessageId?.let { replyToMessageId ->
            ReplyReferencePreview(
                replyToMessageId = replyToMessageId,
                target = replyTarget,
                isMine = isMine,
                onJumpToMessage = onJumpToMessage,
            )
            Spacer(Modifier.height(4.dp))
        }

        val hypernote = message.hypernote
        if (hypernote != null) {
            HypernoteRenderer(
                messageId = message.id,
                hypernote = hypernote,
                onAction = { actionName, messageId, form ->
                    onHypernoteAction(messageId, actionName, form)
                },
            )
        } else {
            if (message.media.isNotEmpty()) {
                for (attachment in message.media) {
                    MediaAttachmentContent(
                        attachment = attachment,
                        isMine = isMine,
                        onDownload = { onDownloadMedia(message.id, attachment.originalHashHex) },
                        onOpenImage = { onOpenImage(attachment) },
                    )
                    Spacer(Modifier.height(3.dp))
                }
            }

            for (segment in segments) {
                when (segment) {
                    is MessageSegment.Markdown -> {
                        Box(
                            modifier = Modifier
                                .fillMaxWidth()
                                .draggable(
                                    orientation = Orientation.Horizontal,
                                    state = rememberDraggableState { delta ->
                                        // Only allow rightward swipe up to threshold + a bit of resistance
                                        val newOffset = (swipeOffset.value + delta).coerceIn(0f, swipeThreshold * 1.2f)
                                        coroutineScope.launch { swipeOffset.snapTo(newOffset) }
                                        if (swipeOffset.value >= swipeThreshold && !replyTriggered) {
                                            replyTriggered = true
                                            haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                                        }
                                    },
                                    onDragStopped = {
                                        if (replyTriggered) {
                                            onReplyTo(message)
                                            replyTriggered = false
                                        }
                                        coroutineScope.launch { swipeOffset.animateTo(0f) }
                                    },
                                ),
                        ) {
                            // Reply icon revealed behind the bubble as it swipes
                            if (swipeOffset.value > 8f) {
                                Icon(
                                    Icons.AutoMirrored.Filled.Reply,
                                    contentDescription = "Reply",
                                    tint =
                                        MaterialTheme.colorScheme.primary.copy(
                                            alpha = (swipeOffset.value / swipeThreshold).coerceIn(0f, 1f),
                                        ),
                                    modifier = Modifier
                                        .align(Alignment.CenterStart)
                                        .padding(start = 8.dp)
                                        .size(20.dp),
                                )
                            }
                            Row(
                                verticalAlignment = Alignment.Bottom,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .offset { IntOffset(swipeOffset.value.roundToInt(), 0) },
                                horizontalArrangement = if (isMine) Arrangement.End else Arrangement.Start,
                            ) {
                                // Use a Box to anchor the DropdownMenu to the bubble.
                                var showMenu by remember { mutableStateOf(false) }
                                Box {
                                    Box(
                                        modifier =
                                            Modifier
                                                .clip(messageBubbleShape(position = position, isMine = isMine))
                                                .background(bubbleColor)
                                                .combinedClickable(
                                                    onClick = {},
                                                    onLongClick = {
                                                        showMenu = true
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
                                    DropdownMenu(
                                        expanded = showMenu,
                                        onDismissRequest = { showMenu = false },
                                    ) {
                                        for (emoji in QUICK_REACTIONS) {
                                            DropdownMenuItem(
                                                text = { Text("React $emoji") },
                                                onClick = {
                                                    onReact(message.id, emoji)
                                                    showMenu = false
                                                },
                                            )
                                        }
                                        DropdownMenuItem(
                                            text = { Text("Reply") },
                                            onClick = {
                                                onReplyTo(message)
                                                showMenu = false
                                            },
                                        )
                                        DropdownMenuItem(
                                            text = { Text("Copy text") },
                                            onClick = {
                                                clipboardManager.setText(AnnotatedString(message.displayContent))
                                                Toast.makeText(ctx, "Copied", Toast.LENGTH_SHORT).show()
                                                showMenu = false
                                            },
                                        )
                                        if (message.delivery is MessageDeliveryState.Failed) {
                                            DropdownMenuItem(
                                                text = { Text("Retry") },
                                                onClick = {
                                                    onRetryMessage(message.id)
                                                    showMenu = false
                                                },
                                            )
                                        }
                                    }
                                }
                            }
                        }
                    }
                    is MessageSegment.PikaHtml -> {
                        Box(
                            modifier =
                                Modifier
                                    .clip(messageBubbleShape(position = position, isMine = isMine))
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
        }
        if (message.reactions.isNotEmpty()) {
            ReactionChipsRow(
                isMine = isMine,
                reactions = message.reactions,
                onToggleReaction = { emoji ->
                    onReact(message.id, emoji)
                },
            )
        }
        if (showFooter) {
            Row(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 1.dp),
                horizontalArrangement = if (isMine) Arrangement.End else Arrangement.Start,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    text = formattedTime,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    textAlign = if (isMine) TextAlign.End else TextAlign.Start,
                )
                if (isMine) {
                    Spacer(Modifier.width(4.dp))
                    DeliveryStateIcon(
                        delivery = message.delivery,
                        tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        onClick =
                            if (message.delivery is MessageDeliveryState.Failed) {
                                { onRetryMessage(message.id) }
                            } else {
                                null
                            },
                    )
                }
            }
        }
        Spacer(Modifier.height(2.dp))
    }
}

@Composable
private fun ReplyReferencePreview(
    replyToMessageId: String,
    target: ChatMessage?,
    isMine: Boolean,
    onJumpToMessage: (String) -> Unit,
) {
    val sender = remember(target) {
        when {
            target == null -> "Original message"
            target.isMine -> "You"
            !target.senderName.isNullOrBlank() -> target.senderName!!
            else -> target.senderPubkey.take(8)
        }
    }
    val snippet = remember(target) {
        val text = target?.displayContent?.trim().orEmpty()
        when {
            target == null -> "Original message not loaded"
            text.isEmpty() -> "(empty message)"
            else -> text.lineSequence().first().let { first ->
                if (first.length > 80) first.take(80) + "…" else first
            }
        }
    }

    val modifier =
        Modifier
            .clip(MaterialTheme.shapes.medium)
            .background(
                if (isMine) {
                    MaterialTheme.colorScheme.primary.copy(alpha = 0.14f)
                } else {
                    MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.6f)
                },
            )
            .padding(horizontal = 10.dp, vertical = 6.dp)
            .widthIn(max = 280.dp)

    Row(
        modifier =
            if (target != null) {
                modifier.clickable { onJumpToMessage(replyToMessageId) }
            } else {
                modifier
            },
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            modifier =
                Modifier
                    .width(2.dp)
                    .height(28.dp)
                    .background(
                        if (isMine) {
                            MaterialTheme.colorScheme.onPrimary.copy(alpha = 0.8f)
                        } else {
                            MaterialTheme.colorScheme.primary
                        },
                    ),
        )
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                text = sender,
                style = MaterialTheme.typography.labelSmall,
                color =
                    if (isMine) {
                        MaterialTheme.colorScheme.onPrimary.copy(alpha = 0.86f)
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                maxLines = 1,
            )
            Text(
                text = snippet,
                style = MaterialTheme.typography.bodySmall,
                color =
                    if (isMine) {
                        MaterialTheme.colorScheme.onPrimary.copy(alpha = 0.8f)
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
private fun ReplyComposerPreview(
    message: ChatMessage,
    onClear: () -> Unit,
) {
    val sender =
        when {
            message.isMine -> "You"
            !message.senderName.isNullOrBlank() -> message.senderName!!
            else -> message.senderPubkey.take(8)
        }
    val snippet =
        message.displayContent.trim().lineSequence().firstOrNull()?.let {
            if (it.length > 80) it.take(80) + "…" else it
        } ?: "(empty message)"

    Row(
        modifier =
            Modifier
                .fillMaxWidth()
                .clip(MaterialTheme.shapes.medium)
                .background(MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.35f))
                .padding(horizontal = 10.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(
            modifier = Modifier.width(2.dp).height(28.dp).background(MaterialTheme.colorScheme.primary),
        )
        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                text = "Replying to $sender",
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
            )
            Text(
                text = snippet,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        TextButton(onClick = onClear) {
            Text("Cancel")
        }
    }
}
