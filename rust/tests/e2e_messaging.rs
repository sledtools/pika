//! Focused relay-backed multi-app FFI messaging behavior.
//!
//! `pikahut` provides the local fixture, but this file remains the semantic owner for the
//! message/call state transitions it asserts through `FfiApp`. `pikahut` selectors only pin a
//! few of these tests as higher-level regression boundaries.

use std::path::Path;
use std::time::{Duration, Instant};

use pika_core::{AppAction, AuthState, CallStatus, FfiApp};
use rusqlite::Connection;
use tempfile::tempdir;

mod support;
use support::{create_or_open_dm_chat, wait_until, write_config, write_config_with_chat_server};

fn open_chat(app: &FfiApp, chat_id: &str, timeout: Duration) {
    app.dispatch(AppAction::OpenChat {
        chat_id: chat_id.to_owned(),
    });
    wait_until("chat opened", timeout, || {
        app.state()
            .current_chat
            .as_ref()
            .map(|chat| chat.chat_id == chat_id)
            .unwrap_or(false)
    });
}

fn send_message_and_wait_sent(app: &FfiApp, chat_id: &str, content: &str, timeout: Duration) {
    app.dispatch(AppAction::SendMessage {
        chat_id: chat_id.to_owned(),
        content: content.into(),
        kind: None,
        reply_to_message_id: None,
    });

    wait_until("message sent", timeout, || {
        app.state()
            .current_chat
            .as_ref()
            .and_then(|chat| {
                chat.messages
                    .iter()
                    .find(|message| message.content == content)
            })
            .map(|message| matches!(message.delivery, pika_core::MessageDeliveryState::Sent))
            .unwrap_or(false)
    });
}

fn wait_for_current_chat_message(app: &FfiApp, content: &str, timeout: Duration) {
    wait_until("current chat has message", timeout, || {
        app.state()
            .current_chat
            .as_ref()
            .and_then(|chat| {
                chat.messages
                    .iter()
                    .find(|message| message.content == content)
            })
            .is_some()
    });
}

fn wait_for_account_created_nsec(collector: &support::Collector) -> String {
    wait_until("AccountCreated update", Duration::from_secs(10), || {
        collector
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|update| matches!(update, pika_core::AppUpdate::AccountCreated { .. }))
    });

    collector
        .0
        .lock()
        .unwrap()
        .iter()
        .find_map(|update| match update {
            pika_core::AppUpdate::AccountCreated { nsec, .. } => Some(nsec.clone()),
            _ => None,
        })
        .expect("missing AccountCreated update with nsec")
}

fn create_chat_server_dm_and_open(app: &FfiApp, peer_npub: &str, timeout: Duration) -> String {
    app.dispatch(AppAction::CreateChat {
        peer_npub: peer_npub.to_owned(),
    });

    let create_deadline = Instant::now() + timeout;
    let chat_id = loop {
        if let Some(chat_id) = support::dm_chat_id_for_peer(app, peer_npub) {
            break chat_id;
        }
        let state = app.state();
        if let Some(toast) = state.toast {
            let chats: Vec<String> = state
                .chat_list
                .iter()
                .map(|chat| {
                    format!(
                        "{}:{}:{}",
                        chat.chat_id,
                        chat.group_name.clone().unwrap_or_default(),
                        chat.members.len()
                    )
                })
                .collect();
            panic!("chat-server create chat failed: {toast}; chats={chats:?}");
        }
        if Instant::now() >= create_deadline {
            let chats: Vec<String> = state
                .chat_list
                .iter()
                .map(|chat| {
                    format!(
                        "{}:{}:{}",
                        chat.chat_id,
                        chat.group_name.clone().unwrap_or_default(),
                        chat.members.len()
                    )
                })
                .collect();
            panic!("chat-server direct chat did not appear; chats={chats:?}");
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    open_chat(app, &chat_id, Duration::from_secs(20));
    chat_id
}

fn load_chat_server_binding(data_dir: &Path, chat_id: &str) -> (String, String, u64) {
    let db_path = data_dir.join("profiles.sqlite3");
    let conn = Connection::open(db_path).expect("open profile db");
    conn.query_row(
        "SELECT server_url, room_id, last_synced_seq FROM chat_server_rooms WHERE chat_id = ?1",
        [chat_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .expect("load chat server room binding")
}

#[test]
fn relay_backed_dm_delivery_reaches_peer_chat_state() {
    // `pikahut` now owns the fuller end-user contract for "create DM, send first message, peer
    // sees chat shell + preview/unread state". This test stays as the narrower semantic owner for
    // the relay-backed `FfiApp` state transition that the peer can open the DM and observe the
    // delivered message.
    let infra = support::TestInfra::start_relay();

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &infra.relay_url);
    write_config(&dir_b.path().to_string_lossy(), &infra.relay_url);

    let alice = FfiApp::new(
        dir_a.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );
    let bob = FfiApp::new(
        dir_b.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );

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

    let chat_id = create_or_open_dm_chat(&alice, &bob_npub, Duration::from_secs(60));
    wait_until("bob chat id matches", Duration::from_secs(20), || {
        bob.state().chat_list.iter().any(|c| c.chat_id == chat_id)
    });

    send_message_and_wait_sent(&alice, &chat_id, "hi-from-alice", Duration::from_secs(10));

    open_chat(&bob, &chat_id, Duration::from_secs(20));
    wait_for_current_chat_message(&bob, "hi-from-alice", Duration::from_secs(20));
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
}

#[test]
fn chat_server_dm_delivery_reaches_peer_chat_state() {
    let infra = support::TestInfra::start_relay_and_chat_server();
    let chat_server_url = infra.chat_server_url.as_ref().expect("chat_server_url");

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config_with_chat_server(
        &dir_a.path().to_string_lossy(),
        &infra.relay_url,
        chat_server_url,
    );
    write_config_with_chat_server(
        &dir_b.path().to_string_lossy(),
        &infra.relay_url,
        chat_server_url,
    );

    let alice = FfiApp::new(
        dir_a.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );
    let bob = FfiApp::new(
        dir_b.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );

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

    let chat_id = create_chat_server_dm_and_open(&alice, &bob_npub, Duration::from_secs(30));
    wait_until("bob chat id matches", Duration::from_secs(45), || {
        bob.state().chat_list.iter().any(|c| c.chat_id == chat_id)
    });

    send_message_and_wait_sent(
        &alice,
        &chat_id,
        "hi-from-alice-chat-server",
        Duration::from_secs(20),
    );

    open_chat(&bob, &chat_id, Duration::from_secs(30));
    wait_for_current_chat_message(&bob, "hi-from-alice-chat-server", Duration::from_secs(30));
    let bob_state = bob.state();
    let msg = bob_state
        .current_chat
        .as_ref()
        .unwrap()
        .messages
        .iter()
        .find(|m| m.content == "hi-from-alice-chat-server")
        .unwrap();
    assert!(!msg.is_mine);
}

#[test]
fn chat_server_dm_resume_after_restart_keeps_room_binding_and_syncs_new_messages() {
    let infra = support::TestInfra::start_relay_and_chat_server();
    let chat_server_url = infra.chat_server_url.as_ref().expect("chat_server_url");

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config_with_chat_server(
        &dir_a.path().to_string_lossy(),
        &infra.relay_url,
        chat_server_url,
    );
    write_config_with_chat_server(
        &dir_b.path().to_string_lossy(),
        &infra.relay_url,
        chat_server_url,
    );

    let alice = FfiApp::new(
        dir_a.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );
    let bob = FfiApp::new(
        dir_b.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );
    let bob_updates = support::Collector::new();
    bob.listen_for_updates(Box::new(bob_updates.clone()));

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
    let bob_nsec = wait_for_account_created_nsec(&bob_updates);

    let chat_id = create_chat_server_dm_and_open(&alice, &bob_npub, Duration::from_secs(30));
    wait_until("bob chat id matches", Duration::from_secs(45), || {
        bob.state()
            .chat_list
            .iter()
            .any(|chat| chat.chat_id == chat_id)
    });

    send_message_and_wait_sent(
        &alice,
        &chat_id,
        "before-restart-chat-server",
        Duration::from_secs(20),
    );
    open_chat(&bob, &chat_id, Duration::from_secs(30));
    wait_for_current_chat_message(&bob, "before-restart-chat-server", Duration::from_secs(30));

    let binding_before_restart = load_chat_server_binding(dir_b.path(), &chat_id);
    assert_eq!(
        binding_before_restart.0,
        chat_server_url.trim_end_matches('/')
    );
    assert!(binding_before_restart.2 > 0);

    drop(bob);

    let bob_after_restart = FfiApp::new(
        dir_b.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );
    bob_after_restart.dispatch(AppAction::RestoreSession { nsec: bob_nsec });
    wait_until(
        "bob restored session logged in",
        Duration::from_secs(15),
        || {
            matches!(bob_after_restart.state().auth, AuthState::LoggedIn { .. })
                && bob_after_restart
                    .state()
                    .chat_list
                    .iter()
                    .any(|chat| chat.chat_id == chat_id)
        },
    );

    let binding_after_restart = load_chat_server_binding(dir_b.path(), &chat_id);
    assert_eq!(binding_after_restart.0, binding_before_restart.0);
    assert_eq!(binding_after_restart.1, binding_before_restart.1);
    assert_eq!(binding_after_restart.2, binding_before_restart.2);

    open_chat(&bob_after_restart, &chat_id, Duration::from_secs(10));
    wait_for_current_chat_message(
        &bob_after_restart,
        "before-restart-chat-server",
        Duration::from_secs(10),
    );

    send_message_and_wait_sent(
        &alice,
        &chat_id,
        "after-restart-chat-server",
        Duration::from_secs(20),
    );
    wait_for_current_chat_message(
        &bob_after_restart,
        "after-restart-chat-server",
        Duration::from_secs(30),
    );

    let binding_after_sync = load_chat_server_binding(dir_b.path(), &chat_id);
    assert_eq!(binding_after_sync.0, binding_before_restart.0);
    assert_eq!(binding_after_sync.1, binding_before_restart.1);
    assert!(binding_after_sync.2 > binding_after_restart.2);
}

#[test]
fn call_invite_with_invalid_relay_auth_is_rejected() {
    let infra = support::TestInfra::start_relay();

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &infra.relay_url);
    write_config(&dir_b.path().to_string_lossy(), &infra.relay_url);

    let alice = FfiApp::new(
        dir_a.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );
    let bob = FfiApp::new(
        dir_b.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );

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

    let chat_id = create_or_open_dm_chat(&alice, &bob_npub, Duration::from_secs(60));
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

    let app = FfiApp::new(
        dir.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );
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

#[test]
fn call_end_signal_is_received_by_peer() {
    let infra = support::TestInfra::start_relay();

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &infra.relay_url);
    write_config(&dir_b.path().to_string_lossy(), &infra.relay_url);

    let alice = FfiApp::new(
        dir_a.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );
    let bob = FfiApp::new(
        dir_b.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );

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

    let chat_id = create_or_open_dm_chat(&alice, &bob_npub, Duration::from_secs(60));
    wait_until("bob sees chat", Duration::from_secs(30), || {
        bob.state().chat_list.iter().any(|c| c.chat_id == chat_id)
    });

    // Alice starts a call — bob should see it as Ringing.
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
    wait_until("bob ringing", Duration::from_secs(15), || {
        bob.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ringing))
            .unwrap_or(false)
    });

    // Alice hangs up — bob should see the call end.
    alice.dispatch(AppAction::EndCall);
    wait_until("alice call ended", Duration::from_secs(10), || {
        alice
            .state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ended { .. }))
            .unwrap_or(false)
    });
    wait_until(
        "bob call ended by peer hangup",
        Duration::from_secs(15),
        || {
            bob.state()
                .active_call
                .as_ref()
                .map(|c| matches!(c.status, CallStatus::Ended { .. }))
                .unwrap_or(false)
        },
    );

    // Verify bob's end reason reflects the remote hangup.
    let bob_reason = bob
        .state()
        .active_call
        .as_ref()
        .and_then(|c| match &c.status {
            CallStatus::Ended { reason } => Some(reason.clone()),
            _ => None,
        });
    assert_eq!(
        bob_reason.as_deref(),
        Some("user_hangup"),
        "bob should see the peer's hangup reason"
    );
}
