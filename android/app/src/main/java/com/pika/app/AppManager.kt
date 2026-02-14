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
    private val audioFocus = AndroidAudioFocusManager(context.applicationContext)
    private val rust: FfiApp
    private var lastRevApplied: ULong = 0UL
    private val listening = AtomicBoolean(false)

    var state: AppState by mutableStateOf(
        AppState(
            rev = 0UL,
            router = com.pika.app.rust.Router(
                defaultScreen = com.pika.app.rust.Screen.Login,
                screenStack = emptyList(),
            ),
            auth = com.pika.app.rust.AuthState.LoggedOut,
            busy = com.pika.app.rust.BusyState(
                creatingAccount = false,
                loggingIn = false,
                creatingChat = false,
            ),
            chatList = emptyList(),
            currentChat = null,
            activeCall = null,
            toast = null,
        ),
    )
        private set

    init {
        val dataDir = context.filesDir.absolutePath
        rust = FfiApp(dataDir)
        val initial = rust.state()
        state = initial
        audioFocus.syncForCall(initial.activeCall)
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

            // The stream is full-state snapshots; drop anything stale.
            if (updateRev <= lastRevApplied) return@post

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
            }
            audioFocus.syncForCall(state.activeCall)
        }
    }

    private fun AppUpdate.rev(): ULong =
        when (this) {
            is AppUpdate.FullState -> this.v1.rev
            is AppUpdate.AccountCreated -> this.rev
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
