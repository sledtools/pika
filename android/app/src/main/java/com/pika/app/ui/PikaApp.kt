package com.pika.app.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.rust.Screen
import com.pika.app.ui.screens.ChatListScreen
import com.pika.app.ui.screens.ChatScreen
import com.pika.app.ui.screens.GroupInfoScreen
import com.pika.app.ui.screens.LoginScreen
import com.pika.app.ui.screens.NewChatScreen
import com.pika.app.ui.screens.NewGroupChatScreen

@Composable
fun PikaApp(manager: AppManager) {
    val snackbarHostState = remember { SnackbarHostState() }
    val state = manager.state

    LaunchedEffect(state.toast) {
        val msg = state.toast ?: return@LaunchedEffect
        snackbarHostState.showSnackbar(message = msg)
        manager.dispatch(AppAction.ClearToast)
    }

    Scaffold(
        modifier = Modifier.fillMaxSize(),
        snackbarHost = { SnackbarHost(hostState = snackbarHostState) },
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
                        is Screen.Chat -> ChatScreen(manager = manager, chatId = screen.chatId, padding = padding)
                        is Screen.GroupInfo -> GroupInfoScreen(manager = manager, chatId = screen.chatId, padding = padding)
                        is Screen.Login -> LoginScreen(manager = manager, padding = padding)
                    }
                }
            }
        }
    }
}
