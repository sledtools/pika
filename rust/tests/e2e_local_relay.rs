use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use nostr_sdk::filter::MatchEventOptions;
use nostr_sdk::nostr::{Event, EventId, Filter, Kind, PublicKey};
use nostr_sdk::prelude::{
    Alphabet, Client, EventBuilder, RelayPoolNotification, SingleLetterTag, Tag,
};
use pika_core::{AppAction, AppReconciler, AppUpdate, AuthState, CallStatus, FfiApp};
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

fn write_config(data_dir: &str, relay_url: &str) {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let v = serde_json::json!({
        "disable_network": false,
        "relay_urls": [relay_url],
        "key_package_relay_urls": [relay_url],
        "call_moq_url": "ws://moq.local/anon",
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
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{what}: condition not met within {timeout:?}");
}

#[derive(Clone, Copy, Debug)]
struct CallStatsSnapshot {
    tx_frames: u64,
    rx_frames: u64,
    rx_dropped: u64,
    jitter_buffer_ms: u32,
}

fn call_stats_snapshot(app: &FfiApp) -> Option<CallStatsSnapshot> {
    let call = app.state().active_call?;
    let debug = call.debug?;
    Some(CallStatsSnapshot {
        tx_frames: debug.tx_frames,
        rx_frames: debug.rx_frames,
        rx_dropped: debug.rx_dropped,
        jitter_buffer_ms: debug.jitter_buffer_ms,
    })
}

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

struct RelayState {
    events: Vec<Event>,
    event_ids: HashSet<EventId>,
    conns: HashMap<u64, ConnEntry>,
    delivered: Vec<(u64, String, EventId)>, // (conn_id, sub_id, event_id)
    sent_text: Vec<(u64, String)>,          // (conn_id, raw json)
    // Test knob: reject group message publishes (kind 445) by responding OK(false).
    reject_kind445: bool,
}

struct ConnEntry {
    tx: mpsc::UnboundedSender<Message>,
    subs: HashMap<String, Vec<Filter>>,
}

fn start_local_relay() -> (LocalRelayHandle, JoinHandle<()>) {
    let (url_tx, url_rx) = std::sync::mpsc::channel::<(String, oneshot::Sender<()>)>();
    let state = Arc::new(Mutex::new(RelayState {
        events: Vec::new(),
        event_ids: HashSet::new(),
        conns: HashMap::new(),
        delivered: Vec::new(),
        sent_text: Vec::new(),
        reject_kind445: false,
    }));

    let state_for_thread = state.clone();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind relay");
            let addr: SocketAddr = listener.local_addr().expect("local addr");
            let url = format!("ws://{}", addr);
            let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
            url_tx.send((url, shutdown_tx)).unwrap();

            let next_conn_id = Arc::new(AtomicU64::new(1));
            let state = state_for_thread;

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        // Best-effort graceful shutdown to avoid noisy client-side "connection reset" logs.
                        let conns: Vec<mpsc::UnboundedSender<Message>> = {
                            let st = state.lock().unwrap();
                            st.conns.values().map(|c| c.tx.clone()).collect()
                        };
                        for tx in conns {
                            let _ = tx.send(Message::Close(None));
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        break;
                    }
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

                            // Writer task
                            let writer_state = state.clone();
                            let writer = tokio::spawn(async move {
                                while let Some(msg) = out_rx.recv().await {
                                    let text = match &msg {
                                        Message::Text(t) => Some(t.to_string()),
                                        _ => None,
                                    };
                                    if ws_tx.send(msg).await.is_ok() {
                                        if let Some(text) = text {
                                            let mut st = writer_state.lock().unwrap();
                                            st.sent_text.push((conn_id, text));
                                            if st.sent_text.len() > 500 {
                                                st.sent_text.drain(0..200);
                                            }
                                        }
                                    } else {
                                        break;
                                    }
                                }
                            });

                            // Reader loop
                            while let Some(Ok(msg)) = ws_rx.next().await {
                                match msg {
                                    Message::Text(text) => handle_client_msg(&state, conn_id, &text),
                                    Message::Ping(p) => {
                                        let _ = out_tx.send(Message::Pong(p));
                                    }
                                    Message::Close(_) => break,
                                    _ => {}
                                }
                            }

                            {
                                let mut st = state.lock().unwrap();
                                st.conns.remove(&conn_id);
                            }

                            writer.abort();
                        });
                    }
                }
            }
        });
    });

    let (url, shutdown_tx) = url_rx.recv().unwrap();
    let handle = LocalRelayHandle {
        url,
        shutdown: Arc::new(Mutex::new(Some(shutdown_tx))),
        state,
    };
    (handle, thread)
}

fn send_json(state: &Arc<Mutex<RelayState>>, conn_id: u64, v: serde_json::Value) -> bool {
    let text = v.to_string();
    let tx = {
        let st = state.lock().unwrap();
        st.conns.get(&conn_id).map(|c| c.tx.clone())
    };
    if let Some(tx) = tx {
        return tx.send(Message::Text(text.into())).is_ok();
    }
    false
}

type SubSnapshot = Vec<(String, Vec<Filter>)>;
type ConnSnapshot = Vec<(u64, SubSnapshot)>;

fn broadcast_event(state: &Arc<Mutex<RelayState>>, ev: &Event) {
    let conns: ConnSnapshot = {
        let st = state.lock().unwrap();
        st.conns
            .iter()
            .map(|(id, c)| {
                let subs: SubSnapshot = c
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
                let v = serde_json::json!(["EVENT", sub_id, ev]);
                if send_json(state, conn_id, v) {
                    let mut st = state.lock().unwrap();
                    st.delivered.push((conn_id, sub_id.clone(), ev.id));
                }
            }
        }
    }
}

fn handle_client_msg(state: &Arc<Mutex<RelayState>>, conn_id: u64, text: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let Some(arr) = v.as_array() else {
        return;
    };
    let Some(typ) = arr.first().and_then(|x| x.as_str()) else {
        return;
    };

    match typ {
        "EVENT" => {
            let Some(ev_v) = arr.get(1) else { return };
            let Ok(ev) = serde_json::from_value::<Event>(ev_v.clone()) else {
                return;
            };

            let reject = {
                let st = state.lock().unwrap();
                st.reject_kind445 && ev.kind == Kind::MlsGroupMessage
            };
            if reject {
                let v = serde_json::json!(["OK", ev.id, false, "blocked by test relay"]);
                let _ = send_json(state, conn_id, v);
                return;
            }

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

            // Always ACK OK(true) for MVP.
            let v = serde_json::json!(["OK", ev.id, true, ""]);
            let _ = send_json(state, conn_id, v);

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

            // Send stored events matching filters, then EOSE.
            let events: Vec<Event> = {
                let st = state.lock().unwrap();
                st.events.clone()
            };
            for ev in events {
                if filters
                    .iter()
                    .any(|f| f.match_event(&ev, MatchEventOptions::new()))
                {
                    let v = serde_json::json!(["EVENT", sub_id, ev]);
                    let _ = send_json(state, conn_id, v);
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

#[test]
fn local_relay_delivers_events_to_nostr_sdk_notifications() {
    let (relay, relay_thread) = start_local_relay();

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let sub_keys = nostr_sdk::prelude::Keys::generate();
        let sub = Client::new(sub_keys.clone());
        sub.add_relay(relay.url.clone()).await.unwrap();
        sub.connect().await;
        let mut rx = sub.notifications();

        let h = "test-h-value";
        let filter = Filter::new()
            .kind(Kind::MlsGroupMessage)
            .custom_tags(SingleLetterTag::lowercase(Alphabet::H), vec![h.to_string()]);
        sub.subscribe(filter, None).await.unwrap();

        let pub_keys = nostr_sdk::prelude::Keys::generate();
        let pubc = Client::new(pub_keys.clone());
        pubc.add_relay(relay.url.clone()).await.unwrap();
        pubc.connect().await;

        let tag = Tag::parse(["h", h]).unwrap();
        let event = EventBuilder::new(Kind::MlsGroupMessage, "hello-local-relay")
            .tags([tag])
            .sign_with_keys(&pub_keys)
            .unwrap();
        pubc.send_event(&event).await.unwrap();

        let start = Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(5) {
                panic!("nostr-sdk subscriber did not receive event from local relay");
            }
            match rx.recv().await {
                Ok(RelayPoolNotification::Event { event, .. }) => {
                    if event.kind == Kind::MlsGroupMessage && event.content == "hello-local-relay" {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
    });

    drop(relay);
    relay_thread.join().unwrap();
}

#[test]
fn alice_sends_bob_receives_over_local_relay() {
    let (general_relay, general_thread) = start_local_relay();

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &general_relay.url);
    write_config(&dir_b.path().to_string_lossy(), &general_relay.url);

    let alice = FfiApp::new(dir_a.path().to_string_lossy().to_string(), String::new());
    let bob = FfiApp::new(dir_b.path().to_string_lossy().to_string(), String::new());

    #[derive(Clone)]
    struct Collector {
        updates: Arc<Mutex<Vec<AppUpdate>>>,
    }
    impl AppReconciler for Collector {
        fn reconcile(&self, update: AppUpdate) {
            self.updates.lock().unwrap().push(update);
        }
    }
    let bob_updates = Arc::new(Mutex::new(Vec::<AppUpdate>::new()));
    bob.listen_for_updates(Box::new(Collector {
        updates: bob_updates.clone(),
    }));

    alice.dispatch(AppAction::CreateAccount);
    bob.dispatch(AppAction::CreateAccount);

    wait_until("alice logged in", Duration::from_secs(5), || {
        matches!(alice.state().auth, AuthState::LoggedIn { .. })
    });
    wait_until("bob logged in", Duration::from_secs(5), || {
        matches!(bob.state().auth, AuthState::LoggedIn { .. })
    });

    let (bob_npub, bob_pubkey_hex) = match bob.state().auth {
        AuthState::LoggedIn { npub, pubkey } => (npub, pubkey),
        _ => unreachable!(),
    };

    // Ensure Bob has published a key package (kind 443) to the relay.
    let bob_pubkey = PublicKey::parse(&bob_pubkey_hex).expect("pubkey parse");
    wait_until("bob key package published", Duration::from_secs(10), || {
        let st = general_relay.state.lock().unwrap();
        st.events
            .iter()
            .any(|e| e.kind == Kind::MlsKeyPackage && e.pubkey == bob_pubkey)
    });

    // Alice creates a DM with Bob (fetch KP, create group, giftwrap welcome).
    alice.dispatch(AppAction::CreateChat {
        peer_npub: bob_npub,
    });

    wait_until("alice chat opened", Duration::from_secs(10), || {
        alice.state().current_chat.is_some()
    });
    wait_until("bob has chat", Duration::from_secs(10), || {
        !bob.state().chat_list.is_empty()
    });

    let chat_id = alice.state().current_chat.as_ref().unwrap().chat_id.clone();
    wait_until("bob chat id matches", Duration::from_secs(10), || {
        bob.state().chat_list.iter().any(|c| c.chat_id == chat_id)
    });

    // Ensure Bob's relay subscription is actively filtering for this group's `#h` tag
    // before Alice publishes any 445 messages (avoids races in local-relay tests).
    wait_until(
        "bob subscribed to kind445 #h",
        Duration::from_secs(10),
        || {
            let st = general_relay.state.lock().unwrap();

            // Find Bob's connection by looking for a filter that targets `#p = bob_pubkey_hex`.
            let bob_conn_id = st.conns.iter().find_map(|(conn_id, conn)| {
                for filters in conn.subs.values() {
                    for f in filters {
                        if let Ok(v) = serde_json::to_value(f) {
                            if v.get("#p")
                                .and_then(|x| x.as_array())
                                .map(|a| a.iter().any(|p| p.as_str() == Some(&bob_pubkey_hex)))
                                .unwrap_or(false)
                            {
                                return Some(*conn_id);
                            }
                        }
                    }
                }
                None
            });
            let Some(bob_conn_id) = bob_conn_id else {
                return false;
            };

            let Some(conn) = st.conns.get(&bob_conn_id) else {
                return false;
            };

            // Confirm the group subscription includes this chat id as a `#h` value.
            for filters in conn.subs.values() {
                for f in filters {
                    if let Ok(v) = serde_json::to_value(f) {
                        let h_ok = v
                            .get("#h")
                            .and_then(|x| x.as_array())
                            .map(|a| a.iter().any(|h| h.as_str() == Some(chat_id.as_str())))
                            .unwrap_or(false);
                        let kind_ok = v
                            .get("kinds")
                            .and_then(|x| x.as_array())
                            .map(|a| a.iter().any(|k| k.as_i64() == Some(445)))
                            .unwrap_or(false);
                        if h_ok && kind_ok {
                            return true;
                        }
                    }
                }
            }
            false
        },
    );

    let bob_conn_id = {
        let st = general_relay.state.lock().unwrap();
        st.conns
            .iter()
            .find_map(|(conn_id, conn)| {
                for filters in conn.subs.values() {
                    for f in filters {
                        if let Ok(v) = serde_json::to_value(f) {
                            if v.get("#p")
                                .and_then(|x| x.as_array())
                                .map(|a| a.iter().any(|p| p.as_str() == Some(&bob_pubkey_hex)))
                                .unwrap_or(false)
                            {
                                return Some(*conn_id);
                            }
                        }
                    }
                }
                None
            })
            .expect("bob conn id")
    };

    alice.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: "hi-from-alice".into(),
        kind: None,
        reply_to_message_id: None,
    });

    // Alice should mark the message as Sent after OK from relay.
    wait_until("alice message sent", Duration::from_secs(10), || {
        alice
            .state()
            .current_chat
            .as_ref()
            .and_then(|c| c.messages.iter().find(|m| m.content == "hi-from-alice"))
            .map(|m| matches!(m.delivery, pika_core::MessageDeliveryState::Sent))
            .unwrap_or(false)
    });

    // Relay should have received the wrapper event tagged with `h = chat_id`.
    wait_until("relay has kind445 for chat", Duration::from_secs(5), || {
        let st = general_relay.state.lock().unwrap();
        st.events.iter().any(|e| {
            if e.kind != Kind::MlsGroupMessage {
                return false;
            }
            let Ok(v) = serde_json::to_value(e) else {
                return false;
            };
            v.get("tags")
                .and_then(|t| t.as_array())
                .map(|tags| {
                    tags.iter().any(|tag| {
                        tag.as_array().and_then(|a| {
                            if a.len() >= 2 && a[0].as_str() == Some("h") {
                                a[1].as_str()
                            } else {
                                None
                            }
                        }) == Some(chat_id.as_str())
                    })
                })
                .unwrap_or(false)
        })
    });

    let wrapper_event_id = {
        let st = general_relay.state.lock().unwrap();
        st.events
            .iter()
            .find(|e| {
                if e.kind != Kind::MlsGroupMessage {
                    return false;
                }
                let Ok(v) = serde_json::to_value(e) else {
                    return false;
                };
                v.get("tags")
                    .and_then(|t| t.as_array())
                    .map(|tags| {
                        tags.iter().any(|tag| {
                            tag.as_array().and_then(|a| {
                                if a.len() >= 2 && a[0].as_str() == Some("h") {
                                    a[1].as_str()
                                } else {
                                    None
                                }
                            }) == Some(chat_id.as_str())
                        })
                    })
                    .unwrap_or(false)
            })
            .map(|e| e.id)
            .expect("wrapper event id")
    };

    // If this fails, the app is emitting invalid Nostr events (clients will drop them).
    {
        let st = general_relay.state.lock().unwrap();
        let ev = st
            .events
            .iter()
            .find(|e| e.id == wrapper_event_id)
            .expect("wrapper event present");
        ev.verify().expect("kind445 wrapper event must verify");
    }

    // Prove the relay actually delivered the 445 event to Bob's subscription.
    wait_until("relay delivered 445 to bob", Duration::from_secs(5), || {
        let st = general_relay.state.lock().unwrap();
        st.delivered
            .iter()
            .any(|(cid, _sid, eid)| *cid == bob_conn_id && *eid == wrapper_event_id)
    });

    // Bob should observe the message either in the preview or as an unread increment.
    // If this times out, dump any toast errors we observed.
    let start = Instant::now();
    loop {
        let ok = bob
            .state()
            .chat_list
            .iter()
            .find(|c| c.chat_id == chat_id)
            .map(|c| c.unread_count > 0 || c.last_message.is_some())
            .unwrap_or(false);
        if ok {
            break;
        }
        if start.elapsed() > Duration::from_secs(10) {
            let toasts: Vec<String> = bob_updates
                .lock()
                .unwrap()
                .iter()
                .filter_map(|u| match u {
                    AppUpdate::FullState(s) => s.toast.clone(),
                    _ => None,
                })
                .collect();
            let sent_to_bob: Vec<String> = {
                let st = general_relay.state.lock().unwrap();
                st.sent_text
                    .iter()
                    .filter(|(cid, _)| *cid == bob_conn_id)
                    .rev()
                    .take(10)
                    .map(|(_, t)| t.clone())
                    .collect()
            };
            panic!(
                "bob received message: timeout; toasts={toasts:?}; bob_chat_list={:?}; relay_sent_to_bob(last10)={sent_to_bob:?}",
                bob.state().chat_list
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Open the chat and validate message attributes.
    bob.dispatch(AppAction::OpenChat { chat_id });
    wait_until(
        "bob opened chat has message",
        Duration::from_secs(10),
        || {
            bob.state()
                .current_chat
                .as_ref()
                .and_then(|c| c.messages.iter().find(|m| m.content == "hi-from-alice"))
                .is_some()
        },
    );
    let msg = bob
        .state()
        .current_chat
        .unwrap()
        .messages
        .into_iter()
        .find(|m| m.content == "hi-from-alice")
        .unwrap();
    assert!(!msg.is_mine);

    // Preview should match plaintext.
    wait_until("bob preview updated", Duration::from_secs(5), || {
        bob.state()
            .chat_list
            .iter()
            .find(|c| c.chat_id == bob.state().current_chat.as_ref().unwrap().chat_id)
            .and_then(|c| c.last_message.clone())
            .as_deref()
            == Some("hi-from-alice")
    });
    let preview = bob
        .state()
        .chat_list
        .iter()
        .find(|c| c.chat_id == bob.state().current_chat.as_ref().unwrap().chat_id)
        .and_then(|c| c.last_message.clone());
    assert_eq!(preview.as_deref(), Some("hi-from-alice"));

    drop(general_relay);
    general_thread.join().unwrap();
}

#[test]
fn send_failure_then_retry_succeeds_over_local_relay() {
    let (relay, relay_thread) = start_local_relay();

    let dir = tempdir().unwrap();
    write_config(&dir.path().to_string_lossy(), &relay.url);

    let app = FfiApp::new(dir.path().to_string_lossy().to_string(), String::new());
    app.dispatch(AppAction::CreateAccount);
    wait_until("logged in", Duration::from_secs(10), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    let my_npub = match app.state().auth {
        AuthState::LoggedIn { npub, .. } => npub,
        _ => unreachable!(),
    };

    // Note-to-self group (no peer key package fetch).
    app.dispatch(AppAction::CreateChat { peer_npub: my_npub });
    wait_until("chat opened", Duration::from_secs(10), || {
        app.state().current_chat.is_some()
    });

    let chat_id = app.state().current_chat.as_ref().unwrap().chat_id.clone();

    // Publishing is now fire-and-forget (optimistic): the app reports
    // Sent immediately regardless of relay acceptance.  Verify that even
    // when the relay rejects kind-445 events, the UI still shows Sent.
    {
        let mut st = relay.state.lock().unwrap();
        st.reject_kind445 = true;
    }

    let content = "retry-me";
    app.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: content.into(),
        kind: None,
        reply_to_message_id: None,
    });

    wait_until(
        "message optimistically sent",
        Duration::from_secs(10),
        || {
            app.state()
                .current_chat
                .as_ref()
                .and_then(|c| c.messages.iter().find(|m| m.content == content))
                .map(|m| matches!(m.delivery, pika_core::MessageDeliveryState::Sent))
                .unwrap_or(false)
        },
    );

    drop(relay);
    relay_thread.join().unwrap();
}

#[test]
#[ignore = "disabled: flaky call test"]
fn call_invite_accept_end_flow_over_local_relay() {
    let (relay, relay_thread) = start_local_relay();

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &relay.url);
    write_config(&dir_b.path().to_string_lossy(), &relay.url);

    let alice = FfiApp::new(dir_a.path().to_string_lossy().to_string(), String::new());
    let bob = FfiApp::new(dir_b.path().to_string_lossy().to_string(), String::new());

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

    alice.dispatch(AppAction::CreateChat {
        peer_npub: bob_npub,
    });
    wait_until("alice chat opened", Duration::from_secs(60), || {
        alice.state().current_chat.is_some()
    });
    wait_until("bob has chat", Duration::from_secs(60), || {
        !bob.state().chat_list.is_empty()
    });

    let chat_id = alice.state().current_chat.as_ref().unwrap().chat_id.clone();
    wait_until("bob chat id matches", Duration::from_secs(60), || {
        bob.state().chat_list.iter().any(|c| c.chat_id == chat_id)
    });

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
    wait_until("bob ringing", Duration::from_secs(10), || {
        bob.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ringing))
            .unwrap_or(false)
    });

    let call_id = alice
        .state()
        .active_call
        .as_ref()
        .map(|c| c.call_id.clone())
        .expect("alice call id");
    wait_until("bob has same call id", Duration::from_secs(10), || {
        bob.state()
            .active_call
            .as_ref()
            .map(|c| c.call_id == call_id)
            .unwrap_or(false)
    });

    bob.dispatch(AppAction::AcceptCall {
        chat_id: chat_id.clone(),
    });
    wait_until("bob connecting or active", Duration::from_secs(10), || {
        bob.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Connecting | CallStatus::Active))
            .unwrap_or(false)
    });
    wait_until(
        "alice connecting or active",
        Duration::from_secs(10),
        || {
            alice
                .state()
                .active_call
                .as_ref()
                .map(|c| matches!(c.status, CallStatus::Connecting | CallStatus::Active))
                .unwrap_or(false)
        },
    );

    wait_until(
        "bob active with media stats",
        Duration::from_secs(20),
        || {
            bob.state()
                .active_call
                .as_ref()
                .map(|c| {
                    matches!(c.status, CallStatus::Active)
                        && c.debug
                            .as_ref()
                            .map(|d| d.tx_frames > 0 && d.rx_frames > 0)
                            .unwrap_or(false)
                })
                .unwrap_or(false)
        },
    );
    wait_until(
        "alice active with media stats",
        Duration::from_secs(20),
        || {
            alice
                .state()
                .active_call
                .as_ref()
                .map(|c| {
                    matches!(c.status, CallStatus::Active)
                        && c.debug
                            .as_ref()
                            .map(|d| d.tx_frames > 0 && d.rx_frames > 0)
                            .unwrap_or(false)
                })
                .unwrap_or(false)
        },
    );

    let alice_overlap_start = call_stats_snapshot(&alice).expect("alice call stats");
    let bob_overlap_start = call_stats_snapshot(&bob).expect("bob call stats");
    wait_until(
        "full-duplex overlap stats progress on both sides",
        Duration::from_secs(10),
        || {
            let Some(alice_now) = call_stats_snapshot(&alice) else {
                return false;
            };
            let Some(bob_now) = call_stats_snapshot(&bob) else {
                return false;
            };
            alice_now.tx_frames > alice_overlap_start.tx_frames
                && alice_now.rx_frames > alice_overlap_start.rx_frames
                && bob_now.tx_frames > bob_overlap_start.tx_frames
                && bob_now.rx_frames > bob_overlap_start.rx_frames
        },
    );
    let alice_overlap_end = call_stats_snapshot(&alice).expect("alice overlap stats");
    let bob_overlap_end = call_stats_snapshot(&bob).expect("bob overlap stats");
    assert!(
        alice_overlap_end.jitter_buffer_ms <= 240,
        "alice jitter buffer should stay bounded (<= 240ms), got {}ms",
        alice_overlap_end.jitter_buffer_ms
    );
    assert!(
        bob_overlap_end.jitter_buffer_ms <= 240,
        "bob jitter buffer should stay bounded (<= 240ms), got {}ms",
        bob_overlap_end.jitter_buffer_ms
    );
    let alice_drop_delta = alice_overlap_end
        .rx_dropped
        .saturating_sub(alice_overlap_start.rx_dropped);
    let bob_drop_delta = bob_overlap_end
        .rx_dropped
        .saturating_sub(bob_overlap_start.rx_dropped);
    assert!(
        alice_drop_delta <= 8,
        "alice dropped too many frames during overlap window: {alice_drop_delta}"
    );
    assert!(
        bob_drop_delta <= 8,
        "bob dropped too many frames during overlap window: {bob_drop_delta}"
    );

    // While Alice is in-call, a second invite should not replace the active call.
    let second_invite_call_id = "550e8400-e29b-41d4-a716-446655440999";
    let second_invite = serde_json::json!({
        "v": 1,
        "ns": "pika.call",
        "type": "call.invite",
        "call_id": second_invite_call_id,
        "ts_ms": 1730000000000i64,
        "body": {
            "moq_url": "https://moq.local/anon",
            "broadcast_base": format!("pika/calls/{second_invite_call_id}"),
            "relay_auth": "capv1_second_invite_probe",
            "tracks": [{
                "name": "audio0",
                "codec": "opus",
                "sample_rate": 48000,
                "channels": 1,
                "frame_ms": 20
            }]
        }
    })
    .to_string();
    bob.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: second_invite,
        kind: None,
        reply_to_message_id: None,
    });
    wait_until(
        "alice keeps original active call id",
        Duration::from_secs(10),
        || {
            alice
                .state()
                .active_call
                .as_ref()
                .map(|c| c.call_id == call_id)
                .unwrap_or(false)
        },
    );

    // Mute should pause local TX frame generation.
    alice.dispatch(AppAction::ToggleMute);
    wait_until("alice muted", Duration::from_secs(10), || {
        alice
            .state()
            .active_call
            .as_ref()
            .map(|c| c.is_muted)
            .unwrap_or(false)
    });
    std::thread::sleep(Duration::from_millis(250));
    let alice_tx_muted_baseline = alice
        .state()
        .active_call
        .as_ref()
        .and_then(|c| c.debug.as_ref().map(|d| d.tx_frames))
        .unwrap_or(0);
    std::thread::sleep(Duration::from_millis(250));
    let alice_tx_while_muted = alice
        .state()
        .active_call
        .as_ref()
        .and_then(|c| c.debug.as_ref().map(|d| d.tx_frames))
        .unwrap_or(0);
    assert_eq!(
        alice_tx_while_muted, alice_tx_muted_baseline,
        "mute should pause tx frame counter"
    );

    // Unmute should resume TX frame generation.
    alice.dispatch(AppAction::ToggleMute);
    wait_until("alice unmuted", Duration::from_secs(10), || {
        alice
            .state()
            .active_call
            .as_ref()
            .map(|c| !c.is_muted)
            .unwrap_or(false)
    });
    wait_until("alice tx resumes", Duration::from_secs(10), || {
        alice
            .state()
            .active_call
            .as_ref()
            .and_then(|c| c.debug.as_ref().map(|d| d.tx_frames))
            .map(|tx| tx > alice_tx_while_muted)
            .unwrap_or(false)
    });

    alice.dispatch(AppAction::EndCall);
    wait_until("alice ended", Duration::from_secs(10), || {
        alice
            .state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ended { .. }))
            .unwrap_or(false)
    });
    wait_until("bob ended", Duration::from_secs(10), || {
        bob.state()
            .active_call
            .as_ref()
            .map(|c| {
                matches!(
                    c.status,
                    CallStatus::Ended { ref reason } if reason == "user_hangup"
                )
            })
            .unwrap_or(false)
    });

    // Call signals should not appear in chat previews.
    assert_eq!(
        alice
            .state()
            .chat_list
            .iter()
            .find(|c| c.chat_id == chat_id)
            .and_then(|c| c.last_message.clone()),
        None
    );
    assert_eq!(
        bob.state()
            .chat_list
            .iter()
            .find(|c| c.chat_id == chat_id)
            .and_then(|c| c.last_message.clone()),
        None
    );

    drop(relay);
    relay_thread.join().unwrap();
}

#[test]
fn call_invite_with_invalid_relay_auth_is_rejected() {
    let (relay, relay_thread) = start_local_relay();

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &relay.url);
    write_config(&dir_b.path().to_string_lossy(), &relay.url);

    let alice = FfiApp::new(dir_a.path().to_string_lossy().to_string(), String::new());
    let bob = FfiApp::new(dir_b.path().to_string_lossy().to_string(), String::new());

    alice.dispatch(AppAction::CreateAccount);
    bob.dispatch(AppAction::CreateAccount);

    wait_until("alice logged in", Duration::from_secs(10), || {
        matches!(alice.state().auth, AuthState::LoggedIn { .. })
    });
    wait_until("bob logged in", Duration::from_secs(10), || {
        matches!(bob.state().auth, AuthState::LoggedIn { .. })
    });

    let bob_npub = match bob.state().auth {
        AuthState::LoggedIn { npub: bob_npub, .. } => bob_npub,
        _ => unreachable!(),
    };
    let bob_pubkey = PublicKey::parse(&bob_npub).expect("bob pubkey");

    // Avoid race: ensure Bob's key package has been published before Alice tries to fetch it.
    wait_until("bob key package published", Duration::from_secs(10), || {
        let st = relay.state.lock().unwrap();
        st.events
            .iter()
            .any(|e| e.kind == Kind::MlsKeyPackage && e.pubkey == bob_pubkey)
    });

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
    bob.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until("bob opened chat", Duration::from_secs(10), || {
        bob.state().current_chat.is_some()
    });

    let bad_call_id = "550e8400-e29b-41d4-a716-446655441111";
    let bad_invite = serde_json::json!({
        "v": 1,
        "ns": "pika.call",
        "type": "call.invite",
        "call_id": bad_call_id,
        "ts_ms": 1730000000000i64,
        "body": {
            "moq_url": "https://moq.local/anon",
            "broadcast_base": format!("pika/calls/{bad_call_id}"),
            "relay_auth": "capv1_invalid_auth",
            "tracks": [{
                "name": "audio0",
                "codec": "opus",
                "sample_rate": 48000,
                "channels": 1,
                "frame_ms": 20
            }]
        }
    })
    .to_string();
    bob.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: bad_invite,
        kind: Some(10),
        reply_to_message_id: None,
    });

    wait_until(
        "alice rejects invalid relay auth invite",
        Duration::from_secs(10),
        || {
            let st = alice.state();
            st.active_call.is_none()
                && st
                    .toast
                    .as_deref()
                    .map(|t| t.contains("Rejected call invite"))
                    .unwrap_or(false)
        },
    );
    assert!(
        alice.state().active_call.is_none(),
        "invalid relay auth invite must not create ringing state",
    );

    drop(relay);
    relay_thread.join().unwrap();
}

#[test]
fn duplicate_group_message_does_not_duplicate_in_ui() {
    let (general_relay, general_thread) = start_local_relay();

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &general_relay.url);
    write_config(&dir_b.path().to_string_lossy(), &general_relay.url);

    let alice = FfiApp::new(dir_a.path().to_string_lossy().to_string(), String::new());
    let bob = FfiApp::new(dir_b.path().to_string_lossy().to_string(), String::new());

    alice.dispatch(AppAction::CreateAccount);
    bob.dispatch(AppAction::CreateAccount);

    wait_until("alice logged in", Duration::from_secs(10), || {
        matches!(alice.state().auth, AuthState::LoggedIn { .. })
    });
    wait_until("bob logged in", Duration::from_secs(10), || {
        matches!(bob.state().auth, AuthState::LoggedIn { .. })
    });

    let (bob_npub, bob_pubkey_hex) = match bob.state().auth {
        AuthState::LoggedIn { npub, pubkey } => (npub, pubkey),
        _ => unreachable!(),
    };
    let bob_pubkey = PublicKey::parse(&bob_pubkey_hex).expect("pubkey parse");

    // Wait for Bob's key package to land on the relay before Alice tries to fetch it.
    wait_until("bob key package published", Duration::from_secs(10), || {
        let st = general_relay.state.lock().unwrap();
        st.events
            .iter()
            .any(|e| e.kind == Kind::MlsKeyPackage && e.pubkey == bob_pubkey)
    });

    // Create DM (key package fetch + giftwrap welcome).
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
    bob.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until("bob opened chat", Duration::from_secs(10), || {
        bob.state().current_chat.is_some()
    });

    // Send a message from Alice.
    let pre_445 = {
        let st = general_relay.state.lock().unwrap();
        st.events
            .iter()
            .filter(|e| e.kind == Kind::MlsGroupMessage)
            .count()
    };
    let nonce = format!(
        "dup-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let content = format!("hello-{nonce}");
    alice.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: content.clone(),
        kind: None,
        reply_to_message_id: None,
    });

    wait_until("bob received message", Duration::from_secs(20), || {
        bob.state()
            .current_chat
            .as_ref()
            .map(|c| c.messages.iter().filter(|m| m.content == content).count() == 1)
            .unwrap_or(false)
    });

    // Re-broadcast the same kind 445 event to simulate duplicate delivery.
    let ev = {
        let st = general_relay.state.lock().unwrap();
        st.events
            .iter()
            .filter(|e| e.kind == Kind::MlsGroupMessage)
            .skip(pre_445)
            .last()
            .cloned()
            .expect("missing kind 445 event on relay")
    };
    broadcast_event(&general_relay.state, &ev);

    // Bob should still render a single copy of the plaintext message.
    wait_until("no duplicate in UI", Duration::from_secs(10), || {
        bob.state()
            .current_chat
            .as_ref()
            .map(|c| c.messages.iter().filter(|m| m.content == content).count() == 1)
            .unwrap_or(false)
    });

    drop(general_relay);
    general_thread.join().unwrap();
}
