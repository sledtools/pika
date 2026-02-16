package com.pika.app.ui.screens

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import com.pika.app.AppManager
import com.pika.app.rust.AppAction

@Composable
@OptIn(ExperimentalMaterial3Api::class)
fun NewGroupChatScreen(manager: AppManager, padding: PaddingValues) {
    Scaffold(
        modifier = Modifier.padding(padding),
        topBar = {
            TopAppBar(
                title = { Text("New group chat") },
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
        Box(modifier = Modifier.fillMaxSize().padding(inner), contentAlignment = Alignment.Center) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Text("Group chat UI isn’t implemented on Android yet.")
            }
        }
    }
}

