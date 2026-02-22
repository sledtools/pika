package com.pika.app.ui.screens

import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.PersonRemove
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.rust.AuthState
import com.pika.app.rust.MemberInfo
import com.pika.app.ui.Avatar
import com.pika.app.ui.PeerKeyNormalizer
import com.pika.app.ui.PeerKeyValidator
import com.pika.app.ui.TestTags

@Composable
@OptIn(ExperimentalMaterial3Api::class)
fun GroupInfoScreen(manager: AppManager, chatId: String, padding: PaddingValues) {
    val chat = manager.state.currentChat
    if (chat == null || chat.chatId != chatId) {
        Box(modifier = Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
            Text("Loading…")
        }
        return
    }

    val isAdmin = chat.isAdmin
    val groupName = chat.groupName?.trim().takeIf { it?.isNotBlank() == true } ?: "Group"

    var isEditing by remember { mutableStateOf(false) }
    var editedName by remember { mutableStateOf(groupName) }
    var npubInput by remember { mutableStateOf("") }
    var showLeaveDialog by remember { mutableStateOf(false) }
    var memberToRemove by remember { mutableStateOf<MemberInfo?>(null) }

    val (myPubkey, myNpub) = when (val a = manager.state.auth) {
        is AuthState.LoggedIn -> a.pubkey to a.npub
        else -> null to null
    }
    val myProfile = manager.state.myProfile

    Scaffold(
        modifier = Modifier.padding(padding),
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        "Group Info",
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
        LazyColumn(
            modifier = Modifier.fillMaxSize().padding(inner),
            contentPadding = PaddingValues(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            // Group name section
            item {
                Text(
                    "Group Name",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(4.dp))
                if (isEditing) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        OutlinedTextField(
                            value = editedName,
                            onValueChange = { editedName = it },
                            singleLine = true,
                            modifier = Modifier.weight(1f),
                        )
                        Button(
                            onClick = {
                                val trimmed = editedName.trim()
                                if (trimmed.isNotBlank()) {
                                    manager.dispatch(AppAction.RenameGroup(chatId, trimmed))
                                }
                                isEditing = false
                            },
                        ) {
                            Text("Save")
                        }
                        TextButton(onClick = { isEditing = false }) {
                            Text("Cancel")
                        }
                    }
                } else {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            groupName,
                            style = MaterialTheme.typography.headlineSmall,
                            modifier = Modifier.weight(1f),
                        )
                        if (isAdmin) {
                            IconButton(onClick = {
                                editedName = groupName
                                isEditing = true
                            }) {
                                Icon(Icons.Default.Edit, contentDescription = "Edit name")
                            }
                        }
                    }
                }
            }

            // Members section
            item {
                HorizontalDivider()
                Spacer(Modifier.height(4.dp))
                Text(
                    "Members (${chat.members.size + 1})",
                    style = MaterialTheme.typography.titleSmall,
                )
            }

            // "You" row
            item {
                Row(
                    modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Avatar(
                        name = myProfile.name.takeIf { it.isNotBlank() },
                        npub = myNpub ?: "",
                        pictureUrl = myProfile.pictureUrl,
                        size = 40.dp,
                    )
                    Text(
                        "You",
                        style = MaterialTheme.typography.bodyLarge,
                        modifier = Modifier.weight(1f),
                    )
                    if (isAdmin) {
                        Text(
                            "Admin",
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }

            // Member rows
            items(chat.members, key = { it.pubkey }) { member ->
                if (member.pubkey == myPubkey) return@items
                MemberRow(
                    member = member,
                    isAdmin = isAdmin,
                    onRemove = { memberToRemove = member },
                    onTap = { manager.dispatch(AppAction.OpenPeerProfile(member.pubkey)) },
                )
            }

            // Add member section (admin only)
            if (isAdmin) {
                item {
                    HorizontalDivider()
                    Spacer(Modifier.height(4.dp))
                    Text(
                        "Add Member",
                        style = MaterialTheme.typography.titleSmall,
                    )
                    Spacer(Modifier.height(4.dp))

                    val normalized = PeerKeyNormalizer.normalize(npubInput)
                    val isValid = PeerKeyValidator.isValidPeer(normalized)

                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        OutlinedTextField(
                            value = npubInput,
                            onValueChange = { npubInput = it },
                            label = { Text("Peer npub") },
                            singleLine = true,
                            isError = npubInput.isNotEmpty() && !isValid,
                            modifier = Modifier.weight(1f).testTag(TestTags.GROUPINFO_ADD_NPUB),
                        )
                        Button(
                            onClick = {
                                manager.dispatch(AppAction.AddGroupMembers(chatId, listOf(normalized)))
                                npubInput = ""
                            },
                            enabled = isValid,
                            modifier = Modifier.testTag(TestTags.GROUPINFO_ADD_BUTTON),
                        ) {
                            Text("Add")
                        }
                    }
                }
            }

            // Leave group
            item {
                HorizontalDivider()
                Spacer(Modifier.height(4.dp))
                Button(
                    onClick = { showLeaveDialog = true },
                    colors = ButtonDefaults.buttonColors(
                        containerColor = MaterialTheme.colorScheme.error,
                    ),
                    modifier = Modifier.fillMaxWidth().testTag(TestTags.GROUPINFO_LEAVE),
                ) {
                    Text("Leave Group")
                }
            }
        }
    }

    // Leave confirmation dialog
    if (showLeaveDialog) {
        AlertDialog(
            onDismissRequest = { showLeaveDialog = false },
            title = { Text("Leave Group") },
            text = { Text("Are you sure you want to leave this group?") },
            confirmButton = {
                TextButton(onClick = {
                    manager.dispatch(AppAction.LeaveGroup(chatId))
                    showLeaveDialog = false
                }) {
                    Text("Leave", color = MaterialTheme.colorScheme.error)
                }
            },
            dismissButton = {
                TextButton(onClick = { showLeaveDialog = false }) {
                    Text("Cancel")
                }
            },
        )
    }

    // Remove member confirmation dialog
    memberToRemove?.let { member ->
        AlertDialog(
            onDismissRequest = { memberToRemove = null },
            title = { Text("Remove Member") },
            text = { Text("Remove ${member.name ?: truncatedNpub(member.npub)} from the group?") },
            confirmButton = {
                TextButton(onClick = {
                    manager.dispatch(AppAction.RemoveGroupMembers(chatId, listOf(member.pubkey)))
                    memberToRemove = null
                }) {
                    Text("Remove", color = MaterialTheme.colorScheme.error)
                }
            },
            dismissButton = {
                TextButton(onClick = { memberToRemove = null }) {
                    Text("Cancel")
                }
            },
        )
    }
}

@Composable
private fun MemberRow(
    member: MemberInfo,
    isAdmin: Boolean,
    onRemove: () -> Unit,
    onTap: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable { onTap() }
            .padding(vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Avatar(
            name = member.name,
            npub = member.npub,
            pictureUrl = member.pictureUrl,
            size = 40.dp,
        )
        Column(modifier = Modifier.weight(1f)) {
            Text(
                member.name ?: truncatedNpub(member.npub),
                style = MaterialTheme.typography.bodyLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (member.name != null) {
                Text(
                    truncatedNpub(member.npub),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                )
            }
        }
        if (member.isAdmin) {
            Text(
                "Admin",
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (isAdmin) {
            IconButton(onClick = onRemove) {
                Icon(
                    Icons.Default.PersonRemove,
                    contentDescription = "Remove member",
                    tint = MaterialTheme.colorScheme.error,
                )
            }
        }
    }
}

private fun truncatedNpub(npub: String): String {
    if (npub.length <= 20) return npub
    return npub.take(12) + "…" + npub.takeLast(4)
}
