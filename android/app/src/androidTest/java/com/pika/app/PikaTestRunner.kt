package com.pika.app

import android.app.Application
import android.content.Context
import android.os.Bundle
import android.util.Log
import androidx.test.runner.AndroidJUnitRunner
import java.io.File

/**
 * Instrumentation tests default to deterministic/offline (no public relays).
 *
 * For opt-in E2E runs, pass runner args to enable networking and optionally override relays:
 * -Pandroid.testInstrumentationRunnerArguments.pika_e2e=1
 * -Pandroid.testInstrumentationRunnerArguments.pika_disable_network=false
 * -Pandroid.testInstrumentationRunnerArguments.pika_relay_urls=wss://...
 * -Pandroid.testInstrumentationRunnerArguments.pika_key_package_relay_urls=wss://...
 * -Pandroid.testInstrumentationRunnerArguments.pika_call_moq_url=https://...
 * -Pandroid.testInstrumentationRunnerArguments.pika_call_broadcast_prefix=pika/calls
 */
class PikaTestRunner : AndroidJUnitRunner() {
    private var runnerArgs: Bundle? = null

    override fun onCreate(arguments: Bundle?) {
        runnerArgs = arguments
        super.onCreate(arguments)

        // Write config here (not in newApplication): runner args are reliably present, and this
        // still happens before the Activity (and thus Rust FfiApp) is created.
        val args = arguments ?: Bundle()

        val isE2e =
            when (args.getString("pika_e2e")?.trim()?.lowercase()) {
                "1", "true", "yes" -> true
                else -> false
            }

        val disableNetwork =
            when (args.getString("pika_disable_network")?.trim()?.lowercase()) {
                "0", "false", "no" -> false
                "1", "true", "yes" -> true
                else -> !isE2e // default: offline
            }

        val reset =
            when (args.getString("pika_reset")?.trim()?.lowercase()) {
                "1", "true", "yes" -> true
                else -> false
            }

        val relayUrlsRaw = args.getString("pika_relay_urls")?.trim().orEmpty()
        val kpRelayUrlsRaw =
            (args.getString("pika_key_package_relay_urls") ?: args.getString("pika_kp_relay_urls"))
                ?.trim()
                .orEmpty()
        val callMoqUrl =
            args.getString("pika_call_moq_url")?.trim().orEmpty().ifBlank {
                "https://moq.justinmoon.com/anon"
            }
        val callBroadcastPrefix =
            args.getString("pika_call_broadcast_prefix")?.trim().orEmpty().ifBlank { "pika/calls" }

        fun splitCsv(s: String): List<String> =
            s.split(",").map { it.trim() }.filter { it.isNotEmpty() }

        val relayUrls = splitCsv(relayUrlsRaw)
        val kpRelayUrls = splitCsv(kpRelayUrlsRaw).ifEmpty { relayUrls }

        runCatching {
            val filesDir = targetContext.filesDir
            if (reset) {
                filesDir.listFiles()?.forEach { it.deleteRecursively() }
            }

            val config = buildString {
                append('{')
                append("\"disable_network\":")
                append(if (disableNetwork) "true" else "false")
                if (relayUrls.isNotEmpty()) {
                    append(",\"relay_urls\":[")
                    relayUrls.forEachIndexed { i, u ->
                        if (i > 0) append(',')
                        append('"')
                        append(u.replace("\"", "\\\""))
                        append('"')
                    }
                    append(']')
                }
                if (kpRelayUrls.isNotEmpty()) {
                    append(",\"key_package_relay_urls\":[")
                    kpRelayUrls.forEachIndexed { i, u ->
                        if (i > 0) append(',')
                        append('"')
                        append(u.replace("\"", "\\\""))
                        append('"')
                    }
                    append(']')
                }
                append(",\"call_moq_url\":\"")
                append(callMoqUrl.replace("\"", "\\\""))
                append('"')
                append(",\"call_broadcast_prefix\":\"")
                append(callBroadcastPrefix.replace("\"", "\\\""))
                append('"')
                append('}')
            }

            val path = File(filesDir, "pika_config.json")
            path.writeText(config)
            Log.i(
                "PikaTestRunner",
                "wrote pika_config.json (len=${config.length}) disable_network=$disableNetwork relays=${relayUrls.size} kp_relays=${kpRelayUrls.size}",
            )
        }.onFailure { e ->
            Log.e("PikaTestRunner", "failed to write pika_config.json: ${e}", e)
        }
    }

    override fun newApplication(cl: ClassLoader?, className: String?, context: Context?): Application {
        return super.newApplication(cl, className, context)
    }

    private fun reflectRunnerArgs(): Bundle? =
        runCatching {
            // Android's Instrumentation stores args in a private field (no accessible getter from
            // our package). Reflection keeps the test runner configurable without extra variants.
            val f = android.app.Instrumentation::class.java.getDeclaredField("mArguments")
            f.isAccessible = true
            f.get(this) as? Bundle
        }.getOrNull()
}
