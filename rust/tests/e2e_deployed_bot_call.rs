//! FfiApp-level e2e test: call the deployed bot over public relays + real MOQ relay.
//!
//! This test exercises the exact same code path as the iOS/Android app:
//! FfiApp::new → Login → CreateChat → StartCall → media flows → EndCall.
//!
//! Run:
//!   source .env && cargo test -p pika_core --test e2e_deployed_bot_call -- --ignored --nocapture
//!
//! Env:
//!   PIKA_TEST_NSEC          (required)
//!   PIKA_BOT_NPUB           (optional, defaults to the deployed bot)
//!   PIKA_AUDIO_FIXTURE      (optional, defaults to speech_prompt.wav)

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pika_core::{AppAction, AppReconciler, AppUpdate, AuthState, CallStatus, FfiApp};
use tempfile::tempdir;

const DEFAULT_BOT_NPUB: &str = "npub1z6ujr8rad5zp9sr9w22rkxm0truulf2jntrks6rlwskhdmqsawpqmnjlcp";

const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.primal.net",
    "wss://nos.lol",
    "wss://relay.damus.io",
];

const DEFAULT_KP_RELAYS: &[&str] = &[
    "wss://nostr-pub.wellorder.net",
    "wss://nostr-01.yakihonne.com",
    "wss://nostr-02.yakihonne.com",
    "wss://relay.satlantis.io",
];

const DEFAULT_MOQ_URL: &str = "https://us-east.moq.logos.surf/anon";

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_csv_or(key: &str, defaults: &[&str]) -> Vec<String> {
    if let Ok(s) = std::env::var(key) {
        let v: Vec<String> = s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
        if !v.is_empty() {
            return v;
        }
    }
    defaults.iter().map(|s| s.to_string()).collect()
}

fn write_config(data_dir: &str, relays: &[String], kp_relays: &[String], moq_url: &str) {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let v = serde_json::json!({
        "disable_network": false,
        "relay_urls": relays,
        "key_package_relay_urls": kp_relays,
        "call_moq_url": moq_url,
        "call_broadcast_prefix": "pika/calls",
        "call_audio_backend": "synthetic",
    });
    std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{what}: condition not met within {timeout:?}");
}

#[derive(Clone)]
struct Collector(Arc<Mutex<Vec<AppUpdate>>>);

impl Collector {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    fn last_toast(&self) -> Option<String> {
        self.0.lock().unwrap().iter().rev().find_map(|u| match u {
            AppUpdate::FullState(s) => s.toast.clone(),
            _ => None,
        })
    }
}

impl AppReconciler for Collector {
    fn reconcile(&self, update: AppUpdate) {
        self.0.lock().unwrap().push(update);
    }
}

fn status_label(s: &CallStatus) -> &'static str {
    match s {
        CallStatus::Offering => "Offering",
        CallStatus::Ringing => "Ringing",
        CallStatus::Connecting => "Connecting",
        CallStatus::Active => "Active",
        CallStatus::Ended { .. } => "Ended",
    }
}

/// Full FfiApp call test against the deployed bot.
///
/// This is the same code path the iOS/Android app takes.
#[test]
#[ignore]
fn call_deployed_bot_via_ffi_app() {
    pika_core::init_rustls_crypto_provider();

    // --- config ---
    let nsec = match std::env::var("PIKA_TEST_NSEC")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        Some(v) => v.trim().to_string(),
        None => {
            eprintln!("SKIP: set PIKA_TEST_NSEC to run this test");
            return;
        }
    };
    let bot_npub = env_or("PIKA_BOT_NPUB", DEFAULT_BOT_NPUB);
    let relays = env_csv_or("PIKA_E2E_RELAYS", DEFAULT_RELAYS);
    let kp_relays = env_csv_or("PIKA_E2E_KP_RELAYS", DEFAULT_KP_RELAYS);
    let moq_url = env_or("PIKA_CALL_MOQ_URL", DEFAULT_MOQ_URL);

    // Set audio fixture so the caller sends real speech instead of a sine wave.
    if std::env::var("PIKA_AUDIO_FIXTURE").is_err() {
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/speech_prompt.wav"
        );
        if std::path::Path::new(fixture).exists() {
            std::env::set_var("PIKA_AUDIO_FIXTURE", fixture);
            eprintln!("[test] audio fixture: {fixture}");
        }
    }

    eprintln!("[test] bot_npub={bot_npub}");
    eprintln!("[test] relays={relays:?}");
    eprintln!("[test] kp_relays={kp_relays:?}");
    eprintln!("[test] moq_url={moq_url}");

    // --- FfiApp setup (same as iOS AppManager) ---
    let dir = tempdir().unwrap();
    write_config(&dir.path().to_string_lossy(), &relays, &kp_relays, &moq_url);

    let app = FfiApp::new(dir.path().to_string_lossy().to_string(), String::new());
    let collector = Collector::new();
    app.listen_for_updates(Box::new(collector.clone()));

    // --- Login ---
    app.dispatch(AppAction::Login { nsec });
    wait_until("logged in", Duration::from_secs(20), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });
    let self_npub = match &app.state().auth {
        AuthState::LoggedIn { npub, .. } => npub.clone(),
        _ => unreachable!(),
    };
    eprintln!("[test] logged in as {self_npub}");

    // --- Create chat with bot ---
    app.dispatch(AppAction::CreateChat {
        peer_npub: bot_npub.clone(),
    });
    wait_until("chat opened", Duration::from_secs(120), || {
        app.state().current_chat.is_some()
    });
    let chat_id = app.state().current_chat.as_ref().unwrap().chat_id.clone();
    eprintln!("[test] chat_id={chat_id}");

    // --- Ping/pong to confirm bot is responsive ---
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let ping = format!("ping:{nonce}");
    let pong = format!("pong:{nonce}");
    app.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: ping,
    });
    wait_until("bot pong", Duration::from_secs(30), || {
        app.state()
            .current_chat
            .as_ref()
            .map(|c| c.messages.iter().any(|m| m.content == pong))
            .unwrap_or(false)
    });
    eprintln!("[test] PASS: ping/pong");

    // --- Start call ---
    app.dispatch(AppAction::StartCall {
        chat_id: chat_id.clone(),
    });

    // Wait for call to become active (bot must accept).
    let call_start = Instant::now();
    wait_until("call active", Duration::from_secs(60), || {
        if let Some(t) = collector.last_toast() {
            eprintln!("[test] toast: {t}");
        }
        let st = app.state();
        if let Some(call) = st.active_call.as_ref() {
            let lbl = status_label(&call.status);
            if call_start.elapsed().as_millis() % 2000 < 150 {
                eprintln!(
                    "[test] call_status={lbl} call_id={} t={}ms",
                    call.call_id,
                    call_start.elapsed().as_millis()
                );
            }
            matches!(call.status, CallStatus::Active)
        } else {
            false
        }
    });

    let call_id = app
        .state()
        .active_call
        .as_ref()
        .map(|c| c.call_id.clone())
        .unwrap();
    eprintln!("[test] PASS: call active, call_id={call_id}");

    // --- Wait for debug stats to appear ---
    wait_until("call debug present", Duration::from_secs(10), || {
        app.state()
            .active_call
            .as_ref()
            .and_then(|c| c.debug.as_ref())
            .is_some()
    });

    // --- Verify tx frames are flowing (we are publishing audio) ---
    wait_until("tx frames flowing", Duration::from_secs(10), || {
        app.state()
            .active_call
            .as_ref()
            .and_then(|c| c.debug.as_ref())
            .map(|d| d.tx_frames > 10)
            .unwrap_or(false)
    });
    eprintln!("[test] PASS: tx frames flowing");

    // --- Wait for rx frames (bot publishes TTS audio back) ---
    let media_window = Duration::from_secs(20);
    let media_start = Instant::now();
    let mut max_rx: u64 = 0;
    while media_start.elapsed() < media_window {
        if let Some(dbg) = app
            .state()
            .active_call
            .as_ref()
            .and_then(|c| c.debug.as_ref())
        {
            if dbg.rx_frames > max_rx {
                max_rx = dbg.rx_frames;
            }
            eprintln!(
                "[test] media tx={} rx={} drop={} jitter={}ms",
                dbg.tx_frames, dbg.rx_frames, dbg.rx_dropped, dbg.jitter_buffer_ms
            );
        }
        if max_rx >= 10 {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    eprintln!("[test] media window done: max_rx={max_rx}");
    assert!(
        max_rx >= 5,
        "expected at least 5 rx frames from bot (got {max_rx}); subscriber may be stalling after 1 frame"
    );
    eprintln!("[test] PASS: rx frames received ({max_rx})");

    // --- End call ---
    app.dispatch(AppAction::EndCall);
    wait_until("call ended", Duration::from_secs(15), || {
        app.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ended { .. }))
            .unwrap_or(false)
    });
    eprintln!("[test] PASS: call ended cleanly");

    // --- Summary ---
    let st = app.state();
    if let Some(d) = st.active_call.as_ref().and_then(|c| c.debug.as_ref()) {
        eprintln!(
            "[test] final stats: tx={} rx={} drop={}",
            d.tx_frames, d.rx_frames, d.rx_dropped
        );
    }
    eprintln!("[test] PASS: call_deployed_bot_via_ffi_app");
}
