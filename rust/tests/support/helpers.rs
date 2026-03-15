#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pika_core::{AppAction, AppReconciler, AppUpdate, AuthState, FfiApp};

// Shared helper layer for the focused relay-backed multi-app FFI tests in `rust/tests/`.
// Keep selector-specific orchestration in `crates/pikahut/tests/support.rs`; that layer owns
// fixture lifecycle and CI-facing boundaries, while these helpers stay with the narrower
// `FfiApp` semantic owners.

pub fn wait_until(what: &str, timeout: Duration, f: impl FnMut() -> bool) {
    wait_until_with_poll(what, timeout, Duration::from_millis(50), f);
}

pub fn wait_until_with_poll(
    what: &str,
    timeout: Duration,
    poll: Duration,
    mut f: impl FnMut() -> bool,
) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return;
        }
        std::thread::sleep(poll);
    }
    panic!("{what}: condition not met within {timeout:?}");
}

pub fn write_config(data_dir: &str, relay_url: &str) {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let v = serde_json::json!({
        "disable_network": false,
        "disable_agent_allowlist_probe": true,
        "relay_urls": [relay_url],
        "key_package_relay_urls": [relay_url],
        "call_moq_url": "ws://moq.local/anon",
        "call_broadcast_prefix": "pika/calls",
        "call_audio_backend": "synthetic",
    });
    std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
}

pub fn write_config_with_moq(
    data_dir: &str,
    relay_url: &str,
    kp_relay_url: Option<&str>,
    moq_url: &str,
) {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let mut v = serde_json::json!({
        "disable_network": false,
        "disable_agent_allowlist_probe": true,
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

pub fn write_config_multi(data_dir: &str, relays: &[String], kp_relays: &[String], moq_url: &str) {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let v = serde_json::json!({
        "disable_network": false,
        "disable_agent_allowlist_probe": true,
        "relay_urls": relays,
        "key_package_relay_urls": kp_relays,
        "call_moq_url": moq_url,
        "call_broadcast_prefix": "pika/calls",
        "call_audio_backend": "synthetic",
    });
    std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
}

pub fn create_account_and_wait(app: &FfiApp) {
    app.dispatch(AppAction::CreateAccount);
    wait_until("logged in", Duration::from_secs(10), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });
}

pub fn get_logged_in_npub(app: &FfiApp) -> String {
    match app.state().auth {
        AuthState::LoggedIn { npub, .. } => npub,
        _ => panic!("not logged in"),
    }
}

pub fn dm_chat_id_for_peer(app: &FfiApp, peer_npub: &str) -> Option<String> {
    let state = app.state();
    if let Some(chat) = state
        .current_chat
        .as_ref()
        .filter(|chat| chat.members.iter().any(|member| member.npub == peer_npub))
    {
        return Some(chat.chat_id.clone());
    }
    state
        .chat_list
        .iter()
        .find(|chat| chat.members.iter().any(|member| member.npub == peer_npub))
        .map(|chat| chat.chat_id.clone())
}

pub fn create_or_open_dm_chat(app: &FfiApp, peer_npub: &str, timeout: Duration) -> String {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(chat_id) = dm_chat_id_for_peer(app, peer_npub) {
            app.dispatch(AppAction::OpenChat {
                chat_id: chat_id.clone(),
            });
            wait_until("chat opened", Duration::from_secs(20), || {
                app.state()
                    .current_chat
                    .as_ref()
                    .map(|chat| chat.chat_id == chat_id)
                    .unwrap_or(false)
            });
            return chat_id;
        }
        app.dispatch(AppAction::CreateChat {
            peer_npub: peer_npub.to_owned(),
        });
        std::thread::sleep(Duration::from_millis(500));
    }
    panic!("chat for peer {peer_npub} was not ready within {timeout:?}");
}

#[derive(Clone)]
pub struct Collector(pub Arc<Mutex<Vec<AppUpdate>>>);

impl Collector {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    #[allow(dead_code)]
    pub fn last_toast(&self) -> Option<String> {
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
