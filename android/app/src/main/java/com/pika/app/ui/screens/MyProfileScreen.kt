package com.pika.app.ui.screens

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.util.Base64
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
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
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.surfaceColorAtElevation
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Visibility
import androidx.compose.material.icons.filled.VisibilityOff
import androidx.core.content.pm.PackageInfoCompat
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.ui.Avatar
import com.pika.app.ui.QrCode
import com.pika.app.ui.TestTags
import java.io.ByteArrayOutputStream

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
    var showWipeConfirm by remember { mutableStateOf(false) }
    var isLoadingPhoto by remember { mutableStateOf(false) }
    var buildNumberTapCount by remember { mutableStateOf(0) }
    val developerModeEnabled = manager.state.developerMode

    val nsec = remember { manager.getNsec() }

    val hasChanges = nameDraft.trim() != profile.name.trim() ||
        aboutDraft.trim() != profile.about.trim()
    val appVersionDisplay = remember {
        runCatching {
            val packageInfo = ctx.packageManager.getPackageInfo(ctx.packageName, 0)
            val versionName = packageInfo.versionName ?: "unknown"
            val versionCode = PackageInfoCompat.getLongVersionCode(packageInfo)
            "v$versionName ($versionCode)"
        }.getOrDefault("unknown")
    }

    fun copyValue(value: String, label: String) {
        clipboard.setText(AnnotatedString(value))
        Toast.makeText(ctx, "$label copied", Toast.LENGTH_SHORT).show()
    }

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
        val prepared = prepareProfilePhotoUpload(ctx, uri)
        isLoadingPhoto = false
        if (prepared != null) {
            val (bytes, mimeType) = prepared
            val base64 = Base64.encodeToString(bytes, Base64.NO_WRAP)
            manager.dispatch(AppAction.UploadMyProfileImage(base64, mimeType))
        } else {
            Toast.makeText(ctx, "Could not read that image", Toast.LENGTH_SHORT).show()
        }
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
    ) {
        LazyColumn(
            modifier = Modifier
                .padding(horizontal = 20.dp)
                .testTag(TestTags.MYPROFILE_SHEET_LIST),
            contentPadding = PaddingValues(top = 4.dp, bottom = 28.dp),
            verticalArrangement = Arrangement.spacedBy(18.dp),
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
                ProfileSectionCard(title = "Profile") {
                    OutlinedTextField(
                        value = nameDraft,
                        onValueChange = { nameDraft = it },
                        label = { Text("Name") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    OutlinedTextField(
                        value = aboutDraft,
                        onValueChange = { aboutDraft = it },
                        label = { Text("About") },
                        maxLines = 4,
                        modifier = Modifier.fillMaxWidth(),
                    )
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
            }

            // Public key section
            item {
                ProfileSectionCard(title = "Public Key") {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            npub,
                            style = MaterialTheme.typography.bodyMedium,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f),
                        )
                        IconButton(onClick = {
                            copyValue(npub, "npub")
                        }, modifier = Modifier.testTag(TestTags.MYPROFILE_COPY_NPUB)) {
                            Icon(Icons.Default.ContentCopy, contentDescription = "Copy npub", modifier = Modifier.size(18.dp))
                        }
                    }
                }
            }

            // QR code
            item {
                val qr = remember(npub) { QrCode.encode(npub, 512).asImageBitmap() }
                ProfileSectionCard {
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
            }

            // Private key section
            if (nsec != null) {
                item {
                    ProfileSectionCard(title = "Private Key (nsec)") {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(
                                if (showNsec) nsec else "\u2022".repeat(24),
                                style = MaterialTheme.typography.bodyMedium,
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
                                copyValue(nsec, "nsec")
                            }) {
                                Icon(Icons.Default.ContentCopy, contentDescription = "Copy nsec", modifier = Modifier.size(18.dp))
                            }
                        }
                        Text(
                            "Keep this private. Anyone with your nsec can control your account.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.error,
                        )
                    }
                }
            }

            // App version / build
            item {
                ProfileSectionCard(title = "App Version") {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        TextButton(
                            onClick = {
                                if (developerModeEnabled) {
                                    Toast.makeText(ctx, "Developer mode already enabled", Toast.LENGTH_SHORT).show()
                                } else {
                                    buildNumberTapCount += 1
                                    val remaining = 7 - buildNumberTapCount
                                    if (remaining <= 0) {
                                        manager.enableDeveloperMode()
                                        Toast.makeText(ctx, "Developer mode enabled", Toast.LENGTH_SHORT).show()
                                    } else {
                                        val noun = if (remaining == 1) "tap" else "taps"
                                        Toast.makeText(
                                            ctx,
                                            "$remaining $noun away from developer mode",
                                            Toast.LENGTH_SHORT,
                                        ).show()
                                    }
                                }
                            },
                            modifier = Modifier.weight(1f),
                        ) {
                            Text(
                                appVersionDisplay,
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurface,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                        IconButton(onClick = { copyValue(appVersionDisplay, "Version") }) {
                            Icon(
                                Icons.Default.ContentCopy,
                                contentDescription = "Copy app version",
                                modifier = Modifier.size(18.dp),
                            )
                        }
                    }
                    if (developerModeEnabled) {
                        Text(
                            "Developer mode enabled.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }

            if (developerModeEnabled) {
                item {
                    ProfileSectionCard(title = "Developer Mode") {
                        Button(
                            onClick = { showWipeConfirm = true },
                            colors = ButtonDefaults.buttonColors(
                                containerColor = MaterialTheme.colorScheme.error,
                            ),
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Text("Wipe All Local Data")
                        }
                        Text(
                            "Deletes all local Pika data on this device and logs out immediately.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }

            // Logout
            item {
                ProfileSectionCard {
                    Button(
                        onClick = { showLogoutConfirm = true },
                        colors = ButtonDefaults.buttonColors(
                            containerColor = MaterialTheme.colorScheme.error,
                        ),
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag(TestTags.MYPROFILE_LOGOUT),
                    ) {
                        Text("Log out")
                    }
                    Text(
                        "You can log back in with your nsec.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }

    if (showLogoutConfirm) {
        AlertDialog(
            onDismissRequest = { showLogoutConfirm = false },
            title = { Text("Log out?") },
            text = { Text("You can log back in with your nsec.") },
            confirmButton = {
                TextButton(
                    onClick = {
                        manager.logout()
                        showLogoutConfirm = false
                        onDismiss()
                    },
                    modifier = Modifier.testTag(TestTags.MYPROFILE_LOGOUT_CONFIRM),
                ) {
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

    if (showWipeConfirm) {
        AlertDialog(
            onDismissRequest = { showWipeConfirm = false },
            title = { Text("Wipe all local data?") },
            text = { Text("This deletes local databases, caches, and local state. This cannot be undone.") },
            confirmButton = {
                TextButton(onClick = {
                    manager.wipeLocalDataForDeveloperTools()
                    showWipeConfirm = false
                    onDismiss()
                }) {
                    Text("Wipe All Local Data", color = MaterialTheme.colorScheme.error)
                }
            },
            dismissButton = {
                TextButton(onClick = { showWipeConfirm = false }) {
                    Text("Cancel")
                }
            },
        )
    }
}

@Composable
private fun ProfileSectionCard(
    title: String? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors =
            CardDefaults.cardColors(
                containerColor = MaterialTheme.colorScheme.surfaceColorAtElevation(3.dp),
            ),
        shape = MaterialTheme.shapes.large,
    ) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (title != null) {
                ProfileSectionTitle(title)
            }
            content()
        }
    }
}

@Composable
private fun ProfileSectionTitle(text: String) {
    Text(
        text = text,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

private fun prepareProfilePhotoUpload(ctx: Context, uri: Uri): Pair<ByteArray, String>? {
    val originalBytes = ctx.contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: return null
    if (originalBytes.isEmpty()) return null

    val originalMime = ctx.contentResolver.getType(uri)?.lowercase().orEmpty()
    val fallbackMime = if (originalMime.startsWith("image/")) originalMime else "image/jpeg"

    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(originalBytes, 0, originalBytes.size, bounds)
    if (bounds.outWidth <= 0 || bounds.outHeight <= 0) {
        return originalBytes to fallbackMime
    }

    val maxDimension = 1600
    var sample = 1
    while (bounds.outWidth / sample > maxDimension || bounds.outHeight / sample > maxDimension) {
        sample *= 2
    }

    val decodeOpts = BitmapFactory.Options().apply { inSampleSize = sample }
    val bitmap = BitmapFactory.decodeByteArray(originalBytes, 0, originalBytes.size, decodeOpts)
        ?: return originalBytes to fallbackMime

    val targetMime = if (originalMime == "image/png") "image/png" else "image/jpeg"
    val output = ByteArrayOutputStream()
    val compressed =
        if (targetMime == "image/png") {
            bitmap.compress(Bitmap.CompressFormat.PNG, 100, output)
        } else {
            bitmap.compress(Bitmap.CompressFormat.JPEG, 86, output)
        }
    bitmap.recycle()
    if (!compressed) return originalBytes to fallbackMime

    val processed = output.toByteArray()
    if (processed.isEmpty()) return originalBytes to fallbackMime
    return processed to targetMime
}
