package com.pika.app.ui.screens

import android.widget.Toast
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
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.rust.PeerProfileState
import com.pika.app.ui.Avatar
import com.pika.app.ui.QrCode

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PeerProfileSheet(
    manager: AppManager,
    profile: PeerProfileState,
    onDismiss: () -> Unit,
) {
    val ctx = LocalContext.current
    val clipboard = LocalClipboardManager.current
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)

    ModalBottomSheet(
        onDismissRequest = {
            manager.dispatch(AppAction.ClosePeerProfile)
            onDismiss()
        },
        sheetState = sheetState,
    ) {
        LazyColumn(
            modifier = Modifier.padding(horizontal = 20.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            // Avatar
            item {
                Column(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Avatar(
                        name = profile.name,
                        npub = profile.npub,
                        pictureUrl = profile.pictureUrl,
                        size = 96.dp,
                    )
                }
            }

            // Name and about
            if (profile.name != null || profile.about != null) {
                item {
                    Text("Profile", style = MaterialTheme.typography.titleSmall)
                    Spacer(Modifier.height(4.dp))
                    if (profile.name != null) {
                        Text(
                            profile.name!!,
                            style = MaterialTheme.typography.headlineSmall,
                        )
                    }
                    if (profile.about != null) {
                        Text(
                            profile.about!!,
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }

            // Public key
            item {
                HorizontalDivider()
                Text("Public Key", style = MaterialTheme.typography.titleSmall)
                Spacer(Modifier.height(4.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        profile.npub,
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    IconButton(onClick = {
                        clipboard.setText(AnnotatedString(profile.npub))
                        Toast.makeText(ctx, "Copied", Toast.LENGTH_SHORT).show()
                    }) {
                        Icon(Icons.Default.ContentCopy, contentDescription = "Copy npub", modifier = Modifier.size(18.dp))
                    }
                }
            }

            // QR code
            item {
                val qr = remember(profile.npub) { QrCode.encode(profile.npub, 512).asImageBitmap() }
                Column(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Image(
                        bitmap = qr,
                        contentDescription = "Peer npub QR",
                        modifier = Modifier.size(200.dp).clip(MaterialTheme.shapes.medium),
                    )
                }
            }

            // Follow / Unfollow
            item {
                HorizontalDivider()
                Spacer(Modifier.height(4.dp))
                if (profile.isFollowed) {
                    Button(
                        onClick = { manager.dispatch(AppAction.UnfollowUser(profile.pubkey)) },
                        colors = ButtonDefaults.buttonColors(
                            containerColor = MaterialTheme.colorScheme.error,
                        ),
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text("Unfollow")
                    }
                } else {
                    Button(
                        onClick = { manager.dispatch(AppAction.FollowUser(profile.pubkey)) },
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text("Follow")
                    }
                }
                Spacer(Modifier.height(24.dp))
            }
        }
    }
}
