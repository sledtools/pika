use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pika_core::{AppAction, AppReconciler, AppUpdate, AuthState, CallStatus, FfiApp, Screen};
use tempfile::tempdir;

fn write_config(data_dir: &str, disable_network: bool) {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let v = serde_json::json!({
        "disable_network": disable_network,
        "call_moq_url": "https://moq.local/anon",
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
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{what}: condition not met within {timeout:?}");
}

struct TestReconciler {
    updates: Arc<Mutex<Vec<AppUpdate>>>,
}

impl TestReconciler {
    fn new() -> (Self, Arc<Mutex<Vec<AppUpdate>>>) {
        let updates = Arc::new(Mutex::new(vec![]));
        (
            Self {
                updates: updates.clone(),
            },
            updates,
        )
    }
}

impl AppReconciler for TestReconciler {
    fn reconcile(&self, update: AppUpdate) {
        self.updates.lock().unwrap().push(update);
    }
}

#[test]
fn create_account_navigates_to_chat_list() {
    let dir = tempdir().unwrap();
    write_config(&dir.path().to_string_lossy(), true);
    let app = FfiApp::new(dir.path().to_string_lossy().to_string(), String::new());
    let (reconciler, updates) = TestReconciler::new();
    app.listen_for_updates(Box::new(reconciler));

    assert_eq!(app.state().router.default_screen, Screen::Login);
    assert!(matches!(app.state().auth, AuthState::LoggedOut));

    app.dispatch(AppAction::CreateAccount);
    wait_until("logged in", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });
    wait_until("navigated to chat list", Duration::from_secs(2), || {
        app.state().router.default_screen == Screen::ChatList
    });

    let s = app.state();
    assert!(matches!(s.auth, AuthState::LoggedIn { .. }));
    assert_eq!(s.router.default_screen, Screen::ChatList);

    wait_until("updates emitted", Duration::from_secs(2), || {
        !updates.lock().unwrap().is_empty()
    });

    let up = updates.lock().unwrap();
    // Revs must be strictly increasing by 1.
    for w in up.windows(2) {
        assert_eq!(w[0].rev() + 1, w[1].rev());
    }
}

#[test]
fn push_and_pop_stack_updates_router() {
    let dir = tempdir().unwrap();
    write_config(&dir.path().to_string_lossy(), true);
    let app = FfiApp::new(dir.path().to_string_lossy().to_string(), String::new());
    app.dispatch(AppAction::CreateAccount);
    wait_until("logged in", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    app.dispatch(AppAction::PushScreen {
        screen: Screen::NewChat,
    });
    wait_until("screen pushed", Duration::from_secs(2), || {
        app.state().router.screen_stack == vec![Screen::NewChat]
    });

    // Native reports a pop.
    app.dispatch(AppAction::UpdateScreenStack { stack: vec![] });
    wait_until("screen stack popped", Duration::from_secs(2), || {
        app.state().router.screen_stack.is_empty()
    });
}

#[test]
fn send_message_creates_pending_then_sent() {
    let dir = tempdir().unwrap();
    write_config(&dir.path().to_string_lossy(), true);
    let app = FfiApp::new(dir.path().to_string_lossy().to_string(), String::new());
    app.dispatch(AppAction::CreateAccount);
    wait_until("logged in", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    let npub = match app.state().auth {
        AuthState::LoggedIn { ref npub, .. } => npub.clone(),
        _ => panic!("expected logged in"),
    };
    // Use "note to self" flow for deterministic offline tests.
    app.dispatch(AppAction::CreateChat { peer_npub: npub });
    wait_until("chat created", Duration::from_secs(2), || {
        !app.state().chat_list.is_empty()
    });

    let chat_id = app.state().chat_list[0].chat_id.clone();
    app.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until("chat opened", Duration::from_secs(2), || {
        app.state().current_chat.is_some()
    });

    app.dispatch(AppAction::SendMessage {
        chat_id,
        content: "hello".into(),
    });
    wait_until("message appears", Duration::from_secs(2), || {
        app.state()
            .current_chat
            .as_ref()
            .and_then(|c| c.messages.last())
            .map(|m| m.content == "hello")
            .unwrap_or(false)
    });

    let s1 = app.state();
    let chat = s1.current_chat.unwrap();
    let msg = chat.messages.last().unwrap();
    assert_eq!(msg.content, "hello");
    assert!(
        matches!(msg.delivery, pika_core::MessageDeliveryState::Pending)
            || matches!(msg.delivery, pika_core::MessageDeliveryState::Sent)
    );

    wait_until("message sent", Duration::from_secs(2), || {
        app.state()
            .current_chat
            .as_ref()
            .and_then(|c| c.messages.iter().find(|m| m.content == "hello"))
            .map(|m| matches!(m.delivery, pika_core::MessageDeliveryState::Sent))
            .unwrap_or(false)
    });
}

#[test]
fn start_call_toggle_mute_and_end_transitions_state() {
    let dir = tempdir().unwrap();
    write_config(&dir.path().to_string_lossy(), true);
    let app = FfiApp::new(dir.path().to_string_lossy().to_string(), String::new());
    app.dispatch(AppAction::CreateAccount);
    wait_until("logged in", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    let npub = match app.state().auth {
        AuthState::LoggedIn { ref npub, .. } => npub.clone(),
        _ => panic!("expected logged in"),
    };
    app.dispatch(AppAction::CreateChat { peer_npub: npub });
    wait_until("chat created", Duration::from_secs(2), || {
        !app.state().chat_list.is_empty()
    });
    let chat_id = app.state().chat_list[0].chat_id.clone();

    app.dispatch(AppAction::StartCall {
        chat_id: chat_id.clone(),
    });
    wait_until("call offering", Duration::from_secs(2), || {
        app.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Offering))
            .unwrap_or(false)
    });

    app.dispatch(AppAction::ToggleMute);
    wait_until("call muted", Duration::from_secs(2), || {
        app.state()
            .active_call
            .as_ref()
            .map(|c| c.is_muted)
            .unwrap_or(false)
    });

    app.dispatch(AppAction::EndCall);
    wait_until("call ended", Duration::from_secs(2), || {
        app.state()
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
}

#[test]
fn logout_resets_state() {
    let dir = tempdir().unwrap();
    write_config(&dir.path().to_string_lossy(), true);
    let app = FfiApp::new(dir.path().to_string_lossy().to_string(), String::new());
    app.dispatch(AppAction::CreateAccount);
    wait_until("logged in", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    let npub = match app.state().auth {
        AuthState::LoggedIn { ref npub, .. } => npub.clone(),
        _ => panic!("expected logged in"),
    };
    app.dispatch(AppAction::CreateChat { peer_npub: npub });
    wait_until("chat created", Duration::from_secs(2), || {
        !app.state().chat_list.is_empty()
    });

    let chat_id = app.state().chat_list[0].chat_id.clone();
    app.dispatch(AppAction::OpenChat { chat_id });
    wait_until("chat opened", Duration::from_secs(2), || {
        app.state().current_chat.is_some()
    });

    app.dispatch(AppAction::Logout);
    wait_until("logged out", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedOut)
    });

    let s = app.state();
    assert!(matches!(s.auth, AuthState::LoggedOut));
    assert_eq!(s.router.default_screen, Screen::Login);
    assert!(s.chat_list.is_empty());
    assert!(s.current_chat.is_none());
}

#[test]
fn restore_session_recovers_chat_history() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config(&data_dir, true);

    let app = FfiApp::new(data_dir.clone(), String::new());
    let (reconciler, updates) = TestReconciler::new();
    app.listen_for_updates(Box::new(reconciler));
    app.dispatch(AppAction::CreateAccount);
    wait_until("logged in", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    let my_npub = match app.state().auth {
        AuthState::LoggedIn { ref npub, .. } => npub.clone(),
        _ => panic!("expected logged in"),
    };
    app.dispatch(AppAction::CreateChat { peer_npub: my_npub });
    wait_until("chat created", Duration::from_secs(2), || {
        !app.state().chat_list.is_empty()
    });

    let chat_id = app.state().chat_list[0].chat_id.clone();
    app.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: "persist-me".into(),
    });
    wait_until("message persisted", Duration::from_secs(2), || {
        app.state()
            .chat_list
            .iter()
            .find(|c| c.chat_id == chat_id)
            .and_then(|c| c.last_message.as_deref())
            == Some("persist-me")
    });

    // Grab the generated nsec from the update stream (spec-v2 requirement).
    let nsec = {
        wait_until("AccountCreated update", Duration::from_secs(2), || {
            updates
                .lock()
                .unwrap()
                .iter()
                .any(|u| matches!(u, AppUpdate::AccountCreated { .. }))
        });
        let up = updates.lock().unwrap();
        up.iter()
            .find_map(|u| match u {
                AppUpdate::AccountCreated { nsec: s, .. } => Some(s.clone()),
                _ => None,
            })
            .expect("missing AccountCreated update with nsec")
    };

    // New process instance restores from the same encrypted per-identity DB.
    let app2 = FfiApp::new(data_dir, String::new());
    app2.dispatch(AppAction::RestoreSession { nsec });
    wait_until("restored session logged in", Duration::from_secs(2), || {
        matches!(app2.state().auth, AuthState::LoggedIn { .. })
            && !app2.state().chat_list.is_empty()
    });

    let s = app2.state();
    assert!(matches!(s.auth, AuthState::LoggedIn { .. }));
    assert!(!s.chat_list.is_empty());
    let summary = s.chat_list.iter().find(|c| c.chat_id == chat_id).unwrap();
    assert_eq!(summary.last_message.as_deref(), Some("persist-me"));

    app2.dispatch(AppAction::OpenChat { chat_id });
    wait_until(
        "chat opened has persisted message",
        Duration::from_secs(2),
        || {
            app2.state()
                .current_chat
                .as_ref()
                .map(|c| c.messages.iter().any(|m| m.content == "persist-me"))
                .unwrap_or(false)
        },
    );
    let s2 = app2.state();
    let chat = s2.current_chat.unwrap();
    assert!(chat.messages.iter().any(|m| m.content == "persist-me"));
}

#[test]
fn paging_loads_older_messages_in_pages() {
    let dir = tempdir().unwrap();
    write_config(&dir.path().to_string_lossy(), true);
    let app = FfiApp::new(dir.path().to_string_lossy().to_string(), String::new());
    app.dispatch(AppAction::CreateAccount);
    wait_until("logged in", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    let npub = match app.state().auth {
        AuthState::LoggedIn { ref npub, .. } => npub.clone(),
        _ => panic!("expected logged in"),
    };
    app.dispatch(AppAction::CreateChat { peer_npub: npub });
    wait_until("chat created", Duration::from_secs(2), || {
        !app.state().chat_list.is_empty()
    });

    let chat_id = app.state().chat_list[0].chat_id.clone();

    // CreateChat pushes into the chat; pop back to chat list so initial open uses the default
    // newest-50 paging behavior.
    app.dispatch(AppAction::UpdateScreenStack { stack: vec![] });
    wait_until("back to chat list", Duration::from_secs(2), || {
        app.state().current_chat.is_none()
    });

    // Create > 50 messages while the chat is NOT open (so initial open loads newest 50).
    for i in 0..81 {
        app.dispatch(AppAction::SendMessage {
            chat_id: chat_id.clone(),
            content: format!("m{i}"),
        });
    }
    wait_until(
        "all messages visible in list",
        Duration::from_secs(5),
        || {
            app.state()
                .chat_list
                .iter()
                .find(|c| c.chat_id == chat_id)
                .and_then(|c| c.last_message.as_deref())
                == Some("m80")
        },
    );

    app.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until(
        "chat opened newest 50 loaded",
        Duration::from_secs(5),
        || {
            app.state()
                .current_chat
                .as_ref()
                .map(|c| c.messages.len() == 50 && c.can_load_older)
                .unwrap_or(false)
        },
    );

    let s = app.state();
    let chat = s.current_chat.unwrap();
    assert_eq!(chat.messages.len(), 50);
    assert!(chat.can_load_older);
    let oldest = chat.messages.first().unwrap().id.clone();

    // Load one page.
    app.dispatch(AppAction::LoadOlderMessages {
        chat_id: chat_id.clone(),
        before_message_id: oldest,
        limit: 30,
    });
    wait_until("first page loaded", Duration::from_secs(5), || {
        app.state()
            .current_chat
            .as_ref()
            .map(|c| c.messages.len() == 80 && c.can_load_older)
            .unwrap_or(false)
    });
    let s2 = app.state();
    let chat2 = s2.current_chat.unwrap();
    assert_eq!(chat2.messages.len(), 80);
    assert!(chat2.can_load_older);

    // Load last page.
    let oldest2 = chat2.messages.first().unwrap().id.clone();
    app.dispatch(AppAction::LoadOlderMessages {
        chat_id: chat_id.clone(),
        before_message_id: oldest2,
        limit: 30,
    });
    wait_until("last page loaded", Duration::from_secs(5), || {
        app.state()
            .current_chat
            .as_ref()
            .map(|c| c.messages.len() == 81)
            .unwrap_or(false)
    });
    let s3 = app.state();
    let chat3 = s3.current_chat.unwrap();
    assert_eq!(chat3.messages.len(), 81);

    // One more load should now report no more history.
    let oldest3 = chat3.messages.first().unwrap().id.clone();
    app.dispatch(AppAction::LoadOlderMessages {
        chat_id,
        before_message_id: oldest3,
        limit: 30,
    });
    wait_until("no more history", Duration::from_secs(5), || {
        app.state()
            .current_chat
            .as_ref()
            .map(|c| !c.can_load_older)
            .unwrap_or(false)
    });
}

#[test]
fn restore_session_with_invalid_nsec_shows_toast_and_stays_logged_out() {
    let dir = tempdir().unwrap();
    write_config(&dir.path().to_string_lossy(), true);
    let app = FfiApp::new(dir.path().to_string_lossy().to_string(), String::new());

    app.dispatch(AppAction::RestoreSession {
        nsec: "not-a-real-nsec".into(),
    });

    wait_until("toast shown", Duration::from_secs(2), || {
        app.state().toast.is_some()
    });
    let s = app.state();
    assert!(matches!(s.auth, AuthState::LoggedOut));
    assert_eq!(s.router.default_screen, Screen::Login);
    assert!(s
        .toast
        .unwrap_or_default()
        .to_lowercase()
        .contains("invalid nsec"));
}

#[test]
fn create_chat_with_invalid_peer_npub_shows_toast_and_does_not_navigate() {
    let dir = tempdir().unwrap();
    write_config(&dir.path().to_string_lossy(), true);
    let app = FfiApp::new(dir.path().to_string_lossy().to_string(), String::new());

    app.dispatch(AppAction::CreateAccount);
    wait_until("logged in", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    app.dispatch(AppAction::CreateChat {
        peer_npub: "nope".into(),
    });
    wait_until("toast shown", Duration::from_secs(2), || {
        app.state().toast.is_some()
    });

    let s = app.state();
    assert!(s.current_chat.is_none());
    assert!(s.chat_list.is_empty());
    assert_eq!(s.router.default_screen, Screen::ChatList);
    assert!(s
        .toast
        .unwrap_or_default()
        .to_lowercase()
        .contains("invalid npub"));
}
