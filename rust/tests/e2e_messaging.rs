//! E2E messaging tests: account creation, key packages, group chat, message delivery, dedup.
//!
//! Uses pikahub for local infrastructure (default). All tests run in pre-merge.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pika_core::{AppAction, AppReconciler, AppUpdate, AuthState, FfiApp};
use tempfile::tempdir;

#[path = "support/mod.rs"]
mod support;

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

#[test]
fn alice_sends_bob_receives() {
    let infra = support::TestInfra::start_relay();

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &infra.relay_url);
    write_config(&dir_b.path().to_string_lossy(), &infra.relay_url);

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

    alice.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: "hi-from-alice".into(),
        kind: None,
        reply_to_message_id: None,
    });

    wait_until("alice message sent", Duration::from_secs(10), || {
        alice
            .state()
            .current_chat
            .as_ref()
            .and_then(|c| c.messages.iter().find(|m| m.content == "hi-from-alice"))
            .map(|m| matches!(m.delivery, pika_core::MessageDeliveryState::Sent))
            .unwrap_or(false)
    });

    wait_until(
        "bob preview/unread updated",
        Duration::from_secs(20),
        || {
            bob.state()
                .chat_list
                .iter()
                .find(|c| c.chat_id == chat_id)
                .map(|c| c.unread_count > 0 || c.last_message.is_some())
                .unwrap_or(false)
        },
    );

    bob.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until(
        "bob opened chat has message",
        Duration::from_secs(20),
        || {
            bob.state()
                .current_chat
                .as_ref()
                .and_then(|c| c.messages.iter().find(|m| m.content == "hi-from-alice"))
                .is_some()
        },
    );
    let bob_state = bob.state();
    let msg = bob_state
        .current_chat
        .as_ref()
        .unwrap()
        .messages
        .iter()
        .find(|m| m.content == "hi-from-alice")
        .unwrap();
    assert!(!msg.is_mine);

    wait_until("bob preview updated", Duration::from_secs(10), || {
        bob.state()
            .chat_list
            .iter()
            .find(|c| c.chat_id == bob.state().current_chat.as_ref().unwrap().chat_id)
            .and_then(|c| c.last_message.clone())
            .as_deref()
            == Some("hi-from-alice")
    });
}

#[test]
fn call_invite_with_invalid_relay_auth_is_rejected() {
    let infra = support::TestInfra::start_relay();

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &infra.relay_url);
    write_config(&dir_b.path().to_string_lossy(), &infra.relay_url);

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
}

#[test]
fn optimistic_send_shows_sent_even_on_rejection() {
    // Tests that SendMessage immediately shows Sent status (optimistic delivery).
    // This is app-layer behavior that doesn't depend on relay acceptance.
    let infra = support::TestInfra::start_relay();

    let dir = tempdir().unwrap();
    write_config(&dir.path().to_string_lossy(), &infra.relay_url);

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
    let content = "optimistic-test";
    app.dispatch(AppAction::SendMessage {
        chat_id,
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
}
