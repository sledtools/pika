package com.pika.app.ui.screens

import android.Manifest
import android.content.pm.PackageManager
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.core.content.ContextCompat
import com.pika.app.AppManager
import com.pika.app.rust.AppAction
import com.pika.app.rust.CallState
import com.pika.app.rust.CallStatus
import com.pika.app.ui.TestTags

@Composable
fun CallSurface(manager: AppManager, chatId: String, onDismiss: () -> Unit) {
    val ctx = LocalContext.current
    val activeCall = manager.state.activeCall
    val callForChat = activeCall?.takeIf { it.chatId == chatId }
    val hasLiveCallElsewhere = activeCall?.let { it.chatId != chatId && it.isLive } ?: false
    val shouldEnableProximityLock = callForChat?.shouldEnableProximityLock == true
    val sensorManager = remember(ctx) { ctx.getSystemService(SensorManager::class.java) }
    val proximitySensor = remember(sensorManager) { sensorManager?.getDefaultSensor(Sensor.TYPE_PROXIMITY) }

    var pendingMicAction by remember { mutableStateOf<PendingMicAction?>(null) }
    var isProximityLocked by remember { mutableStateOf(false) }

    DisposableEffect(shouldEnableProximityLock, sensorManager, proximitySensor) {
        val managerForSensor = sensorManager
        val sensor = proximitySensor
        if (!shouldEnableProximityLock || managerForSensor == null || sensor == null) {
            isProximityLocked = false
            return@DisposableEffect onDispose { isProximityLocked = false }
        }

        val listener =
            object : SensorEventListener {
                override fun onSensorChanged(event: SensorEvent?) {
                    val distance = event?.values?.firstOrNull() ?: return
                    isProximityLocked = distance < sensor.maximumRange
                }

                override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) = Unit
            }
        managerForSensor.registerListener(listener, sensor, SensorManager.SENSOR_DELAY_NORMAL)
        onDispose {
            managerForSensor.unregisterListener(listener)
            isProximityLocked = false
        }
    }

    val micPermissionLauncher =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            val action = pendingMicAction
            pendingMicAction = null
            if (granted && action != null) {
                dispatchMicAction(manager, chatId, action)
            } else if (!granted) {
                Toast.makeText(ctx, "Microphone permission is required for calls.", Toast.LENGTH_SHORT).show()
            }
        }

    val dispatchWithMicPermission: (PendingMicAction) -> Unit = { action ->
        val hasMic =
            ContextCompat.checkSelfPermission(ctx, Manifest.permission.RECORD_AUDIO) ==
                PackageManager.PERMISSION_GRANTED
        if (hasMic) {
            dispatchMicAction(manager, chatId, action)
        } else {
            pendingMicAction = action
            micPermissionLauncher.launch(Manifest.permission.RECORD_AUDIO)
        }
    }

    Dialog(
        onDismissRequest = onDismiss,
        properties =
            DialogProperties(
                usePlatformDefaultWidth = false,
                dismissOnBackPress = true,
                dismissOnClickOutside = false,
            ),
    ) {
        Surface(
            modifier = Modifier.fillMaxSize(),
            color = MaterialTheme.colorScheme.background,
        ) {
            Box(modifier = Modifier.fillMaxSize()) {
                val scrollState = rememberScrollState()
                Column(
                    modifier =
                        Modifier
                            .fillMaxSize()
                            .verticalScroll(scrollState)
                            .padding(horizontal = 20.dp, vertical = 24.dp)
                            .padding(bottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()),
                    verticalArrangement = Arrangement.spacedBy(24.dp),
                ) {
                    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.SpaceBetween,
                        ) {
                            Text(
                                text = "Call",
                                style = MaterialTheme.typography.headlineMedium,
                            )
                            IconButton(onClick = onDismiss) {
                                Icon(Icons.Default.Close, contentDescription = "Close call screen")
                            }
                        }

                        if (callForChat != null) {
                            Text(
                                text = callStatusText(callForChat),
                                style = MaterialTheme.typography.titleMedium,
                            )
                            callForChat.debug?.let { debug ->
                                Text(
                                    text = "tx ${debug.txFrames}  rx ${debug.rxFrames}  drop ${debug.rxDropped}",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                        } else {
                            Text(
                                text =
                                    if (hasLiveCallElsewhere) {
                                        "Another call is already active."
                                    } else {
                                        "Start a call from this chat."
                                    },
                                style = MaterialTheme.typography.titleMedium,
                            )
                        }
                    }

                    Column(
                        modifier = Modifier.fillMaxWidth(),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        when (callForChat?.status) {
                            null -> {
                                Button(
                                    onClick = { dispatchWithMicPermission(PendingMicAction.Start) },
                                    enabled = !hasLiveCallElsewhere,
                                    modifier = Modifier.fillMaxWidth().testTag(TestTags.CHAT_CALL_START),
                                ) {
                                    Text("Start Call")
                                }
                            }
                            is CallStatus.Ringing -> {
                                Button(
                                    onClick = { dispatchWithMicPermission(PendingMicAction.Accept) },
                                    modifier = Modifier.fillMaxWidth().testTag(TestTags.CHAT_CALL_ACCEPT),
                                ) {
                                    Text("Accept")
                                }
                                Button(
                                    onClick = { manager.dispatch(AppAction.RejectCall(chatId)) },
                                    modifier = Modifier.fillMaxWidth().testTag(TestTags.CHAT_CALL_REJECT),
                                ) {
                                    Text("Reject")
                                }
                            }
                            is CallStatus.Offering, is CallStatus.Connecting, is CallStatus.Active -> {
                                Button(
                                    onClick = { manager.dispatch(AppAction.ToggleMute) },
                                    modifier = Modifier.fillMaxWidth().testTag(TestTags.CHAT_CALL_MUTE),
                                ) {
                                    Text(if (callForChat.isMuted) "Unmute" else "Mute")
                                }
                                Button(
                                    onClick = { manager.dispatch(AppAction.EndCall) },
                                    modifier = Modifier.fillMaxWidth().testTag(TestTags.CHAT_CALL_END),
                                ) {
                                    Text("End")
                                }
                            }
                            is CallStatus.Ended -> {
                                Button(
                                    onClick = { dispatchWithMicPermission(PendingMicAction.Start) },
                                    modifier = Modifier.fillMaxWidth().testTag(TestTags.CHAT_CALL_START),
                                ) {
                                    Text("Start Again")
                                }
                            }
                        }

                        if (callForChat?.status is CallStatus.Ended) {
                            Spacer(Modifier.height(8.dp))
                            Text(
                                text = "You can close this screen or start another call.",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }

                if (isProximityLocked) {
                    Box(
                        modifier =
                            Modifier
                                .fillMaxSize()
                                .background(Color.Black)
                                .clickable(
                                    interactionSource = remember { MutableInteractionSource() },
                                    indication = null,
                                    onClick = {},
                                ),
                    )
                }
            }
        }
    }
}

private enum class PendingMicAction {
    Start,
    Accept,
}

private fun dispatchMicAction(manager: AppManager, chatId: String, action: PendingMicAction) {
    when (action) {
        PendingMicAction.Start -> {
            manager.dispatch(AppAction.OpenChat(chatId))
            manager.dispatch(AppAction.StartCall(chatId))
        }
        PendingMicAction.Accept -> {
            manager.dispatch(AppAction.OpenChat(chatId))
            manager.dispatch(AppAction.AcceptCall(chatId))
        }
    }
}

private fun callStatusText(call: CallState): String =
    when (val status = call.status) {
        is CallStatus.Offering -> "Calling..."
        is CallStatus.Ringing -> "Incoming call"
        is CallStatus.Connecting -> "Connecting..."
        is CallStatus.Active -> "Call active"
        is CallStatus.Ended -> "Call ended: ${status.reason}"
    }
