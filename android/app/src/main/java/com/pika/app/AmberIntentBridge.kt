package com.pika.app

import android.app.Activity
import android.content.Intent
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import java.lang.ref.WeakReference
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicReference

data class AmberIntentOutcome(
    val ok: Boolean,
    val resultCode: Int,
    val data: Intent? = null,
    val error: String? = null,
)

object AmberIntentBridge {
    private val activityRef = AtomicReference<WeakReference<ComponentActivity>?>(null)
    private val launcherRef = AtomicReference<ActivityResultLauncher<Intent>?>(null)
    private val pending = AtomicReference<CompletableFuture<AmberIntentOutcome>?>(null)

    fun bind(activity: ComponentActivity, launcher: ActivityResultLauncher<Intent>) {
        activityRef.set(WeakReference(activity))
        launcherRef.set(launcher)
    }

    fun unbind(activity: ComponentActivity) {
        val current = activityRef.get()?.get()
        if (current === activity) {
            activityRef.set(null)
            launcherRef.set(null)
        }
    }

    fun complete(resultCode: Int, data: Intent?) {
        val future = pending.getAndSet(null) ?: return
        // Bring Pika back to the foreground — Amber may leave its own activity
        // on top of our task stack even after finishing the result handshake.
        activityRef.get()?.get()?.let { activity ->
            if (!activity.hasWindowFocus()) {
                val bringBack = activity.packageManager.getLaunchIntentForPackage(activity.packageName)
                if (bringBack != null) {
                    bringBack.addFlags(
                        Intent.FLAG_ACTIVITY_REORDER_TO_FRONT or
                            Intent.FLAG_ACTIVITY_SINGLE_TOP or
                            Intent.FLAG_ACTIVITY_CLEAR_TOP,
                    )
                    activity.startActivity(bringBack)
                }
            }
        }
        future.complete(
            AmberIntentOutcome(
                ok = resultCode == Activity.RESULT_OK,
                resultCode = resultCode,
                data = data,
            ),
        )
    }

    fun launch(intent: Intent, timeoutMs: Long = 120_000L): AmberIntentOutcome {
        val activity = activityRef.get()?.get()
        val launcher = launcherRef.get()
        if (activity == null || launcher == null) {
            return AmberIntentOutcome(
                ok = false,
                resultCode = Activity.RESULT_CANCELED,
                error = "signer unavailable: activity bridge not ready",
            )
        }

        val future = CompletableFuture<AmberIntentOutcome>()
        if (!pending.compareAndSet(null, future)) {
            return AmberIntentOutcome(
                ok = false,
                resultCode = Activity.RESULT_CANCELED,
                error = "signer unavailable: another signer request is already pending",
            )
        }

        activity.runOnUiThread {
            runCatching {
                launcher.launch(intent)
            }.onFailure { err ->
                val pendingFuture = pending.getAndSet(null)
                pendingFuture?.complete(
                    AmberIntentOutcome(
                        ok = false,
                        resultCode = Activity.RESULT_CANCELED,
                        error = "signer unavailable: ${err.message ?: "launch failed"}",
                    ),
                )
            }
        }

        return try {
            future.get(timeoutMs, TimeUnit.MILLISECONDS)
        } catch (_: TimeoutException) {
            pending.compareAndSet(future, null)
            AmberIntentOutcome(
                ok = false,
                resultCode = Activity.RESULT_CANCELED,
                error = "timeout",
            )
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            pending.compareAndSet(future, null)
            AmberIntentOutcome(
                ok = false,
                resultCode = Activity.RESULT_CANCELED,
                error = "canceled",
            )
        }
    }
}
