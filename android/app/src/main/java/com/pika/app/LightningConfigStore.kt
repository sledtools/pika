package com.pika.app

import android.content.Context
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey

enum class LightningNodeType(val label: String, val configFields: List<ConfigField>) {
    LND("LND", listOf(ConfigField.URL, ConfigField.MACAROON)),
    CLN("Core Lightning", listOf(ConfigField.URL, ConfigField.RUNE)),
    PHOENIXD("Phoenixd", listOf(ConfigField.URL, ConfigField.PASSWORD)),
    NWC("Nostr Wallet Connect", listOf(ConfigField.NWC_URI)),
    STRIKE("Strike", listOf(ConfigField.API_KEY)),
    BLINK("Blink", listOf(ConfigField.API_KEY)),
    SPEED("Speed", listOf(ConfigField.API_KEY)),
}

enum class ConfigField(val label: String, val placeholder: String, val isSecret: Boolean = false) {
    URL("URL", "https://your-node:8080"),
    MACAROON("Macaroon (hex)", "0201036c6e64...", isSecret = true),
    RUNE("Rune", "your-rune-here", isSecret = true),
    PASSWORD("Password", "your-password", isSecret = true),
    NWC_URI("NWC Connection URI", "nostr+walletconnect://...", isSecret = true),
    API_KEY("API Key", "your-api-key", isSecret = true),
}

data class LightningConfig(
    val nodeType: LightningNodeType,
    val values: Map<ConfigField, String>,
) {
    val isComplete: Boolean
        get() = nodeType.configFields.all { field ->
            values[field]?.isNotBlank() == true
        }
}

class LightningConfigStore(context: Context) {
    private val appContext = context.applicationContext

    private val prefs by lazy {
        val masterKey =
            MasterKey.Builder(appContext)
                .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
                .build()

        EncryptedSharedPreferences.create(
            appContext,
            "pika.lightning",
            masterKey,
            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
        )
    }

    fun load(): LightningConfig? {
        val typeRaw = prefs.getString(KEY_NODE_TYPE, null) ?: return null
        val nodeType = runCatching { LightningNodeType.valueOf(typeRaw) }.getOrNull() ?: return null
        val values = mutableMapOf<ConfigField, String>()
        for (field in nodeType.configFields) {
            val value = prefs.getString("field_${field.name}", null)
            if (value != null) values[field] = value
        }
        return LightningConfig(nodeType = nodeType, values = values)
    }

    fun save(config: LightningConfig) {
        val editor = prefs.edit()
        editor.putString(KEY_NODE_TYPE, config.nodeType.name)
        // Clear all field keys first
        for (field in ConfigField.entries) {
            editor.remove("field_${field.name}")
        }
        // Write current fields
        for ((field, value) in config.values) {
            editor.putString("field_${field.name}", value)
        }
        editor.apply()
    }

    fun clear() {
        val editor = prefs.edit()
        editor.remove(KEY_NODE_TYPE)
        for (field in ConfigField.entries) {
            editor.remove("field_${field.name}")
        }
        editor.apply()
    }

    companion object {
        private const val KEY_NODE_TYPE = "ln_node_type"
    }
}
