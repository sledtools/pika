package com.pika.app.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.TextButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.foundation.Image
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.rust.AuthState
import com.pika.app.rust.ChatSummary
import com.pika.app.rust.Screen
import com.pika.app.ui.theme.PikaBlue
import com.pika.app.ui.QrCode
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Logout
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Person

@Composable
@OptIn(ExperimentalMaterial3Api::class)
fun ChatListScreen(manager: AppManager, padding: PaddingValues) {
    val clipboard = LocalClipboardManager.current
    val (showMyNpub, setShowMyNpub) = remember { mutableStateOf(false) }
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
                        IconButton(onClick = { setShowMyNpub(true) }) {
                            Icon(Icons.Default.Person, contentDescription = "My npub")
                        }
                    }
                    IconButton(onClick = { manager.dispatch(AppAction.PushScreen(Screen.NewChat)) }) {
                        Icon(Icons.Default.Add, contentDescription = "New Chat")
                    }
                    IconButton(onClick = { manager.logout() }) {
                        Icon(Icons.AutoMirrored.Filled.Logout, contentDescription = "Logout")
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

    if (showMyNpub && myNpub != null) {
        val qr = remember(myNpub) { QrCode.encode(myNpub, 512).asImageBitmap() }
        AlertDialog(
            onDismissRequest = { setShowMyNpub(false) },
            title = { Text("My npub") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Image(
                        bitmap = qr,
                        contentDescription = "My npub QR",
                        modifier = Modifier.size(220.dp).clip(MaterialTheme.shapes.medium),
                    )
                    Text(myNpub)
                }
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        clipboard.setText(AnnotatedString(myNpub))
                        setShowMyNpub(false)
                    },
                ) {
                    Text("Copy")
                }
            },
            dismissButton = {
                TextButton(onClick = { setShowMyNpub(false) }) { Text("Close") }
            },
        )
    }
}

@Composable
private fun ChatRow(chat: ChatSummary, selfPubkey: String?, onClick: () -> Unit) {
    val title = chatTitle(chat, selfPubkey)
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
            Box(
                modifier =
                    Modifier
                        .size(44.dp)
                        .clip(CircleShape)
                        .background(PikaBlue.copy(alpha = 0.12f)),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    title.take(1).uppercase(),
                    style = MaterialTheme.typography.titleMedium,
                    color = PikaBlue,
                )
            }
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
