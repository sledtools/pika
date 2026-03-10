package com.pika.app

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.OpenableColumns
import android.util.Log
import android.widget.Toast
import android.webkit.MimeTypeMap
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.pika.app.rust.AgentKind
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
import com.pika.app.rust.MediaBatchItem
import com.pika.app.rust.isValidPeerKey
import com.pika.app.rust.MyProfileState
import com.pika.app.rust.Screen
import com.pika.app.rust.share.ShareAckStatus
import com.pika.app.rust.share.ShareDispatchAck
import com.pika.app.rust.share.ShareDispatchKind
import com.pika.app.rust.share.ShareEnqueueRequest
import com.pika.app.rust.share.ShareException
import com.pika.app.rust.share.SharePayloadKind
import com.pika.app.rust.share.shareAck
import com.pika.app.rust.share.shareDequeueBatch
import com.pika.app.rust.share.shareEnqueue
import com.pika.app.rust.share.shareGc
import java.io.File
import java.util.Locale
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import org.json.JSONObject

class AppManager private constructor(context: Context) : AppReconciler {
    private val shareTag = "PikaShare"
    private val shareQueueDirName = "share_queue"
    private val shareMediaDirName = "media"
    private val defaultShareImageMime = "image/jpeg"

    private val appContext = context.applicationContext
    private val mainHandler = Handler(Looper.getMainLooper())
    private val secureStore = SecureAuthStore(appContext)
    private val amberClient = AmberSignerClient(appContext)
    private val signerRequestLock = Any()
    private val audioFocus = AndroidAudioFocusManager(appContext)
    private val rust: FfiApp
    private val shareRootDir: String = appContext.filesDir.absolutePath
    private var lastRevApplied: ULong = 0UL
    private val listening = AtomicBoolean(false)
    private var pendingShareDraft: PendingShareDraft? by mutableStateOf(null)

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
                startingAgent = false,
                fetchingFollowList = false,
            ),
            chatList = emptyList(),
            currentChat = null,
            followList = emptyList(),
            peerProfile = null,
            activeCall = null,
            callTimeline = emptyList(),
            toast = null,
            developerMode = false,
            showAgentMarketplace = false,
            updateRequired = false,
            agentButton = null,
            agentProvisioning = null,
            voiceRecording = null,
            mediaGallery = null,
        ),
    )
        private set

    init {
        // Ensure call config is present before Rust bootstraps. If the file already exists (e.g.
        // created by tooling), only fill missing keys to avoid clobbering overrides.
        ensureDefaultConfig(appContext)

        val dataDir = context.filesDir.absolutePath
        val appVersion = try {
            appContext.packageManager.getPackageInfo(appContext.packageName, 0).versionName ?: "0.0.0"
        } catch (_: Exception) { "0.0.0" }
        rust = FfiApp(dataDir, "", appVersion)
        rust.setExternalSignerBridge(AmberRustBridge())
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
        // Default external signer support to enabled.
        // If callers provided an explicit value, respect it.
        if (!obj.has("enable_external_signer")) {
            obj.put("enable_external_signer", true)
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

    fun loginWithBunker(bunkerUri: String) {
        rust.dispatch(AppAction.BeginBunkerLogin(bunkerUri = bunkerUri))
    }

    fun loginWithNostrConnect() {
        rust.dispatch(AppAction.BeginNostrConnectLogin)
    }

    fun logout() {
        secureStore.clear()
        rust.dispatch(AppAction.Logout)
    }

    fun isDeveloperModeEnabled(): Boolean = state.developerMode

    fun enableDeveloperMode() {
        rust.dispatch(AppAction.EnableDeveloperMode)
    }

    fun isShowAgentMarketplaceEnabled(): Boolean = state.showAgentMarketplace

    fun setShowAgentMarketplaceEnabled(enabled: Boolean) {
        rust.dispatch(AppAction.SetShowAgentMarketplace(enabled))
    }

    fun ensureAgent() {
        rust.dispatch(AppAction.EnsureAgent)
    }

    fun ensureAgent(kind: AgentKind) {
        rust.dispatch(AppAction.EnsureAgentKind(kind))
    }

    fun wipeLocalDataForDeveloperTools() {
        secureStore.clear()
        rust.dispatch(AppAction.WipeLocalData)
        pendingShareDraft = null
    }

    fun getNsec(): String? = secureStore.load()?.nsec

    fun pendingShareSelectionSummary(): String? = pendingShareDraft?.summary

    fun hasPendingShareSelection(): Boolean = pendingShareDraft != null

    fun dismissPendingShareSelection() {
        pendingShareDraft = null
    }

    fun onChatListChatSelected(chatId: String) {
        val draft = pendingShareDraft
        if (draft == null) {
            rust.dispatch(AppAction.OpenChat(chatId))
            return
        }

        val request =
            draft.toEnqueueRequest(
                chatId = chatId,
                clientRequestId = UUID.randomUUID().toString(),
                createdAtMs = System.currentTimeMillis().coerceAtLeast(0).toULong(),
            )
        val queued =
            runCatching {
                shareEnqueue(rootDir = shareRootDir, request = request)
            }
        if (queued.isFailure) {
            val err = queued.exceptionOrNull()
            Log.w(shareTag, "Failed to queue incoming share", err)
            // Exit chooser mode on failure so normal app navigation is not locked behind
            // a stale share draft.
            pendingShareDraft = null
            showShareToast("Could not share item: ${describeShareError(err)}")
            return
        }

        pendingShareDraft = null
        processPendingShareQueue(openFirstChat = true)
    }

    fun onForeground() {
        // Foreground is a lifecycle signal; Rust owns state changes and side effects.
        rust.dispatch(AppAction.Foregrounded)
        processPendingShareQueue(openFirstChat = false)
    }

    fun handleIncomingIntent(intent: Intent?) {
        extractChatDeepLinkNpub(intent)?.let { npub ->
            rust.dispatch(AppAction.CreateChat(peerNpub = npub))
            return
        }

        extractIncomingShareDraft(intent)?.let { draft ->
            pendingShareDraft = draft
            maybePresentShareChooser()
            return
        }

        val callbackUrl = extractNostrConnectCallback(intent) ?: return
        rust.dispatch(AppAction.NostrConnectCallback(url = callbackUrl))
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
            } else if (update is AppUpdate.BunkerSessionDescriptor) {
                if (update.bunkerUri.isNotBlank() && update.clientNsec.isNotBlank()) {
                    secureStore.saveBunker(update.bunkerUri, update.clientNsec)
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
                is AppUpdate.BunkerSessionDescriptor -> {
                    if (update.bunkerUri.isNotBlank() && update.clientNsec.isNotBlank()) {
                        secureStore.saveBunker(update.bunkerUri, update.clientNsec)
                    }
                    state = state.copy(rev = updateRev)
                }
            }
            syncSecureStoreWithAuthState()
            audioFocus.syncForCall(state.activeCall)
            maybePresentShareChooser()
        }
    }

    private fun AppUpdate.rev(): ULong =
        when (this) {
            is AppUpdate.FullState -> this.v1.rev
            is AppUpdate.AccountCreated -> this.rev
            is AppUpdate.BunkerSessionDescriptor -> this.rev
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
            StoredAuthMode.BUNKER -> {
                val bunkerUri = stored.bunkerUri?.trim().orEmpty()
                val clientNsec = stored.bunkerClientNsec?.trim().orEmpty()
                if (bunkerUri.isBlank() || clientNsec.isBlank()) return
                rust.dispatch(
                    AppAction.RestoreSessionBunker(
                        bunkerUri = bunkerUri,
                        clientNsec = clientNsec,
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
                        if (secureStore.load()?.mode != StoredAuthMode.LOCAL_NSEC) {
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
                    is AuthMode.BunkerSigner -> {
                        val existing = secureStore.load()
                        val clientNsec =
                            existing
                                ?.takeIf { it.mode == StoredAuthMode.BUNKER }
                                ?.bunkerClientNsec
                                ?.trim()
                                .orEmpty()
                        if (clientNsec.isNotBlank()) {
                            secureStore.saveBunker(
                                bunkerUri = mode.bunkerUri,
                                bunkerClientNsec = clientNsec,
                            )
                        }
                    }
                }
            }
        }
    }

    private fun maybePresentShareChooser() {
        if (pendingShareDraft == null) return
        if (state.auth !is AuthState.LoggedIn) return

        val current = state.router.screenStack.lastOrNull() ?: state.router.defaultScreen
        if (current !is Screen.ChatList) {
            rust.dispatch(AppAction.UpdateScreenStack(emptyList()))
        }
    }

    private fun processPendingShareQueue(openFirstChat: Boolean) {
        if (state.auth !is AuthState.LoggedIn) return
        // While the chooser is visible, a draft image may exist on disk but not yet be
        // referenced by queue metadata; running GC here could delete it as an orphan.
        if (pendingShareDraft != null) return

        runCatching {
            shareGc(rootDir = shareRootDir, nowMsOverride = 0UL)
        }.onFailure { err ->
            Log.w(shareTag, "Failed share queue maintenance", err)
        }

        val jobs =
            runCatching {
                shareDequeueBatch(rootDir = shareRootDir, nowMsOverride = 0UL, limit = 64u)
            }.getOrElse { err ->
                Log.w(shareTag, "Failed to dequeue shared content", err)
                return
            }

        if (jobs.isEmpty()) return

        var firstOpenedChatId: String? = null
        for (job in jobs) {
            when (val kind = job.kind) {
                is ShareDispatchKind.Message -> {
                    rust.dispatch(
                        AppAction.SendMessage(
                            chatId = job.chatId,
                            content = kind.content,
                            kind = null,
                            replyToMessageId = null,
                        ),
                    )
                }
                is ShareDispatchKind.Media -> {
                    rust.dispatch(
                        AppAction.SendChatMedia(
                            chatId = job.chatId,
                            dataBase64 = kind.dataBase64,
                            mimeType = kind.mimeType,
                            filename = kind.filename,
                            caption = kind.caption,
                        ),
                    )
                }
                is ShareDispatchKind.MediaBatch -> {
                    val batchItems = kind.items.map { entry ->
                        MediaBatchItem(
                            dataBase64 = entry.dataBase64,
                            mimeType = entry.mimeType,
                            filename = entry.filename,
                        )
                    }
                    rust.dispatch(
                        AppAction.SendChatMediaBatch(
                            chatId = job.chatId,
                            items = batchItems,
                            caption = kind.caption,
                        ),
                    )
                }
            }

            runCatching {
                shareAck(
                    rootDir = shareRootDir,
                    ack =
                        ShareDispatchAck(
                            itemId = job.itemId,
                            status = ShareAckStatus.ACCEPTED_BY_CORE,
                            errorCode = null,
                            errorMessage = null,
                        ),
                )
            }.onFailure { err ->
                Log.w(shareTag, "Failed share queue ack for ${job.itemId}", err)
            }

            if (openFirstChat && firstOpenedChatId == null) {
                firstOpenedChatId = job.chatId
            }
        }

        if (openFirstChat) {
            firstOpenedChatId?.let { rust.dispatch(AppAction.OpenChat(it)) }
        }
    }

    private fun showShareToast(message: String) {
        mainHandler.post {
            Toast.makeText(appContext, message, Toast.LENGTH_LONG).show()
        }
    }

    private fun describeShareError(err: Throwable?): String {
        val raw = err?.message?.trim().orEmpty()
        if (raw.isNotEmpty()) return raw
        return err?.javaClass?.simpleName ?: "unknown error"
    }

    private fun extractIncomingShareDraft(intent: Intent?): PendingShareDraft? {
        if (intent?.action != Intent.ACTION_SEND) return null

        val sharedText =
            intent.getCharSequenceExtra(Intent.EXTRA_TEXT)
                ?.toString()
                ?.trim()
                .orEmpty()

        val streamUri = streamUriFromIntent(intent)
        if (streamUri != null) {
            val mimeType = resolveIncomingMimeType(intent.type, streamUri)
            if (!mimeType.startsWith("image/")) {
                Log.i(shareTag, "Ignoring unsupported share mime type: $mimeType")
                return null
            }

            val filename = resolveIncomingFilename(streamUri, mimeType)
            val relativePath = saveIncomingMedia(streamUri, filename) ?: return null
            return PendingShareDraft.Image(
                mediaRelativePath = relativePath,
                mediaMimeType = mimeType,
                mediaFilename = filename,
                composeText = sharedText,
            )
        }

        if (sharedText.isBlank()) return null

        val payloadKind =
            if (looksLikeWebUrl(sharedText)) {
                SharePayloadKind.URL
            } else {
                SharePayloadKind.TEXT
            }
        return PendingShareDraft.Text(
            payloadKind = payloadKind,
            payloadText = sharedText,
            composeText = "",
        )
    }

    private fun resolveIncomingMimeType(intentType: String?, streamUri: Uri): String {
        val intentMime = intentType?.trim()?.lowercase(Locale.US).orEmpty()
        if (intentMime.startsWith("image/")) {
            return intentMime
        }

        val resolverMime = appContext.contentResolver.getType(streamUri)?.trim()?.lowercase(Locale.US).orEmpty()
        if (resolverMime.startsWith("image/")) {
            return resolverMime
        }

        return defaultShareImageMime
    }

    private fun resolveIncomingFilename(streamUri: Uri, mimeType: String): String {
        val displayName =
            appContext.contentResolver.query(
                streamUri,
                arrayOf(OpenableColumns.DISPLAY_NAME),
                null,
                null,
                null,
            )?.use { cursor ->
                if (!cursor.moveToFirst()) return@use null
                val idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (idx < 0) return@use null
                cursor.getString(idx)
            }

        val cleanedDisplay = displayName?.trim().orEmpty()
        val baseNameRaw =
            if (cleanedDisplay.isNotBlank()) {
                File(cleanedDisplay).nameWithoutExtension
            } else {
                "shared-image"
            }
        val baseName = sanitizeFilenameBase(baseNameRaw)
        val extFromDisplay = sanitizedExtension(File(cleanedDisplay).extension)
        val extFromMime =
            sanitizedExtension(
                MimeTypeMap.getSingleton().getExtensionFromMimeType(mimeType.lowercase(Locale.US)),
            )
        val ext = extFromDisplay ?: extFromMime ?: "jpg"
        return "$baseName.$ext"
    }

    private fun saveIncomingMedia(streamUri: Uri, filename: String): String? {
        val ext = sanitizedExtension(File(filename).extension) ?: "jpg"
        val outName = "${UUID.randomUUID()}.$ext"
        val relativePath = "$shareQueueDirName/$shareMediaDirName/$outName"
        val outFile = File(shareRootDir, relativePath)
        outFile.parentFile?.mkdirs()

        return runCatching {
            appContext.contentResolver.openInputStream(streamUri)?.use { input ->
                outFile.outputStream().use { output ->
                    input.copyTo(output)
                }
            } ?: error("no readable stream for shared media")

            if (outFile.length() <= 0L) {
                error("shared media payload is empty")
            }
            relativePath
        }.onFailure { err ->
            Log.w(shareTag, "Failed to persist shared media", err)
            outFile.delete()
        }.getOrNull()
    }

    private fun streamUriFromIntent(intent: Intent): Uri? {
        @Suppress("DEPRECATION")
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            intent.getParcelableExtra(Intent.EXTRA_STREAM)
        }
    }

    private fun sanitizeFilenameBase(raw: String): String {
        val cleaned =
            raw.trim()
                .replace("[^A-Za-z0-9._-]".toRegex(), "-")
                .trim('-')
        return cleaned.ifBlank { "shared-image" }
    }

    private fun sanitizedExtension(raw: String?): String? {
        val value = raw?.trim()?.lowercase(Locale.US).orEmpty()
        if (value.isBlank() || value.length > 12) return null
        return if (value.all { it.isLetterOrDigit() }) value else null
    }

    private fun looksLikeWebUrl(value: String): Boolean {
        val parsed = runCatching { Uri.parse(value.trim()) }.getOrNull() ?: return false
        val scheme = parsed.scheme?.lowercase(Locale.US) ?: return false
        return scheme == "http" || scheme == "https"
    }

    private inline fun <T> withSignerRequestLock(block: () -> T): T = synchronized(signerRequestLock) { block() }

    private inner class AmberRustBridge : ExternalSignerBridge {
        override fun openUrl(url: String): ExternalSignerResult =
            withSignerRequestLock {
                val trimmed = url.trim()
                val launchUrl = withNostrConnectCallback(trimmed)
                if (trimmed.isEmpty()) {
                    return@withSignerRequestLock ExternalSignerResult(
                        ok = false,
                        value = null,
                        errorKind = ExternalSignerErrorKind.INVALID_RESPONSE,
                        errorMessage = "missing URL",
                    )
                }
                val uri =
                    runCatching { Uri.parse(launchUrl) }.getOrElse {
                        return@withSignerRequestLock ExternalSignerResult(
                            ok = false,
                            value = null,
                            errorKind = ExternalSignerErrorKind.INVALID_RESPONSE,
                            errorMessage = "invalid URL",
                        )
                    }
                val intent =
                    Intent(Intent.ACTION_VIEW, uri).apply {
                        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    }
                val canHandle = intent.resolveActivity(appContext.packageManager) != null
                if (!canHandle) {
                    return@withSignerRequestLock ExternalSignerResult(
                        ok = false,
                        value = null,
                        errorKind = ExternalSignerErrorKind.SIGNER_UNAVAILABLE,
                        errorMessage = "no app can handle URL",
                    )
                }
                return@withSignerRequestLock runCatching {
                    appContext.startActivity(intent)
                    ExternalSignerResult(
                        ok = true,
                        value = null,
                        errorKind = null,
                        errorMessage = null,
                    )
                }.getOrElse { err ->
                    ExternalSignerResult(
                        ok = false,
                        value = null,
                        errorKind = ExternalSignerErrorKind.OTHER,
                        errorMessage = err.message ?: "failed to open URL",
                    )
                }
            }

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
        internal val NOSTR_CONNECT_CALLBACK_SCHEME = BuildConfig.PIKA_URL_SCHEME.lowercase()
        internal const val NOSTR_CONNECT_CALLBACK_HOST = "nostrconnect-return"
        internal val NOSTR_CONNECT_CALLBACK_URL =
            "$NOSTR_CONNECT_CALLBACK_SCHEME://$NOSTR_CONNECT_CALLBACK_HOST"

        private val CALLBACK_QUERY_REGEX = Regex("(^|[?&])callback=", RegexOption.IGNORE_CASE)
        internal fun withNostrConnectCallback(raw: String): String {
            val trimmed = raw.trim()
            if (!trimmed.startsWith("nostrconnect://", ignoreCase = true)) {
                return trimmed
            }
            if (CALLBACK_QUERY_REGEX.containsMatchIn(trimmed)) {
                return trimmed
            }

            val appended =
                runCatching {
                    val parsed = Uri.parse(trimmed)
                    parsed
                        .buildUpon()
                        .appendQueryParameter("callback", NOSTR_CONNECT_CALLBACK_URL)
                        .build()
                        .toString()
                }.getOrNull()
            if (!appended.isNullOrBlank()) {
                return appended
            }

            val encoded = Uri.encode(NOSTR_CONNECT_CALLBACK_URL)
            val separator = if (trimmed.contains("?")) "&" else "?"
            return "$trimmed${separator}callback=$encoded"
        }

        internal fun extractChatDeepLinkNpub(intent: Intent?): String? {
            if (intent?.action != Intent.ACTION_VIEW) return null
            val data = intent.data ?: return null
            if (!data.scheme.equals(NOSTR_CONNECT_CALLBACK_SCHEME, ignoreCase = true)) return null
            if (!data.host.equals("chat", ignoreCase = true)) return null
            val npub = data.pathSegments?.firstOrNull() ?: return null
            if (!isValidPeerKey(npub)) return null
            return npub
        }

        internal fun extractNostrConnectCallback(intent: Intent?): String? {
            if (intent?.action != Intent.ACTION_VIEW) return null
            val data = intent.data ?: return null
            if (!data.scheme.equals(NOSTR_CONNECT_CALLBACK_SCHEME, ignoreCase = true)) return null
            if (!data.host.equals(NOSTR_CONNECT_CALLBACK_HOST, ignoreCase = true)) return null
            return data.toString()
        }

        @Volatile
        private var instance: AppManager? = null

        fun getInstance(context: Context): AppManager =
            instance ?: synchronized(this) {
                instance ?: AppManager(context.applicationContext).also { instance = it }
            }
    }
}

internal sealed class PendingShareDraft {
    abstract val summary: String

    abstract fun toEnqueueRequest(
        chatId: String,
        clientRequestId: String,
        createdAtMs: ULong,
    ): ShareEnqueueRequest

    data class Text(
        val payloadKind: SharePayloadKind,
        val payloadText: String,
        val composeText: String,
    ) : PendingShareDraft() {
        override val summary: String
            get() =
                when (payloadKind) {
                    SharePayloadKind.URL -> "Select a chat to share this link."
                    else -> "Select a chat to share this text."
                }

        override fun toEnqueueRequest(
            chatId: String,
            clientRequestId: String,
            createdAtMs: ULong,
        ): ShareEnqueueRequest =
            ShareEnqueueRequest(
                chatId = chatId,
                composeText = composeText,
                payloadKind = payloadKind,
                payloadText = payloadText,
                mediaRelativePath = null,
                mediaMimeType = null,
                mediaFilename = null,
                mediaBatch = null,
                clientRequestId = clientRequestId,
                createdAtMs = createdAtMs,
            )
    }

    data class Image(
        val mediaRelativePath: String,
        val mediaMimeType: String,
        val mediaFilename: String,
        val composeText: String,
    ) : PendingShareDraft() {
        override val summary: String = "Select a chat to share this image."

        override fun toEnqueueRequest(
            chatId: String,
            clientRequestId: String,
            createdAtMs: ULong,
        ): ShareEnqueueRequest =
            ShareEnqueueRequest(
                chatId = chatId,
                composeText = composeText,
                payloadKind = SharePayloadKind.IMAGE,
                payloadText = null,
                mediaRelativePath = mediaRelativePath,
                mediaMimeType = mediaMimeType,
                mediaFilename = mediaFilename,
                mediaBatch = null,
                clientRequestId = clientRequestId,
                createdAtMs = createdAtMs,
            )
    }
}
