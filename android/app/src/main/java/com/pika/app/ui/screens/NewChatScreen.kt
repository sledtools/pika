package com.pika.app.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.rust.AuthState
import com.pika.app.rust.FollowListEntry
import com.pika.app.rust.isValidPeerKey
import com.pika.app.ui.Avatar
import com.pika.app.ui.TestTags

@Composable
@OptIn(ExperimentalMaterial3Api::class)
fun NewChatScreen(manager: AppManager, padding: PaddingValues) {
    val clipboard = LocalClipboardManager.current
    var npub by remember { mutableStateOf("") }
    var showScanner by remember { mutableStateOf(false) }
    var searchText by remember { mutableStateOf("") }
    val peer = npub.trim()
    val isValidPeer = isValidPeerKey(peer)
    val isLoading = manager.state.busy.creatingChat
    val isFetchingFollows = manager.state.busy.fetchingFollowList
    val followList = manager.state.followList
    val myNpub = (manager.state.auth as? AuthState.LoggedIn)?.npub

    val filteredFollows = remember(followList, searchText, myNpub) {
        val base = followList.filter { it.npub != myNpub }
        if (searchText.isBlank()) base
        else {
            val query = searchText.lowercase()
            base.filter { entry ->
                entry.name?.lowercase()?.contains(query) == true ||
                    entry.username?.lowercase()?.contains(query) == true ||
                    entry.npub.lowercase().contains(query) ||
                    entry.pubkey.lowercase().contains(query)
            }
        }
    }

    LaunchedEffect(Unit) {
        manager.dispatch(AppAction.RefreshFollowList)
    }

    Scaffold(
        modifier = Modifier.padding(padding),
        topBar = {
            TopAppBar(
                title = { Text("New chat") },
                navigationIcon = {
                    IconButton(
                        onClick = {
                            val stack = manager.state.router.screenStack
                            manager.dispatch(AppAction.UpdateScreenStack(stack.dropLast(1)))
                        },
                        enabled = !isLoading,
                    ) {
                        Icon(Icons.Default.ArrowBack, contentDescription = "Back")
                    }
                },
            )
        },
    ) { inner ->
        LazyColumn(
            modifier = Modifier.fillMaxSize().padding(inner),
            contentPadding = PaddingValues(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            // Manual entry
            item {
                OutlinedTextField(
                    value = npub,
                    onValueChange = { npub = it },
                    label = { Text("Profile code") },
                    singleLine = true,
                    enabled = !isLoading,
                    isError = peer.isNotEmpty() && !isValidPeer,
                    modifier = Modifier.fillMaxWidth().testTag(TestTags.NEWCHAT_PEER_NPUB),
                )
            }
            item {
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    TextButton(
                        onClick = { showScanner = true },
                        enabled = !isLoading,
                        modifier = Modifier.testTag(TestTags.NEWCHAT_SCAN_QR),
                    ) {
                        Text("Scan QR")
                    }
                    TextButton(
                        onClick = {
                            val raw = clipboard.getText()?.text.orEmpty()
                            npub = raw
                        },
                        enabled = !isLoading,
                        modifier = Modifier.testTag(TestTags.NEWCHAT_PASTE),
                    ) {
                        Text("Paste")
                    }
                }
                if (peer.isNotEmpty() && !isValidPeer) {
                    Text(
                        "Enter a valid profile code.",
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
            item {
                Button(
                    onClick = { manager.dispatch(AppAction.CreateChat(peer)) },
                    enabled = isValidPeer && !isLoading,
                    modifier = Modifier.fillMaxWidth().testTag(TestTags.NEWCHAT_START),
                ) {
                    if (isLoading) {
                        Row {
                            CircularProgressIndicator(
                                modifier = Modifier.size(20.dp),
                                strokeWidth = 2.dp,
                            )
                            Spacer(modifier = Modifier.width(8.dp))
                            Text("Creating…")
                        }
                    } else {
                        Text("Start chat")
                    }
                }
            }

            // Follow list
            item {
                HorizontalDivider()
                Spacer(Modifier.height(4.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Follows", style = MaterialTheme.typography.titleSmall)
                    if (isFetchingFollows) {
                        Spacer(Modifier.width(8.dp))
                        CircularProgressIndicator(modifier = Modifier.size(14.dp), strokeWidth = 2.dp)
                    }
                }
                Spacer(Modifier.height(4.dp))
                OutlinedTextField(
                    value = searchText,
                    onValueChange = { searchText = it },
                    label = { Text("Search follows") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
            }

            if (isFetchingFollows && followList.isEmpty()) {
                item {
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(vertical = 16.dp),
                        horizontalArrangement = Arrangement.Center,
                    ) {
                        CircularProgressIndicator(modifier = Modifier.size(20.dp), strokeWidth = 2.dp)
                        Spacer(Modifier.width(8.dp))
                        Text("Loading follows…")
                    }
                }
            } else if (followList.isEmpty()) {
                item {
                    Text(
                        "No follows found.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(vertical = 8.dp),
                    )
                }
            } else {
                items(filteredFollows, key = { it.pubkey }) { entry ->
                    FollowChatRow(
                        entry = entry,
                        enabled = !isLoading,
                        onClick = { manager.dispatch(AppAction.CreateChat(entry.npub)) },
                    )
                }
            }
        }
    }

    if (showScanner) {
        QrScannerDialog(
            onDismiss = { showScanner = false },
            onScanned = { scanned ->
                npub = scanned
                showScanner = false
            },
        )
    }
}

@Composable
private fun FollowChatRow(
    entry: FollowListEntry,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(enabled = enabled) { onClick() }
            .padding(vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Avatar(
            name = entry.name,
            npub = entry.npub,
            pictureUrl = entry.pictureUrl,
            size = 40.dp,
        )
        Column(modifier = Modifier.weight(1f)) {
            if (entry.name != null) {
                Text(
                    entry.name!!,
                    style = MaterialTheme.typography.bodyLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Text(
                truncatedNpub(entry.npub),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
            )
        }
    }
}

private fun truncatedNpub(npub: String): String {
    if (npub.length <= 20) return npub
    return npub.take(12) + "\u2026" + npub.takeLast(4)
}
