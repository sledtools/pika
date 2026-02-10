package com.pika.app.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.rust.Screen
import com.pika.app.ui.TestTags
import com.pika.app.ui.PeerKeyValidator
import com.pika.app.ui.PeerKeyNormalizer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material3.MaterialTheme

@Composable
@OptIn(ExperimentalMaterial3Api::class)
fun NewChatScreen(manager: AppManager, padding: PaddingValues) {
    val clipboard = LocalClipboardManager.current
    var npub by remember { mutableStateOf("") }
    var showScanner by remember { mutableStateOf(false) }
    val peer = PeerKeyNormalizer.normalize(npub)
    val isValidPeer = PeerKeyValidator.isValidPeer(peer)
    val isLoading = manager.state.busy.creatingChat

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
        Column(
            modifier = Modifier.padding(inner).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            OutlinedTextField(
                value = npub,
                onValueChange = { npub = it },
                label = { Text("Peer npub") },
                singleLine = true,
                enabled = !isLoading,
                isError = peer.isNotEmpty() && !isValidPeer,
                modifier = Modifier.fillMaxWidth().testTag(TestTags.NEWCHAT_PEER_NPUB),
            )
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
                        npub = PeerKeyNormalizer.normalize(raw)
                    },
                    enabled = !isLoading,
                    modifier = Modifier.testTag(TestTags.NEWCHAT_PASTE),
                ) {
                    Text("Paste")
                }
            }
            if (peer.isNotEmpty() && !isValidPeer) {
                Text(
                    "Enter a valid npub1… or 64-char hex pubkey.",
                    color = MaterialTheme.colorScheme.error,
                )
            }
            Button(
                onClick = {
                    manager.dispatch(AppAction.CreateChat(peer))
                },
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
