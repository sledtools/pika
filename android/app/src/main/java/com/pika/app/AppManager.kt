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
import com.pika.app.rust.AuthMode
import com.pika.app.rust.AuthState
import com.pika.app.rust.ExternalSignerBridge
import com.pika.app.rust.ExternalSignerErrorKind
import com.pika.app.rust.ExternalSignerHandshakeResult
import com.pika.app.rust.ExternalSignerResult
import com.pika.app.rust.FfiApp
import com.pika.app.rust.MyProfileState
import java.io.File
import java.util.concurrent.atomic.AtomicBoolean
import org.json.JSONObject

class AppManager private constructor(context: Context) : AppReconciler {
    private val appContext = context.applicationContext
    private val mainHandler = Handler(Looper.getMainLooper())
    private val secureStore = SecureAuthStore(appContext)
    private val amberClient = AmberSignerClient(appContext)
    private val signerRequestLock = Any()
    private val audioFocus = AndroidAudioFocusManager(appContext)
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
            myProfile = MyProfileState(name = "", about = "", pictureUrl = null),
            busy = com.pika.app.rust.BusyState(
                creatingAccount = false,
                loggingIn = false,
                creatingChat = false,
                fetchingFollowList = false,
            ),
            chatList = emptyList(),
            currentChat = null,
            followList = emptyList(),
            peerProfile = null,
            activeCall = null,
            toast = null,
        ),
    )
        private set

    init {
        // Ensure call config is present before Rust bootstraps. If the file already exists (e.g.
        // created by tooling), only fill missing keys to avoid clobbering overrides.
        ensureDefaultConfig(appContext)

        val dataDir = appContext.filesDir.absolutePath
        rust = FfiApp(dataDir)
        if (BuildConfig.ENABLE_AMBER_SIGNER) {
            rust.setExternalSignerBridge(AmberRustBridge())
        }
        val initial = rust.state()
        state = initial
        audioFocus.syncForCall(initial.activeCall)
        lastRevApplied = initial.rev
        startListening()
        restoreSessionFromSecureStore()
    }

    private fun ensureDefaultConfig(context: Context) {
        val filesDir = context.filesDir
        val path = File(filesDir, "pika_config.json")
        val defaultMoqUrl = "https://us-east.moq.logos.surf/anon"
        val defaultBroadcastPrefix = "pika/calls"

        val obj =
            runCatching {
                if (path.exists()) {
                    JSONObject(path.readText())
                } else {
                    JSONObject()
                }
            }.getOrElse { JSONObject() }

        fun isMissingOrBlank(key: String): Boolean {
            if (!obj.has(key)) return true
            val v = obj.optString(key, "").trim()
            return v.isEmpty()
        }

        if (isMissingOrBlank("call_moq_url")) {
            obj.put("call_moq_url", defaultMoqUrl)
        }
        if (isMissingOrBlank("call_broadcast_prefix")) {
            obj.put("call_broadcast_prefix", defaultBroadcastPrefix)
        }
        // Keep Rust-side signer gating in sync with Android build-time flag.
        // If callers provided an explicit value, respect it.
        if (!obj.has("enable_external_signer")) {
            obj.put("enable_external_signer", BuildConfig.ENABLE_AMBER_SIGNER)
        }

        runCatching {
            val tmp = File(filesDir, "pika_config.json.tmp")
            tmp.writeText(obj.toString())
            if (!tmp.renameTo(path)) {
                // Fallback for devices that don't allow rename across filesystems (shouldn't happen in app filesDir).
                path.writeText(obj.toString())
                tmp.delete()
            }
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
        val trimmed = nsec.trim()
        if (trimmed.isNotBlank()) {
            secureStore.saveLocalNsec(trimmed)
        }
        rust.dispatch(AppAction.Login(trimmed))
    }

    fun loginWithAmber() {
        val currentUserHint =
            secureStore
                .load()
                ?.takeIf { it.mode == StoredAuthMode.EXTERNAL_SIGNER }
                ?.currentUser
                ?.trim()
                ?.takeIf { it.isNotEmpty() }
        rust.dispatch(AppAction.BeginExternalSignerLogin(currentUserHint = currentUserHint))
    }

    fun logout() {
        secureStore.clear()
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
                val existing = secureStore.load()?.nsec.orEmpty()
                if (existing.isBlank() && update.nsec.isNotBlank()) {
                    secureStore.saveLocalNsec(update.nsec)
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
                        secureStore.saveLocalNsec(update.nsec)
                    }
                    state = state.copy(rev = updateRev)
                }
            }
            syncSecureStoreWithAuthState()
            audioFocus.syncForCall(state.activeCall)
        }
    }

    private fun AppUpdate.rev(): ULong =
        when (this) {
            is AppUpdate.FullState -> this.v1.rev
            is AppUpdate.AccountCreated -> this.rev
        }

    private fun restoreSessionFromSecureStore() {
        val stored = secureStore.load() ?: return
        when (stored.mode) {
            StoredAuthMode.LOCAL_NSEC -> {
                val nsec = stored.nsec?.trim().orEmpty()
                if (nsec.isNotEmpty()) {
                    rust.dispatch(AppAction.RestoreSession(nsec))
                }
            }
            StoredAuthMode.EXTERNAL_SIGNER -> {
                if (!BuildConfig.ENABLE_AMBER_SIGNER) return
                val pubkey = stored.pubkey?.trim().orEmpty()
                val signerPackage = stored.signerPackage?.trim().orEmpty()
                if (pubkey.isBlank() || signerPackage.isBlank()) return
                val currentUser = stored.currentUser?.trim().takeUnless { it.isNullOrEmpty() } ?: pubkey
                rust.dispatch(
                    AppAction.RestoreSessionExternalSigner(
                        pubkey = pubkey,
                        signerPackage = signerPackage,
                        currentUser = currentUser,
                    ),
                )
            }
        }
    }

    private fun syncSecureStoreWithAuthState() {
        when (val auth = state.auth) {
            is AuthState.LoggedOut -> Unit
            is AuthState.LoggedIn -> {
                when (val mode = auth.mode) {
                    is AuthMode.LocalNsec -> {
                        if (secureStore.load()?.mode == StoredAuthMode.EXTERNAL_SIGNER) {
                            secureStore.clear()
                        }
                    }
                    is AuthMode.ExternalSigner -> {
                        secureStore.saveExternalSigner(
                            pubkey = mode.pubkey,
                            signerPackage = mode.signerPackage,
                            currentUser = mode.currentUser,
                        )
                    }
                }
            }
        }
    }

    private inline fun <T> withSignerRequestLock(block: () -> T): T = synchronized(signerRequestLock) { block() }

    private inner class AmberRustBridge : ExternalSignerBridge {
        override fun requestPublicKey(currentUserHint: String?): ExternalSignerHandshakeResult =
            withSignerRequestLock {
                amberClient.requestPublicKey(currentUserHint).toExternalSignerHandshakeResult()
            }

        override fun signEvent(
            signerPackage: String,
            currentUser: String,
            unsignedEventJson: String,
        ): ExternalSignerResult =
            withSignerRequestLock {
                amberClient.signEvent(signerPackage, currentUser, unsignedEventJson).toExternalSignerResult()
            }

        override fun nip44Encrypt(
            signerPackage: String,
            currentUser: String,
            peerPubkey: String,
            content: String,
        ): ExternalSignerResult =
            withSignerRequestLock {
                amberClient.nip44Encrypt(signerPackage, currentUser, peerPubkey, content).toExternalSignerResult()
            }

        override fun nip44Decrypt(
            signerPackage: String,
            currentUser: String,
            peerPubkey: String,
            payload: String,
        ): ExternalSignerResult =
            withSignerRequestLock {
                amberClient.nip44Decrypt(signerPackage, currentUser, peerPubkey, payload).toExternalSignerResult()
            }

        override fun nip04Encrypt(
            signerPackage: String,
            currentUser: String,
            peerPubkey: String,
            content: String,
        ): ExternalSignerResult =
            withSignerRequestLock {
                amberClient.nip04Encrypt(signerPackage, currentUser, peerPubkey, content).toExternalSignerResult()
            }

        override fun nip04Decrypt(
            signerPackage: String,
            currentUser: String,
            peerPubkey: String,
            payload: String,
        ): ExternalSignerResult =
            withSignerRequestLock {
                amberClient.nip04Decrypt(signerPackage, currentUser, peerPubkey, payload).toExternalSignerResult()
            }

        private fun AmberPublicKeyResult.toExternalSignerHandshakeResult(): ExternalSignerHandshakeResult =
            ExternalSignerHandshakeResult(
                ok = ok,
                pubkey = pubkey,
                signerPackage = signerPackage,
                currentUser = currentUser,
                errorKind = kind?.toExternalSignerErrorKind(),
                errorMessage = message,
            )

        private fun AmberResult.toExternalSignerResult(): ExternalSignerResult =
            ExternalSignerResult(
                ok = ok,
                value = value,
                errorKind = kind?.toExternalSignerErrorKind(),
                errorMessage = message,
            )

        private fun AmberErrorKind.toExternalSignerErrorKind(): ExternalSignerErrorKind =
            when (this) {
                AmberErrorKind.REJECTED -> ExternalSignerErrorKind.REJECTED
                AmberErrorKind.CANCELED -> ExternalSignerErrorKind.CANCELED
                AmberErrorKind.TIMEOUT -> ExternalSignerErrorKind.TIMEOUT
                AmberErrorKind.SIGNER_UNAVAILABLE -> ExternalSignerErrorKind.SIGNER_UNAVAILABLE
                AmberErrorKind.PACKAGE_MISMATCH -> ExternalSignerErrorKind.PACKAGE_MISMATCH
                AmberErrorKind.INVALID_RESPONSE -> ExternalSignerErrorKind.INVALID_RESPONSE
                AmberErrorKind.OTHER -> ExternalSignerErrorKind.OTHER
            }
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
