package com.pika.app.ui.screens

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.Visibility
import androidx.compose.material.icons.filled.VisibilityOff
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.ui.TestTags

@Composable
fun LoginScreen(manager: AppManager, padding: PaddingValues) {
    var nsec by rememberSaveable { mutableStateOf("") }
    var nsecVisible by rememberSaveable { mutableStateOf(false) }
    var bunkerUri by rememberSaveable { mutableStateOf("") }
    var showAdvanced by rememberSaveable { mutableStateOf(false) }
    val busy = manager.state.busy
    val createBusy = busy.creatingAccount
    val loginBusy = busy.loggingIn
    val anyBusy = createBusy || loginBusy
    val advancedRotation by animateFloatAsState(targetValue = if (showAdvanced) 180f else 0f, label = "advanced_rotation")

    Column(
        modifier =
            Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(horizontal = 28.dp, vertical = 24.dp),
        verticalArrangement = Arrangement.SpaceBetween,
    ) {
        Column(
            modifier = Modifier.fillMaxWidth().weight(1f),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            Text(
                text = "Pika",
                style = MaterialTheme.typography.displaySmall,
            )
            Text(
                text = "Encrypted messaging over Nostr",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }

        Column(
            modifier = Modifier.fillMaxWidth(),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Button(
                onClick = {
                    manager.dispatch(AppAction.CreateAccount)
                },
                enabled = !anyBusy,
                modifier = Modifier.fillMaxWidth().testTag(TestTags.LOGIN_CREATE_ACCOUNT),
            ) {
                if (createBusy) {
                    CircularProgressIndicator(
                        modifier = Modifier.size(20.dp),
                        strokeWidth = 2.dp,
                    )
                } else {
                    Text("Create Account")
                }
            }

            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                HorizontalDivider(modifier = Modifier.weight(1f))
                Text(
                    text = "or",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                HorizontalDivider(modifier = Modifier.weight(1f))
            }

            OutlinedTextField(
                value = nsec,
                onValueChange = { nsec = it },
                singleLine = true,
                enabled = !anyBusy,
                label = { Text("nsec") },
                visualTransformation = if (nsecVisible) VisualTransformation.None else PasswordVisualTransformation(),
                keyboardOptions =
                    KeyboardOptions(
                        autoCorrect = false,
                        keyboardType = KeyboardType.Password,
                        imeAction = ImeAction.Done,
                    ),
                trailingIcon = {
                    Row {
                        if (nsec.isNotEmpty()) {
                            IconButton(onClick = { nsec = "" }) {
                                Icon(Icons.Default.Clear, contentDescription = "Clear")
                            }
                        }
                        IconButton(onClick = { nsecVisible = !nsecVisible }) {
                            Icon(
                                if (nsecVisible) Icons.Default.VisibilityOff else Icons.Default.Visibility,
                                contentDescription = if (nsecVisible) "Hide" else "Show",
                            )
                        }
                    }
                },
                modifier = Modifier.fillMaxWidth().testTag(TestTags.LOGIN_NSEC),
            )

            Button(
                onClick = {
                    manager.loginWithNsec(nsec.trim())
                },
                enabled = !anyBusy && nsec.isNotBlank(),
                modifier = Modifier.fillMaxWidth().testTag(TestTags.LOGIN_LOGIN),
            ) {
                if (loginBusy) {
                    CircularProgressIndicator(
                        modifier = Modifier.size(20.dp),
                        strokeWidth = 2.dp,
                    )
                } else {
                    Text("Log In")
                }
            }

            TextButton(
                onClick = { showAdvanced = !showAdvanced },
                enabled = !anyBusy,
                modifier = Modifier.align(Alignment.CenterHorizontally),
            ) {
                Text("Advanced")
                Icon(
                    imageVector = Icons.Default.ExpandMore,
                    contentDescription = null,
                    modifier = Modifier.rotate(advancedRotation),
                )
            }

            AnimatedVisibility(visible = showAdvanced) {
                Column(
                    modifier = Modifier.fillMaxWidth(),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    OutlinedTextField(
                        value = bunkerUri,
                        onValueChange = { bunkerUri = it },
                        singleLine = true,
                        enabled = !anyBusy,
                        label = { Text("bunker URI") },
                        trailingIcon = {
                            if (bunkerUri.isNotEmpty()) {
                                IconButton(onClick = { bunkerUri = "" }) {
                                    Icon(Icons.Default.Clear, contentDescription = "Clear")
                                }
                            }
                        },
                        modifier = Modifier.fillMaxWidth().testTag(TestTags.LOGIN_BUNKER_URI),
                    )

                    Button(
                        onClick = {
                            manager.loginWithBunker(bunkerUri.trim())
                        },
                        enabled = !anyBusy && bunkerUri.isNotBlank(),
                        modifier = Modifier.fillMaxWidth().testTag(TestTags.LOGIN_WITH_BUNKER),
                    ) {
                        Text("Log In with Bunker")
                    }

                    Button(
                        onClick = {
                            manager.loginWithNostrConnect()
                        },
                        enabled = !anyBusy,
                        modifier = Modifier.fillMaxWidth().testTag(TestTags.LOGIN_WITH_NOSTR_CONNECT),
                    ) {
                        Text("Log In with Nostr Connect")
                    }

                    TextButton(
                        onClick = {
                            manager.loginWithAmber()
                        },
                        enabled = !anyBusy,
                        modifier = Modifier.fillMaxWidth().testTag(TestTags.LOGIN_WITH_AMBER),
                    ) {
                        Text("Log In with Amber")
                    }
                }
            }
        }
    }
}
