package com.pika.app

import android.content.Context
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.pika.app.rust.AppAction
import com.pika.app.rust.AppReconciler
import com.pika.app.rust.AppState
import com.pika.app.rust.AppUpdate
import com.pika.app.rust.FfiApp
import java.util.concurrent.atomic.AtomicBoolean

class AppManager private constructor(context: Context) : AppReconciler {
    private val mainHandler = Handler(Looper.getMainLooper())
    private val secureStore = SecureNsecStore(context)
    private val rust: FfiApp
    private var lastRevApplied: ULong = 0UL
    private val listening = AtomicBoolean(false)
    private val resyncing = AtomicBoolean(false)
    private var maxRevSeenDuringResync: ULong = 0UL

    var state: AppState by mutableStateOf(
        AppState(
            rev = 0UL,
            router = com.pika.app.rust.Router(
                defaultScreen = com.pika.app.rust.Screen.Login,
                screenStack = emptyList(),
            ),
            auth = com.pika.app.rust.AuthState.LoggedOut,
            chatList = emptyList(),
            currentChat = null,
            toast = null,
        ),
    )
        private set

    init {
        val dataDir = context.filesDir.absolutePath
        rust = FfiApp(dataDir)
        val initial = rust.state()
        state = initial
        lastRevApplied = initial.rev
        startListening()

        val storedNsec = secureStore.getNsec()
        if (!storedNsec.isNullOrBlank()) {
            rust.dispatch(AppAction.RestoreSession(storedNsec))
        }
    }

    private fun startListening() {
        if (!listening.compareAndSet(false, true)) return
        rust.listenForUpdates(this)
    }

    fun dispatch(action: AppAction) {
        rust.dispatch(action)
    }

    fun loginWithNsec(nsec: String) {
        if (nsec.isNotBlank()) {
            secureStore.setNsec(nsec)
        }
        rust.dispatch(AppAction.Login(nsec))
    }

    fun logout() {
        secureStore.clearNsec()
        rust.dispatch(AppAction.Logout)
    }

    fun onForeground() {
        // Foreground is a lifecycle signal; Rust owns state changes and side effects.
        rust.dispatch(AppAction.Foregrounded)
    }

    override fun reconcile(update: AppUpdate) {
        mainHandler.post {
            val updateRev = update.rev()

            // Side-effect updates must not be lost: `AccountCreated` carries an `nsec` that isn't in
            // AppState snapshots (by design). Store it even if the update is stale w.r.t. rev.
            if (update is AppUpdate.AccountCreated) {
                val existing = secureStore.getNsec().orEmpty()
                if (existing.isBlank() && update.nsec.isNotBlank()) {
                    secureStore.setNsec(update.nsec)
                }
            }

            // After a resync, older updates can still be in-flight on the main handler queue.
            // Drop them. Only treat *forward* gaps as a reason to resync.
            if (updateRev <= lastRevApplied) return@post
            // While resyncing, drop updates but remember the newest rev we've observed so we can
            // resync again if the snapshot is behind (prevents falling permanently behind).
            if (resyncing.get()) {
                if (updateRev > maxRevSeenDuringResync) {
                    maxRevSeenDuringResync = updateRev
                }
                return@post
            }
            if (updateRev > lastRevApplied + 1UL) {
                if (updateRev > maxRevSeenDuringResync) {
                    maxRevSeenDuringResync = updateRev
                }
                requestResync()
                return@post
            }

            lastRevApplied = updateRev
            when (update) {
                is AppUpdate.FullState -> state = update.v1
                is AppUpdate.AccountCreated -> {
                    // Required by spec-v2: native stores nsec; Rust never persists it.
                    if (update.nsec.isNotBlank()) {
                        secureStore.setNsec(update.nsec)
                    }
                    state = state.copy(rev = updateRev)
                }
                is AppUpdate.RouterChanged -> state = state.copy(rev = updateRev, router = update.router)
                is AppUpdate.AuthChanged -> state = state.copy(rev = updateRev, auth = update.auth)
                is AppUpdate.ChatListChanged -> state = state.copy(rev = updateRev, chatList = update.chatList)
                is AppUpdate.CurrentChatChanged -> state = state.copy(rev = updateRev, currentChat = update.currentChat)
                is AppUpdate.ToastChanged -> {
                    state = state.copy(rev = updateRev, toast = update.toast)
                }
            }
        }
    }

    private fun requestResync() {
        if (!resyncing.compareAndSet(false, true)) return
        Thread {
            val snapshot = rust.state()
            mainHandler.post {
                state = snapshot
                if (snapshot.rev > lastRevApplied) {
                    lastRevApplied = snapshot.rev
                }
                val maxSeen = maxRevSeenDuringResync
                maxRevSeenDuringResync = 0UL
                resyncing.set(false)

                // If newer updates arrived while the snapshot was in-flight and the snapshot is
                // behind, resync again (coalesced) rather than dropping ourselves out of date.
                if (maxSeen > snapshot.rev) {
                    maxRevSeenDuringResync = maxSeen
                    requestResync()
                }
            }
        }.start()
    }

    private fun AppUpdate.rev(): ULong =
        when (this) {
            is AppUpdate.FullState -> this.v1.rev
            is AppUpdate.AccountCreated -> this.rev
            is AppUpdate.RouterChanged -> this.rev
            is AppUpdate.AuthChanged -> this.rev
            is AppUpdate.ChatListChanged -> this.rev
            is AppUpdate.CurrentChatChanged -> this.rev
            is AppUpdate.ToastChanged -> this.rev
        }

    companion object {
        @Volatile
        private var instance: AppManager? = null

        fun getInstance(context: Context): AppManager =
            instance ?: synchronized(this) {
                instance ?: AppManager(context.applicationContext).also { instance = it }
            }
    }
}
