package com.pika.app

import com.pika.app.rust.AudioPlayoutReceiver
import com.pika.app.rust.CallState
import com.pika.app.rust.FfiApp
import java.util.concurrent.atomic.AtomicBoolean

internal class AndroidCallAudioIo(
    private val rust: FfiApp,
) : AudioPlayoutReceiver {
    private val running = AtomicBoolean(false)
    @Volatile private var muted: Boolean = false
    @Volatile private var captureThread: Thread? = null

    fun syncForCall(call: CallState?) {
        val live = call?.isLive ?: false
        muted = call?.isMuted ?: false
        if (live) {
            startIfNeeded()
        } else {
            stopIfNeeded()
        }
    }

    override fun `onAudioPlayoutFrame`(`callId`: String, `pcm`: List<Short>) {
        // Step-4 scaffold: callback is wired; real AudioTrack playout lands in native bridge stage.
    }

    private fun startIfNeeded() {
        if (!running.compareAndSet(false, true)) return
        rust.`setAudioPlayoutReceiver`(this)
        rust.`notifyAudioDeviceRouteChanged`("android.default")
        rust.`notifyAudioInterruptionChanged`(false, "call_live")

        val t =
            Thread {
                val silentFrame = List(960) { 0.toShort() }
                while (running.get()) {
                    if (!muted) {
                        rust.`sendAudioCaptureFrame`(silentFrame)
                    }
                    try {
                        Thread.sleep(20)
                    } catch (_: InterruptedException) {
                        break
                    }
                }
            }
        t.isDaemon = true
        t.start()
        captureThread = t
    }

    private fun stopIfNeeded() {
        if (!running.compareAndSet(true, false)) return
        rust.`notifyAudioInterruptionChanged`(true, "call_stopped")
        captureThread?.interrupt()
        captureThread = null
    }
}
