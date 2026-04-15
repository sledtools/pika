package com.pika.app

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import com.pika.app.ui.PikaApp
import com.pika.app.ui.theme.PikaTheme

class MainActivity : ComponentActivity() {
    private lateinit var manager: AppManager
    private val amberIntentLauncher =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            AmberIntentBridge.complete(result.resultCode, result.data)
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Initialize Android process context before Rust opens local chat state.
        Keyring.init(applicationContext)

        manager = AppManager.getInstance(applicationContext)
        AmberIntentBridge.bind(this, amberIntentLauncher)
        manager.handleIncomingIntent(intent)

        setContent {
            PikaTheme {
                PikaApp(manager = manager)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        manager.handleIncomingIntent(intent)
    }

    override fun onResume() {
        super.onResume()
        manager.onForeground()
    }

    override fun onDestroy() {
        AmberIntentBridge.unbind(this)
        super.onDestroy()
    }
}
