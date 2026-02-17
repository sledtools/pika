package com.pika.app.ui.screens

import android.net.Uri
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Visibility
import androidx.compose.material.icons.filled.VisibilityOff
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.ui.Avatar
import com.pika.app.ui.QrCode
import android.util.Base64

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MyProfileSheet(
    manager: AppManager,
    npub: String,
    onDismiss: () -> Unit,
) {
    val ctx = LocalContext.current
    val clipboard = LocalClipboardManager.current
    val profile = manager.state.myProfile
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)

    var nameDraft by remember { mutableStateOf(profile.name) }
    var aboutDraft by remember { mutableStateOf(profile.about) }
    var didSyncDrafts by remember { mutableStateOf(false) }
    var showNsec by remember { mutableStateOf(false) }
    var showLogoutConfirm by remember { mutableStateOf(false) }
    var isLoadingPhoto by remember { mutableStateOf(false) }

    val nsec = remember { manager.getNsec() }

    val hasChanges = nameDraft.trim() != profile.name.trim() ||
        aboutDraft.trim() != profile.about.trim()

    LaunchedEffect(Unit) {
        manager.dispatch(AppAction.RefreshMyProfile)
    }

    LaunchedEffect(profile) {
        if (!didSyncDrafts || !hasChanges) {
            nameDraft = profile.name
            aboutDraft = profile.about
            didSyncDrafts = true
        }
    }

    val photoLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.GetContent(),
    ) { uri: Uri? ->
        if (uri == null) return@rememberLauncherForActivityResult
        isLoadingPhoto = true
        val bytes = ctx.contentResolver.openInputStream(uri)?.use { it.readBytes() }
        isLoadingPhoto = false
        if (bytes != null && bytes.isNotEmpty()) {
            val mimeType = ctx.contentResolver.getType(uri) ?: "image/jpeg"
            val base64 = Base64.encodeToString(bytes, Base64.NO_WRAP)
            manager.dispatch(AppAction.UploadMyProfileImage(base64, mimeType))
        }
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
    ) {
        LazyColumn(
            modifier = Modifier.padding(horizontal = 20.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            // Photo section
            item {
                Column(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Avatar(
                        name = profile.name.takeIf { it.isNotBlank() },
                        npub = npub,
                        pictureUrl = profile.pictureUrl,
                        size = 96.dp,
                    )
                    Spacer(Modifier.height(8.dp))
                    if (isLoadingPhoto) {
                        CircularProgressIndicator(modifier = Modifier.size(20.dp), strokeWidth = 2.dp)
                    }
                    OutlinedButton(onClick = { photoLauncher.launch("image/*") }) {
                        Text("Upload Photo")
                    }
                }
            }

            // Profile editing
            item {
                Text("Profile", style = MaterialTheme.typography.titleSmall)
                Spacer(Modifier.height(4.dp))
                OutlinedTextField(
                    value = nameDraft,
                    onValueChange = { nameDraft = it },
                    label = { Text("Name") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(8.dp))
                OutlinedTextField(
                    value = aboutDraft,
                    onValueChange = { aboutDraft = it },
                    label = { Text("About") },
                    maxLines = 4,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(4.dp))
                Button(
                    onClick = {
                        manager.dispatch(AppAction.SaveMyProfile(nameDraft.trim(), aboutDraft.trim()))
                        Toast.makeText(ctx, "Profile saved", Toast.LENGTH_SHORT).show()
                    },
                    enabled = hasChanges,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text("Save Changes")
                }
            }

            // Public key section
            item {
                HorizontalDivider()
                Text("Public Key", style = MaterialTheme.typography.titleSmall)
                Spacer(Modifier.height(4.dp))
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        npub,
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    IconButton(onClick = {
                        clipboard.setText(AnnotatedString(npub))
                        Toast.makeText(ctx, "Copied", Toast.LENGTH_SHORT).show()
                    }) {
                        Icon(Icons.Default.ContentCopy, contentDescription = "Copy npub", modifier = Modifier.size(18.dp))
                    }
                }
            }

            // QR code
            item {
                val qr = remember(npub) { QrCode.encode(npub, 512).asImageBitmap() }
                Column(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Image(
                        bitmap = qr,
                        contentDescription = "My npub QR",
                        modifier = Modifier.size(200.dp).clip(MaterialTheme.shapes.medium),
                    )
                }
            }

            // Private key section
            if (nsec != null) {
                item {
                    HorizontalDivider()
                    Text("Private Key (nsec)", style = MaterialTheme.typography.titleSmall)
                    Spacer(Modifier.height(4.dp))
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            if (showNsec) nsec else "\u2022".repeat(24),
                            style = MaterialTheme.typography.bodySmall,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f),
                        )
                        IconButton(onClick = { showNsec = !showNsec }) {
                            Icon(
                                if (showNsec) Icons.Default.VisibilityOff else Icons.Default.Visibility,
                                contentDescription = if (showNsec) "Hide nsec" else "Show nsec",
                                modifier = Modifier.size(18.dp),
                            )
                        }
                        IconButton(onClick = {
                            clipboard.setText(AnnotatedString(nsec))
                            Toast.makeText(ctx, "Copied", Toast.LENGTH_SHORT).show()
                        }) {
                            Icon(Icons.Default.ContentCopy, contentDescription = "Copy nsec", modifier = Modifier.size(18.dp))
                        }
                    }
                    Text(
                        "Keep this private. Anyone with your nsec can control your account.",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }

            // Logout
            item {
                HorizontalDivider()
                Spacer(Modifier.height(4.dp))
                Button(
                    onClick = { showLogoutConfirm = true },
                    colors = ButtonDefaults.buttonColors(
                        containerColor = MaterialTheme.colorScheme.error,
                    ),
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text("Log out")
                }
                Text(
                    "You can log back in with your nsec.",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(24.dp))
            }
        }
    }

    if (showLogoutConfirm) {
        AlertDialog(
            onDismissRequest = { showLogoutConfirm = false },
            title = { Text("Log out?") },
            text = { Text("You can log back in with your nsec.") },
            confirmButton = {
                TextButton(onClick = {
                    manager.logout()
                    showLogoutConfirm = false
                    onDismiss()
                }) {
                    Text("Log out", color = MaterialTheme.colorScheme.error)
                }
            },
            dismissButton = {
                TextButton(onClick = { showLogoutConfirm = false }) {
                    Text("Cancel")
                }
            },
        )
    }
}
