package com.pika.app.ui.screens

import androidx.compose.foundation.layout.Arrangement
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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.pika.app.AppManager
import com.pika.app.rust.AppAction

@Composable
@OptIn(ExperimentalMaterial3Api::class)
fun GroupInfoScreen(manager: AppManager, chatId: String, padding: PaddingValues) {
    val chat = manager.state.chatList.firstOrNull { it.chatId == chatId }
    val title = chat?.groupName?.trim().takeIf { !it.isNullOrBlank() } ?: "Group info"

    Scaffold(
        modifier = Modifier.padding(padding),
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        text = title,
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
        Box(modifier = Modifier.fillMaxSize().padding(inner), contentAlignment = Alignment.Center) {
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier.padding(16.dp),
            ) {
                Text("Group details", style = MaterialTheme.typography.titleMedium)
                if (chat == null) {
                    Text("Missing chat in state.")
                } else {
                    Text("Members: ${chat.members.size}")
                }
            }
        }
    }
}

