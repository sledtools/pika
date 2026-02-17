package com.pika.app.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
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
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.rust.AuthState
import com.pika.app.rust.ChatSummary
import com.pika.app.rust.Screen
import com.pika.app.ui.Avatar
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.GroupAdd
import androidx.compose.material.icons.filled.Person

@Composable
@OptIn(ExperimentalMaterial3Api::class)
fun ChatListScreen(manager: AppManager, padding: PaddingValues) {
    var showMyProfile by remember { mutableStateOf(false) }
    val (myNpub, myPubkey) =
        when (val a = manager.state.auth) {
            is AuthState.LoggedIn -> a.npub to a.pubkey
            else -> null to null
        }

    Scaffold(
        modifier = Modifier.padding(padding),
        topBar = {
            TopAppBar(
                title = { Text("Chats") },
                colors =
                    TopAppBarDefaults.topAppBarColors(
                        containerColor = Color.Transparent,
                    ),
                actions = {
                    if (myNpub != null) {
                        IconButton(onClick = { showMyProfile = true }) {
                            Icon(Icons.Default.Person, contentDescription = "Profile")
                        }
                    }
                    IconButton(onClick = { manager.dispatch(AppAction.PushScreen(Screen.NewChat)) }) {
                        Icon(Icons.Default.Add, contentDescription = "New Chat")
                    }
                    IconButton(onClick = { manager.dispatch(AppAction.PushScreen(Screen.NewGroupChat)) }) {
                        Icon(Icons.Default.GroupAdd, contentDescription = "New Group")
                    }
                },
            )
        },
    ) { inner ->
        LazyColumn(
            modifier = Modifier.padding(inner),
            contentPadding = PaddingValues(vertical = 6.dp),
        ) {
            items(manager.state.chatList, key = { it.chatId }) { chat ->
                ChatRow(
                    chat = chat,
                    selfPubkey = myPubkey,
                    onClick = { manager.dispatch(AppAction.OpenChat(chat.chatId)) },
                )
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
}

@Composable
private fun ChatRow(chat: ChatSummary, selfPubkey: String?, onClick: () -> Unit) {
    val title = chatTitle(chat, selfPubkey)
    val peer = if (!chat.isGroup) {
        chat.members.firstOrNull { selfPubkey == null || it.pubkey != selfPubkey }
            ?: chat.members.firstOrNull()
    } else null
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
                name = peer?.name ?: chat.groupName,
                npub = peer?.npub ?: chat.chatId,
                pictureUrl = peer?.pictureUrl,
            )
        }

        Column(modifier = Modifier.weight(1f)) {
            Text(
                text = title,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.titleMedium,
            )
            Spacer(modifier = Modifier.height(2.dp))
            Text(
                text = chat.lastMessage ?: "No messages yet",
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

private fun chatTitle(chat: ChatSummary, selfPubkey: String?): String {
    if (chat.isGroup) {
        return chat.groupName?.trim().takeIf { !it.isNullOrBlank() } ?: "Group chat"
    }
    val peer =
        chat.members.firstOrNull { selfPubkey == null || it.pubkey != selfPubkey }
            ?: chat.members.firstOrNull()
    return peer?.name?.trim().takeIf { !it.isNullOrBlank() } ?: peer?.npub ?: "Chat"
}
