package com.pika.app

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.database.Cursor
import android.net.Uri

enum class AmberErrorKind {
    REJECTED,
    CANCELED,
    TIMEOUT,
    SIGNER_UNAVAILABLE,
    PACKAGE_MISMATCH,
    INVALID_RESPONSE,
    OTHER,
}

data class AmberResult(
    val ok: Boolean,
    val value: String? = null,
    val kind: AmberErrorKind? = null,
    val message: String? = null,
)

data class AmberPublicKeyResult(
    val ok: Boolean,
    val pubkey: String? = null,
    val signerPackage: String? = null,
    val currentUser: String? = null,
    val kind: AmberErrorKind? = null,
    val message: String? = null,
)

class AmberSignerClient(
    private val appContext: Context,
) {
    fun requestPublicKey(currentUserHint: String?): AmberPublicKeyResult {
        val candidates = signerPackageCandidates()
        if (candidates.isEmpty()) {
            return AmberPublicKeyResult(
                ok = false,
                kind = AmberErrorKind.SIGNER_UNAVAILABLE,
                message = "no Amber-compatible signer is installed",
            )
        }

        for (pkg in candidates) {
            val result =
                requestViaIntent(
                    type = TYPE_GET_PUBLIC_KEY,
                    payload = "",
                    packageName = pkg,
                    currentUser = currentUserHint,
                    peerPubkey = null,
                )
            if (!result.ok) {
                if (result.kind == AmberErrorKind.SIGNER_UNAVAILABLE) {
                    continue
                }
                return AmberPublicKeyResult(
                    ok = false,
                    kind = result.kind,
                    message = result.message,
                )
            }

            val intentData =
                result.valueIntent
                    ?: return AmberPublicKeyResult(
                        ok = false,
                        kind = AmberErrorKind.INVALID_RESPONSE,
                        message = "invalid response: missing intent result",
                    )
            val pubkey = intentData.getStringExtra("result")?.trim().orEmpty()
            if (pubkey.isBlank()) {
                return AmberPublicKeyResult(
                    ok = false,
                    kind = AmberErrorKind.INVALID_RESPONSE,
                    message = "invalid response: missing pubkey",
                )
            }
            val responsePackage = intentData.getStringExtra("package")?.trim().orEmpty()
            if (responsePackage.isNotBlank() && responsePackage != pkg) {
                return AmberPublicKeyResult(
                    ok = false,
                    kind = AmberErrorKind.PACKAGE_MISMATCH,
                    message = "package mismatch",
                )
            }
            val signerPackage = responsePackage.ifBlank { pkg }
            val currentUser =
                intentData
                    .getStringExtra("current_user")
                    ?.trim()
                    ?.takeIf { it.isNotEmpty() }
                    ?: currentUserHint
                        ?.trim()
                        ?.takeIf { it.isNotEmpty() }
                    ?: pubkey
            return AmberPublicKeyResult(
                ok = true,
                pubkey = pubkey,
                signerPackage = signerPackage,
                currentUser = currentUser,
            )
        }

        return AmberPublicKeyResult(
            ok = false,
            kind = AmberErrorKind.SIGNER_UNAVAILABLE,
            message = "signer unavailable",
        )
    }

    fun signEvent(signerPackage: String, currentUser: String, unsignedEventJson: String): AmberResult =
        requestWithProviderFallback(
            type = TYPE_SIGN_EVENT,
            payload = unsignedEventJson,
            signerPackage = signerPackage,
            currentUser = currentUser,
            peerPubkey = null,
            returnKind = ReturnKind.EVENT_JSON,
        )

    fun nip44Encrypt(
        signerPackage: String,
        currentUser: String,
        peerPubkey: String,
        content: String,
    ): AmberResult =
        requestWithProviderFallback(
            type = TYPE_NIP44_ENCRYPT,
            payload = content,
            signerPackage = signerPackage,
            currentUser = currentUser,
            peerPubkey = peerPubkey,
            returnKind = ReturnKind.RESULT,
        )

    fun nip44Decrypt(
        signerPackage: String,
        currentUser: String,
        peerPubkey: String,
        payload: String,
    ): AmberResult =
        requestWithProviderFallback(
            type = TYPE_NIP44_DECRYPT,
            payload = payload,
            signerPackage = signerPackage,
            currentUser = currentUser,
            peerPubkey = peerPubkey,
            returnKind = ReturnKind.RESULT,
        )

    fun nip04Encrypt(
        signerPackage: String,
        currentUser: String,
        peerPubkey: String,
        content: String,
    ): AmberResult =
        requestWithProviderFallback(
            type = TYPE_NIP04_ENCRYPT,
            payload = content,
            signerPackage = signerPackage,
            currentUser = currentUser,
            peerPubkey = peerPubkey,
            returnKind = ReturnKind.RESULT,
        )

    fun nip04Decrypt(
        signerPackage: String,
        currentUser: String,
        peerPubkey: String,
        payload: String,
    ): AmberResult =
        requestWithProviderFallback(
            type = TYPE_NIP04_DECRYPT,
            payload = payload,
            signerPackage = signerPackage,
            currentUser = currentUser,
            peerPubkey = peerPubkey,
            returnKind = ReturnKind.RESULT,
        )

    private fun requestWithProviderFallback(
        type: String,
        payload: String,
        signerPackage: String,
        currentUser: String,
        peerPubkey: String?,
        returnKind: ReturnKind,
    ): AmberResult {
        val providerResult =
            queryViaProvider(
                type = type,
                payload = payload,
                signerPackage = signerPackage,
                currentUser = currentUser,
                peerPubkey = peerPubkey,
                returnKind = returnKind,
            )
        if (providerResult.ok) {
            return providerResult
        }
        if (providerResult.kind != AmberErrorKind.SIGNER_UNAVAILABLE &&
            providerResult.kind != AmberErrorKind.REJECTED &&
            providerResult.kind != AmberErrorKind.INVALID_RESPONSE
        ) {
            return providerResult
        }

        val intentResult = requestViaIntent(
            type = type,
            payload = payload,
            packageName = signerPackage,
            currentUser = currentUser,
            peerPubkey = peerPubkey,
        )
        return intentResult.toAmberResult(returnKind)
    }

    private fun queryViaProvider(
        type: String,
        payload: String,
        signerPackage: String,
        currentUser: String,
        peerPubkey: String?,
        returnKind: ReturnKind,
    ): AmberResult {
        val endpoint = type.uppercase()
        val uri = Uri.parse("content://$signerPackage.$endpoint")
        val projection = arrayOf(payload, peerPubkey.orEmpty(), currentUser)
        return runCatching {
            appContext.contentResolver.query(uri, projection, null, null, null)?.use { cursor ->
                parseProviderCursor(cursor, returnKind)
            } ?: AmberResult(
                ok = false,
                kind = AmberErrorKind.SIGNER_UNAVAILABLE,
                message = "signer unavailable",
            )
        }.getOrElse { err ->
            AmberResult(
                ok = false,
                kind = AmberErrorKind.SIGNER_UNAVAILABLE,
                message = "signer unavailable: ${err.message ?: "query failed"}",
            )
        }
    }

    private fun parseProviderCursor(cursor: Cursor, returnKind: ReturnKind): AmberResult {
        if (!cursor.moveToFirst()) {
            return AmberResult(
                ok = false,
                kind = AmberErrorKind.INVALID_RESPONSE,
                message = "invalid response: empty cursor",
            )
        }
        val rejectedIdx = cursor.getColumnIndex("rejected")
        if (rejectedIdx >= 0) {
            val rejected = cursor.getString(rejectedIdx)
            if (!rejected.isNullOrEmpty()) {
                return AmberResult(
                    ok = false,
                    kind = AmberErrorKind.REJECTED,
                    message = "rejected",
                )
            }
        }

        val result =
            when (returnKind) {
                ReturnKind.RESULT -> {
                    val resultIdx = cursor.getColumnIndex("result")
                    if (resultIdx < 0) {
                        return AmberResult(
                            ok = false,
                            kind = AmberErrorKind.INVALID_RESPONSE,
                            message = "invalid response: missing result column",
                        )
                    }
                    cursor.getString(resultIdx)?.trim().orEmpty()
                }
                ReturnKind.EVENT_JSON -> {
                    val eventIdx = cursor.getColumnIndex("event")
                    val eventJson =
                        if (eventIdx >= 0) {
                            cursor.getString(eventIdx)?.trim().orEmpty()
                        } else {
                            ""
                        }
                    if (eventJson.isNotBlank()) {
                        eventJson
                    } else {
                        val resultIdx = cursor.getColumnIndex("result")
                        if (resultIdx >= 0) {
                            cursor.getString(resultIdx)?.trim().orEmpty()
                        } else {
                            ""
                        }
                    }
                }
            }
        if (result.isBlank()) {
            return AmberResult(
                ok = false,
                kind = AmberErrorKind.INVALID_RESPONSE,
                message = "invalid response: empty result",
            )
        }
        return AmberResult(ok = true, value = result)
    }

    private data class IntentCallResult(
        val ok: Boolean,
        val valueIntent: Intent? = null,
        val kind: AmberErrorKind? = null,
        val message: String? = null,
    ) {
        fun toAmberResult(returnKind: ReturnKind): AmberResult {
            if (!ok) {
                return AmberResult(ok = false, kind = kind, message = message)
            }
            val result =
                when (returnKind) {
                    ReturnKind.RESULT -> valueIntent?.getStringExtra("result")?.trim().orEmpty()
                    ReturnKind.EVENT_JSON -> {
                        val eventJson = valueIntent?.getStringExtra("event")?.trim().orEmpty()
                        if (eventJson.isNotBlank()) {
                            eventJson
                        } else {
                            valueIntent?.getStringExtra("result")?.trim().orEmpty()
                        }
                    }
                }
            if (result.isBlank()) {
                return AmberResult(
                    ok = false,
                    kind = AmberErrorKind.INVALID_RESPONSE,
                    message = "invalid response: missing result",
                )
            }
            return AmberResult(ok = true, value = result)
        }
    }

    private fun requestViaIntent(
        type: String,
        payload: String,
        packageName: String,
        currentUser: String?,
        peerPubkey: String?,
    ): IntentCallResult {
        val encodedPayload = if (payload.isEmpty()) "" else Uri.encode(payload)
        val intent =
            Intent(Intent.ACTION_VIEW).apply {
                data = Uri.parse("nostrsigner:$encodedPayload")
                setPackage(packageName)
                putExtra("type", type)
                if (!currentUser.isNullOrBlank()) {
                    putExtra("current_user", currentUser)
                }
                if (!peerPubkey.isNullOrBlank()) {
                    putExtra("pubKey", peerPubkey)
                }
            }

        val outcome = AmberIntentBridge.launch(intent)
        if (!outcome.ok) {
            val error = outcome.error.orEmpty()
            val kind =
                when {
                    error.contains("timeout", ignoreCase = true) -> AmberErrorKind.TIMEOUT
                    error.contains("signer unavailable", ignoreCase = true) -> AmberErrorKind.SIGNER_UNAVAILABLE
                    outcome.resultCode == Activity.RESULT_CANCELED -> AmberErrorKind.CANCELED
                    else -> AmberErrorKind.SIGNER_UNAVAILABLE
                }
            return IntentCallResult(
                ok = false,
                kind = kind,
                message = outcome.error ?: "canceled",
            )
        }

        val data = outcome.data
        if (data?.hasExtra("rejected") == true) {
            return IntentCallResult(
                ok = false,
                kind = AmberErrorKind.REJECTED,
                message = "rejected",
            )
        }

        return IntentCallResult(ok = true, valueIntent = data)
    }

    private fun signerPackageCandidates(): List<String> {
        val known = listOf(DEFAULT_AMBER_PACKAGE, DEBUG_AMBER_PACKAGE)
        val discovered =
            runCatching {
                val queryIntent = Intent(Intent.ACTION_VIEW, Uri.parse("nostrsigner:"))
                appContext.packageManager
                    .queryIntentActivities(queryIntent, 0)
                    .mapNotNull { it.activityInfo?.packageName }
            }.getOrDefault(emptyList())
        return (known + discovered).distinct()
    }

    companion object {
        private const val DEFAULT_AMBER_PACKAGE = "com.greenart7c3.nostrsigner"
        private const val DEBUG_AMBER_PACKAGE = "com.greenart7c3.nostrsigner.debug"

        private const val TYPE_GET_PUBLIC_KEY = "get_public_key"
        private const val TYPE_SIGN_EVENT = "sign_event"
        private const val TYPE_NIP44_ENCRYPT = "nip44_encrypt"
        private const val TYPE_NIP44_DECRYPT = "nip44_decrypt"
        private const val TYPE_NIP04_ENCRYPT = "nip04_encrypt"
        private const val TYPE_NIP04_DECRYPT = "nip04_decrypt"
    }
}

private enum class ReturnKind {
    RESULT,
    EVENT_JSON,
}
