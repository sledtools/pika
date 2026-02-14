package com.pika.app

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import com.pika.app.rust.CallState
import com.pika.app.rust.CallStatus

internal class AndroidAudioFocusManager(context: Context) {
    private val audioManager = context.getSystemService(AudioManager::class.java)
    private var focusRequest: AudioFocusRequest? = null
    private var hasFocus = false
    private val focusChangeListener =
        AudioManager.OnAudioFocusChangeListener { change ->
            if (change == AudioManager.AUDIOFOCUS_LOSS) {
                hasFocus = false
            }
        }

    fun syncForCall(call: CallState?) {
        if (call != null && isLiveCallStatus(call.status)) {
            requestFocus()
        } else {
            abandonFocus()
        }
    }

    private fun requestFocus() {
        val manager = audioManager ?: return
        if (hasFocus) return
        val req =
            focusRequest ?: AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_EXCLUSIVE)
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                        .build(),
                )
                .setOnAudioFocusChangeListener(focusChangeListener)
                .setAcceptsDelayedFocusGain(false)
                .build()
                .also { focusRequest = it }

        hasFocus = manager.requestAudioFocus(req) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
    }

    private fun abandonFocus() {
        val manager = audioManager ?: return
        if (!hasFocus) return
        focusRequest?.let { manager.abandonAudioFocusRequest(it) }
        hasFocus = false
    }
}

private fun isLiveCallStatus(status: CallStatus): Boolean =
    when (status) {
        is CallStatus.Offering,
        is CallStatus.Ringing,
        is CallStatus.Connecting,
        is CallStatus.Active,
        -> true
        is CallStatus.Ended -> false
    }
