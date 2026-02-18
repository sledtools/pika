package com.pika.app

import android.content.Context
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey

enum class StoredAuthMode {
    LOCAL_NSEC,
    EXTERNAL_SIGNER,
    BUNKER,
}

data class StoredAuthDescriptor(
    val mode: StoredAuthMode,
    val nsec: String? = null,
    val pubkey: String? = null,
    val signerPackage: String? = null,
    val currentUser: String? = null,
    val bunkerUri: String? = null,
    val bunkerClientNsec: String? = null,
)

class SecureAuthStore(context: Context) {
    private val appContext = context.applicationContext

    private val prefs by lazy {
        val masterKey =
            MasterKey.Builder(appContext)
                .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
                .build()

        EncryptedSharedPreferences.create(
            appContext,
            "pika.secure",
            masterKey,
            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
        )
    }

    fun load(): StoredAuthDescriptor? {
        val modeRaw = prefs.getString(KEY_AUTH_MODE, null)
        return when (modeRaw) {
            MODE_LOCAL_NSEC -> {
                val nsec = prefs.getString(KEY_NSEC, null) ?: return null
                StoredAuthDescriptor(mode = StoredAuthMode.LOCAL_NSEC, nsec = nsec)
            }
            MODE_EXTERNAL_SIGNER -> {
                val pubkey = prefs.getString(KEY_EXT_PUBKEY, null) ?: return null
                val signerPackage = prefs.getString(KEY_EXT_PACKAGE, null) ?: return null
                val currentUser = prefs.getString(KEY_EXT_CURRENT_USER, null) ?: pubkey
                StoredAuthDescriptor(
                    mode = StoredAuthMode.EXTERNAL_SIGNER,
                    pubkey = pubkey,
                    signerPackage = signerPackage,
                    currentUser = currentUser,
                )
            }
            MODE_BUNKER -> {
                val bunkerUri = prefs.getString(KEY_BUNKER_URI, null) ?: return null
                val bunkerClientNsec = prefs.getString(KEY_BUNKER_CLIENT_NSEC, null) ?: return null
                StoredAuthDescriptor(
                    mode = StoredAuthMode.BUNKER,
                    bunkerUri = bunkerUri,
                    bunkerClientNsec = bunkerClientNsec,
                )
            }
            null -> {
                // Backward compatibility: previous versions only stored nsec.
                val nsec = prefs.getString(KEY_NSEC, null) ?: return null
                StoredAuthDescriptor(mode = StoredAuthMode.LOCAL_NSEC, nsec = nsec)
            }
            else -> null
        }
    }

    fun saveLocalNsec(nsec: String) {
        prefs
            .edit()
            .putString(KEY_AUTH_MODE, MODE_LOCAL_NSEC)
            .putString(KEY_NSEC, nsec)
            .remove(KEY_EXT_PUBKEY)
            .remove(KEY_EXT_PACKAGE)
            .remove(KEY_EXT_CURRENT_USER)
            .remove(KEY_BUNKER_URI)
            .remove(KEY_BUNKER_CLIENT_NSEC)
            .apply()
    }

    fun saveExternalSigner(pubkey: String, signerPackage: String, currentUser: String) {
        prefs
            .edit()
            .putString(KEY_AUTH_MODE, MODE_EXTERNAL_SIGNER)
            .putString(KEY_EXT_PUBKEY, pubkey)
            .putString(KEY_EXT_PACKAGE, signerPackage)
            .putString(KEY_EXT_CURRENT_USER, currentUser)
            .remove(KEY_NSEC)
            .remove(KEY_BUNKER_URI)
            .remove(KEY_BUNKER_CLIENT_NSEC)
            .apply()
    }

    fun saveBunker(bunkerUri: String, bunkerClientNsec: String) {
        prefs
            .edit()
            .putString(KEY_AUTH_MODE, MODE_BUNKER)
            .putString(KEY_BUNKER_URI, bunkerUri)
            .putString(KEY_BUNKER_CLIENT_NSEC, bunkerClientNsec)
            .remove(KEY_NSEC)
            .remove(KEY_EXT_PUBKEY)
            .remove(KEY_EXT_PACKAGE)
            .remove(KEY_EXT_CURRENT_USER)
            .apply()
    }

    fun clear() {
        prefs
            .edit()
            .remove(KEY_AUTH_MODE)
            .remove(KEY_NSEC)
            .remove(KEY_EXT_PUBKEY)
            .remove(KEY_EXT_PACKAGE)
            .remove(KEY_EXT_CURRENT_USER)
            .remove(KEY_BUNKER_URI)
            .remove(KEY_BUNKER_CLIENT_NSEC)
            .apply()
    }

    companion object {
        private const val KEY_AUTH_MODE = "auth_mode"
        private const val KEY_NSEC = "nsec"
        private const val KEY_EXT_PUBKEY = "external_pubkey"
        private const val KEY_EXT_PACKAGE = "external_signer_package"
        private const val KEY_EXT_CURRENT_USER = "external_current_user"
        private const val KEY_BUNKER_URI = "bunker_uri"
        private const val KEY_BUNKER_CLIENT_NSEC = "bunker_client_nsec"

        private const val MODE_LOCAL_NSEC = "local_nsec"
        private const val MODE_EXTERNAL_SIGNER = "external_signer"
        private const val MODE_BUNKER = "bunker"
    }
}
