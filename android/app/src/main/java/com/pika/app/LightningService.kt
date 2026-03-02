package com.pika.app

import android.util.Log
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL

private const val TAG = "LightningService"

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
        Log.i(TAG, "createInvoice: ${config.nodeType.label}, amount=$amountSats, memo=$memo")
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

    data class PaymentResult(
        val paymentHash: String?,
        val preimage: String?,
        val amountSats: Long?,
    )

    suspend fun payInvoice(bolt11: String): Result<PaymentResult> = withContext(Dispatchers.IO) {
        Log.i(TAG, "payInvoice: ${config.nodeType.label}, invoice=${bolt11.take(30)}...")
        runCatching {
            when (config.nodeType) {
                LightningNodeType.LND -> payLndInvoice(bolt11)
                LightningNodeType.CLN -> payClnInvoice(bolt11)
                LightningNodeType.PHOENIXD -> payPhoenixdInvoice(bolt11)
                LightningNodeType.NWC -> throw UnsupportedOperationException("NWC pay not yet implemented")
                LightningNodeType.STRIKE -> payStrikeInvoice(bolt11)
                LightningNodeType.BLINK -> payBlinkInvoice(bolt11)
                LightningNodeType.SPEED -> paySpeedInvoice(bolt11)
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

        // Step 1: Get the default BTC wallet ID
        val walletQuery = JSONObject().apply {
            put("query", "query { me { defaultAccount { wallets { id walletCurrency } } } }")
        }

        Log.d(TAG, "Blink: fetching wallet ID")
        val walletConn = URL("https://api.blink.sv/graphql").openConnection() as HttpURLConnection
        walletConn.requestMethod = "POST"
        walletConn.setRequestProperty("Content-Type", "application/json")
        walletConn.setRequestProperty("X-API-KEY", apiKey)
        walletConn.doOutput = true
        walletConn.outputStream.use { it.write(walletQuery.toString().toByteArray()) }

        val walletResponseCode = walletConn.responseCode
        val walletResponse = if (walletResponseCode in 200..299) {
            walletConn.inputStream.bufferedReader().readText()
        } else {
            val errBody = walletConn.errorStream?.bufferedReader()?.readText() ?: "no body"
            Log.e(TAG, "Blink wallet query failed: HTTP $walletResponseCode: $errBody")
            throw RuntimeException("Blink API error ($walletResponseCode): $errBody")
        }
        Log.d(TAG, "Blink wallet response: $walletResponse")

        val walletJson = JSONObject(walletResponse)
        val wallets = walletJson.getJSONObject("data")
            .getJSONObject("me")
            .getJSONObject("defaultAccount")
            .getJSONArray("wallets")

        var btcWalletId: String? = null
        for (i in 0 until wallets.length()) {
            val w = wallets.getJSONObject(i)
            if (w.getString("walletCurrency") == "BTC") {
                btcWalletId = w.getString("id")
                break
            }
        }
        if (btcWalletId == null) throw RuntimeException("No BTC wallet found in Blink account")
        Log.d(TAG, "Blink BTC wallet ID: $btcWalletId")

        // Step 2: Create invoice
        val graphql = """
            mutation LnInvoiceCreate(${'$'}input: LnInvoiceCreateInput!) {
              lnInvoiceCreate(input: ${'$'}input) {
                invoice { paymentRequest paymentHash satoshis }
                errors { message }
              }
            }
        """.trimIndent()

        val body = JSONObject().apply {
            put("query", graphql)
            put("variables", JSONObject().apply {
                put("input", JSONObject().apply {
                    put("walletId", btcWalletId)
                    put("amount", amountSats)
                    if (!memo.isNullOrBlank()) put("memo", memo)
                })
            })
        }

        Log.d(TAG, "Blink: creating invoice for $amountSats sats")
        val conn = URL("https://api.blink.sv/graphql").openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.setRequestProperty("X-API-KEY", apiKey)
        conn.doOutput = true
        conn.outputStream.use { it.write(body.toString().toByteArray()) }

        val responseCode = conn.responseCode
        val response = if (responseCode in 200..299) {
            conn.inputStream.bufferedReader().readText()
        } else {
            val errBody = conn.errorStream?.bufferedReader()?.readText() ?: "no body"
            Log.e(TAG, "Blink invoice create failed: HTTP $responseCode: $errBody")
            throw RuntimeException("Blink API error ($responseCode): $errBody")
        }
        Log.d(TAG, "Blink invoice response: $response")

        val json = JSONObject(response)
        val data = json.optJSONObject("data")
            ?: throw RuntimeException("Blink: no data in response: $response")
        val invoiceResult = data.optJSONObject("lnInvoiceCreate")
            ?: throw RuntimeException("Blink: no lnInvoiceCreate in response: $response")

        val errors = invoiceResult.optJSONArray("errors")
        if (errors != null && errors.length() > 0) {
            val errMsg = errors.getJSONObject(0).getString("message")
            throw RuntimeException("Blink error: $errMsg")
        }

        val invoice = invoiceResult.getJSONObject("invoice")
        val bolt11 = invoice.getString("paymentRequest")
        val paymentHash = invoice.optString("paymentHash", null)

        Log.i(TAG, "Blink invoice created: ${bolt11.take(30)}...")
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

    // ═══ PAY INVOICE IMPLEMENTATIONS ════════════════════════════════════════════

    private fun payLndInvoice(bolt11: String): PaymentResult {
        val baseUrl = config.values[ConfigField.URL]!!.trimEnd('/')
        val macaroon = config.values[ConfigField.MACAROON]!!

        val body = JSONObject().apply {
            put("payment_request", bolt11)
            put("fee_limit", JSONObject().apply { put("percent", "1") })
        }

        val conn = URL("$baseUrl/v1/channels/transactions").openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.setRequestProperty("Grpc-Metadata-macaroon", macaroon)
        conn.doOutput = true
        conn.outputStream.use { it.write(body.toString().toByteArray()) }

        val responseCode = conn.responseCode
        val response = readResponse(conn, responseCode)
        val json = JSONObject(response)

        val payError = json.optString("payment_error", "")
        if (payError.isNotBlank()) throw RuntimeException("LND payment error: $payError")

        return PaymentResult(
            paymentHash = json.optString("payment_hash", null),
            preimage = json.optString("payment_preimage", null),
            amountSats = json.optLong("value_sat", 0),
        )
    }

    private fun payClnInvoice(bolt11: String): PaymentResult {
        val baseUrl = config.values[ConfigField.URL]!!.trimEnd('/')
        val rune = config.values[ConfigField.RUNE]!!

        val body = JSONObject().apply { put("bolt11", bolt11) }

        val conn = URL("$baseUrl/v1/pay").openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.setRequestProperty("Rune", rune)
        conn.doOutput = true
        conn.outputStream.use { it.write(body.toString().toByteArray()) }

        val responseCode = conn.responseCode
        val response = readResponse(conn, responseCode)
        val json = JSONObject(response)

        return PaymentResult(
            paymentHash = json.optString("payment_hash", null),
            preimage = json.optString("payment_preimage", null),
            amountSats = json.optLong("amount_sent_msat", 0) / 1000,
        )
    }

    private fun payPhoenixdInvoice(bolt11: String): PaymentResult {
        val baseUrl = config.values[ConfigField.URL]!!.trimEnd('/')
        val password = config.values[ConfigField.PASSWORD]!!

        val formData = "invoice=$bolt11"

        val conn = URL("$baseUrl/payinvoice").openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/x-www-form-urlencoded")
        val auth = android.util.Base64.encodeToString("$password:".toByteArray(), android.util.Base64.NO_WRAP)
        conn.setRequestProperty("Authorization", "Basic $auth")
        conn.doOutput = true
        conn.outputStream.use { it.write(formData.toByteArray()) }

        val responseCode = conn.responseCode
        val response = readResponse(conn, responseCode)
        val json = JSONObject(response)

        return PaymentResult(
            paymentHash = json.optString("paymentHash", null),
            preimage = json.optString("paymentPreimage", null),
            amountSats = json.optLong("recipientAmountSat", 0),
        )
    }

    private fun payStrikeInvoice(bolt11: String): PaymentResult {
        val apiKey = config.values[ConfigField.API_KEY]!!

        val body = JSONObject().apply {
            put("lnInvoice", bolt11)
        }

        val conn = URL("https://api.strike.me/v1/payment-quotes/lightning").openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.setRequestProperty("Authorization", "Bearer $apiKey")
        conn.doOutput = true
        conn.outputStream.use { it.write(body.toString().toByteArray()) }

        val responseCode = conn.responseCode
        val response = readResponse(conn, responseCode)
        val quoteJson = JSONObject(response)
        val paymentQuoteId = quoteJson.getString("paymentQuoteId")

        // Execute the quote
        val execConn = URL("https://api.strike.me/v1/payment-quotes/$paymentQuoteId/execute").openConnection() as HttpURLConnection
        execConn.requestMethod = "PATCH"
        execConn.setRequestProperty("Authorization", "Bearer $apiKey")

        val execResponseCode = execConn.responseCode
        val execResponse = readResponse(execConn, execResponseCode)
        Log.d(TAG, "Strike pay response: $execResponse")

        return PaymentResult(paymentHash = paymentQuoteId, preimage = null, amountSats = null)
    }

    private fun payBlinkInvoice(bolt11: String): PaymentResult {
        val apiKey = config.values[ConfigField.API_KEY]!!

        // Get BTC wallet ID
        val walletQuery = JSONObject().apply {
            put("query", "query { me { defaultAccount { wallets { id walletCurrency } } } }")
        }
        val walletConn = URL("https://api.blink.sv/graphql").openConnection() as HttpURLConnection
        walletConn.requestMethod = "POST"
        walletConn.setRequestProperty("Content-Type", "application/json")
        walletConn.setRequestProperty("X-API-KEY", apiKey)
        walletConn.doOutput = true
        walletConn.outputStream.use { it.write(walletQuery.toString().toByteArray()) }

        val walletResponse = readResponse(walletConn, walletConn.responseCode)
        val wallets = JSONObject(walletResponse).getJSONObject("data")
            .getJSONObject("me").getJSONObject("defaultAccount").getJSONArray("wallets")

        var btcWalletId: String? = null
        for (i in 0 until wallets.length()) {
            val w = wallets.getJSONObject(i)
            if (w.getString("walletCurrency") == "BTC") { btcWalletId = w.getString("id"); break }
        }
        if (btcWalletId == null) throw RuntimeException("No BTC wallet found in Blink account")

        // Pay
        val graphql = """
            mutation LnInvoicePaymentSend(${'$'}input: LnInvoicePaymentInput!) {
              lnInvoicePaymentSend(input: ${'$'}input) {
                status
                errors { message }
              }
            }
        """.trimIndent()

        val body = JSONObject().apply {
            put("query", graphql)
            put("variables", JSONObject().apply {
                put("input", JSONObject().apply {
                    put("walletId", btcWalletId)
                    put("paymentRequest", bolt11)
                })
            })
        }

        Log.d(TAG, "Blink: paying invoice")
        val conn = URL("https://api.blink.sv/graphql").openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.setRequestProperty("X-API-KEY", apiKey)
        conn.doOutput = true
        conn.outputStream.use { it.write(body.toString().toByteArray()) }

        val responseCode = conn.responseCode
        val response = readResponse(conn, responseCode)
        Log.d(TAG, "Blink pay response: $response")

        val json = JSONObject(response)
        val payResult = json.getJSONObject("data").getJSONObject("lnInvoicePaymentSend")
        val errors = payResult.optJSONArray("errors")
        if (errors != null && errors.length() > 0) {
            throw RuntimeException("Blink: ${errors.getJSONObject(0).getString("message")}")
        }
        val status = payResult.getString("status")
        Log.i(TAG, "Blink payment status: $status")

        return PaymentResult(paymentHash = null, preimage = null, amountSats = null)
    }

    private fun paySpeedInvoice(bolt11: String): PaymentResult {
        val apiKey = config.values[ConfigField.API_KEY]!!

        val body = JSONObject().apply { put("invoice", bolt11) }

        val conn = URL("https://api.tryspeed.com/lightning/invoices/pay").openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.setRequestProperty("speed-api-key", apiKey)
        conn.doOutput = true
        conn.outputStream.use { it.write(body.toString().toByteArray()) }

        val responseCode = conn.responseCode
        val response = readResponse(conn, responseCode)
        val json = JSONObject(response)

        return PaymentResult(
            paymentHash = json.optString("payment_hash", null),
            preimage = json.optString("payment_preimage", null),
            amountSats = null,
        )
    }

    // ─── Helpers ────────────────────────────────────────────────────────────────

    private fun readResponse(conn: HttpURLConnection, code: Int): String {
        return if (code in 200..299) {
            conn.inputStream.bufferedReader().readText()
        } else {
            val errBody = conn.errorStream?.bufferedReader()?.readText() ?: "no body"
            Log.e(TAG, "HTTP $code: $errBody")
            throw RuntimeException("HTTP $code: $errBody")
        }
    }
}
