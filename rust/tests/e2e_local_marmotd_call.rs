//! Local end-to-end call test: FfiApp caller ↔ marmotd daemon (bot).
//!
//! Validates the full call signaling path using:
//! - A local Nostr relay for MLS signaling
//! - A locally spawned `marmotd daemon` as the bot/callee
//! - A locally spawned `moq-relay` for media transport
//!
//! This exercises the exact same code path as the deployed OpenClaw bot,
//! but without depending on the remote deployment.
//!
//! Requires:
//! - `marmotd` built from the workspace crate `crates/marmotd` (run `just e2e-local-marmotd`)
//! - `moq-relay` on PATH (available in `nix develop`)

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use nostr_sdk::filter::MatchEventOptions;
use nostr_sdk::nostr::{Event, EventId, Filter};
use pika_core::{AppAction, AuthState, CallStatus, FfiApp};
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

#[path = "support/mod.rs"]
mod support;

fn marmotd_binary() -> String {
    if let Ok(bin) = std::env::var("MARMOTD_BIN") {
        if !bin.trim().is_empty() {
            return bin;
        }
    }
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    repo_root
        .join("target/debug/marmotd")
        .to_string_lossy()
        .to_string()
}

fn write_config(data_dir: &str, relay_url: &str, kp_relay_url: Option<&str>, moq_url: &str) {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let mut v = serde_json::json!({
        "disable_network": false,
        "relay_urls": [relay_url],
        "call_moq_url": moq_url,
        "call_broadcast_prefix": "pika/calls",
        "call_audio_backend": "synthetic",
    });
    if let Some(kp) = kp_relay_url {
        v.as_object_mut().unwrap().insert(
            "key_package_relay_urls".to_string(),
            serde_json::json!([kp]),
        );
    }
    std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{what}: condition not met within {timeout:?}");
}

// --- Minimal local Nostr relay (same as e2e_real_moq_relay.rs) ---

#[derive(Clone)]
struct LocalRelayHandle {
    url: String,
    shutdown: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl Drop for LocalRelayHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.lock().unwrap().take() {
            let _ = tx.send(());
        }
    }
}

struct ConnEntry {
    tx: mpsc::UnboundedSender<Message>,
    subs: HashMap<String, Vec<Filter>>,
}

struct RelayState {
    events: Vec<Event>,
    event_ids: HashSet<EventId>,
    conns: HashMap<u64, ConnEntry>,
}

type SubSnapshot = (String, Vec<Filter>);
type ConnSnapshot = (u64, Vec<SubSnapshot>);

fn send_json(state: &Arc<Mutex<RelayState>>, conn_id: u64, v: serde_json::Value) -> bool {
    let tx = {
        let st = state.lock().unwrap();
        st.conns.get(&conn_id).map(|c| c.tx.clone())
    };
    if let Some(tx) = tx {
        return tx.send(Message::Text(v.to_string().into())).is_ok();
    }
    false
}

fn broadcast_event(state: &Arc<Mutex<RelayState>>, ev: &Event) {
    let conns: Vec<ConnSnapshot> = {
        let st = state.lock().unwrap();
        st.conns
            .iter()
            .map(|(id, c)| {
                let subs: Vec<SubSnapshot> = c
                    .subs
                    .iter()
                    .map(|(sid, filters)| (sid.clone(), filters.clone()))
                    .collect();
                (*id, subs)
            })
            .collect()
    };
    for (conn_id, subs) in conns {
        for (sub_id, filters) in subs {
            if filters
                .iter()
                .any(|f| f.match_event(ev, MatchEventOptions::new()))
            {
                let _ = send_json(state, conn_id, serde_json::json!(["EVENT", sub_id, ev]));
            }
        }
    }
}

fn handle_client_msg(state: &Arc<Mutex<RelayState>>, conn_id: u64, text: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let Some(arr) = v.as_array() else { return };
    let Some(typ) = arr.first().and_then(|x| x.as_str()) else {
        return;
    };
    match typ {
        "EVENT" => {
            let Some(ev_v) = arr.get(1) else { return };
            let Ok(ev) = serde_json::from_value::<Event>(ev_v.clone()) else {
                return;
            };
            let is_new = {
                let mut st = state.lock().unwrap();
                if st.event_ids.contains(&ev.id) {
                    false
                } else {
                    st.event_ids.insert(ev.id);
                    st.events.push(ev.clone());
                    true
                }
            };
            let _ = send_json(state, conn_id, serde_json::json!(["OK", ev.id, true, ""]));
            if is_new {
                broadcast_event(state, &ev);
            }
        }
        "REQ" => {
            let Some(sub_id) = arr.get(1).and_then(|x| x.as_str()).map(|s| s.to_string()) else {
                return;
            };
            let mut filters: Vec<Filter> = Vec::new();
            for f in arr.iter().skip(2) {
                if let Ok(filter) = serde_json::from_value::<Filter>(f.clone()) {
                    filters.push(filter);
                }
            }
            if filters.is_empty() {
                return;
            }
            {
                let mut st = state.lock().unwrap();
                if let Some(conn) = st.conns.get_mut(&conn_id) {
                    conn.subs.insert(sub_id.clone(), filters.clone());
                }
            }
            let events: Vec<Event> = {
                let st = state.lock().unwrap();
                st.events.clone()
            };
            for ev in events {
                if filters
                    .iter()
                    .any(|f| f.match_event(&ev, MatchEventOptions::new()))
                {
                    let _ = send_json(state, conn_id, serde_json::json!(["EVENT", sub_id, ev]));
                }
            }
            let _ = send_json(state, conn_id, serde_json::json!(["EOSE", sub_id]));
        }
        "CLOSE" => {
            let Some(sub_id) = arr.get(1).and_then(|x| x.as_str()) else {
                return;
            };
            let mut st = state.lock().unwrap();
            if let Some(conn) = st.conns.get_mut(&conn_id) {
                conn.subs.remove(sub_id);
            }
        }
        _ => {}
    }
}

fn start_local_relay() -> (LocalRelayHandle, JoinHandle<()>) {
    let (url_tx, url_rx) = std::sync::mpsc::channel::<(String, oneshot::Sender<()>)>();
    let state = Arc::new(Mutex::new(RelayState {
        events: Vec::new(),
        event_ids: HashSet::new(),
        conns: HashMap::new(),
    }));
    let state_for_thread = state.clone();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            let url = format!("ws://{}", addr);
            let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
            url_tx.send((url, shutdown_tx)).unwrap();
            let next_conn_id = Arc::new(AtomicU64::new(1));
            let state = state_for_thread;
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accept = listener.accept() => {
                        let (stream, _) = match accept {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let state = state.clone();
                        let next_conn_id = next_conn_id.clone();
                        tokio::spawn(async move {
                            let ws = match tokio_tungstenite::accept_async(stream).await {
                                Ok(ws) => ws,
                                Err(_) => return,
                            };
                            let (mut ws_tx, mut ws_rx) = ws.split();
                            let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
                            let conn_id = next_conn_id.fetch_add(1, Ordering::Relaxed);
                            {
                                let mut st = state.lock().unwrap();
                                st.conns.insert(conn_id, ConnEntry {
                                    tx: out_tx.clone(),
                                    subs: HashMap::new(),
                                });
                            }
                            let writer = tokio::spawn(async move {
                                while let Some(msg) = out_rx.recv().await {
                                    if ws_tx.send(msg).await.is_err() { break; }
                                }
                            });
                            while let Some(Ok(msg)) = ws_rx.next().await {
                                match msg {
                                    Message::Text(text) => handle_client_msg(&state, conn_id, &text),
                                    Message::Ping(p) => { let _ = out_tx.send(Message::Pong(p)); }
                                    Message::Close(_) => break,
                                    _ => {}
                                }
                            }
                            { state.lock().unwrap().conns.remove(&conn_id); }
                            writer.abort();
                        });
                    }
                }
            }
        });
    });
    let (url, shutdown_tx) = url_rx.recv().unwrap();
    (
        LocalRelayHandle {
            url,
            shutdown: Arc::new(Mutex::new(Some(shutdown_tx))),
        },
        thread,
    )
}

// --- JSONL helpers for marmotd daemon ---

struct DaemonHandle {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout_lines: Arc<Mutex<Vec<serde_json::Value>>>,
    stderr_thread: Option<JoinHandle<()>>,
    stdout_thread: Option<JoinHandle<()>>,
}

impl DaemonHandle {
    fn spawn(relay_url: &str, state_dir: &str) -> Self {
        let bin = marmotd_binary();
        assert!(
            std::path::Path::new(&bin).exists(),
            "marmotd binary not found at {bin}. Build it: just e2e-local-marmotd"
        );
        let use_real_ai = std::env::var("OPENAI_API_KEY").is_ok();
        eprintln!(
            "[daemon] spawning {bin} daemon --relay {relay_url} --state-dir {state_dir} real_ai={use_real_ai}"
        );
        let mut cmd = Command::new(&bin);
        cmd.arg("daemon")
            .arg("--relay")
            .arg(relay_url)
            .arg("--state-dir")
            .arg(state_dir);
        if use_real_ai {
            cmd.env("OPENAI_API_KEY", std::env::var("OPENAI_API_KEY").unwrap());
        } else {
            cmd.env("MARMOT_STT_FIXTURE_TEXT", "hello from fixture")
                .env("MARMOT_TTS_FIXTURE", "1");
        }
        let mut child = cmd
            .env(
                "MARMOT_ECHO_MODE",
                std::env::var("MARMOT_ECHO_MODE").unwrap_or_default(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn marmotd failed: {e}"));

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let stderr_thread = std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                eprintln!("[marmotd stderr] {line}");
            }
        });

        let stdout_lines: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_for_thread = stdout_lines.clone();
        let stdout_thread = std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                eprintln!("[marmotd stdout] {line}");
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                    lines_for_thread.lock().unwrap().push(v);
                }
            }
        });

        Self {
            child,
            stdin,
            stdout_lines,
            stderr_thread: Some(stderr_thread),
            stdout_thread: Some(stdout_thread),
        }
    }

    fn send_cmd(&mut self, v: serde_json::Value) {
        let s = serde_json::to_string(&v).unwrap();
        writeln!(self.stdin, "{s}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn wait_for_event(
        &self,
        what: &str,
        timeout: Duration,
        pred: impl Fn(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        let start = Instant::now();
        let mut last_idx = 0;
        while start.elapsed() < timeout {
            let lines = self.stdout_lines.lock().unwrap();
            for i in last_idx..lines.len() {
                if pred(&lines[i]) {
                    return lines[i].clone();
                }
            }
            last_idx = lines.len();
            drop(lines);
            std::thread::sleep(Duration::from_millis(50));
        }
        let lines = self.stdout_lines.lock().unwrap();
        eprintln!("[daemon] all {what} stdout events ({} total):", lines.len());
        for (i, l) in lines.iter().enumerate() {
            eprintln!("  [{i}] {l}");
        }
        panic!("{what}: daemon event not received within {timeout:?}");
    }

    fn pubkey(&self) -> String {
        let lines = self.stdout_lines.lock().unwrap();
        for l in lines.iter() {
            if l.get("type").and_then(|t| t.as_str()) == Some("ready") {
                return l
                    .get("pubkey")
                    .and_then(|p| p.as_str())
                    .unwrap()
                    .to_string();
            }
        }
        panic!("daemon ready event not found");
    }

    fn npub(&self) -> String {
        let lines = self.stdout_lines.lock().unwrap();
        for l in lines.iter() {
            if l.get("type").and_then(|t| t.as_str()) == Some("ready") {
                return l.get("npub").and_then(|p| p.as_str()).unwrap().to_string();
            }
        }
        panic!("daemon ready event not found");
    }
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(t) = self.stderr_thread.take() {
            let _ = t.join();
        }
        if let Some(t) = self.stdout_thread.take() {
            let _ = t.join();
        }
    }
}

// --- Core test logic (relay URL parameterized) ---

fn run_marmotd_call_test(relay_url: &str, moq_url: &str) {
    let bin = marmotd_binary();
    if !std::path::Path::new(&bin).exists() {
        eprintln!("SKIP: marmotd binary not found at {bin}");
        eprintln!("Build it: just e2e-local-marmotd");
        return;
    }

    // When using real AI, feed speech audio instead of a sine wave so Whisper
    // can produce a real transcript.
    if std::env::var("OPENAI_API_KEY").is_ok() {
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/speech_prompt.wav"
        );
        if std::path::Path::new(fixture).exists() {
            std::env::set_var("PIKA_AUDIO_FIXTURE", fixture);
            eprintln!("[test] audio fixture: {fixture}");
        }
    }

    eprintln!("[test] using relay: {relay_url}");

    // Spawn marmotd daemon as the "bot".
    let daemon_state = tempdir().unwrap();
    let mut daemon = DaemonHandle::spawn(relay_url, &daemon_state.path().to_string_lossy());

    // Wait for daemon to be ready.
    daemon.wait_for_event("daemon ready", Duration::from_secs(15), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("ready")
    });
    let daemon_npub = daemon.npub();
    let daemon_pubkey = daemon.pubkey();
    eprintln!("[test] daemon pubkey={daemon_pubkey} npub={daemon_npub}");

    // Simulate what channel.ts does after sidecar is ready:
    // 1. set_relays (adds additional relays beyond the --relay arg)
    // 2. publish_keypackage
    let extra_relays: Vec<&str> =
        if relay_url.starts_with("ws://127.") || relay_url.starts_with("ws://localhost") {
            // Local relay: no extra relays to add
            vec![relay_url]
        } else {
            // Public relay: simulate the deployed config with 3 relays
            vec![
                "wss://relay.primal.net",
                "wss://nos.lol",
                "wss://relay.damus.io",
            ]
        };

    daemon.send_cmd(serde_json::json!({
        "cmd": "set_relays",
        "request_id": "sr1",
        "relays": extra_relays
    }));
    daemon.wait_for_event("set_relays ok", Duration::from_secs(15), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("ok")
            && v.get("request_id").and_then(|id| id.as_str()) == Some("sr1")
    });
    eprintln!("[test] daemon relays set to {:?}", extra_relays);

    daemon.send_cmd(serde_json::json!({
        "cmd": "publish_keypackage",
        "request_id": "kp1",
        "relays": extra_relays
    }));
    daemon.wait_for_event("kp published", Duration::from_secs(15), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("ok")
            && v.get("request_id").and_then(|id| id.as_str()) == Some("kp1")
    });
    eprintln!("[test] daemon key package published");

    // Create FfiApp as the caller (use same relays as daemon).
    let caller_dir = tempdir().unwrap();
    // For the config, use the first relay as primary
    write_config(
        &caller_dir.path().to_string_lossy(),
        extra_relays[0],
        Some(extra_relays[0]),
        moq_url,
    );
    let caller = FfiApp::new(
        caller_dir.path().to_string_lossy().to_string(),
        String::new(),
    );

    caller.dispatch(AppAction::CreateAccount);
    wait_until("caller logged in", Duration::from_secs(10), || {
        matches!(caller.state().auth, AuthState::LoggedIn { .. })
    });
    eprintln!("[test] caller logged in");

    // Create MLS group with daemon.
    caller.dispatch(AppAction::CreateChat {
        peer_npub: daemon_npub.clone(),
    });
    wait_until("caller chat opened", Duration::from_secs(30), || {
        caller.state().current_chat.is_some()
    });
    let chat_id = caller
        .state()
        .current_chat
        .as_ref()
        .unwrap()
        .chat_id
        .clone();
    eprintln!("[test] chat created: {chat_id}");

    // Daemon should see the welcome. We need to accept it via JSONL.
    let welcome = daemon.wait_for_event("daemon welcome_received", Duration::from_secs(30), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("welcome_received")
    });
    let wrapper_id = welcome
        .get("wrapper_event_id")
        .and_then(|x| x.as_str())
        .unwrap()
        .to_string();
    eprintln!("[test] daemon got welcome, accepting wrapper={wrapper_id}");

    daemon.send_cmd(serde_json::json!({
        "cmd": "accept_welcome",
        "request_id": "acc1",
        "wrapper_event_id": wrapper_id
    }));
    daemon.wait_for_event("daemon group_joined", Duration::from_secs(30), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("group_joined")
    });
    eprintln!("[test] daemon joined group");

    // --- Test 1: ping/pong (text messaging) ---
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let ping_msg = format!("ping:{nonce}");
    let pong_msg = format!("pong:{nonce}");
    eprintln!("[test] sending ping: {ping_msg}");

    caller.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: ping_msg.clone(),
        kind: None,
        reply_to_message_id: None,
    });

    // Daemon should receive the message.
    let msg = daemon.wait_for_event(
        "daemon message_received (ping)",
        Duration::from_secs(30),
        |v| {
            v.get("type").and_then(|t| t.as_str()) == Some("message_received")
                && v.get("content")
                    .and_then(|c| c.as_str())
                    .map(|c| c == ping_msg)
                    .unwrap_or(false)
        },
    );
    eprintln!("[test] daemon received ping: {:?}", msg.get("content"));

    // Simulate what channel.ts does: send the pong reply.
    let nostr_group_id = msg
        .get("nostr_group_id")
        .and_then(|x| x.as_str())
        .unwrap()
        .to_string();

    daemon.send_cmd(serde_json::json!({
        "cmd": "send_message",
        "request_id": "pong1",
        "nostr_group_id": nostr_group_id,
        "content": pong_msg
    }));
    daemon.wait_for_event("pong send ok", Duration::from_secs(15), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("ok")
            && v.get("request_id").and_then(|id| id.as_str()) == Some("pong1")
    });

    // Caller should receive the pong.
    wait_until("caller received pong", Duration::from_secs(30), || {
        caller
            .state()
            .current_chat
            .as_ref()
            .and_then(|c| c.messages.iter().find(|m| m.content == pong_msg))
            .is_some()
    });
    eprintln!("[test] PASS: ping/pong works");

    // --- Test 2: call signaling ---
    eprintln!("[test] starting call...");
    caller.dispatch(AppAction::StartCall {
        chat_id: chat_id.clone(),
    });
    wait_until("caller offering", Duration::from_secs(10), || {
        caller
            .state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Offering))
            .unwrap_or(false)
    });
    eprintln!("[test] caller is Offering");

    // Daemon should see the call invite.
    let invite = daemon.wait_for_event(
        "daemon call_invite_received",
        Duration::from_secs(30),
        |v| v.get("type").and_then(|t| t.as_str()) == Some("call_invite_received"),
    );
    let call_id = invite
        .get("call_id")
        .and_then(|x| x.as_str())
        .unwrap()
        .to_string();
    eprintln!("[test] daemon received call invite: call_id={call_id}");

    // Accept the call (simulating what channel.ts does).
    daemon.send_cmd(serde_json::json!({
        "cmd": "accept_call",
        "request_id": "accept1",
        "call_id": call_id
    }));

    // Daemon should emit call_session_started.
    daemon.wait_for_event(
        "daemon call_session_started",
        Duration::from_secs(30),
        |v| v.get("type").and_then(|t| t.as_str()) == Some("call_session_started"),
    );
    eprintln!("[test] daemon call session started");

    // Caller should reach Connecting or Active.
    wait_until(
        "caller connecting or active",
        Duration::from_secs(30),
        || {
            caller
                .state()
                .active_call
                .as_ref()
                .map(|c| matches!(c.status, CallStatus::Connecting | CallStatus::Active))
                .unwrap_or(false)
        },
    );
    eprintln!("[test] caller is Connecting/Active");

    // Wait for Active with media frames flowing.
    wait_until(
        "caller active with tx frames",
        Duration::from_secs(30),
        || {
            caller
                .state()
                .active_call
                .as_ref()
                .map(|c| {
                    matches!(c.status, CallStatus::Active)
                        && c.debug.as_ref().map(|d| d.tx_frames > 5).unwrap_or(false)
                })
                .unwrap_or(false)
        },
    );
    eprintln!("[test] caller is Active with tx frames flowing");

    let require_rx = std::env::var("MARMOT_ECHO_MODE")
        .map(|v| !v.trim().is_empty() && v.trim() != "0")
        .unwrap_or(false);
    let use_real_ai = std::env::var("OPENAI_API_KEY").is_ok();

    if require_rx {
        wait_until(
            "caller receiving echoed frames",
            Duration::from_secs(15),
            || {
                caller
                    .state()
                    .active_call
                    .as_ref()
                    .and_then(|c| c.debug.as_ref().map(|d| d.rx_frames > 0))
                    .unwrap_or(false)
            },
        );
    } else if use_real_ai {
        // Real AI: keep call active for 4s+ so STT accumulates its 3s window.
        // Synthetic audio won't produce a real transcript, but proves the pipeline works.
        eprintln!("[test] real AI mode: waiting for daemon to receive 200+ frames (4s)...");
        daemon.wait_for_event(
            "daemon accumulating audio for real STT",
            Duration::from_secs(30),
            |v| {
                v.get("type").and_then(|t| t.as_str()) == Some("call_debug")
                    && v.get("call_id")
                        .and_then(|c| c.as_str())
                        .map(|c| c == call_id)
                        .unwrap_or(false)
                    && v.get("rx_frames")
                        .and_then(|n| n.as_u64())
                        .map(|n| n >= 200)
                        .unwrap_or(false)
            },
        );
        eprintln!("[test] daemon received 200+ frames, STT pipeline running without errors");
    } else {
        // Fixture STT: just wait for daemon to receive some frames.
        daemon.wait_for_event(
            "daemon stt receiving frames",
            Duration::from_secs(20),
            |v| {
                v.get("type").and_then(|t| t.as_str()) == Some("call_debug")
                    && v.get("call_id")
                        .and_then(|c| c.as_str())
                        .map(|c| c == call_id)
                        .unwrap_or(false)
                    && v.get("rx_frames")
                        .and_then(|n| n.as_u64())
                        .map(|n| n > 0)
                        .unwrap_or(false)
            },
        );
    }

    let rx_frames = caller
        .state()
        .active_call
        .as_ref()
        .and_then(|c| c.debug.as_ref().map(|d| d.rx_frames))
        .unwrap_or(0);
    eprintln!("[test] caller rx_frames={rx_frames}");
    if require_rx {
        assert!(
            rx_frames >= 5,
            "echo mode active but rx_frames={rx_frames} (need >=5; subscriber may be stalling)"
        );
    }

    if !require_rx {
        // In fixture mode, wait for transcript. In real AI mode, synthetic audio
        // won't produce a transcript, so skip that check.
        if !use_real_ai {
            let transcript = daemon.wait_for_event(
                "daemon call_transcript_final",
                Duration::from_secs(20),
                |v| {
                    v.get("type").and_then(|t| t.as_str()) == Some("call_transcript_final")
                        && v.get("call_id")
                            .and_then(|c| c.as_str())
                            .map(|c| c == call_id)
                            .unwrap_or(false)
                },
            );
            let text = transcript
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            eprintln!("[test] transcript final: {text:?}");
            assert!(
                text.contains("hello from fixture"),
                "expected fixture transcript, got {text:?}"
            );
        }

        // Test TTS: command daemon to publish audio response back into the call.
        let tts_text = "This is a test of the text to speech system.";
        eprintln!("[test] sending TTS: {tts_text:?}");
        daemon.send_cmd(serde_json::json!({
            "cmd": "send_audio_response",
            "request_id": "tts1",
            "call_id": call_id,
            "tts_text": tts_text,
        }));
        let tts_timeout = if use_real_ai {
            Duration::from_secs(45)
        } else {
            Duration::from_secs(30)
        };
        let tts_result = daemon.wait_for_event("send_audio_response result", tts_timeout, |v| {
            v.get("request_id").and_then(|id| id.as_str()) == Some("tts1")
        });
        let tts_ok = tts_result
            .get("type")
            .and_then(|t| t.as_str())
            .map(|t| t == "ok")
            .unwrap_or(false);
        eprintln!("[test] TTS result: {tts_result}");
        assert!(tts_ok, "TTS publish failed: {tts_result}");

        // Wait for caller to receive TTS audio frames.
        wait_until(
            "caller receiving TTS frames",
            Duration::from_secs(30),
            || {
                caller
                    .state()
                    .active_call
                    .as_ref()
                    .and_then(|c| c.debug.as_ref().map(|d| d.rx_frames > 0))
                    .unwrap_or(false)
            },
        );
        let final_rx = caller
            .state()
            .active_call
            .as_ref()
            .and_then(|c| c.debug.as_ref().map(|d| d.rx_frames))
            .unwrap_or(0);
        eprintln!("[test] caller received {final_rx} TTS frames");
    }

    // End the call.
    caller.dispatch(AppAction::EndCall);
    wait_until("caller call ended", Duration::from_secs(10), || {
        caller
            .state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ended { .. }))
            .unwrap_or(true)
    });

    if let Some(debug) = caller
        .state()
        .active_call
        .as_ref()
        .and_then(|c| c.debug.as_ref())
    {
        eprintln!(
            "[test] caller final: tx={} rx={} dropped={}",
            debug.tx_frames, debug.rx_frames, debug.rx_dropped
        );
    }

    eprintln!("[test] PASS: marmotd call test on {relay_url}");

    drop(daemon);
}

// --- The actual tests ---

#[test]
#[ignore] // requires `marmotd` + `moq-relay` binaries (use `nix develop`)
fn call_with_local_marmotd() {
    if std::env::var("PIKA_E2E_LOCAL").unwrap_or_default().trim() != "1" {
        eprintln!("SKIP: set PIKA_E2E_LOCAL=1 to run local marmotd tests");
        return;
    }
    let _ = rustls::crypto::ring::default_provider().install_default();

    let moq = match support::LocalMoqRelay::spawn() {
        Some(v) => v,
        None => return,
    };

    let (relay, relay_thread) = start_local_relay();
    run_marmotd_call_test(&relay.url, &moq.url);
    drop(relay);
    relay_thread.join().unwrap();
}

#[test]
#[ignore] // nondeterministic: public relay
fn call_with_local_marmotd_primal() {
    if std::env::var("PIKA_E2E_PUBLIC").ok().as_deref() != Some("1") {
        eprintln!("SKIP: set PIKA_E2E_PUBLIC=1 to run this test (it uses a public relay)");
        return;
    }
    pika_core::init_rustls_crypto_provider();

    let moq = match support::LocalMoqRelay::spawn() {
        Some(v) => v,
        None => return,
    };

    run_marmotd_call_test("wss://relay.primal.net", &moq.url);
}
