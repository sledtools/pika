package com.pika.app.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Close
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
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.rust.AuthState
import com.pika.app.rust.FollowListEntry
import com.pika.app.rust.isValidPeerKey
import com.pika.app.rust.normalizePeerKey
import com.pika.app.ui.Avatar
import com.pika.app.ui.TestTags

@Composable
@OptIn(ExperimentalMaterial3Api::class)
fun NewGroupChatScreen(manager: AppManager, padding: PaddingValues) {
    val clipboard = LocalClipboardManager.current
    var groupName by remember { mutableStateOf("") }
    val selectedNpubs = remember { mutableStateListOf<String>() }
    var searchText by remember { mutableStateOf("") }
    var npubInput by remember { mutableStateOf("") }
    var showScanner by remember { mutableStateOf(false) }
    var showManualEntry by remember { mutableStateOf(false) }

    val isCreating = manager.state.busy.creatingChat
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

    val canCreate = groupName.isNotBlank() && selectedNpubs.isNotEmpty() && !isCreating

    LaunchedEffect(Unit) {
        manager.dispatch(AppAction.RefreshFollowList)
    }

    Scaffold(
        modifier = Modifier.padding(padding),
        topBar = {
            TopAppBar(
                title = { Text("New group") },
                navigationIcon = {
                    IconButton(
                        onClick = {
                            val stack = manager.state.router.screenStack
                            manager.dispatch(AppAction.UpdateScreenStack(stack.dropLast(1)))
                        },
                        enabled = !isCreating,
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
            // Group name
            item {
                OutlinedTextField(
                    value = groupName,
                    onValueChange = { groupName = it },
                    label = { Text("Group name") },
                    singleLine = true,
                    enabled = !isCreating,
                    modifier = Modifier.fillMaxWidth().testTag(TestTags.NEWGROUP_NAME),
                )
            }

            // Selected members chips
            if (selectedNpubs.isNotEmpty()) {
                item {
                    Text(
                        "Selected (${selectedNpubs.size})",
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.height(4.dp))
                    Row(
                        modifier = Modifier.horizontalScroll(rememberScrollState()),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        selectedNpubs.forEach { npub ->
                            SelectedChip(
                                npub = npub,
                                followList = followList,
                                enabled = !isCreating,
                                onRemove = { selectedNpubs.remove(npub) },
                            )
                        }
                    }
                }
            }

            // Manual entry toggle
            item {
                TextButton(onClick = { showManualEntry = !showManualEntry }) {
                    Text(if (showManualEntry) "Hide manual entry" else "Add member manually")
                }
            }

            // Manual entry
            if (showManualEntry) {
                item {
                    val normalizedInput = normalizePeerKey(npubInput)
                    val isValidInput = isValidPeerKey(normalizedInput)

                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        OutlinedTextField(
                            value = npubInput,
                            onValueChange = { npubInput = it },
                            label = { Text("npub or hex pubkey") },
                            singleLine = true,
                            enabled = !isCreating,
                            isError = npubInput.isNotEmpty() && !isValidInput,
                            modifier = Modifier.weight(1f).testTag(TestTags.NEWGROUP_PEER_NPUB),
                        )
                        TextButton(
                            onClick = { showScanner = true },
                            enabled = !isCreating,
                        ) {
                            Text("QR")
                        }
                        TextButton(
                            onClick = {
                                val raw = clipboard.getText()?.text.orEmpty()
                                npubInput = normalizePeerKey(raw)
                            },
                            enabled = !isCreating,
                        ) {
                            Text("Paste")
                        }
                    }
                    Spacer(Modifier.height(4.dp))
                    Button(
                        onClick = {
                            if (isValidInput && normalizedInput !in selectedNpubs) {
                                selectedNpubs.add(normalizedInput)
                            }
                            npubInput = ""
                        },
                        enabled = isValidInput && !isCreating,
                        modifier = Modifier.testTag(TestTags.NEWGROUP_ADD_MEMBER),
                    ) {
                        Text("Add")
                    }
                }
            }

            // Follow list header
            item {
                HorizontalDivider()
                Spacer(Modifier.height(4.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        "Follows",
                        style = MaterialTheme.typography.titleSmall,
                    )
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

            // Follow list content
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
                    val isSelected = entry.npub in selectedNpubs
                    FollowRow(
                        entry = entry,
                        isSelected = isSelected,
                        enabled = !isCreating,
                        onClick = {
                            if (isSelected) selectedNpubs.remove(entry.npub)
                            else selectedNpubs.add(entry.npub)
                        },
                    )
                }
            }

            // Create button
            item {
                Spacer(Modifier.height(8.dp))
                Button(
                    onClick = {
                        manager.dispatch(
                            AppAction.CreateGroupChat(
                                selectedNpubs.toList(),
                                groupName.trim(),
                            ),
                        )
                    },
                    enabled = canCreate,
                    modifier = Modifier.fillMaxWidth().testTag(TestTags.NEWGROUP_CREATE),
                ) {
                    if (isCreating) {
                        Row {
                            CircularProgressIndicator(
                                modifier = Modifier.size(20.dp),
                                strokeWidth = 2.dp,
                            )
                            Spacer(Modifier.width(8.dp))
                            Text("Creating…")
                        }
                    } else {
                        Text("Create Group")
                    }
                }
            }
        }
    }

    if (showScanner) {
        QrScannerDialog(
            onDismiss = { showScanner = false },
            onScanned = { scanned ->
                val normalized = normalizePeerKey(scanned)
                if (isValidPeerKey(normalized) && normalized !in selectedNpubs) {
                    selectedNpubs.add(normalized)
                } else {
                    npubInput = scanned
                    showManualEntry = true
                }
                showScanner = false
            },
        )
    }
}

@Composable
private fun SelectedChip(
    npub: String,
    followList: List<FollowListEntry>,
    enabled: Boolean,
    onRemove: () -> Unit,
) {
    val displayName = followList.firstOrNull { it.npub == npub }?.name ?: truncatedNpub(npub)
    Row(
        modifier = Modifier
            .clip(CircleShape)
            .background(MaterialTheme.colorScheme.secondaryContainer)
            .padding(start = 10.dp, end = 4.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            displayName,
            style = MaterialTheme.typography.labelMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Spacer(Modifier.width(2.dp))
        IconButton(
            onClick = onRemove,
            enabled = enabled,
            modifier = Modifier.size(20.dp),
        ) {
            Icon(
                Icons.Default.Close,
                contentDescription = "Remove",
                modifier = Modifier.size(14.dp),
            )
        }
    }
}

@Composable
private fun FollowRow(
    entry: FollowListEntry,
    isSelected: Boolean,
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
        if (isSelected) {
            Icon(
                Icons.Default.Check,
                contentDescription = "Selected",
                tint = MaterialTheme.colorScheme.primary,
            )
        }
    }
}

private fun truncatedNpub(npub: String): String {
    if (npub.length <= 20) return npub
    return npub.take(12) + "…" + npub.takeLast(4)
}
