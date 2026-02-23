use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nostr_sdk::prelude::{Keys, NostrSigner, Url};
use nostr_sdk::ToBech32;
use pika_core::{
    AppAction, AppReconciler, AppUpdate, AuthMode, AuthState, BunkerConnectError,
    BunkerConnectErrorKind, BunkerConnectOutput, BunkerSignerConnector, CallStatus,
    ExternalSignerBridge, ExternalSignerErrorKind, ExternalSignerHandshakeResult,
    ExternalSignerResult, FfiApp, Screen,
};
use tempfile::tempdir;

fn write_config(data_dir: &str, disable_network: bool) {
    write_config_with_external_signer(data_dir, disable_network, None);
}

fn write_config_with_external_signer(
    data_dir: &str,
    disable_network: bool,
    enable_external_signer: Option<bool>,
) {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let mut v = serde_json::json!({
        "disable_network": disable_network,
        "call_moq_url": "https://moq.local/anon",
        "call_broadcast_prefix": "pika/calls",
        "call_audio_backend": "synthetic",
    });
    if let Some(enabled) = enable_external_signer {
        v["enable_external_signer"] = serde_json::Value::Bool(enabled);
    }
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

fn query_param(url: &str, key: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    parsed
        .query_pairs()
        .find_map(|(k, v)| if k == key { Some(v.into_owned()) } else { None })
}

fn nostrconnect_client_pubkey(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(ToString::to_string))
}

fn nostrconnect_metadata(url: &str) -> Option<serde_json::Value> {
    let raw = query_param(url, "metadata")?;
    serde_json::from_str(&raw).ok()
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

#[derive(Clone)]
struct MockExternalSignerBridge {
    handshake_result: Arc<Mutex<ExternalSignerHandshakeResult>>,
    last_hint: Arc<Mutex<Option<String>>>,
    open_url_result: Arc<Mutex<ExternalSignerResult>>,
    last_opened_url: Arc<Mutex<Option<String>>>,
    open_url_calls: Arc<Mutex<u64>>,
}

impl MockExternalSignerBridge {
    fn new(handshake_result: ExternalSignerHandshakeResult) -> Self {
        Self {
            handshake_result: Arc::new(Mutex::new(handshake_result)),
            last_hint: Arc::new(Mutex::new(None)),
            open_url_result: Arc::new(Mutex::new(ExternalSignerResult {
                ok: true,
                value: None,
                error_kind: None,
                error_message: None,
            })),
            last_opened_url: Arc::new(Mutex::new(None)),
            open_url_calls: Arc::new(Mutex::new(0)),
        }
    }

    fn last_hint(&self) -> Option<String> {
        self.last_hint.lock().unwrap().clone()
    }

    fn last_opened_url(&self) -> Option<String> {
        self.last_opened_url.lock().unwrap().clone()
    }

    fn open_url_calls(&self) -> u64 {
        *self.open_url_calls.lock().unwrap()
    }
}

impl ExternalSignerBridge for MockExternalSignerBridge {
    fn open_url(&self, url: String) -> ExternalSignerResult {
        *self.last_opened_url.lock().unwrap() = Some(url);
        let mut calls = self.open_url_calls.lock().unwrap();
        *calls += 1;
        self.open_url_result.lock().unwrap().clone()
    }

    fn request_public_key(
        &self,
        current_user_hint: Option<String>,
    ) -> ExternalSignerHandshakeResult {
        *self.last_hint.lock().unwrap() = current_user_hint;
        self.handshake_result.lock().unwrap().clone()
    }

    fn sign_event(
        &self,
        _signer_package: String,
        _current_user: String,
        _unsigned_event_json: String,
    ) -> ExternalSignerResult {
        ExternalSignerResult {
            ok: false,
            value: None,
            error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
            error_message: Some("signer unavailable".into()),
        }
    }

    fn nip44_encrypt(
        &self,
        _signer_package: String,
        _current_user: String,
        _peer_pubkey: String,
        _content: String,
    ) -> ExternalSignerResult {
        ExternalSignerResult {
            ok: false,
            value: None,
            error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
            error_message: Some("signer unavailable".into()),
        }
    }

    fn nip44_decrypt(
        &self,
        _signer_package: String,
        _current_user: String,
        _peer_pubkey: String,
        _payload: String,
    ) -> ExternalSignerResult {
        ExternalSignerResult {
            ok: false,
            value: None,
            error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
            error_message: Some("signer unavailable".into()),
        }
    }

    fn nip04_encrypt(
        &self,
        _signer_package: String,
        _current_user: String,
        _peer_pubkey: String,
        _content: String,
    ) -> ExternalSignerResult {
        ExternalSignerResult {
            ok: false,
            value: None,
            error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
            error_message: Some("signer unavailable".into()),
        }
    }

    fn nip04_decrypt(
        &self,
        _signer_package: String,
        _current_user: String,
        _peer_pubkey: String,
        _payload: String,
    ) -> ExternalSignerResult {
        ExternalSignerResult {
            ok: false,
            value: None,
            error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
            error_message: Some("signer unavailable".into()),
        }
    }
}

#[derive(Clone)]
struct MockBunkerSignerConnector {
    result: Arc<Mutex<Result<BunkerConnectOutput, BunkerConnectError>>>,
    last_bunker_uri: Arc<Mutex<Option<String>>>,
    last_client_pubkey: Arc<Mutex<Option<String>>>,
}

impl MockBunkerSignerConnector {
    fn success(canonical_bunker_uri: &str) -> (Self, String) {
        let signer_keys = Keys::generate();
        let user_pubkey = signer_keys.public_key();
        let output = BunkerConnectOutput {
            user_pubkey,
            canonical_bunker_uri: canonical_bunker_uri.to_string(),
            signer: Arc::new(signer_keys) as Arc<dyn NostrSigner>,
        };
        (
            Self {
                result: Arc::new(Mutex::new(Ok(output))),
                last_bunker_uri: Arc::new(Mutex::new(None)),
                last_client_pubkey: Arc::new(Mutex::new(None)),
            },
            user_pubkey.to_hex(),
        )
    }

    fn failure(kind: BunkerConnectErrorKind, message: &str) -> Self {
        Self {
            result: Arc::new(Mutex::new(Err(BunkerConnectError {
                kind,
                message: message.to_string(),
            }))),
            last_bunker_uri: Arc::new(Mutex::new(None)),
            last_client_pubkey: Arc::new(Mutex::new(None)),
        }
    }

    fn last_bunker_uri(&self) -> Option<String> {
        self.last_bunker_uri.lock().unwrap().clone()
    }

    fn last_client_pubkey(&self) -> Option<String> {
        self.last_client_pubkey.lock().unwrap().clone()
    }
}

impl BunkerSignerConnector for MockBunkerSignerConnector {
    fn connect(
        &self,
        _runtime: &tokio::runtime::Runtime,
        bunker_uri: &str,
        client_keys: Keys,
    ) -> Result<BunkerConnectOutput, BunkerConnectError> {
        *self.last_bunker_uri.lock().unwrap() = Some(bunker_uri.to_string());
        *self.last_client_pubkey.lock().unwrap() = Some(client_keys.public_key().to_hex());
        self.result.lock().unwrap().clone()
    }

    fn prepare(
        &self,
        _runtime: &tokio::runtime::Runtime,
        _bunker_uri: &str,
        _client_keys: Keys,
    ) -> Result<nostr_connect::prelude::NostrConnect, BunkerConnectError> {
        // Mock: return an error so the code falls through to the `connect` path.
        Err(BunkerConnectError {
            kind: BunkerConnectErrorKind::Other,
            message: "mock: prepare not supported".to_string(),
        })
    }

    fn finish(
        &self,
        _runtime: &tokio::runtime::Runtime,
        _signer: nostr_connect::prelude::NostrConnect,
    ) -> Result<BunkerConnectOutput, BunkerConnectError> {
        self.result.lock().unwrap().clone()
    }
}

#[derive(Clone)]
struct SequenceBunkerSignerConnector {
    results: Arc<Mutex<Vec<Result<BunkerConnectOutput, BunkerConnectError>>>>,
    seen_uris: Arc<Mutex<Vec<String>>>,
}

impl SequenceBunkerSignerConnector {
    fn new(results: Vec<Result<BunkerConnectOutput, BunkerConnectError>>) -> Self {
        Self {
            results: Arc::new(Mutex::new(results)),
            seen_uris: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn seen_uris(&self) -> Vec<String> {
        self.seen_uris.lock().unwrap().clone()
    }
}

impl BunkerSignerConnector for SequenceBunkerSignerConnector {
    fn connect(
        &self,
        _runtime: &tokio::runtime::Runtime,
        bunker_uri: &str,
        _client_keys: Keys,
    ) -> Result<BunkerConnectOutput, BunkerConnectError> {
        self.seen_uris.lock().unwrap().push(bunker_uri.to_string());
        let mut results = self.results.lock().unwrap();
        if results.is_empty() {
            return Err(BunkerConnectError {
                kind: BunkerConnectErrorKind::Other,
                message: "sequence connector exhausted".to_string(),
            });
        }
        results.remove(0)
    }

    fn prepare(
        &self,
        _runtime: &tokio::runtime::Runtime,
        _bunker_uri: &str,
        _client_keys: Keys,
    ) -> Result<nostr_connect::prelude::NostrConnect, BunkerConnectError> {
        Err(BunkerConnectError {
            kind: BunkerConnectErrorKind::Other,
            message: "mock: prepare not supported".to_string(),
        })
    }

    fn finish(
        &self,
        _runtime: &tokio::runtime::Runtime,
        _signer: nostr_connect::prelude::NostrConnect,
    ) -> Result<BunkerConnectOutput, BunkerConnectError> {
        Err(BunkerConnectError {
            kind: BunkerConnectErrorKind::Other,
            message: "mock: finish not supported".to_string(),
        })
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
        chat_id: chat_id.clone(),
        content: "hello".into(),
        kind: None,
        reply_to_message_id: None,
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
    let first_message_id = msg.id.clone();
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

    app.dispatch(AppAction::SendMessage {
        chat_id,
        content: "reply".into(),
        kind: None,
        reply_to_message_id: Some(first_message_id.clone()),
    });
    wait_until("reply appears", Duration::from_secs(2), || {
        app.state()
            .current_chat
            .as_ref()
            .and_then(|c| c.messages.last())
            .map(|m| {
                m.content == "reply"
                    && m.reply_to_message_id.as_deref() == Some(first_message_id.as_str())
            })
            .unwrap_or(false)
    });
}

#[test]
fn send_message_with_unresolvable_reply_falls_back_to_plain_message() {
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

    app.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until("chat opened", Duration::from_secs(2), || {
        app.state().current_chat.is_some()
    });

    let missing_reply_target =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
    app.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: "plain despite stale reply".into(),
        kind: None,
        reply_to_message_id: Some(missing_reply_target),
    });

    wait_until("message appears", Duration::from_secs(2), || {
        app.state()
            .current_chat
            .as_ref()
            .and_then(|c| c.messages.last())
            .map(|m| {
                m.content == "plain despite stale reply"
                    && m.reply_to_message_id.is_none()
                    && (matches!(m.delivery, pika_core::MessageDeliveryState::Pending)
                        || matches!(m.delivery, pika_core::MessageDeliveryState::Sent))
            })
            .unwrap_or(false)
    });

    wait_until(
        "stale reply send succeeds as plain text",
        Duration::from_secs(2),
        || {
            app.state()
                .current_chat
                .as_ref()
                .and_then(|c| c.messages.last())
                .map(|m| {
                    m.content == "plain despite stale reply"
                        && m.reply_to_message_id.is_none()
                        && matches!(m.delivery, pika_core::MessageDeliveryState::Sent)
                })
                .unwrap_or(false)
        },
    );
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
fn wipe_local_data_removes_persistent_files() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_path_buf();
    write_config(&data_dir.to_string_lossy(), true);

    let app = FfiApp::new(data_dir.to_string_lossy().to_string(), String::new());
    app.dispatch(AppAction::CreateAccount);
    wait_until("logged in", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    let pubkey = match app.state().auth {
        AuthState::LoggedIn { ref pubkey, .. } => pubkey.clone(),
        _ => panic!("expected logged in"),
    };

    let mdk_path = data_dir.join("mls").join(&pubkey).join("mdk.sqlite3");
    wait_until("mdk db created", Duration::from_secs(2), || {
        mdk_path.exists()
    });

    let profile_db_path = data_dir.join("profiles.sqlite3");
    wait_until("profile db created", Duration::from_secs(2), || {
        profile_db_path.exists()
    });

    let push_device_id_path = data_dir.join("push_device_id.txt");
    wait_until("push device id created", Duration::from_secs(2), || {
        push_device_id_path.exists()
    });
    let migration_sentinel = data_dir.join(".migrated_to_app_group");
    std::fs::write(&migration_sentinel, b"").unwrap();
    let old_push_device_id = std::fs::read_to_string(&push_device_id_path).unwrap();
    std::fs::write(data_dir.join("dev_wipe_marker.txt"), b"x").unwrap();

    app.dispatch(AppAction::WipeLocalData);
    wait_until("logged out", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedOut)
    });

    // wipe_local_data() runs after emit_auth() on the core thread, so the
    // shared state may show LoggedOut before file cleanup + recreation is
    // complete. Wait for the recreated push_device_id to appear.
    wait_until("push device id recreated", Duration::from_secs(2), || {
        push_device_id_path.exists()
    });

    assert!(!mdk_path.exists());
    assert!(!data_dir.join("dev_wipe_marker.txt").exists());
    assert!(migration_sentinel.exists());
    assert!(profile_db_path.exists());
    let new_push_device_id = std::fs::read_to_string(&push_device_id_path).unwrap();
    assert!(!new_push_device_id.trim().is_empty());
    assert_ne!(old_push_device_id.trim(), new_push_device_id.trim());
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
        kind: None,
        reply_to_message_id: None,
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
            kind: None,
            reply_to_message_id: None,
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

#[test]
fn begin_external_signer_login_is_owned_by_rust_and_logs_in() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    let bridge = MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: true,
        pubkey: Some("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798".into()),
        signer_package: Some("com.greenart7c3.nostrsigner".into()),
        current_user: Some("amber-user-1".into()),
        error_kind: None,
        error_message: None,
    });
    app.set_external_signer_bridge(Box::new(bridge.clone()));

    app.dispatch(AppAction::BeginExternalSignerLogin {
        current_user_hint: Some("hint-user".into()),
    });
    wait_until("external signer logged in", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    let s = app.state();
    assert!(!s.busy.logging_in);
    assert_eq!(s.router.default_screen, Screen::ChatList);
    match s.auth {
        AuthState::LoggedIn {
            mode:
                AuthMode::ExternalSigner {
                    pubkey,
                    signer_package,
                    current_user,
                },
            ..
        } => {
            assert_eq!(
                pubkey,
                "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
            );
            assert_eq!(signer_package, "com.greenart7c3.nostrsigner");
            assert_eq!(current_user, "amber-user-1");
        }
        other => panic!("expected external signer auth mode, got {other:?}"),
    }
    assert_eq!(bridge.last_hint().as_deref(), Some("hint-user"));
}

#[test]
fn begin_external_signer_login_failure_shows_rust_toast_and_clears_busy() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    let bridge = MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: false,
        pubkey: None,
        signer_package: None,
        current_user: None,
        error_kind: Some(ExternalSignerErrorKind::Timeout),
        error_message: Some("timeout".into()),
    });
    app.set_external_signer_bridge(Box::new(bridge));

    app.dispatch(AppAction::BeginExternalSignerLogin {
        current_user_hint: None,
    });

    wait_until("toast shown", Duration::from_secs(2), || {
        app.state().toast.is_some()
    });
    let s = app.state();
    assert!(matches!(s.auth, AuthState::LoggedOut));
    assert!(!s.busy.logging_in);
    assert!(s
        .toast
        .unwrap_or_default()
        .to_lowercase()
        .contains("timed out"));
}

#[test]
fn restore_session_external_signer_keeps_current_user_in_auth_state() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    let bridge = MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: false,
        pubkey: None,
        signer_package: None,
        current_user: None,
        error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
        error_message: Some("unused".into()),
    });
    app.set_external_signer_bridge(Box::new(bridge));

    app.dispatch(AppAction::RestoreSessionExternalSigner {
        pubkey: "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798".into(),
        signer_package: "com.greenart7c3.nostrsigner".into(),
        current_user: "restored-user".into(),
    });

    wait_until(
        "restored external signer logged in",
        Duration::from_secs(2),
        || matches!(app.state().auth, AuthState::LoggedIn { .. }),
    );
    let s = app.state();
    assert!(!s.busy.logging_in);
    match s.auth {
        AuthState::LoggedIn {
            mode: AuthMode::ExternalSigner { current_user, .. },
            ..
        } => assert_eq!(current_user, "restored-user"),
        other => panic!("expected external signer auth mode, got {other:?}"),
    }
}

#[test]
fn begin_bunker_login_is_owned_by_rust_and_emits_descriptor_update() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    let (reconciler, updates) = TestReconciler::new();
    app.listen_for_updates(Box::new(reconciler));

    let canonical_bunker_uri =
        "bunker://79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798?relay=wss://relay.example.com";
    let (connector, expected_user_pubkey) =
        MockBunkerSignerConnector::success(canonical_bunker_uri);
    app.set_bunker_signer_connector_for_tests(Arc::new(connector.clone()));

    app.dispatch(AppAction::BeginBunkerLogin {
        bunker_uri:
            "bunker://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa?relay=wss://relay.input"
                .into(),
    });

    wait_until("bunker logged in", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    let s = app.state();
    assert!(!s.busy.logging_in);
    assert_eq!(s.router.default_screen, Screen::ChatList);
    match s.auth {
        AuthState::LoggedIn {
            pubkey,
            mode: AuthMode::BunkerSigner { bunker_uri },
            ..
        } => {
            assert_eq!(pubkey, expected_user_pubkey);
            assert_eq!(bunker_uri, canonical_bunker_uri);
        }
        other => panic!("expected bunker signer auth mode, got {other:?}"),
    }
    assert_eq!(
        connector.last_bunker_uri().as_deref(),
        Some("bunker://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa?relay=wss://relay.input")
    );

    wait_until("bunker descriptor update", Duration::from_secs(2), || {
        updates
            .lock()
            .unwrap()
            .iter()
            .any(|u| matches!(u, AppUpdate::BunkerSessionDescriptor { .. }))
    });
    let descriptor = updates.lock().unwrap().iter().find_map(|u| match u {
        AppUpdate::BunkerSessionDescriptor {
            bunker_uri,
            client_nsec,
            ..
        } => Some((bunker_uri.clone(), client_nsec.clone())),
        _ => None,
    });
    let (descriptor_uri, descriptor_client_nsec) = descriptor.expect("descriptor update");
    assert_eq!(descriptor_uri, canonical_bunker_uri);
    assert!(!descriptor_client_nsec.trim().is_empty());
}

#[test]
fn begin_bunker_login_failure_shows_toast_and_clears_busy() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    let connector = MockBunkerSignerConnector::failure(
        BunkerConnectErrorKind::InvalidUri,
        "invalid bunker URI",
    );
    app.set_bunker_signer_connector_for_tests(Arc::new(connector));

    app.dispatch(AppAction::BeginBunkerLogin {
        bunker_uri: "not-a-uri".into(),
    });

    wait_until("toast shown", Duration::from_secs(2), || {
        app.state().toast.is_some()
    });
    let s = app.state();
    assert!(matches!(s.auth, AuthState::LoggedOut));
    assert!(!s.busy.logging_in);
    assert!(s
        .toast
        .unwrap_or_default()
        .to_lowercase()
        .contains("invalid bunker uri"));
}

#[test]
fn begin_nostr_connect_login_launches_uri_and_logs_in() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    let bridge = MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: false,
        pubkey: None,
        signer_package: None,
        current_user: None,
        error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
        error_message: Some("unused".into()),
    });
    app.set_external_signer_bridge(Box::new(bridge.clone()));

    let canonical_bunker_uri =
        "bunker://79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798?relay=wss://relay.example.com";
    let (connector, _expected_user_pubkey) =
        MockBunkerSignerConnector::success(canonical_bunker_uri);
    app.set_bunker_signer_connector_for_tests(Arc::new(connector.clone()));

    app.dispatch(AppAction::BeginNostrConnectLogin);

    wait_until("nostrconnect uri opened", Duration::from_secs(2), || {
        bridge.last_opened_url().is_some()
    });

    // Rust should wait for the app callback before attempting the signer handshake.
    assert!(matches!(app.state().auth, AuthState::LoggedOut));
    assert!(app.state().busy.logging_in);
    assert!(connector.last_bunker_uri().is_none());
    app.dispatch(AppAction::NostrConnectCallback {
        url: "pika://nostrconnect-return".into(),
    });

    app.inject_nostr_connect_connect_response_for_tests(
        "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798".into(),
    );

    wait_until("nostrconnect logged in", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    let opened_url = bridge
        .last_opened_url()
        .expect("expected bridge open_url call");
    assert!(opened_url.starts_with("nostrconnect://"));
    assert!(opened_url.contains("secret="));
    assert!(opened_url.contains("metadata="));
    assert!(opened_url.contains("perms="));
    assert!(opened_url.contains("relay="));
    assert_eq!(query_param(&opened_url, "name").as_deref(), Some("Pika"));
    assert_eq!(
        query_param(&opened_url, "url").as_deref(),
        Some("https://pikachat.org")
    );
    let metadata = nostrconnect_metadata(&opened_url).expect("metadata JSON");
    assert_eq!(metadata.get("name").and_then(|v| v.as_str()), Some("Pika"));
    assert_eq!(
        metadata.get("url").and_then(|v| v.as_str()),
        Some("https://pikachat.org")
    );
    let connect_uri = connector
        .last_bunker_uri()
        .expect("expected bunker connect URI for signer bootstrap");
    assert!(connect_uri
        .starts_with("bunker://79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"));
    assert!(connect_uri.contains("relay="));
    assert!(connect_uri.contains("secret="));
    assert!(!app.state().busy.logging_in);
}

#[test]
fn begin_nostr_connect_login_retries_bunker_without_secret_on_new_secret_reject() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    let bridge = MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: false,
        pubkey: None,
        signer_package: None,
        current_user: None,
        error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
        error_message: Some("unused".into()),
    });
    app.set_external_signer_bridge(Box::new(bridge));

    let signer_keys = Keys::generate();
    let remote_signer_pubkey = signer_keys.public_key().to_hex();
    let output = BunkerConnectOutput {
        user_pubkey: signer_keys.public_key(),
        canonical_bunker_uri: format!(
            "bunker://{remote_signer_pubkey}?relay=wss://relay.example.com"
        ),
        signer: Arc::new(signer_keys) as Arc<dyn NostrSigner>,
    };
    let connector = SequenceBunkerSignerConnector::new(vec![
        Err(BunkerConnectError {
            kind: BunkerConnectErrorKind::Rejected,
            message: "We don't accept connect requests with new secret.".into(),
        }),
        Ok(output),
    ]);
    app.set_bunker_signer_connector_for_tests(Arc::new(connector.clone()));

    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until("nostrconnect pending", Duration::from_secs(2), || {
        app.state().busy.logging_in
    });
    app.dispatch(AppAction::NostrConnectCallback {
        url: "pika://nostrconnect-return".into(),
    });
    app.inject_nostr_connect_connect_response_for_tests(remote_signer_pubkey.clone());

    wait_until("nostrconnect logged in", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });
    assert!(!app.state().busy.logging_in);

    let seen = connector.seen_uris();
    assert_eq!(seen.len(), 2, "expected first call + retry");
    assert!(
        seen[0].contains("secret="),
        "first attempt should include secret"
    );
    assert!(
        !seen[1].contains("secret="),
        "retry attempt should drop secret query parameter"
    );
}

#[test]
fn begin_nostr_connect_login_does_not_retry_without_new_secret_marker() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    let bridge = MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: false,
        pubkey: None,
        signer_package: None,
        current_user: None,
        error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
        error_message: Some("unused".into()),
    });
    app.set_external_signer_bridge(Box::new(bridge));

    let signer_keys = Keys::generate();
    let remote_signer_pubkey = signer_keys.public_key().to_hex();
    let output = BunkerConnectOutput {
        user_pubkey: signer_keys.public_key(),
        canonical_bunker_uri: format!(
            "bunker://{remote_signer_pubkey}?relay=wss://relay.example.com"
        ),
        signer: Arc::new(signer_keys) as Arc<dyn NostrSigner>,
    };
    let connector = SequenceBunkerSignerConnector::new(vec![
        Err(BunkerConnectError {
            kind: BunkerConnectErrorKind::Rejected,
            message: "Request rejected by signer policy".into(),
        }),
        Ok(output),
    ]);
    app.set_bunker_signer_connector_for_tests(Arc::new(connector.clone()));

    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until("nostrconnect pending", Duration::from_secs(2), || {
        app.state().busy.logging_in
    });
    app.dispatch(AppAction::NostrConnectCallback {
        url: "pika://nostrconnect-return".into(),
    });
    app.inject_nostr_connect_connect_response_for_tests(remote_signer_pubkey);

    wait_until("login failed with toast", Duration::from_secs(2), || {
        app.state().toast.is_some() && !app.state().busy.logging_in
    });
    assert!(matches!(app.state().auth, AuthState::LoggedOut));
    assert!(app
        .state()
        .toast
        .unwrap_or_default()
        .to_lowercase()
        .contains("rejected"));
    assert_eq!(
        connector.seen_uris().len(),
        1,
        "should not retry when error does not mention new secret"
    );
}

#[test]
fn begin_nostr_connect_login_reuses_persisted_secret() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir.clone(), String::new());
    let bridge = MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: false,
        pubkey: None,
        signer_package: None,
        current_user: None,
        error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
        error_message: Some("unused".into()),
    });
    app.set_external_signer_bridge(Box::new(bridge.clone()));

    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until(
        "first nostrconnect uri opened",
        Duration::from_secs(2),
        || bridge.last_opened_url().is_some(),
    );
    let first_url = bridge.last_opened_url().expect("first opened URL");
    let first_secret = query_param(&first_url, "secret").expect("first secret query");
    let first_client_pubkey =
        nostrconnect_client_pubkey(&first_url).expect("first nostrconnect host pubkey");
    assert_eq!(first_secret.len(), 32);

    let first_open_calls = bridge.open_url_calls();
    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until(
        "second nostrconnect uri opened",
        Duration::from_secs(2),
        || bridge.open_url_calls() > first_open_calls,
    );
    let second_url = bridge.last_opened_url().expect("second opened URL");
    let second_secret = query_param(&second_url, "secret").expect("second secret query");
    let second_client_pubkey =
        nostrconnect_client_pubkey(&second_url).expect("second nostrconnect host pubkey");
    assert_eq!(first_secret, second_secret);
    assert_eq!(first_client_pubkey, second_client_pubkey);

    drop(app);

    let app_after_restart = FfiApp::new(data_dir, String::new());
    let bridge_after_restart = MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: false,
        pubkey: None,
        signer_package: None,
        current_user: None,
        error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
        error_message: Some("unused".into()),
    });
    app_after_restart.set_external_signer_bridge(Box::new(bridge_after_restart.clone()));
    app_after_restart.dispatch(AppAction::BeginNostrConnectLogin);

    wait_until(
        "nostrconnect uri opened after restart",
        Duration::from_secs(2),
        || bridge_after_restart.last_opened_url().is_some(),
    );
    let restarted_url = bridge_after_restart
        .last_opened_url()
        .expect("opened URL after restart");
    let restarted_secret = query_param(&restarted_url, "secret").expect("restarted secret query");
    let restarted_client_pubkey =
        nostrconnect_client_pubkey(&restarted_url).expect("restarted nostrconnect host pubkey");
    assert_eq!(first_secret, restarted_secret);
    assert_eq!(first_client_pubkey, restarted_client_pubkey);
}

#[test]
fn reset_nostr_connect_pairing_rotates_persisted_client_pair() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    let bridge = MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: false,
        pubkey: None,
        signer_package: None,
        current_user: None,
        error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
        error_message: Some("unused".into()),
    });
    app.set_external_signer_bridge(Box::new(bridge.clone()));

    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until(
        "first nostrconnect uri opened",
        Duration::from_secs(2),
        || bridge.last_opened_url().is_some(),
    );
    let first_url = bridge.last_opened_url().expect("first opened URL");
    let first_secret = query_param(&first_url, "secret").expect("first secret query");
    let first_client_pubkey =
        nostrconnect_client_pubkey(&first_url).expect("first nostrconnect host pubkey");

    app.dispatch(AppAction::ResetNostrConnectPairing);
    wait_until("busy cleared after reset", Duration::from_secs(2), || {
        !app.state().busy.logging_in
    });
    assert!(app
        .state()
        .toast
        .unwrap_or_default()
        .to_lowercase()
        .contains("pairing reset"));

    let first_open_calls = bridge.open_url_calls();
    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until(
        "second nostrconnect uri opened",
        Duration::from_secs(2),
        || bridge.open_url_calls() > first_open_calls,
    );
    let second_url = bridge.last_opened_url().expect("second opened URL");
    let second_secret = query_param(&second_url, "secret").expect("second secret query");
    let second_client_pubkey =
        nostrconnect_client_pubkey(&second_url).expect("second nostrconnect host pubkey");

    assert_ne!(first_secret, second_secret);
    assert_ne!(first_client_pubkey, second_client_pubkey);
}

#[test]
fn pending_nostr_connect_login_survives_app_restart() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir.clone(), String::new());
    let bridge = MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: false,
        pubkey: None,
        signer_package: None,
        current_user: None,
        error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
        error_message: Some("unused".into()),
    });
    app.set_external_signer_bridge(Box::new(bridge.clone()));

    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until("nostrconnect uri opened", Duration::from_secs(2), || {
        bridge.last_opened_url().is_some()
    });
    assert!(app.state().busy.logging_in);

    drop(app);

    let app_after_restart = FfiApp::new(data_dir, String::new());
    let canonical_bunker_uri =
        "bunker://79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798?relay=wss://relay.example.com";
    let (connector, _expected_user_pubkey) =
        MockBunkerSignerConnector::success(canonical_bunker_uri);
    app_after_restart.set_bunker_signer_connector_for_tests(Arc::new(connector.clone()));

    wait_until(
        "restart restored pending busy state",
        Duration::from_secs(2),
        || app_after_restart.state().busy.logging_in,
    );
    app_after_restart.dispatch(AppAction::NostrConnectCallback {
        url: "pika://nostrconnect-return".into(),
    });

    app_after_restart.inject_nostr_connect_connect_response_for_tests(
        "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798".into(),
    );

    wait_until(
        "nostrconnect logged in after restart",
        Duration::from_secs(2),
        || matches!(app_after_restart.state().auth, AuthState::LoggedIn { .. }),
    );
    assert!(!app_after_restart.state().busy.logging_in);
    assert!(connector.last_bunker_uri().is_some());
}

#[test]
fn begin_nostr_connect_login_without_bridge_shows_toast() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    app.dispatch(AppAction::BeginNostrConnectLogin);

    wait_until("toast shown", Duration::from_secs(2), || {
        app.state().toast.is_some()
    });
    let s = app.state();
    assert!(matches!(s.auth, AuthState::LoggedOut));
    assert!(!s.busy.logging_in);
    assert!(s
        .toast
        .unwrap_or_default()
        .to_lowercase()
        .contains("bridge unavailable"));
}

#[test]
fn nostr_connect_callback_without_pending_is_noop() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    app.dispatch(AppAction::NostrConnectCallback {
        url: "pika://nostrconnect-return".into(),
    });

    // No-op: no pending login, no busy/toast changes.
    let s = app.state();
    assert!(matches!(s.auth, AuthState::LoggedOut));
    assert!(!s.busy.logging_in);
    assert!(s.toast.is_none());
}

#[test]
fn nostr_connect_callback_without_connect_response_does_not_log_in() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    let bridge = MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: false,
        pubkey: None,
        signer_package: None,
        current_user: None,
        error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
        error_message: Some("unused".into()),
    });
    app.set_external_signer_bridge(Box::new(bridge));

    let canonical_bunker_uri =
        "bunker://79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798?relay=wss://relay.example.com";
    let (connector, _expected_user_pubkey) =
        MockBunkerSignerConnector::success(canonical_bunker_uri);
    app.set_bunker_signer_connector_for_tests(Arc::new(connector.clone()));

    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until("nostrconnect pending", Duration::from_secs(2), || {
        app.state().busy.logging_in
    });

    app.dispatch(AppAction::NostrConnectCallback {
        url: "pika://nostrconnect-return?remote_signer_pubkey=79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798".into(),
    });

    std::thread::sleep(Duration::from_millis(150));
    assert!(matches!(app.state().auth, AuthState::LoggedOut));
    assert!(app.state().busy.logging_in);
    assert!(connector.last_bunker_uri().is_none());
}

#[test]
fn nostr_connect_connect_response_without_callback_does_not_log_in() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    let bridge = MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: false,
        pubkey: None,
        signer_package: None,
        current_user: None,
        error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
        error_message: Some("unused".into()),
    });
    app.set_external_signer_bridge(Box::new(bridge));

    let canonical_bunker_uri =
        "bunker://79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798?relay=wss://relay.example.com";
    let (connector, _expected_user_pubkey) =
        MockBunkerSignerConnector::success(canonical_bunker_uri);
    app.set_bunker_signer_connector_for_tests(Arc::new(connector.clone()));

    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until("nostrconnect pending", Duration::from_secs(2), || {
        app.state().busy.logging_in
    });

    app.inject_nostr_connect_connect_response_for_tests(
        "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798".into(),
    );

    std::thread::sleep(Duration::from_millis(150));
    assert!(matches!(app.state().auth, AuthState::LoggedOut));
    assert!(app.state().busy.logging_in);
    assert!(connector.last_bunker_uri().is_none());
}

#[test]
fn foregrounded_continues_pending_nostr_connect_login() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    let bridge = MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: false,
        pubkey: None,
        signer_package: None,
        current_user: None,
        error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
        error_message: Some("unused".into()),
    });
    app.set_external_signer_bridge(Box::new(bridge));

    let canonical_bunker_uri =
        "bunker://79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798?relay=wss://relay.example.com";
    let (connector, _expected_user_pubkey) =
        MockBunkerSignerConnector::success(canonical_bunker_uri);
    app.set_bunker_signer_connector_for_tests(Arc::new(connector));

    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until("nostrconnect pending", Duration::from_secs(2), || {
        app.state().busy.logging_in
    });
    assert!(matches!(app.state().auth, AuthState::LoggedOut));

    // Simulate callback before the signer response arrives, then foreground retry.
    app.dispatch(AppAction::NostrConnectCallback {
        url: "pika://nostrconnect-return".into(),
    });
    app.dispatch(AppAction::Foregrounded);

    app.inject_nostr_connect_connect_response_for_tests(
        "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798".into(),
    );

    wait_until(
        "nostrconnect logged in via foreground",
        Duration::from_secs(2),
        || matches!(app.state().auth, AuthState::LoggedIn { .. }),
    );
    assert!(!app.state().busy.logging_in);
}

#[test]
fn restore_session_bunker_uses_stored_client_key_and_logs_in() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    let canonical_bunker_uri =
        "bunker://79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798?relay=wss://relay.restore";
    let (connector, _expected_user_pubkey) =
        MockBunkerSignerConnector::success(canonical_bunker_uri);
    app.set_bunker_signer_connector_for_tests(Arc::new(connector.clone()));

    let client_keys = Keys::generate();
    let client_nsec = client_keys.secret_key().to_bech32().unwrap();
    let expected_client_pubkey = client_keys.public_key().to_hex();
    app.dispatch(AppAction::RestoreSessionBunker {
        bunker_uri:
            "bunker://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb?relay=wss://relay.restore.input"
                .into(),
        client_nsec,
    });

    wait_until("bunker restored", Duration::from_secs(2), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });
    wait_until("busy cleared", Duration::from_secs(2), || {
        !app.state().busy.logging_in
    });

    let s = app.state();
    match s.auth {
        AuthState::LoggedIn {
            mode: AuthMode::BunkerSigner { bunker_uri },
            ..
        } => assert_eq!(bunker_uri, canonical_bunker_uri),
        other => panic!("expected bunker signer auth mode, got {other:?}"),
    }
    assert_eq!(
        connector.last_client_pubkey().as_deref(),
        Some(expected_client_pubkey.as_str())
    );
}

#[test]
fn restore_session_bunker_with_invalid_client_key_shows_toast() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().to_string();
    write_config_with_external_signer(&data_dir, true, Some(true));

    let app = FfiApp::new(data_dir, String::new());
    app.dispatch(AppAction::RestoreSessionBunker {
        bunker_uri:
            "bunker://79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798?relay=wss://relay.example.com"
                .into(),
        client_nsec: "not-a-valid-client-key".into(),
    });

    wait_until("toast shown", Duration::from_secs(2), || {
        app.state().toast.is_some()
    });
    let s = app.state();
    assert!(matches!(s.auth, AuthState::LoggedOut));
    assert!(!s.busy.logging_in);
    assert!(s
        .toast
        .unwrap_or_default()
        .to_lowercase()
        .contains("invalid bunker client key"));
}

#[test]
fn react_to_message_shows_reaction_on_message() {
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

    let npub = match app.state().auth {
        AuthState::LoggedIn { ref npub, .. } => npub.clone(),
        _ => panic!("expected logged in"),
    };
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

    // Send a message to react to.
    app.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: "react to me".into(),
        kind: None,
        reply_to_message_id: None,
    });
    wait_until("message appears", Duration::from_secs(2), || {
        app.state()
            .current_chat
            .as_ref()
            .and_then(|c| c.messages.last())
            .map(|m| m.content == "react to me")
            .unwrap_or(false)
    });

    let message_id = app
        .state()
        .current_chat
        .as_ref()
        .unwrap()
        .messages
        .last()
        .unwrap()
        .id
        .clone();

    // React with a thumbs up.
    app.dispatch(AppAction::ReactToMessage {
        chat_id: chat_id.clone(),
        message_id: message_id.clone(),
        emoji: "\u{1F44D}".into(),
    });
    wait_until("reaction appears", Duration::from_secs(2), || {
        app.state()
            .current_chat
            .as_ref()
            .and_then(|c| c.messages.iter().find(|m| m.id == message_id))
            .map(|m| !m.reactions.is_empty())
            .unwrap_or(false)
    });

    let s = app.state();
    let msg = s
        .current_chat
        .as_ref()
        .unwrap()
        .messages
        .iter()
        .find(|m| m.id == message_id)
        .unwrap();
    assert_eq!(msg.reactions.len(), 1);
    assert_eq!(msg.reactions[0].emoji, "\u{1F44D}");
    assert_eq!(msg.reactions[0].count, 1);
    assert!(msg.reactions[0].reacted_by_me);

    // The reaction should not appear as a separate chat message.
    assert_eq!(
        s.current_chat.as_ref().unwrap().messages.len(),
        1,
        "reaction must not appear as a separate message"
    );

    // --- Close and reopen the chat to verify reactions survive a reload ---
    app.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until(
        "chat reopened with reaction",
        Duration::from_secs(2),
        || {
            app.state()
                .current_chat
                .as_ref()
                .and_then(|c| c.messages.iter().find(|m| m.id == message_id))
                .map(|m| !m.reactions.is_empty())
                .unwrap_or(false)
        },
    );
    let s = app.state();
    let chat = s.current_chat.as_ref().unwrap();
    assert_eq!(
        chat.messages.len(),
        1,
        "no leaked reaction messages after reopen"
    );
    let msg = chat.messages.iter().find(|m| m.id == message_id).unwrap();
    assert_eq!(msg.reactions.len(), 1);
    assert_eq!(msg.reactions[0].emoji, "\u{1F44D}");
    assert!(msg.reactions[0].reacted_by_me);

    // --- Restore session in a fresh app instance (simulates the other party
    //     or the same user on a new device loading chat history from storage) ---
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

    let app2 = FfiApp::new(data_dir, String::new());
    app2.dispatch(AppAction::RestoreSession { nsec });
    wait_until("restored session logged in", Duration::from_secs(2), || {
        matches!(app2.state().auth, AuthState::LoggedIn { .. })
            && !app2.state().chat_list.is_empty()
    });

    app2.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until("restored chat has reaction", Duration::from_secs(2), || {
        app2.state()
            .current_chat
            .as_ref()
            .and_then(|c| c.messages.iter().find(|m| m.id == message_id))
            .map(|m| !m.reactions.is_empty())
            .unwrap_or(false)
    });
    let s2 = app2.state();
    let chat2 = s2.current_chat.as_ref().unwrap();
    assert_eq!(
        chat2.messages.len(),
        1,
        "no leaked reaction messages after restore"
    );
    let msg2 = chat2.messages.iter().find(|m| m.id == message_id).unwrap();
    assert_eq!(msg2.reactions.len(), 1);
    assert_eq!(msg2.reactions[0].emoji, "\u{1F44D}");
    assert_eq!(msg2.reactions[0].count, 1);
    assert!(msg2.reactions[0].reacted_by_me);
}

#[test]
fn reactions_survive_interleaved_typing_indicators() {
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
    app.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until("chat opened", Duration::from_secs(2), || {
        app.state().current_chat.is_some()
    });

    // Send a message.
    app.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: "hello".into(),
        kind: None,
        reply_to_message_id: None,
    });
    wait_until("message appears", Duration::from_secs(2), || {
        app.state()
            .current_chat
            .as_ref()
            .and_then(|c| c.messages.last())
            .map(|m| m.content == "hello")
            .unwrap_or(false)
    });

    let message_id = app
        .state()
        .current_chat
        .as_ref()
        .unwrap()
        .messages
        .last()
        .unwrap()
        .id
        .clone();

    // Send many typing indicators to pollute storage pagination.
    for _ in 0..60 {
        app.dispatch(AppAction::TypingStarted {
            chat_id: chat_id.clone(),
        });
        // Reset debounce by waiting or just let the debounce block them.
    }

    // React to the message.
    app.dispatch(AppAction::ReactToMessage {
        chat_id: chat_id.clone(),
        message_id: message_id.clone(),
        emoji: "\u{2764}\u{FE0F}".into(),
    });
    wait_until(
        "reaction visible despite typing indicators",
        Duration::from_secs(2),
        || {
            app.state()
                .current_chat
                .as_ref()
                .and_then(|c| c.messages.iter().find(|m| m.id == message_id))
                .map(|m| !m.reactions.is_empty())
                .unwrap_or(false)
        },
    );

    let s = app.state();
    let chat = s.current_chat.as_ref().unwrap();
    let msg = chat.messages.iter().find(|m| m.id == message_id).unwrap();
    assert_eq!(msg.reactions[0].emoji, "\u{2764}\u{FE0F}");
    assert!(msg.reactions[0].reacted_by_me);
    // Typing indicators must not leak as messages.
    assert_eq!(chat.messages.len(), 1);
}
