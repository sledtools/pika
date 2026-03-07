package com.pika.app.ui

import android.content.Intent
import android.net.Uri
import android.widget.Toast
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.clickable
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.material3.Scaffold
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.rust.AgentProvisioningPhase
import com.pika.app.rust.Screen
import com.pika.app.ui.screens.CallSurface
import com.pika.app.ui.screens.ChatListScreen
import com.pika.app.ui.screens.ChatScreen
import com.pika.app.ui.screens.GroupInfoScreen
import com.pika.app.ui.screens.LoginScreen
import com.pika.app.ui.screens.NewChatScreen
import com.pika.app.ui.screens.NewGroupChatScreen
import com.pika.app.ui.screens.PeerProfileSheet

@Composable
fun PikaApp(manager: AppManager) {
    val context = LocalContext.current
    val state = manager.state
    var callSurfaceChatId by rememberSaveable { mutableStateOf<String?>(null) }
    var isCallSurfacePresented by rememberSaveable { mutableStateOf(false) }

    LaunchedEffect(state.toast) {
        val msg = state.toast ?: return@LaunchedEffect
        Toast.makeText(context, msg, Toast.LENGTH_LONG).show()
    }

    LaunchedEffect(state.activeCall?.callId, state.activeCall?.status) {
        val activeCall = state.activeCall
        if (activeCall == null) {
            isCallSurfacePresented = false
            callSurfaceChatId = null
            return@LaunchedEffect
        }

        if (activeCall.shouldAutoPresentCallScreen) {
            callSurfaceChatId = activeCall.chatId
            isCallSurfacePresented = true
        }
    }

    Scaffold(
        modifier = Modifier.fillMaxSize(),
        topBar = {
            if (state.updateRequired) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(Color(0xFF2196F3))
                        .clickable {
                            val intent = Intent(Intent.ACTION_VIEW, Uri.parse(
                                "https://play.google.com/store/apps/details?id=${context.packageName}"
                            ))
                            context.startActivity(intent)
                        }
                        .padding(horizontal = 16.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(
                        Icons.Default.ArrowUpward,
                        contentDescription = null,
                        tint = Color.White,
                        modifier = Modifier.size(20.dp),
                    )
                    Text(
                        "A new version of Pika is available. Please update.",
                        color = Color.White,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.padding(start = 8.dp),
                    )
                    Spacer(Modifier.weight(1f))
                }
            }
        },
    ) { padding ->
        val router = state.router
        when (router.defaultScreen) {
            is Screen.Login -> LoginScreen(manager = manager, padding = padding)
            else -> {
                BackHandler(enabled = router.screenStack.isNotEmpty()) {
                    val stack = router.screenStack
                    if (stack.isNotEmpty()) {
                        manager.dispatch(AppAction.UpdateScreenStack(stack.dropLast(1)))
                    }
                }

                val current = router.screenStack.lastOrNull() ?: router.defaultScreen
                AnimatedContent(
                    targetState = current,
                    transitionSpec = { fadeIn() togetherWith fadeOut() },
                    label = "router",
                ) { screen ->
                    when (screen) {
                        is Screen.ChatList -> ChatListScreen(manager = manager, padding = padding)
                        is Screen.NewChat -> NewChatScreen(manager = manager, padding = padding)
                        is Screen.NewGroupChat -> NewGroupChatScreen(manager = manager, padding = padding)
                        is Screen.Chat ->
                            ChatScreen(
                                manager = manager,
                                chatId = screen.chatId,
                                padding = padding,
                                onOpenCallSurface = { chatId ->
                                    callSurfaceChatId = chatId
                                    isCallSurfacePresented = true
                                },
                            )
                        is Screen.ChatMedia ->
                            ChatScreen(
                                manager = manager,
                                chatId = screen.chatId,
                                padding = padding,
                                onOpenCallSurface = { chatId ->
                                    callSurfaceChatId = chatId
                                    isCallSurfacePresented = true
                                },
                            )
                        is Screen.GroupInfo -> GroupInfoScreen(manager = manager, chatId = screen.chatId, padding = padding)
                        is Screen.Login -> LoginScreen(manager = manager, padding = padding)
                        is Screen.AgentProvisioning ->
                            AgentProvisioningScreen(
                                manager = manager,
                                padding = padding,
                                onBack = {
                                    manager.dispatch(AppAction.UpdateScreenStack(router.screenStack.dropLast(1)))
                                },
                            )
                    }
                }
            }
        }
    }

    if (isCallSurfacePresented) {
        val chatId = state.activeCall?.chatId ?: callSurfaceChatId
        if (chatId != null) {
            CallSurface(
                manager = manager,
                chatId = chatId,
                onDismiss = {
                    isCallSurfacePresented = false
                    if (state.activeCall == null) {
                        callSurfaceChatId = null
                    }
                },
            )
        }
    }

    state.peerProfile?.let { profile ->
        PeerProfileSheet(
            manager = manager,
            profile = profile,
            onDismiss = {},
        )
    }
}

@Composable
private fun AgentProvisioningScreen(
    manager: AppManager,
    padding: PaddingValues,
    onBack: () -> Unit,
) {
    val provisioning = manager.state.agentProvisioning
    val isError = provisioning?.phase == AgentProvisioningPhase.ERROR

    Column(
        modifier =
            Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(horizontal = 24.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        if (isError) {
            Icon(
                imageVector = Icons.Default.Warning,
                contentDescription = null,
                modifier = Modifier.size(44.dp),
                tint = MaterialTheme.colorScheme.error,
            )
            Spacer(modifier = Modifier.height(16.dp))
        } else {
            CircularProgressIndicator()
            Spacer(modifier = Modifier.height(16.dp))
        }

        Text(
            text = provisioning?.statusMessage ?: "Starting agent...",
            style = MaterialTheme.typography.titleMedium,
            color =
                if (isError) MaterialTheme.colorScheme.error
                else MaterialTheme.colorScheme.onSurface,
        )

        provisioning?.pollAttempt?.let { attempt ->
            val max = provisioning.pollMax ?: 0u
            if (!isError && max > 0u) {
                Spacer(modifier = Modifier.height(8.dp))
                Text(
                    text = "$attempt / $max",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }

        if (!isError && (provisioning?.elapsedSecs ?: 0u) > 0u) {
            Spacer(modifier = Modifier.height(4.dp))
            Text(
                text = "${provisioning?.elapsedSecs}s elapsed",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }

        provisioning?.agentNpub?.let { npub ->
            Spacer(modifier = Modifier.height(12.dp))
            Text(
                text = npub.take(20) + "...",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }

        if (isError) {
            Spacer(modifier = Modifier.height(24.dp))
            Row {
                Button(onClick = { manager.dispatch(AppAction.EnsureAgent) }) {
                    Text("Try Again")
                }
                Spacer(modifier = Modifier.width(12.dp))
                OutlinedButton(onClick = onBack) {
                    Text("Back")
                }
            }
        }
    }
}
