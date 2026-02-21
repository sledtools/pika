//! Laptop-to-deployed-bot call smoke test.
//!
//! Purpose: validate MLS/Nostr signaling + real MOQ media transport end-to-end against the
//! deployed OpenClaw Marmot bot, without requiring a microphone (uses synthetic audio).
//!
//! Usage:
//!   cargo run -p pika_core --bin interop_openclaw_voice -- <bot_npub>
//!
//! Env:
//!   PIKA_TEST_NSEC: required (nsec1...)
//!   PIKA_E2E_RELAYS / PIKA_E2E_KP_RELAYS: optional comma-separated relay URL lists
//!   PIKA_CALL_MOQ_URL: optional (default: https://us-east.moq.logos.surf/anon)
//!   PIKA_CALL_BROADCAST_PREFIX: optional (default: pika/calls)
//!   PIKA_INTEROP_MEDIA_WINDOW_SECS: optional (default: 10)
//!   PIKA_INTEROP_REQUIRE_RX_FRAMES: optional (if set to N>0, fail unless rx_frames increases by >=N)
//!
//! Exit code:
//!   0 on PASS
//!   1 on failure

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nostr_sdk::prelude::{Client, Filter, Keys, Kind, PublicKey, RelayPoolNotification};
use pika_core::{AppAction, AppReconciler, AppUpdate, AuthState, CallStatus, FfiApp};

const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.primal.net",
    "wss://nos.lol",
    "wss://relay.damus.io",
];

// Match rust/src/core/config.rs defaults. Historically, key packages (kind 443) were NIP-70 protected
// in MDK, so these were relays known to accept protected events. MDK now supports unprotected key
// packages (see mdk#168), but we keep this list for compatibility / debugging.
const DEFAULT_KEY_PACKAGE_RELAYS: &[&str] = &[
    "wss://nostr-pub.wellorder.net",
    "wss://nostr-01.yakihonne.com",
    "wss://nostr-02.yakihonne.com",
    "wss://relay.satlantis.io",
];

const DEFAULT_MOQ_URL: &str = "https://us-east.moq.logos.surf/anon";
const DEFAULT_BROADCAST_PREFIX: &str = "pika/calls";

fn usage() -> ! {
    eprintln!(
        "usage: interop_openclaw_voice <bot_npub>\n\
         \n\
         requires env: PIKA_TEST_NSEC\n\
         optional env: PIKA_E2E_RELAYS, PIKA_E2E_KP_RELAYS, PIKA_CALL_MOQ_URL, PIKA_CALL_BROADCAST_PREFIX\n"
    );
    std::process::exit(2);
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

fn parse_csv_env(key: &str) -> Option<Vec<String>> {
    let s = std::env::var(key).ok()?;
    let v: Vec<String> = s
        .split(',')
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
        .collect();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

fn dedup_preserve_order(xs: impl IntoIterator<Item = String>) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for x in xs {
        if seen.insert(x.clone()) {
            out.push(x);
        }
    }
    out
}

fn relays() -> Vec<String> {
    parse_csv_env("PIKA_E2E_RELAYS").unwrap_or_else(|| {
        // For Step 3 debugging, it's often useful to include both "popular" relays and the
        // protected-kind-friendly set to avoid relay split-brain during group creation.
        dedup_preserve_order(
            DEFAULT_RELAYS
                .iter()
                .chain(DEFAULT_KEY_PACKAGE_RELAYS.iter())
                .map(|s| s.to_string()),
        )
    })
}

fn kp_relays() -> Vec<String> {
    parse_csv_env("PIKA_E2E_KP_RELAYS").unwrap_or_else(|| {
        DEFAULT_KEY_PACKAGE_RELAYS
            .iter()
            .map(|s| s.to_string())
            .collect()
    })
}

fn call_moq_url() -> String {
    std::env::var("PIKA_CALL_MOQ_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MOQ_URL.to_string())
}

fn call_broadcast_prefix() -> String {
    std::env::var("PIKA_CALL_BROADCAST_PREFIX")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BROADCAST_PREFIX.to_string())
}

fn media_window() -> Duration {
    let secs = std::env::var("PIKA_INTEROP_MEDIA_WINDOW_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(10);
    Duration::from_secs(secs)
}

fn require_rx_frames() -> u64 {
    std::env::var("PIKA_INTEROP_REQUIRE_RX_FRAMES")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn find_repo_env_file() -> Option<PathBuf> {
    // Prefer explicit override.
    if let Ok(p) = std::env::var("PIKA_ENV_PATH") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }

    // Try a few common locations relative to CWD.
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..6 {
        let candidate = dir.join(".env");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn read_env_var_from_file(path: &Path, key: &str) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        let mut v = v.trim().to_string();
        // Strip single/double quotes if present.
        if (v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')) {
            v = v[1..v.len() - 1].to_string();
        }
        if !v.is_empty() {
            return Some(v);
        }
    }
    None
}

fn test_nsec() -> String {
    if let Ok(v) = std::env::var("PIKA_TEST_NSEC") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    if let Some(path) = find_repo_env_file() {
        if let Some(v) = read_env_var_from_file(&path, "PIKA_TEST_NSEC") {
            return v;
        }
    }
    eprintln!("missing PIKA_TEST_NSEC (set env var, or put it in a .env file)");
    std::process::exit(1);
}

async fn wait_for_keypackage_on_any_relay(
    relays: &[String],
    pubkey: PublicKey,
    timeout: Duration,
) -> bool {
    let keys = Keys::generate();
    let client = Client::new(keys);
    for r in relays {
        let _ = client.add_relay(r).await;
    }
    client.connect().await;

    let filter = Filter::new()
        .author(pubkey)
        .kind(Kind::MlsKeyPackage)
        .limit(1);
    let _ = client.subscribe(filter, None).await;

    let mut rx = client.notifications();
    let start = Instant::now();
    while start.elapsed() < timeout {
        match tokio::time::timeout(Duration::from_millis(250), rx.recv()).await {
            Ok(Ok(RelayPoolNotification::Event { event, .. })) => {
                if event.kind == Kind::MlsKeyPackage && event.pubkey == pubkey {
                    return true;
                }
            }
            Ok(Ok(_other)) => {}
            Ok(Err(_closed)) => return false,
            Err(_elapsed) => {}
        }
    }
    false
}

#[derive(Clone)]
struct Collector(std::sync::Arc<std::sync::Mutex<Vec<AppUpdate>>>);

impl Collector {
    fn new() -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(Vec::new())))
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

fn status_label(s: &CallStatus) -> String {
    match s {
        CallStatus::Offering => "Offering".into(),
        CallStatus::Ringing => "Ringing".into(),
        CallStatus::Connecting => "Connecting".into(),
        CallStatus::Active => "Active".into(),
        CallStatus::Ended { reason } => format!("Ended({reason})"),
    }
}

fn h_tag(event: &nostr_sdk::Event) -> Option<String> {
    event.tags.iter().find_map(|t| {
        if t.as_slice().first().map(|s| s.as_str()) != Some("h") {
            return None;
        }
        t.as_slice().get(1).map(|s| s.to_string())
    })
}

fn is_protected_event(event: &nostr_sdk::Event) -> bool {
    // NIP-70 protected marker is the tag `["-"]`.
    event
        .tags
        .iter()
        .any(|t| t.as_slice().first().map(|s| s.as_str()) == Some("-"))
}

fn write_config(
    data_dir: &Path,
    relays: &[String],
    kp_relays: &[String],
    moq_url: &str,
    broadcast_prefix: &str,
) {
    let path = data_dir.join("pika_config.json");
    let v = serde_json::json!({
        "disable_network": false,
        "relay_urls": relays,
        "key_package_relay_urls": kp_relays,
        "call_moq_url": moq_url,
        "call_broadcast_prefix": broadcast_prefix,
        "call_audio_backend": "synthetic",
    });
    fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
}

fn main() {
    // Must run before any rustls users initialize (nostr-sdk websockets, quinn/moq, etc).
    pika_core::init_rustls_crypto_provider();

    let mut args = std::env::args().skip(1);
    let bot_npub = args.next().unwrap_or_else(|| usage());
    if args.next().is_some() {
        usage();
    }

    let relays = relays();
    let kp_relays = kp_relays();
    let moq_url = call_moq_url();
    let broadcast_prefix = call_broadcast_prefix();
    let window = media_window();
    let require_rx = require_rx_frames();
    let nsec = test_nsec();

    eprintln!("relays={relays:?}");
    eprintln!("kp_relays={kp_relays:?}");
    eprintln!("call_moq_url={moq_url}");
    eprintln!("call_broadcast_prefix={broadcast_prefix}");
    eprintln!("media_window={window:?}");
    if require_rx > 0 {
        eprintln!("require_rx_frames_delta={require_rx}");
    }

    let bot_pubkey = match PublicKey::parse(&bot_npub) {
        Ok(pk) => pk,
        Err(err) => {
            eprintln!("invalid bot npub: {bot_npub} ({err})");
            std::process::exit(1);
        }
    };

    // Unique state dir (keep it around on failure for inspection).
    let data_dir =
        std::env::temp_dir().join(format!("pika-interop-openclaw-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&data_dir).expect("create data dir");
    write_config(&data_dir, &relays, &kp_relays, &moq_url, &broadcast_prefix);

    // Keep a runtime alive for background observers.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

    // Best-effort: confirm the bot key package is visible before attempting CreateChat.
    // If this fails we still proceed, but CreateChat will likely toast an error.
    let kp_ok = rt.block_on(wait_for_keypackage_on_any_relay(
        &kp_relays,
        bot_pubkey,
        Duration::from_secs(60),
    ));
    if !kp_ok {
        eprintln!("warn: bot key package (kind 443) not observed on kp_relays within timeout; continuing anyway");
    }

    let app = FfiApp::new(data_dir.to_string_lossy().to_string(), String::new());
    let collector = Collector::new();
    app.listen_for_updates(Box::new(collector.clone()));

    app.dispatch(AppAction::Login { nsec });
    wait_until("logged in", Duration::from_secs(20), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    let (self_npub, self_pubkey_hex) = match app.state().auth {
        AuthState::LoggedIn { npub, pubkey } => (npub, pubkey),
        _ => unreachable!("waited for login"),
    };
    eprintln!("self_npub={self_npub}");
    eprintln!("self_pubkey={self_pubkey_hex}");
    eprintln!("bot_pubkey={}", bot_pubkey.to_hex());

    app.dispatch(AppAction::CreateChat {
        peer_npub: bot_npub.clone(),
    });
    wait_until("chat opened", Duration::from_secs(120), || {
        app.state().current_chat.is_some()
    });

    let chat_id = app.state().current_chat.as_ref().unwrap().chat_id.clone();
    eprintln!("chat_id={chat_id}");

    // Nostr wire tap: observe group-message traffic authored by us or the bot.
    // This answers "did we publish anything for this chat" and "did the bot publish anything".
    let relays_for_observer = relays.clone();
    let _chat_id_for_observer = chat_id.clone();
    let self_pubkey = PublicKey::from_hex(&self_pubkey_hex).expect("parse self pubkey");
    rt.spawn(async move {
        let keys = Keys::generate();
        let client = Client::new(keys);
        for r in relays_for_observer.iter() {
            let _ = client.add_relay(r).await;
        }
        client.connect().await;

        // Subscribe separately; avoids relying on multi-author filter API details.
        let _ = client
            .subscribe(
                Filter::new()
                    .kind(Kind::MlsGroupMessage)
                    .author(self_pubkey),
                None,
            )
            .await;
        let _ = client
            .subscribe(
                Filter::new()
                    .kind(Kind::MlsGroupMessage)
                    .author(bot_pubkey),
                None,
            )
            .await;

        let mut rx = client.notifications();
        loop {
            match rx.recv().await {
                Ok(RelayPoolNotification::Event { event, relay_url, .. }) => {
                    let ev = event.as_ref();
                    if ev.kind != Kind::MlsGroupMessage {
                        continue;
                    }
                    let who = if ev.pubkey == self_pubkey {
                        "self"
                    } else if ev.pubkey == bot_pubkey {
                        "bot"
                    } else {
                        "other"
                    };
                    let h = h_tag(ev).unwrap_or_else(|| "<none>".into());
                    let prot = is_protected_event(ev);
                    eprintln!(
                        "nostr_tap who={who} relay={relay_url} kind={} id={} created_at={} h={} protected={}",
                        ev.kind.as_u16(),
                        ev.id.to_hex(),
                        ev.created_at.as_secs(),
                        h,
                        prot
                    );
                }
                Ok(_other) => {}
                Err(_closed) => break,
            }
        }
    });

    // Readiness check: attempt a ping. Don't hard-block the run on a specific response, since
    // deployed bot behavior can change (LLM routing, rate limits, etc).
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let ping = format!("ping:{nonce}");
    let pong = format!("pong:{nonce}");
    app.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: ping,
        kind: None,
        reply_to_message_id: None,
    });
    let ping_window = Duration::from_secs(30);
    let ping_start = Instant::now();
    let mut saw_any_bot_message = false;
    let mut saw_expected_pong = false;
    while ping_start.elapsed() < ping_window {
        let st = app.state();
        let Some(chat) = st.current_chat.as_ref() else {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        for m in chat.messages.iter().rev().take(10) {
            if !m.is_mine {
                saw_any_bot_message = true;
            }
            if m.content == pong {
                saw_expected_pong = true;
            }
        }
        if saw_expected_pong {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if saw_expected_pong {
        eprintln!("pong_ok nonce={nonce}");
    } else if saw_any_bot_message {
        eprintln!("warn: bot replied but not with expected pong nonce={nonce}");
    } else {
        eprintln!("warn: no bot reply observed within {ping_window:?} (continuing)");
    }

    // Optional: send a JSON-looking message to validate server-side "message_received" logging
    // hooks without relying on call-signaling parsing.
    if std::env::var("PIKA_INTEROP_JSON_PROBE").ok().as_deref() == Some("1") {
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let probe = format!(r#"{{"probe":"interop_json","ts_ms":{ts_ms}}}"#);
        app.dispatch(AppAction::SendMessage {
            chat_id: chat_id.clone(),
            content: probe,
            kind: None,
            reply_to_message_id: None,
        });
        std::thread::sleep(Duration::from_millis(500));
    }

    app.dispatch(AppAction::StartCall { chat_id });
    let call_deadline = Instant::now() + Duration::from_secs(120);
    let call_start = Instant::now();
    let mut last_toast = collector.last_toast();
    let mut last_call_status: Option<String> = None;
    let mut last_bot_msg_id: Option<String> = None;
    while Instant::now() < call_deadline {
        if let Some(t) = collector.last_toast() {
            if Some(t.clone()) != last_toast {
                eprintln!("toast={t:?}");
                last_toast = Some(t);
            }
        }

        let st = app.state();
        if let Some(chat) = st.current_chat.as_ref() {
            if let Some(m) = chat.messages.iter().rev().find(|m| !m.is_mine) {
                if Some(m.id.clone()) != last_bot_msg_id {
                    // Useful for diagnosing "bot saw the call invite but treated it as a normal message".
                    let snippet: String = m.content.chars().take(240).collect();
                    eprintln!(
                        "bot_msg id={} sender={} content={:?}",
                        m.id, m.sender_pubkey, snippet
                    );
                    last_bot_msg_id = Some(m.id.clone());
                }
            }
        }
        if let Some(call) = st.active_call.as_ref() {
            let lbl = status_label(&call.status);
            if Some(lbl.clone()) != last_call_status {
                eprintln!(
                    "call_status t_ms={} status={} call_id={}",
                    call_start.elapsed().as_millis(),
                    lbl,
                    call.call_id
                );
                last_call_status = Some(lbl.clone());
            }
            if matches!(call.status, CallStatus::Active) {
                break;
            }
            if matches!(call.status, CallStatus::Ended { .. }) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let st = app.state();
    let is_active = st
        .active_call
        .as_ref()
        .map(|c| matches!(c.status, CallStatus::Active))
        .unwrap_or(false);
    if !is_active {
        eprintln!("fail: call never became Active within timeout");
        if let Some(call) = st.active_call.as_ref() {
            eprintln!("final_call_status={}", status_label(&call.status));
        } else {
            eprintln!("final_call_status=None");
        }
        if let Some(t) = collector.last_toast() {
            eprintln!("last_toast={t:?}");
        }
        eprintln!("state_dir={}", data_dir.to_string_lossy());
        std::process::exit(1);
    }

    // Wait for debug to show up.
    wait_until("call debug present", Duration::from_secs(30), || {
        app.state()
            .active_call
            .as_ref()
            .and_then(|c| c.debug.as_ref())
            .is_some()
    });

    let start = app
        .state()
        .active_call
        .as_ref()
        .and_then(|c| c.debug.as_ref())
        .cloned()
        .unwrap();

    // Let media run for a short window; in synthetic mode this should push tx_frames.
    let mut last_toast = collector.last_toast();
    let t0 = Instant::now();
    while t0.elapsed() < window {
        if let Some(t) = collector.last_toast() {
            if Some(t.clone()) != last_toast {
                eprintln!("toast={t:?}");
                last_toast = Some(t);
            }
        }
        if let Some(dbg) = app
            .state()
            .active_call
            .as_ref()
            .and_then(|c| c.debug.as_ref())
        {
            eprintln!(
                "call_debug tx={} rx={} drop={} jitter={}ms rtt={:?}",
                dbg.tx_frames, dbg.rx_frames, dbg.rx_dropped, dbg.jitter_buffer_ms, dbg.last_rtt_ms
            );
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    let end = app
        .state()
        .active_call
        .as_ref()
        .and_then(|c| c.debug.as_ref())
        .cloned()
        .unwrap();

    // Best-effort cleanup.
    app.dispatch(AppAction::EndCall);
    wait_until("call ended", Duration::from_secs(30), || {
        app.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ended { .. }))
            .unwrap_or(false)
    });

    let tx_delta = end.tx_frames.saturating_sub(start.tx_frames);
    let rx_delta = end.rx_frames.saturating_sub(start.rx_frames);

    if tx_delta < 10 {
        eprintln!(
            "fail: expected tx_frames to increase by >=10 over {:?}, got delta={tx_delta} (start={}, end={})",
            window, start.tx_frames, end.tx_frames
        );
        if let Some(t) = collector.last_toast() {
            eprintln!("last_toast={t:?}");
        }
        eprintln!("state_dir={}", data_dir.to_string_lossy());
        std::process::exit(1);
    }

    if require_rx > 0 && rx_delta < require_rx {
        eprintln!(
            "fail: expected rx_frames to increase by >={require_rx} over {:?}, got delta={rx_delta} (start={}, end={})",
            window, start.rx_frames, end.rx_frames
        );
        if let Some(t) = collector.last_toast() {
            eprintln!("last_toast={t:?}");
        }
        eprintln!("state_dir={}", data_dir.to_string_lossy());
        std::process::exit(1);
    }
    if rx_delta == 0 {
        eprintln!(
            "warn: rx_frames did not increase (bot may not be publishing response audio yet)"
        );
    }

    println!("ok: interop openclaw voice PASS");
}
