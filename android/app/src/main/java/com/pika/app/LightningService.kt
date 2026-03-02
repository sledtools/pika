package com.pika.app

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL

/**
 * Lightweight Lightning invoice service that talks directly to LN node HTTP APIs.
 * Supports the same node types as LNI: LND, CLN, Phoenixd, NWC, Strike, Blink, Speed.
 *
 * This is a bridge until LNI Rust crate is integrated into pika_core.
 */
class LightningService(private val config: LightningConfig) {

    data class Invoice(
        val bolt11: String,
        val paymentHash: String?,
        val amountSats: Long,
        val memo: String?,
    )

    suspend fun createInvoice(amountSats: Long, memo: String?): Result<Invoice> = withContext(Dispatchers.IO) {
        runCatching {
            when (config.nodeType) {
                LightningNodeType.LND -> createLndInvoice(amountSats, memo)
                LightningNodeType.CLN -> createClnInvoice(amountSats, memo)
                LightningNodeType.PHOENIXD -> createPhoenixdInvoice(amountSats, memo)
                LightningNodeType.NWC -> createNwcInvoice(amountSats, memo)
                LightningNodeType.STRIKE -> createStrikeInvoice(amountSats, memo)
                LightningNodeType.BLINK -> createBlinkInvoice(amountSats, memo)
                LightningNodeType.SPEED -> createSpeedInvoice(amountSats, memo)
            }
        }
    }

    // ─── LND ────────────────────────────────────────────────────────────────────

    private fun createLndInvoice(amountSats: Long, memo: String?): Invoice {
        val baseUrl = config.values[ConfigField.URL]!!.trimEnd('/')
        val macaroon = config.values[ConfigField.MACAROON]!!

        val body = JSONObject().apply {
            put("value", amountSats.toString())
            if (!memo.isNullOrBlank()) put("memo", memo)
        }

        val conn = URL("$baseUrl/v1/invoices").openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.setRequestProperty("Grpc-Metadata-macaroon", macaroon)
        conn.doOutput = true
        conn.outputStream.use { it.write(body.toString().toByteArray()) }

        val response = conn.inputStream.bufferedReader().readText()
        val json = JSONObject(response)
        val bolt11 = json.getString("payment_request")
        val rHash = json.optString("r_hash", null)

        return Invoice(bolt11 = bolt11, paymentHash = rHash, amountSats = amountSats, memo = memo)
    }

    // ─── CLN ────────────────────────────────────────────────────────────────────

    private fun createClnInvoice(amountSats: Long, memo: String?): Invoice {
        val baseUrl = config.values[ConfigField.URL]!!.trimEnd('/')
        val rune = config.values[ConfigField.RUNE]!!

        val body = JSONObject().apply {
            put("amount_msat", amountSats * 1000)
            put("label", "pika-${System.currentTimeMillis()}")
            put("description", memo ?: "Pika payment")
        }

        val conn = URL("$baseUrl/v1/invoice").openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.setRequestProperty("Rune", rune)
        conn.doOutput = true
        conn.outputStream.use { it.write(body.toString().toByteArray()) }

        val response = conn.inputStream.bufferedReader().readText()
        val json = JSONObject(response)
        val bolt11 = json.getString("bolt11")
        val paymentHash = json.optString("payment_hash", null)

        return Invoice(bolt11 = bolt11, paymentHash = paymentHash, amountSats = amountSats, memo = memo)
    }

    // ─── Phoenixd ───────────────────────────────────────────────────────────────

    private fun createPhoenixdInvoice(amountSats: Long, memo: String?): Invoice {
        val baseUrl = config.values[ConfigField.URL]!!.trimEnd('/')
        val password = config.values[ConfigField.PASSWORD]!!

        val formData = buildString {
            append("amountSat=$amountSats")
            if (!memo.isNullOrBlank()) append("&description=$memo")
        }

        val conn = URL("$baseUrl/createinvoice").openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/x-www-form-urlencoded")
        val auth = android.util.Base64.encodeToString("$password:".toByteArray(), android.util.Base64.NO_WRAP)
        conn.setRequestProperty("Authorization", "Basic $auth")
        conn.doOutput = true
        conn.outputStream.use { it.write(formData.toByteArray()) }

        val response = conn.inputStream.bufferedReader().readText()
        val json = JSONObject(response)
        val bolt11 = json.getString("serialized")
        val paymentHash = json.optString("paymentHash", null)

        return Invoice(bolt11 = bolt11, paymentHash = paymentHash, amountSats = amountSats, memo = memo)
    }

    // ─── NWC (Nostr Wallet Connect) ─────────────────────────────────────────────

    private fun createNwcInvoice(amountSats: Long, memo: String?): Invoice {
        // NWC requires nostr event signing — placeholder for now.
        // Full implementation needs nostr key management from the app.
        throw UnsupportedOperationException(
            "NWC invoice creation requires nostr event signing. Coming soon!"
        )
    }

    // ─── Strike ─────────────────────────────────────────────────────────────────

    private fun createStrikeInvoice(amountSats: Long, memo: String?): Invoice {
        val apiKey = config.values[ConfigField.API_KEY]!!

        val body = JSONObject().apply {
            put("correlationId", "pika-${System.currentTimeMillis()}")
            put("description", memo ?: "Pika payment")
            put("amount", JSONObject().apply {
                put("currency", "BTC")
                put("amount", "%.8f".format(amountSats / 100_000_000.0))
            })
        }

        val conn = URL("https://api.strike.me/v1/invoices").openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.setRequestProperty("Authorization", "Bearer $apiKey")
        conn.doOutput = true
        conn.outputStream.use { it.write(body.toString().toByteArray()) }

        val response = conn.inputStream.bufferedReader().readText()
        val json = JSONObject(response)
        val invoiceId = json.getString("invoiceId")

        // Fetch the BOLT11 quote
        val quoteConn = URL("https://api.strike.me/v1/invoices/$invoiceId/quote").openConnection() as HttpURLConnection
        quoteConn.requestMethod = "POST"
        quoteConn.setRequestProperty("Authorization", "Bearer $apiKey")
        quoteConn.doOutput = true
        quoteConn.outputStream.use { it.write("{}".toByteArray()) }

        val quoteResponse = quoteConn.inputStream.bufferedReader().readText()
        val quoteJson = JSONObject(quoteResponse)
        val bolt11 = quoteJson.getString("lnInvoice")

        return Invoice(bolt11 = bolt11, paymentHash = invoiceId, amountSats = amountSats, memo = memo)
    }

    // ─── Blink ──────────────────────────────────────────────────────────────────

    private fun createBlinkInvoice(amountSats: Long, memo: String?): Invoice {
        val apiKey = config.values[ConfigField.API_KEY]!!

        val graphql = """
            mutation LnInvoiceCreateOnBehalfOfRecipient(${'$'}input: LnInvoiceCreateOnBehalfOfRecipientInput!) {
              lnInvoiceCreateOnBehalfOfRecipient(input: ${'$'}input) {
                invoice { paymentRequest paymentHash satoshis }
                errors { message }
              }
            }
        """.trimIndent()

        val body = JSONObject().apply {
            put("query", graphql)
            put("variables", JSONObject().apply {
                put("input", JSONObject().apply {
                    put("amount", amountSats)
                    if (!memo.isNullOrBlank()) put("memo", memo)
                })
            })
        }

        val conn = URL("https://api.blink.sv/graphql").openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.setRequestProperty("X-API-KEY", apiKey)
        conn.doOutput = true
        conn.outputStream.use { it.write(body.toString().toByteArray()) }

        val response = conn.inputStream.bufferedReader().readText()
        val json = JSONObject(response)
        val invoice = json.getJSONObject("data")
            .getJSONObject("lnInvoiceCreateOnBehalfOfRecipient")
            .getJSONObject("invoice")
        val bolt11 = invoice.getString("paymentRequest")
        val paymentHash = invoice.optString("paymentHash", null)

        return Invoice(bolt11 = bolt11, paymentHash = paymentHash, amountSats = amountSats, memo = memo)
    }

    // ─── Speed ──────────────────────────────────────────────────────────────────

    private fun createSpeedInvoice(amountSats: Long, memo: String?): Invoice {
        val apiKey = config.values[ConfigField.API_KEY]!!

        val body = JSONObject().apply {
            put("amount", amountSats * 1000) // Speed uses msats
            if (!memo.isNullOrBlank()) put("description", memo)
        }

        val conn = URL("https://api.tryspeed.com/lightning/invoices").openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.setRequestProperty("speed-api-key", apiKey)
        conn.doOutput = true
        conn.outputStream.use { it.write(body.toString().toByteArray()) }

        val response = conn.inputStream.bufferedReader().readText()
        val json = JSONObject(response)
        val bolt11 = json.getString("payment_request")
        val paymentHash = json.optString("payment_hash", null)

        return Invoice(bolt11 = bolt11, paymentHash = paymentHash, amountSats = amountSats, memo = memo)
    }
}
