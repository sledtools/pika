package com.pika.app.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.CenterAlignedTopAppBar
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.rust.AuthState
import com.pika.app.rust.ChatSummary
import com.pika.app.rust.Screen
import com.pika.app.ui.Avatar
import com.pika.app.ui.TestTags
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.AutoAwesome
import androidx.compose.material.icons.filled.Archive
import androidx.compose.material.icons.filled.GroupAdd
import com.pika.app.rust.AgentKind

@Composable
@OptIn(ExperimentalMaterial3Api::class)
fun ChatListScreen(manager: AppManager, padding: PaddingValues) {
    var showMyProfile by remember { mutableStateOf(false) }
    var showAgentChooser by remember { mutableStateOf(false) }
    val myProfile = manager.state.myProfile
    val shareSummary = manager.pendingShareSelectionSummary()
    val isShareSelectionMode = shareSummary != null
    val myNpub =
        when (val a = manager.state.auth) {
            is AuthState.LoggedIn -> a.npub
            else -> null
        }

    Scaffold(
        modifier = Modifier.padding(padding),
        topBar = {
            CenterAlignedTopAppBar(
                title = { Text(if (isShareSelectionMode) "Choose Chat" else "Chats") },
                colors =
                    TopAppBarDefaults.centerAlignedTopAppBarColors(
                        containerColor = MaterialTheme.colorScheme.surface,
                    ),
                navigationIcon = {
                    if (myNpub != null) {
                        IconButton(
                            onClick = { showMyProfile = true },
                            modifier = Modifier.testTag(TestTags.CHATLIST_MY_PROFILE),
                        ) {
                            Avatar(
                                name = myProfile.name.takeIf { it.isNotBlank() },
                                npub = myNpub,
                                pictureUrl = myProfile.pictureUrl,
                                size = 28.dp,
                            )
                        }
                    }
                },
                actions = {
                    if (isShareSelectionMode) {
                        TextButton(onClick = { manager.dismissPendingShareSelection() }) {
                            Text("Cancel")
                        }
                    } else {
                        IconButton(onClick = { manager.dispatch(AppAction.PushScreen(Screen.NewChat)) }) {
                            Icon(Icons.Default.Add, contentDescription = "New Chat")
                        }
                        IconButton(onClick = { manager.dispatch(AppAction.PushScreen(Screen.NewGroupChat)) }) {
                            Icon(Icons.Default.GroupAdd, contentDescription = "New Group")
                        }
                        manager.state.agentButton?.let { agent ->
                            IconButton(
                                onClick = {
                                    if (manager.state.showAgentMarketplace) {
                                        showAgentChooser = true
                                    } else {
                                        manager.ensureAgent()
                                    }
                                },
                                enabled = !agent.isBusy,
                                modifier = Modifier.testTag("chatlist_agent"),
                            ) {
                                Icon(Icons.Default.AutoAwesome, contentDescription = agent.title)
                            }
                        }
                    }
                },
            )
        },
    ) { inner ->
        LazyColumn(
            modifier = Modifier.padding(inner),
            contentPadding = PaddingValues(vertical = 6.dp),
        ) {
            if (shareSummary != null) {
                item {
                    PendingShareBanner(summary = shareSummary)
                }
            }
            items(manager.state.chatList, key = { it.chatId }) { chat ->
                if (isShareSelectionMode) {
                    Box(
                        modifier =
                            Modifier
                                .fillMaxWidth()
                                .background(MaterialTheme.colorScheme.surface),
                    ) {
                        ChatRow(
                            chat = chat,
                            onClick = { manager.onChatListChatSelected(chat.chatId) },
                        )
                    }
                } else {
                    val dismissState =
                        rememberSwipeToDismissBoxState(
                            positionalThreshold = { distance -> distance * 0.25f },
                            confirmValueChange = { value ->
                                if (value == SwipeToDismissBoxValue.EndToStart) {
                                    manager.dispatch(AppAction.ArchiveChat(chat.chatId))
                                    // Keep the row from getting visually "stuck" in a dismissed offset.
                                    // The row will disappear when Rust state removes it from chatList.
                                    false
                                } else {
                                    false
                                }
                            },
                        )
                    SwipeToDismissBox(
                        state = dismissState,
                        enableDismissFromStartToEnd = false,
                        enableDismissFromEndToStart = true,
                        backgroundContent = {
                            if (dismissState.dismissDirection == SwipeToDismissBoxValue.EndToStart) {
                                ArchiveSwipeBackground()
                            }
                        },
                        content = {
                            Box(
                                modifier =
                                    Modifier
                                        .fillMaxWidth()
                                        .background(MaterialTheme.colorScheme.surface),
                            ) {
                                ChatRow(
                                    chat = chat,
                                    onClick = { manager.onChatListChatSelected(chat.chatId) },
                                )
                            }
                        },
                    )
                }
            }
        }
    }

    if (showMyProfile && myNpub != null) {
        MyProfileSheet(
            manager = manager,
            npub = myNpub,
            onDismiss = { showMyProfile = false },
        )
    }

    if (showAgentChooser) {
        AlertDialog(
            onDismissRequest = { showAgentChooser = false },
            title = { Text("Choose Agent") },
            text = { Text("Show experimental agent choices when creating a new agent.") },
            confirmButton = {
                TextButton(onClick = {
                    showAgentChooser = false
                    manager.ensureAgent(AgentKind.Openclaw)
                }) {
                    Text("OpenClaw")
                }
            },
            dismissButton = {
                Row {
                    TextButton(onClick = {
                        showAgentChooser = false
                        manager.ensureAgent(AgentKind.Pi)
                    }) {
                        Text("Pi")
                    }
                    OutlinedButton(onClick = { showAgentChooser = false }) {
                        Text("Cancel")
                    }
                }
            },
        )
    }
}

@Composable
private fun PendingShareBanner(summary: String) {
    Box(
        modifier =
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 8.dp)
                .clip(MaterialTheme.shapes.medium)
                .background(MaterialTheme.colorScheme.secondaryContainer),
    ) {
        Text(
            text = summary,
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSecondaryContainer,
        )
    }
}

@Composable
private fun ArchiveSwipeBackground() {
    Box(
        modifier =
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 6.dp)
                .clip(MaterialTheme.shapes.medium)
                .background(MaterialTheme.colorScheme.surfaceContainerHighest),
        contentAlignment = Alignment.CenterEnd,
    ) {
        Row(
            modifier = Modifier.padding(end = 14.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(
                imageVector = Icons.Default.Archive,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                text = "Archive",
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun ChatRow(chat: ChatSummary, onClick: () -> Unit) {
    val peer = if (!chat.isGroup) chat.members.firstOrNull() else null
    Row(
        modifier =
            Modifier
                .fillMaxWidth()
                .clickable { onClick() }
                .padding(horizontal = 16.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        BadgedBox(
            badge = {
                if (chat.unreadCount > 0u) {
                    Badge { Text(chat.unreadCount.toString()) }
                }
            },
        ) {
            Avatar(
                name = peer?.name ?: chat.displayName,
                npub = peer?.npub ?: chat.chatId,
                pictureUrl = peer?.pictureUrl,
            )
        }

        Column(modifier = Modifier.weight(1f)) {
            Text(
                text = chat.displayName,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.titleMedium,
            )
            chat.subtitle?.let { subtitle ->
                Spacer(modifier = Modifier.height(2.dp))
                Text(
                    text = subtitle,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Spacer(modifier = Modifier.height(2.dp))
            Text(
                text = chat.lastMessagePreview,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}
