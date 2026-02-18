//! End-to-end call test using a locally spawned moq-relay.
//!
//! Uses a local Nostr relay for MLS signaling but routes all media frames
//! through a local MOQ relay over QUIC. This validates the full call stack:
//! call_runtime.rs → NetworkRelay → moq-native → QUIC → local moq-relay
//!
//! Requires:
//! - `moq-relay` on PATH (available in `nix develop`)

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
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

// --- Minimal local Nostr relay (copy from e2e_local_relay.rs) ---

#[derive(Clone)]
struct LocalRelayHandle {
    url: String,
    shutdown: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    state: Arc<Mutex<RelayState>>,
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
            state,
        },
        thread,
    )
}

// --- The actual test ---

#[test]
#[ignore] // requires QUIC, plus `moq-relay` installed (use `nix develop`)
fn call_over_real_moq_relay() {
    let moq = match support::LocalMoqRelay::spawn() {
        Some(v) => v,
        None => return,
    };

    // Both ring and aws-lc-rs are in the dep tree. Must pick one explicitly.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Signaling goes through a local Nostr relay.
    // Media goes through a local moq-relay.
    let (relay, relay_thread) = start_local_relay();

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(
        &dir_a.path().to_string_lossy(),
        &relay.url,
        Some(&relay.url),
        &moq.url,
    );
    write_config(
        &dir_b.path().to_string_lossy(),
        &relay.url,
        Some(&relay.url),
        &moq.url,
    );

    let alice = FfiApp::new(dir_a.path().to_string_lossy().to_string(), String::new());
    let bob = FfiApp::new(dir_b.path().to_string_lossy().to_string(), String::new());

    // Create accounts
    alice.dispatch(AppAction::CreateAccount);
    bob.dispatch(AppAction::CreateAccount);
    wait_until("alice logged in", Duration::from_secs(10), || {
        matches!(alice.state().auth, AuthState::LoggedIn { .. })
    });
    wait_until("bob logged in", Duration::from_secs(10), || {
        matches!(bob.state().auth, AuthState::LoggedIn { .. })
    });

    let bob_npub = match bob.state().auth {
        AuthState::LoggedIn { npub, .. } => npub,
        _ => unreachable!(),
    };

    // This test is run via `just nightly` in CI (runs ignored tests). The app publishes key
    // packages asynchronously after login; without waiting here, `CreateChat` can race and fail
    // with `kp_found=false` (flaky).
    wait_until(
        "both key packages visible in relay",
        Duration::from_secs(10),
        || {
            let st = relay.state.lock().unwrap();
            let pubs: HashSet<String> = st
                .events
                .iter()
                .filter(|e| e.kind == nostr_sdk::Kind::MlsKeyPackage)
                .map(|e| e.pubkey.to_hex())
                .collect();
            pubs.len() >= 2
        },
    );

    // Establish MLS group (required for call crypto key derivation)
    alice.dispatch(AppAction::CreateChat {
        peer_npub: bob_npub,
    });
    wait_until("alice chat opened", Duration::from_secs(20), || {
        alice.state().current_chat.is_some()
    });
    wait_until("bob has chat", Duration::from_secs(20), || {
        !bob.state().chat_list.is_empty()
    });

    let chat_id = alice.state().current_chat.as_ref().unwrap().chat_id.clone();
    wait_until("bob chat id matches", Duration::from_secs(20), || {
        bob.state().chat_list.iter().any(|c| c.chat_id == chat_id)
    });

    // Alice starts a call
    alice.dispatch(AppAction::StartCall {
        chat_id: chat_id.clone(),
    });
    wait_until("alice offering", Duration::from_secs(10), || {
        alice
            .state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Offering))
            .unwrap_or(false)
    });

    // Bob sees the ringing
    wait_until("bob ringing", Duration::from_secs(10), || {
        bob.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ringing))
            .unwrap_or(false)
    });

    // Bob accepts the call -- this triggers NetworkRelay.connect() to the local moq-relay.
    bob.dispatch(AppAction::AcceptCall {
        chat_id: chat_id.clone(),
    });

    // Both sides should reach Connecting or Active.
    // NetworkRelay.connect() does a real QUIC handshake, so give more time.
    wait_until("bob connecting or active", Duration::from_secs(30), || {
        bob.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Connecting | CallStatus::Active))
            .unwrap_or(false)
    });
    wait_until(
        "alice connecting or active",
        Duration::from_secs(30),
        || {
            alice
                .state()
                .active_call
                .as_ref()
                .map(|c| matches!(c.status, CallStatus::Connecting | CallStatus::Active))
                .unwrap_or(false)
        },
    );

    // Wait for actual media frames to flow through the local moq-relay.
    // Both sides should be transmitting and receiving encrypted Opus frames.
    wait_until(
        "alice active with tx+rx frames through real relay",
        Duration::from_secs(30),
        || {
            alice
                .state()
                .active_call
                .as_ref()
                .map(|c| {
                    matches!(c.status, CallStatus::Active)
                        && c.debug
                            .as_ref()
                            .map(|d| d.tx_frames > 5 && d.rx_frames > 5)
                            .unwrap_or(false)
                })
                .unwrap_or(false)
        },
    );
    wait_until(
        "bob active with tx+rx frames through real relay",
        Duration::from_secs(30),
        || {
            bob.state()
                .active_call
                .as_ref()
                .map(|c| {
                    matches!(c.status, CallStatus::Active)
                        && c.debug
                            .as_ref()
                            .map(|d| d.tx_frames > 5 && d.rx_frames > 5)
                            .unwrap_or(false)
                })
                .unwrap_or(false)
        },
    );

    // Snapshot stats and verify frames continue flowing
    let alice_tx_before = alice
        .state()
        .active_call
        .as_ref()
        .and_then(|c| c.debug.as_ref().map(|d| d.tx_frames))
        .unwrap_or(0);
    let bob_rx_before = bob
        .state()
        .active_call
        .as_ref()
        .and_then(|c| c.debug.as_ref().map(|d| d.rx_frames))
        .unwrap_or(0);

    std::thread::sleep(Duration::from_secs(2));

    let alice_tx_after = alice
        .state()
        .active_call
        .as_ref()
        .and_then(|c| c.debug.as_ref().map(|d| d.tx_frames))
        .unwrap_or(0);
    let bob_rx_after = bob
        .state()
        .active_call
        .as_ref()
        .and_then(|c| c.debug.as_ref().map(|d| d.rx_frames))
        .unwrap_or(0);

    assert!(
        alice_tx_after > alice_tx_before,
        "alice should keep transmitting: before={alice_tx_before} after={alice_tx_after}"
    );
    assert!(
        bob_rx_after > bob_rx_before,
        "bob should keep receiving: before={bob_rx_before} after={bob_rx_after}"
    );

    // End the call
    alice.dispatch(AppAction::EndCall);
    wait_until("alice call ended", Duration::from_secs(10), || {
        alice
            .state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ended { .. }))
            .unwrap_or(true)
    });
    wait_until("bob call ended", Duration::from_secs(10), || {
        bob.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ended { .. }))
            .unwrap_or(true)
    });

    // Print final stats
    if let Some(debug) = alice
        .state()
        .active_call
        .as_ref()
        .and_then(|c| c.debug.as_ref())
    {
        eprintln!(
            "alice final: tx={} rx={} dropped={}",
            debug.tx_frames, debug.rx_frames, debug.rx_dropped
        );
    }
    if let Some(debug) = bob
        .state()
        .active_call
        .as_ref()
        .and_then(|c| c.debug.as_ref())
    {
        eprintln!(
            "bob final: tx={} rx={} dropped={}",
            debug.tx_frames, debug.rx_frames, debug.rx_dropped
        );
    }

    drop(relay);
    relay_thread.join().unwrap();
}
