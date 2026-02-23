mod call_control;
mod call_runtime;
mod chat_media;
mod chat_media_db;
mod config;
mod interop;
mod profile;
mod profile_db;
mod profile_pics;
mod push;
mod session;
mod storage;

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use flume::Sender;

use crate::actions::AppAction;
use crate::bunker_signer::{
    BunkerConnectError, BunkerSignerConnector, SharedBunkerSignerConnector,
};
use crate::external_signer::{
    user_visible_signer_error, user_visible_signer_error_kind, ExternalSignerBridge,
    ExternalSignerBridgeSigner, ExternalSignerErrorKind, ExternalSignerHandshakeResult,
    SharedExternalSignerBridge,
};
use crate::mdk_support::{open_mdk, PikaMdk};
use crate::state::now_seconds;
use crate::state::{
    AuthMode, AuthState, BusyState, CallDebugStats, CallStatus, ChatMediaAttachment, ChatMessage,
    ChatSummary, ChatViewState, MessageDeliveryState, MyProfileState, Screen,
};
use crate::updates::{AppUpdate, CoreMsg, InternalEvent};

use mdk_core::encrypted_media::types::{EncryptedMediaUpload, MediaReference};
use mdk_core::prelude::{GroupId, MessageProcessingResult, NostrGroupConfigData};
use mdk_storage_traits::groups::Pagination;

/// Load all cached profiles from the on-disk database as `FollowListEntry`.
pub(crate) fn load_cached_profiles(data_dir: &str) -> Vec<crate::state::FollowListEntry> {
    use nostr_sdk::ToBech32;

    let conn = match profile_db::open_profile_db(data_dir) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let profiles = profile_db::load_profiles(&conn);
    let mut entries: Vec<crate::state::FollowListEntry> = profiles
        .into_iter()
        .map(|(pubkey, cache)| {
            let npub = PublicKey::from_hex(&pubkey)
                .ok()
                .and_then(|pk| pk.to_bech32().ok())
                .unwrap_or_else(|| pubkey.clone());
            let picture_url = if cache.picture_url.is_some() {
                let path = profile_pics::cached_path(data_dir, &pubkey);
                if path.exists() {
                    Some(profile_pics::path_to_file_url(&path))
                } else {
                    cache.picture_url.clone()
                }
            } else {
                None
            };
            crate::state::FollowListEntry {
                pubkey,
                npub,
                name: cache.name.filter(|name| !name.trim().is_empty()),
                username: cache.username.filter(|name| !name.trim().is_empty()),
                picture_url,
            }
        })
        .collect();
    entries.sort_by(|a, b| {
        let a_key = a.name.as_deref().unwrap_or(a.npub.as_str()).to_lowercase();
        let b_key = b.name.as_deref().unwrap_or(b.npub.as_str()).to_lowercase();
        a_key.cmp(&b_key)
    });
    entries
}

pub(crate) fn default_app_config_json() -> String {
    config::default_app_config_json()
}

pub(crate) fn relay_reset_config_json(existing_json: Option<&str>) -> String {
    config::relay_reset_config_json(existing_json)
}
use nostr_sdk::prelude::*;

use interop::{
    extract_relays_from_key_package_event, extract_relays_from_key_package_relays_event,
    normalize_peer_key_package_event_for_mdk, referenced_key_package_event_id,
};

const DEFAULT_GROUP_NAME: &str = "DM";
const DEFAULT_GROUP_DESCRIPTION: &str = "";
const IOS_MIGRATION_SENTINEL: &str = ".migrated_to_app_group";

const TYPING_INDICATOR_KIND: Kind = Kind::Custom(20_067);
pub(crate) const CALL_SIGNAL_KIND: Kind = Kind::Custom(10);

const LOCAL_OUTBOX_MAX_PER_CHAT: usize = 8;
const NOSTR_CONNECT_LOGIN_TIMEOUT_SECS: u64 = 95;
const NOSTR_CONNECT_RESPONSE_LOOKBACK_SECS: u64 = 5 * 60;
const NOSTR_CONNECT_PAIRING_KEYRING_ACCOUNT: &str = "nostr_connect_pairing";
const NOSTR_CONNECT_PENDING_KEYRING_ACCOUNT: &str = "nostr_connect_pending";

fn diag_nostr_publish_enabled() -> bool {
    match std::env::var("PIKA_DIAG_NOSTR_PUBLISH") {
        Ok(v) => {
            let t = v.trim();
            !t.is_empty() && t != "0" && !t.eq_ignore_ascii_case("false")
        }
        Err(_) => false,
    }
}

#[derive(Debug, Clone)]
struct GroupMember {
    pubkey: PublicKey,
    name: Option<String>,
    picture_url: Option<String>,
}

#[derive(Debug, Clone)]
struct GroupIndexEntry {
    mls_group_id: GroupId,
    is_group: bool,
    group_name: Option<String>,
    /// Every member except self.
    members: Vec<GroupMember>,
    admin_pubkeys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileCache {
    // Full kind:0 event content JSON (preserved for forward compatibility).
    metadata_json: Option<String>,
    // Derived from metadata_json for fast access:
    name: Option<String>,     // pretty name
    username: Option<String>, // your handle eg @jack
    about: Option<String>,
    picture_url: Option<String>,
    event_created_at: i64,
    // In-memory only (not persisted) — prevents re-fetching within a session.
    // Starts at 0 on load from DB so profiles are always re-checked on app launch.
    last_checked_at: i64,
}

impl ProfileCache {
    fn from_metadata_json(
        metadata_json: Option<String>,
        event_created_at: i64,
        last_checked_at: i64,
    ) -> Self {
        let parsed: Option<Metadata> = metadata_json
            .as_ref()
            .and_then(|json| serde_json::from_str(json).ok());
        let name = parsed
            .as_ref()
            .and_then(|m| m.display_name.clone().or_else(|| m.name.clone()))
            .filter(|s| !s.is_empty());
        let username = parsed
            .as_ref()
            .and_then(|m| m.name.clone())
            .filter(|s| !s.is_empty());
        let about = parsed
            .as_ref()
            .and_then(|m| m.about.clone())
            .filter(|s| !s.is_empty());
        let picture_url = parsed
            .as_ref()
            .and_then(|m| m.picture.clone())
            .filter(|s| !s.is_empty());

        Self {
            metadata_json,
            name,
            username,
            about,
            picture_url,
            event_created_at,
            last_checked_at,
        }
    }

    /// Returns a `file://` URL for the cached picture if the file exists on disk,
    /// otherwise falls back to the remote `picture_url`.
    fn display_picture_url(&self, data_dir: &str, pubkey_hex: &str) -> Option<String> {
        if self.picture_url.is_some() {
            let path = profile_pics::cached_path(data_dir, pubkey_hex);
            if path.exists() {
                return Some(profile_pics::path_to_file_url(&path));
            }
        }
        self.picture_url.clone()
    }
}

#[derive(Debug, Clone)]
struct PendingSend {
    wrapper_event: Event,
    // Track which UI message to update.
    rumor_id_hex: String,
}

#[derive(Debug, Clone)]
struct LocalOutgoing {
    content: String,
    timestamp: i64,
    sender_pubkey: String,
    reply_to_message_id: Option<String>,
    seq: u64,
    media: Vec<ChatMediaAttachment>,
}

#[derive(Debug, Clone)]
struct PendingMediaSend {
    chat_id: String,
    caption: String,
    upload: EncryptedMediaUpload,
    account_pubkey: String,
}

#[derive(Debug, Clone)]
struct PendingMediaDownload {
    chat_id: String,
    account_pubkey: String,
    group_id: GroupId,
    reference: MediaReference,
    encrypted_hash_hex: Option<String>,
}

#[derive(Debug, Clone)]
enum SessionAuthMode {
    LocalNsec,
    ExternalSigner {
        signer_package: String,
        current_user: String,
    },
    BunkerSigner {
        bunker_uri: String,
    },
}

struct PendingNostrConnectLogin {
    attempt_id: u64,
    started_at_unix: i64,
    client_nsec: String,
    relays: Vec<RelayUrl>,
    secret: String,
    callback_received: bool,
    /// Result slot set by the in-flight NIP-46 connect-response waiter.
    /// Contains remote signer pubkey once validated, or a user-visible error.
    connect_response_result: Arc<Mutex<Option<Result<NostrConnectConnectResponse, String>>>>,
}

#[derive(Debug, Clone)]
struct NostrConnectConnectResponse {
    remote_signer_pubkey: PublicKey,
    agreed_secret: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedNostrConnectPairing {
    client_nsec: String,
    secret: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedPendingNostrConnectLogin {
    started_at_unix: i64,
    client_nsec: String,
    relays: Vec<String>,
    secret: String,
    #[serde(default)]
    callback_received: bool,
}

impl SessionAuthMode {
    fn to_state_mode(&self, pubkey_hex: &str) -> AuthMode {
        match self {
            SessionAuthMode::LocalNsec => AuthMode::LocalNsec,
            SessionAuthMode::ExternalSigner {
                signer_package,
                current_user,
            } => AuthMode::ExternalSigner {
                pubkey: pubkey_hex.to_string(),
                signer_package: signer_package.clone(),
                current_user: current_user.clone(),
            },
            SessionAuthMode::BunkerSigner { bunker_uri } => AuthMode::BunkerSigner {
                bunker_uri: bunker_uri.clone(),
            },
        }
    }
}

struct Session {
    pubkey: PublicKey,
    local_keys: Option<Keys>,
    mdk: PikaMdk,
    client: Client,
    alive: Arc<AtomicBool>,

    giftwrap_sub: Option<SubscriptionId>,
    group_sub: Option<SubscriptionId>,

    // chat_id (hex nostr_group_id) -> group info
    groups: HashMap<String, GroupIndexEntry>,
}

pub struct AppCore {
    pub state: crate::state::AppState,
    rev: u64,
    outbox_seq: u64,
    last_outgoing_ts: i64,

    update_sender: Sender<AppUpdate>,
    core_sender: Sender<CoreMsg>,
    shared_state: Arc<RwLock<crate::state::AppState>>,
    external_signer_bridge: SharedExternalSignerBridge,
    bunker_signer_connector: SharedBunkerSignerConnector,

    data_dir: String,
    keychain_group: String,
    config: config::AppConfig,
    runtime: tokio::runtime::Runtime,

    session: Option<Session>,

    subs_recompute_in_flight: bool,
    subs_recompute_dirty: bool,
    subs_recompute_token: u64,

    // Actor-internal UI bookkeeping (spec-v2 paging + delivery state).
    loaded_count: HashMap<String, usize>,
    unread_counts: HashMap<String, u32>,
    delivery_overrides: HashMap<String, HashMap<String, MessageDeliveryState>>, // chat_id -> message_id -> delivery
    pending_sends: HashMap<String, HashMap<String, PendingSend>>, // chat_id -> rumor_id -> wrapper event
    // When MDK storage is eventually consistent, keep a local optimistic outbox so UI can render
    // immediately and reliably (e.g., offline note-to-self).
    local_outbox: HashMap<String, HashMap<String, LocalOutgoing>>, // chat_id -> message_id -> message

    // Typing indicator state: chat_id -> (sender_pubkey -> expires_at_unix_secs).
    // Purely in-memory; never persisted.
    typing_state: HashMap<String, HashMap<String, i64>>,
    // Timestamp of the last typing indicator *we* sent per chat, to debounce.
    last_typing_sent: HashMap<String, i64>,

    // Nostr kind:0 profile cache (survives across session refreshes).
    profiles: HashMap<String, ProfileCache>, // hex pubkey -> cached profile
    profile_db: Option<rusqlite::Connection>,
    chat_media_db: Option<rusqlite::Connection>,

    // Shared HTTP client (profile pic downloads, push notifications).
    http_client: reqwest::Client,
    pfp_semaphore: std::sync::Arc<tokio::sync::Semaphore>,

    // Archived chat IDs -- hidden from the chat list but data stays in MDK.
    archived_chats: HashSet<String>,

    // Push notification state.
    push_device_id: String,
    push_apns_token: Option<String>,
    push_subscribed_chat_ids: HashSet<String>,

    pending_media_sends: HashMap<String, PendingMediaSend>, // request_id -> pending upload metadata
    pending_media_downloads: HashMap<String, PendingMediaDownload>, // request_id -> pending download metadata

    call_runtime: call_runtime::CallRuntime,
    call_session_params: Option<call_control::CallSessionParams>,
    call_timeline_logged_keys: HashSet<String>,
    pending_nostr_connect_login: Option<PendingNostrConnectLogin>,
    next_nostr_connect_attempt_id: u64,
}

impl AppCore {
    pub fn new(
        update_sender: Sender<AppUpdate>,
        core_sender: Sender<CoreMsg>,
        data_dir: String,
        keychain_group: String,
        shared_state: Arc<RwLock<crate::state::AppState>>,
        external_signer_bridge: SharedExternalSignerBridge,
        bunker_signer_connector: SharedBunkerSignerConnector,
    ) -> Self {
        let config = config::load_app_config(&data_dir);
        let state = crate::state::AppState::empty();

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_time()
            .enable_io()
            .build()
            .expect("tokio runtime");

        let run_moq_probe = config.moq_probe_on_start == Some(true);
        let moq_probe_url = config
            .call_moq_url
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);

        profile_pics::ensure_dir(&data_dir);

        let profile_db = match profile_db::open_profile_db(&data_dir) {
            Ok(conn) => Some(conn),
            Err(e) => {
                tracing::warn!(%e, "failed to open profile cache db");
                None
            }
        };
        let chat_media_db = match chat_media_db::open_chat_media_db(&data_dir) {
            Ok(conn) => Some(conn),
            Err(e) => {
                tracing::warn!(%e, "failed to open chat media db");
                None
            }
        };
        let profiles = profile_db
            .as_ref()
            .map(profile_db::load_profiles)
            .unwrap_or_default();

        let push_device_id = Self::load_or_create_push_device_id(&data_dir);
        let push_subscribed_chat_ids = Self::load_push_subscriptions(&data_dir);

        let mut this = Self {
            state,
            rev: 0,
            outbox_seq: 0,
            last_outgoing_ts: 0,
            update_sender,
            core_sender,
            shared_state,
            external_signer_bridge,
            bunker_signer_connector,
            data_dir,
            keychain_group,
            config,
            runtime,
            session: None,
            subs_recompute_in_flight: false,
            subs_recompute_dirty: false,
            subs_recompute_token: 0,
            loaded_count: HashMap::new(),
            unread_counts: HashMap::new(),
            delivery_overrides: HashMap::new(),
            pending_sends: HashMap::new(),
            local_outbox: HashMap::new(),
            profiles,
            profile_db,
            typing_state: HashMap::new(),
            last_typing_sent: HashMap::new(),
            chat_media_db,
            http_client: reqwest::Client::new(),
            pfp_semaphore: profile_pics::new_download_semaphore(),
            archived_chats: HashSet::new(),
            push_device_id,
            push_apns_token: None,
            push_subscribed_chat_ids,
            pending_media_sends: HashMap::new(),
            pending_media_downloads: HashMap::new(),
            call_runtime: call_runtime::CallRuntime::default(),
            call_session_params: None,
            call_timeline_logged_keys: HashSet::new(),
            pending_nostr_connect_login: None,
            next_nostr_connect_attempt_id: 1,
        };

        if run_moq_probe {
            if let Some(moq_url) = moq_probe_url {
                std::thread::spawn(move || {
                    tracing::info!(moq_url = %moq_url, "moq probe: starting");
                    let res =
                        pika_media::network::NetworkRelay::new(&moq_url).and_then(|r| r.connect());
                    match res {
                        Ok(()) => tracing::info!(moq_url = %moq_url, "moq probe: PASS (connected)"),
                        Err(e) => tracing::error!(moq_url = %moq_url, err = ?e, "moq probe: FAIL"),
                    }
                });
            } else {
                tracing::warn!("moq probe: enabled but call_moq_url missing");
            }
        }

        this.resume_pending_nostr_connect_login_from_disk();

        // Ensure FfiApp.state() has an immediately-available snapshot.
        let snapshot = this.state.clone();
        this.commit_state_snapshot(&snapshot);
        this
    }

    pub fn set_video_frame_receiver(
        &mut self,
        receiver: std::sync::Arc<
            std::sync::RwLock<Option<std::sync::Arc<dyn crate::VideoFrameReceiver>>>,
        >,
    ) {
        self.call_runtime.set_video_frame_receiver(receiver);
    }

    fn archived_chats_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.data_dir).join("archived_chats.json")
    }

    fn load_archived_chats(&mut self) {
        let path = self.archived_chats_path();
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(set) = serde_json::from_str::<HashSet<String>>(&data) {
                self.archived_chats = set;
            }
        }
    }

    fn save_archived_chats(&self) {
        let path = self.archived_chats_path();
        if let Ok(json) = serde_json::to_string(&self.archived_chats) {
            let _ = std::fs::write(&path, json);
        }
    }

    fn call_timeline_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.data_dir).join("call_timeline.json")
    }

    fn load_call_timeline(&mut self) {
        let path = self.call_timeline_path();
        let loaded = std::fs::read_to_string(&path)
            .ok()
            .and_then(|data| {
                serde_json::from_str::<Vec<crate::state::CallTimelineEvent>>(&data).ok()
            })
            .unwrap_or_default();

        self.call_timeline_logged_keys.clear();
        self.state.call_timeline.clear();

        for event in loaded {
            if self.call_timeline_logged_keys.insert(event.id.clone()) {
                self.state.call_timeline.push(event);
            }
        }
    }

    fn save_call_timeline(&self) {
        let path = self.call_timeline_path();
        if let Ok(json) = serde_json::to_string(&self.state.call_timeline) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// Returns the list of members currently typing in the given chat (pruning expired entries).
    fn get_active_typers(&mut self, chat_id: &str) -> Vec<crate::state::TypingMember> {
        let now = now_seconds();
        let typers = match self.typing_state.get_mut(chat_id) {
            Some(map) => {
                map.retain(|_, expires| *expires > now);
                map.keys().cloned().collect::<Vec<_>>()
            }
            None => vec![],
        };

        // Resolve display names from the group member list.
        let members: Option<&[GroupMember]> = self
            .session
            .as_ref()
            .and_then(|s| s.groups.get(chat_id))
            .map(|g| g.members.as_slice());

        typers
            .into_iter()
            .map(|pk_hex| {
                let name = members.and_then(|ms| {
                    ms.iter()
                        .find(|m| m.pubkey.to_hex() == pk_hex)
                        .and_then(|m| m.name.clone())
                });
                crate::state::TypingMember {
                    pubkey: pk_hex,
                    name,
                }
            })
            .collect()
    }

    /// Record that `sender_pubkey_hex` is typing in `chat_id` until `expires_at`.
    /// Clears the indicator for that sender when a real message arrives (pass expires_at = 0).
    fn update_typing(&mut self, chat_id: &str, sender_pubkey_hex: &str, expires_at: i64) {
        let map = self.typing_state.entry(chat_id.to_string()).or_default();
        if expires_at <= now_seconds() {
            map.remove(sender_pubkey_hex);
        } else {
            map.insert(sender_pubkey_hex.to_string(), expires_at);
        }
    }

    fn prune_local_outbox(&mut self, chat_id: &str) {
        let Some(m) = self.local_outbox.get_mut(chat_id) else {
            return;
        };
        if m.len() <= LOCAL_OUTBOX_MAX_PER_CHAT {
            return;
        }
        // Keep only the newest N by local sequence number.
        let mut items: Vec<(String, u64)> = m.iter().map(|(id, lm)| (id.clone(), lm.seq)).collect();
        items.sort_by_key(|(_, seq)| std::cmp::Reverse(*seq));
        items.truncate(LOCAL_OUTBOX_MAX_PER_CHAT);
        let keep: std::collections::HashSet<String> = items.into_iter().map(|(id, _)| id).collect();
        m.retain(|id, _| keep.contains(id));
    }

    fn next_rev(&mut self) -> u64 {
        self.rev += 1;
        self.state.rev = self.rev;
        self.rev
    }

    fn next_nostr_connect_attempt_id(&mut self) -> u64 {
        let id = self.next_nostr_connect_attempt_id;
        self.next_nostr_connect_attempt_id = self.next_nostr_connect_attempt_id.saturating_add(1);
        id
    }

    fn commit_state_snapshot(&self, snapshot: &crate::state::AppState) {
        match self.shared_state.write() {
            Ok(mut g) => *g = snapshot.clone(),
            Err(poison) => *poison.into_inner() = snapshot.clone(),
        }
    }

    fn emit_state(&mut self) {
        self.next_rev();
        let snapshot = self.state.clone();
        self.commit_state_snapshot(&snapshot);
        let _ = self.update_sender.send(AppUpdate::FullState(snapshot));
    }

    fn emit_auth(&mut self) {
        self.emit_state();
    }

    fn emit_router(&mut self) {
        self.emit_state();
    }

    fn emit_chat_list(&mut self) {
        self.emit_state();
    }

    fn emit_busy(&mut self) {
        // Busy flags are part of AppState; emit a full snapshot like everything else.
        self.emit_state();
    }

    fn emit_current_chat(&mut self) {
        self.emit_state();
    }

    fn emit_toast(&mut self) {
        self.emit_state();
    }

    fn emit_call_state_with_previous(&mut self, previous_call: Option<crate::state::CallState>) {
        let current = self.state.active_call.clone();
        self.record_call_timeline_transition(previous_call.as_ref(), current.as_ref());
        self.emit_state();
    }

    fn emit_call_state(&mut self) {
        self.emit_state();
    }

    fn record_call_timeline_transition(
        &mut self,
        old: Option<&crate::state::CallState>,
        new: Option<&crate::state::CallState>,
    ) {
        let Some(new) = new else { return };
        let now = now_seconds();

        if new.is_live {
            self.append_call_timeline_event(
                format!("{}:started", new.call_id),
                new.chat_id.clone(),
                "Call started".to_string(),
                now,
            );
            return;
        }

        if let CallStatus::Ended { ref reason } = new.status {
            let previous_status = if old.map(|o| o.call_id.as_str()) == Some(&new.call_id) {
                old.map(|o| &o.status)
            } else {
                None
            };
            let text = call_timeline_ended_text(reason, previous_status, new.started_at);
            self.append_call_timeline_event(
                format!("{}:ended", new.call_id),
                new.chat_id.clone(),
                text,
                now,
            );
        }
    }

    fn append_call_timeline_event(
        &mut self,
        key: String,
        chat_id: String,
        text: String,
        timestamp: i64,
    ) {
        if !self.call_timeline_logged_keys.insert(key.clone()) {
            return;
        }
        self.state
            .call_timeline
            .push(crate::state::CallTimelineEvent {
                id: key,
                chat_id,
                text,
                timestamp,
            });
        // Cap at 20 events per chat to avoid unbounded growth.
        let max_per_chat = 20;
        let total = self.state.call_timeline.len();
        if total > max_per_chat * 10 {
            // Prune oldest events globally when the list gets large.
            self.state.call_timeline = self
                .state
                .call_timeline
                .split_off(total - max_per_chat * 10);
        }
        self.save_call_timeline();
    }

    fn emit_account_created(&mut self, nsec: String, pubkey: String, npub: String) {
        let rev = self.next_rev();
        // Keep snapshot rev in sync with the update stream even though this is a side-effect update.
        let snapshot = self.state.clone();
        self.commit_state_snapshot(&snapshot);
        let _ = self.update_sender.send(AppUpdate::AccountCreated {
            rev,
            nsec,
            pubkey,
            npub,
        });
    }

    fn emit_bunker_session_descriptor(&mut self, bunker_uri: String, client_nsec: String) {
        let rev = self.next_rev();
        let snapshot = self.state.clone();
        self.commit_state_snapshot(&snapshot);
        let _ = self.update_sender.send(AppUpdate::BunkerSessionDescriptor {
            rev,
            bunker_uri,
            client_nsec,
        });
    }

    fn toast(&mut self, msg: impl Into<String>) {
        // Keep toast in state until the UI explicitly clears it. This makes the UX
        // robust to rev-gap resyncs (state() snapshot still contains the toast).
        self.state.toast = Some(msg.into());
        self.emit_toast();
    }

    fn is_logged_in(&self) -> bool {
        self.session.is_some()
    }

    fn upsert_profile(&mut self, pubkey: String, new: ProfileCache) {
        if let Some(existing) = self.profiles.get(&pubkey) {
            if existing.event_created_at > new.event_created_at {
                return; // existing is newer
            }
            // Picture URL changed → download new or remove stale cache.
            if existing.picture_url != new.picture_url {
                if let Some(ref url) = new.picture_url {
                    self.spawn_pfp_download(pubkey.clone(), url.clone());
                } else {
                    let _ =
                        std::fs::remove_file(profile_pics::cached_path(&self.data_dir, &pubkey));
                }
            }
            if *existing == new {
                return; // no change
            }
        } else {
            // Brand new profile → spawn download if there's a picture URL.
            if let Some(ref url) = new.picture_url {
                self.spawn_pfp_download(pubkey.clone(), url.clone());
            }
        }
        self.profiles.insert(pubkey.clone(), new);
        if let Some(conn) = self.profile_db.as_ref() {
            if let Some(cache) = self.profiles.get(&pubkey) {
                profile_db::save_profile(conn, &pubkey, cache);
            }
        }
    }

    fn spawn_pfp_download(&self, pubkey: String, url: String) {
        let client = self.http_client.clone();
        let data_dir = self.data_dir.clone();
        let semaphore = self.pfp_semaphore.clone();
        let tx = self.core_sender.clone();
        self.runtime.spawn(async move {
            match profile_pics::download_image(&client, &data_dir, &pubkey, &url, &semaphore).await
            {
                Ok(_) => {
                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::ProfilePicCached { pubkey, url },
                    )));
                }
                Err(e) => {
                    tracing::debug!(%pubkey, %url, %e, "profile pic download failed");
                }
            }
        });
    }

    fn cache_missing_profile_pics(&self) {
        let my_pubkey = self.session.as_ref().map(|s| s.pubkey.to_hex());

        // Download own profile pic first so it's visible immediately.
        if let Some(ref pk) = my_pubkey {
            if let Some(cache) = self.profiles.get(pk) {
                if let Some(ref url) = cache.picture_url {
                    if !profile_pics::cached_path(&self.data_dir, pk).exists() {
                        self.spawn_pfp_download(pk.clone(), url.clone());
                    }
                }
            }
        }

        // Collect chat member pubkeys so we can prioritize them.
        let mut chat_member_pubkeys: HashSet<String> = HashSet::new();
        if let Some(sess) = self.session.as_ref() {
            for entry in sess.groups.values() {
                for m in &entry.members {
                    chat_member_pubkeys.insert(m.pubkey.to_hex());
                }
            }
        }

        // Download chat members next, then everyone else.
        let mut deferred = Vec::new();
        for (pubkey, cache) in &self.profiles {
            if my_pubkey.as_deref() == Some(pubkey) {
                continue; // already spawned above
            }
            let Some(ref url) = cache.picture_url else {
                continue;
            };
            if profile_pics::cached_path(&self.data_dir, pubkey).exists() {
                continue;
            }
            if chat_member_pubkeys.contains(pubkey) {
                self.spawn_pfp_download(pubkey.clone(), url.clone());
            } else {
                deferred.push((pubkey.clone(), url.clone()));
            }
        }
        for (pubkey, url) in deferred {
            self.spawn_pfp_download(pubkey, url);
        }
    }

    fn external_signer_bridge(&self) -> Option<Arc<dyn ExternalSignerBridge>> {
        match self.external_signer_bridge.read() {
            Ok(slot) => slot.clone(),
            Err(poison) => poison.into_inner().clone(),
        }
    }

    fn bunker_signer_connector(&self) -> Arc<dyn BunkerSignerConnector> {
        match self.bunker_signer_connector.read() {
            Ok(slot) => slot.clone(),
            Err(poison) => poison.into_inner().clone(),
        }
    }

    fn external_signer_handshake_error(
        &self,
        kind: Option<ExternalSignerErrorKind>,
        detail: Option<String>,
    ) -> String {
        if let Some(msg) = user_visible_signer_error_kind(kind.clone()) {
            return msg.to_string();
        }
        let detail = detail.unwrap_or_default();
        let detail = detail.trim();
        if !detail.is_empty() {
            return detail.to_string();
        }
        "External signer login failed".to_string()
    }

    fn start_external_signer_session(
        &mut self,
        pubkey_raw: String,
        signer_package_raw: String,
        current_user_raw: String,
    ) -> anyhow::Result<()> {
        let signer_package = signer_package_raw.trim().to_string();
        if signer_package.is_empty() {
            anyhow::bail!("Missing signer package");
        }
        let pubkey = PublicKey::parse(pubkey_raw.trim())
            .map_err(|e| anyhow::anyhow!("Invalid signer pubkey: {e}"))?;
        let current_user = {
            let trimmed = current_user_raw.trim();
            if trimmed.is_empty() {
                pubkey.to_hex()
            } else {
                trimmed.to_string()
            }
        };

        let bridge = self
            .external_signer_bridge()
            .ok_or_else(|| anyhow::anyhow!("External signer bridge unavailable"))?;
        let signer = ExternalSignerBridgeSigner::new(
            pubkey,
            signer_package.clone(),
            current_user.clone(),
            bridge,
        );
        let mode = SessionAuthMode::ExternalSigner {
            signer_package,
            current_user,
        };
        self.start_session_with_signer(pubkey, Arc::new(signer), None, mode)
    }

    fn bunker_login_error_message(&self, err: &BunkerConnectError) -> String {
        if let Some(msg) = err.user_visible_message() {
            return msg.to_string();
        }

        let detail = err.message.trim();
        if !detail.is_empty() {
            return detail.to_string();
        }
        "Bunker login failed".to_string()
    }

    fn is_bunker_new_secret_rejection(err: &BunkerConnectError) -> bool {
        err.message.to_lowercase().contains("new secret")
    }

    fn open_external_url(&self, url: String) -> anyhow::Result<()> {
        let bridge = self
            .external_signer_bridge()
            .ok_or_else(|| anyhow::anyhow!("External signer bridge unavailable"))?;
        let result = bridge.open_url(url);
        if result.ok {
            return Ok(());
        }

        if let Some(msg) = user_visible_signer_error_kind(result.error_kind) {
            anyhow::bail!("{msg}");
        }

        let detail = result
            .error_message
            .unwrap_or_else(|| "Failed to open signer app".to_string());
        anyhow::bail!("{detail}");
    }

    fn make_nostr_connect_client_uri(
        &self,
        client_keys: &Keys,
        relays: &[RelayUrl],
        secret: &str,
    ) -> anyhow::Result<String> {
        if relays.is_empty() {
            anyhow::bail!("No relays configured for signer login");
        }
        let metadata = serde_json::json!({
            "name": "Pika",
            "url": "https://pikachat.org",
        })
        .to_string();
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("metadata", &metadata);
        query.append_pair("name", "Pika");
        query.append_pair("url", "https://pikachat.org");
        query.append_pair(
            "perms",
            "get_public_key,sign_event,nip44_encrypt,nip44_decrypt,nip04_encrypt,nip04_decrypt",
        );
        query.append_pair("secret", secret);
        for relay in relays {
            query.append_pair("relay", relay.as_str_without_trailing_slash());
        }
        let query = query.finish();
        Ok(format!(
            "nostrconnect://{}?{query}",
            client_keys.public_key().to_hex()
        ))
    }

    fn spawn_nostr_connect_connect_response_waiter(
        &self,
        client_keys: Keys,
        relays: Vec<RelayUrl>,
        secret: String,
        connect_response_result: Arc<Mutex<Option<Result<NostrConnectConnectResponse, String>>>>,
    ) {
        let core_sender = self.core_sender.clone();
        self.runtime.spawn(async move {
            let result =
                Self::wait_for_nostr_connect_connect_response(client_keys, relays, secret).await;
            let mapped = result.map_err(|e| e.to_string());
            match connect_response_result.lock() {
                Ok(mut slot) => {
                    *slot = Some(mapped);
                }
                Err(poison) => {
                    *poison.into_inner() = Some(mapped);
                }
            }
            let _ = core_sender.send(CoreMsg::Internal(Box::new(
                InternalEvent::NostrConnectConnectResponseReady,
            )));
        });
    }

    async fn wait_for_nostr_connect_connect_response(
        client_keys: Keys,
        relays: Vec<RelayUrl>,
        expected_secret: String,
    ) -> anyhow::Result<NostrConnectConnectResponse> {
        let client = Client::new(client_keys.clone());
        for relay in relays {
            client.add_relay(relay).await?;
        }
        client.connect().await;

        // Some signers omit `#p` on connect responses. Subscribe by kind and
        // rely on successful decryption + response validation to identify ours.
        let since_unix = now_seconds()
            .saturating_sub(NOSTR_CONNECT_RESPONSE_LOOKBACK_SECS as i64)
            .max(0) as u64;
        let filter = Filter::new()
            .kind(Kind::NostrConnect)
            .since(Timestamp::from(since_unix));
        client.subscribe(filter, None).await?;

        let mut notifications = client.notifications();
        let timeout_at =
            std::time::Instant::now() + Duration::from_secs(NOSTR_CONNECT_LOGIN_TIMEOUT_SECS);
        let outcome = loop {
            let now = std::time::Instant::now();
            if now >= timeout_at {
                break Err(anyhow::anyhow!("signer connect response timed out"));
            }
            let wait_for = timeout_at.saturating_duration_since(now);
            let notif = match tokio::time::timeout(wait_for, notifications.recv()).await {
                Ok(Ok(n)) => n,
                Ok(Err(_)) => break Err(anyhow::anyhow!("relay notifications closed")),
                Err(_) => break Err(anyhow::anyhow!("signer connect response timed out")),
            };

            let RelayPoolNotification::Event { event, .. } = notif else {
                continue;
            };
            if event.kind != Kind::NostrConnect {
                continue;
            }

            let decrypted = match nip44::decrypt(
                client_keys.secret_key(),
                &event.pubkey,
                event.content.as_str(),
            ) {
                Ok(v) => v,
                Err(nip44_err) => match nip04::decrypt(
                    client_keys.secret_key(),
                    &event.pubkey,
                    event.content.as_str(),
                ) {
                    Ok(v) => v,
                    Err(nip04_err) => {
                        tracing::debug!(
                            nip44_err = %nip44_err,
                            nip04_err = %nip04_err,
                            "nostr_connect: ignoring undecryptable connect response"
                        );
                        continue;
                    }
                },
            };
            let message = match NostrConnectMessage::from_json(decrypted) {
                Ok(msg) => msg,
                Err(e) => {
                    tracing::debug!(%e, "nostr_connect: ignoring unparsable connect response");
                    continue;
                }
            };
            match message {
                NostrConnectMessage::Request { id, method, params } => {
                    let request = match NostrConnectRequest::from_message(method, params) {
                        Ok(req) => req,
                        Err(e) => {
                            tracing::debug!(%e, "nostr_connect: ignoring invalid connect request");
                            continue;
                        }
                    };

                    let NostrConnectRequest::Connect {
                        remote_signer_public_key,
                        secret,
                    } = request
                    else {
                        continue;
                    };

                    if remote_signer_public_key != event.pubkey {
                        tracing::debug!(
                            requested_signer = %remote_signer_public_key.to_hex(),
                            event_signer = %event.pubkey.to_hex(),
                            "nostr_connect: ignoring connect request with mismatched signer pubkey"
                        );
                        continue;
                    }

                    let agreed_secret = match secret.as_deref() {
                        Some(signer_secret) if signer_secret == expected_secret => {
                            expected_secret.clone()
                        }
                        Some(signer_secret) => {
                            if let Some(override_secret) =
                                Self::normalize_nostr_connect_secret(signer_secret)
                            {
                                tracing::warn!(
                                    "nostr_connect: signer provided connect secret in request; adopting signer-provided secret"
                                );
                                override_secret
                            } else {
                                tracing::warn!(
                                    "nostr_connect: signer connect request had invalid secret; proceeding with local secret"
                                );
                                expected_secret.clone()
                            }
                        }
                        None => expected_secret.clone(),
                    };

                    // NIP-46 client-initiated flow: acknowledge signer `connect` request.
                    let ack_message = NostrConnectMessage::response(
                        id,
                        NostrConnectResponse::with_result(ResponseResult::Ack),
                    );
                    let ack_event = match EventBuilder::nostr_connect(
                        &client_keys,
                        event.pubkey,
                        ack_message,
                    ) {
                        Ok(builder) => match builder.sign_with_keys(&client_keys) {
                            Ok(event) => Some(event),
                            Err(e) => {
                                tracing::warn!(
                                    %e,
                                    "nostr_connect: failed to sign connect ack event"
                                );
                                None
                            }
                        },
                        Err(e) => {
                            tracing::warn!(%e, "nostr_connect: failed to build connect ack event");
                            None
                        }
                    };

                    if let Some(event) = ack_event {
                        if let Err(e) = client.send_event(&event).await {
                            tracing::warn!(%e, "nostr_connect: failed to send connect ack event");
                        }
                    }

                    break Ok(NostrConnectConnectResponse {
                        remote_signer_pubkey: event.pubkey,
                        agreed_secret,
                    });
                }
                NostrConnectMessage::Response { result, error, .. } => {
                    if let Some(err) = error {
                        break Err(anyhow::anyhow!("signer rejected connection: {err}"));
                    }
                    let Some(result_value) = result else {
                        continue;
                    };

                    let agreed_secret = if result_value == expected_secret
                        || result_value.eq_ignore_ascii_case("ack")
                        || result_value.eq_ignore_ascii_case("ok")
                        || result_value.eq_ignore_ascii_case("success")
                    {
                        expected_secret.clone()
                    } else if let Some(override_secret) =
                        Self::parse_nostr_connect_secret_override(&result_value)
                    {
                        tracing::warn!(
                            "nostr_connect: signer returned a different connect secret; adopting signer-provided secret"
                        );
                        override_secret
                    } else if !result_value.trim().is_empty() {
                        tracing::warn!(
                            result = %result_value,
                            "nostr_connect: non-standard connect response; proceeding with local secret for interop"
                        );
                        expected_secret.clone()
                    } else {
                        tracing::debug!("nostr_connect: ignoring empty connect response result");
                        continue;
                    };

                    break Ok(NostrConnectConnectResponse {
                        remote_signer_pubkey: event.pubkey,
                        agreed_secret,
                    });
                }
            }
        };

        client.shutdown().await;
        outcome
    }

    fn parse_remote_signer_from_callback(&self, callback_url: &str) -> Option<PublicKey> {
        let parsed = Url::parse(callback_url).ok()?;
        let candidate = parsed.query_pairs().find_map(|(k, v)| {
            if k.eq_ignore_ascii_case("remote_signer_pubkey")
                || k.eq_ignore_ascii_case("remote_signer")
                || k.eq_ignore_ascii_case("signer_pubkey")
            {
                Some(v.to_string())
            } else {
                None
            }
        })?;
        PublicKey::from_hex(&candidate).ok()
    }

    fn make_bunker_uri(
        &self,
        remote_signer_pubkey: PublicKey,
        relays: &[RelayUrl],
        secret: &str,
    ) -> String {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        for relay in relays {
            query.append_pair("relay", relay.as_str_without_trailing_slash());
        }

        if let Some(normalized_secret) = Self::normalize_nostr_connect_secret(secret) {
            query.append_pair("secret", &normalized_secret);
        }

        let query = query.finish();
        if query.is_empty() {
            format!("bunker://{}", remote_signer_pubkey.to_hex())
        } else {
            format!("bunker://{}?{query}", remote_signer_pubkey.to_hex())
        }
    }

    fn bunker_uri_without_secret(raw: &str) -> Option<String> {
        let mut url = Url::parse(raw).ok()?;
        let mut removed_secret = false;
        let mut kept_pairs: Vec<(String, String)> = Vec::new();

        for (k, v) in url.query_pairs() {
            if k.eq_ignore_ascii_case("secret") {
                removed_secret = true;
                continue;
            }
            kept_pairs.push((k.into_owned(), v.into_owned()));
        }

        if !removed_secret {
            return None;
        }

        if kept_pairs.is_empty() {
            url.set_query(None);
            return Some(url.to_string());
        }

        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in kept_pairs {
            serializer.append_pair(&k, &v);
        }
        url.set_query(Some(&serializer.finish()));
        Some(url.to_string())
    }

    fn nostr_connect_redacted_uri_for_log(client_uri: &str) -> String {
        match Url::parse(client_uri) {
            Ok(url) => {
                if let Some(host) = url.host_str() {
                    format!("{}://{}?<redacted>", url.scheme(), host)
                } else {
                    format!("{}://<redacted>", url.scheme())
                }
            }
            Err(_) => "nostrconnect://<redacted>".to_string(),
        }
    }

    fn wait_for_pending_nostr_connect_signer(
        &self,
        pending: &mut PendingNostrConnectLogin,
        callback_url: Option<&str>,
    ) -> anyhow::Result<Option<NostrConnectConnectResponse>> {
        let callback_signer_hint =
            callback_url.and_then(|url| self.parse_remote_signer_from_callback(url));

        let next = match pending.connect_response_result.lock() {
            Ok(mut slot) => slot.take(),
            Err(poison) => poison.into_inner().take(),
        };

        match next {
            Some(Ok(connect_response)) => {
                if let Some(callback_signer) = callback_signer_hint {
                    if callback_signer != connect_response.remote_signer_pubkey {
                        anyhow::bail!("Signer callback pubkey did not match connect response");
                    }
                }
                Ok(Some(connect_response))
            }
            Some(Err(e)) => Err(anyhow::anyhow!("{e}")),
            None => Ok(None),
        }
    }

    fn maybe_write_nostr_connect_debug_snapshot(
        &self,
        client_uri: &str,
        client_nsec: &str,
        secret: &str,
        relays: &[RelayUrl],
    ) {
        let enabled = matches!(
            std::env::var("PIKA_NOSTR_CONNECT_DEBUG_DUMP")
                .ok()
                .as_deref(),
            Some("1") | Some("true") | Some("TRUE")
        );
        if !enabled {
            return;
        }

        let path = std::path::Path::new(&self.data_dir).join("nostr_connect_debug.json");
        let payload = serde_json::json!({
            "generated_at_unix": now_seconds(),
            "client_uri": client_uri,
            "client_nsec": client_nsec,
            "secret": secret,
            "relay_urls": relays.iter().map(|r| r.as_str_without_trailing_slash().to_string()).collect::<Vec<_>>(),
        });

        match serde_json::to_string_pretty(&payload) {
            Ok(json) => {
                if let Err(e) = Self::write_private_file(&path, json.as_bytes()) {
                    tracing::warn!(%e, path = %path.display(), "nostr_connect: failed to write debug snapshot");
                } else {
                    tracing::info!(path = %path.display(), "nostr_connect: wrote debug snapshot");
                }
            }
            Err(e) => {
                tracing::warn!(%e, "nostr_connect: failed to serialize debug snapshot");
            }
        }
    }

    fn make_nostr_connect_secret() -> String {
        // Keep this simple/alphanumeric for broad signer compatibility.
        // 32 hex chars preserves entropy while avoiding punctuation.
        uuid::Uuid::new_v4().simple().to_string()
    }

    fn normalize_nostr_connect_secret(raw: &str) -> Option<String> {
        let candidate = raw.trim();
        if candidate.is_empty() {
            return None;
        }

        if candidate.len() > 256 {
            return None;
        }

        if candidate
            .chars()
            .any(|ch| ch.is_ascii_control() || ch.is_whitespace())
        {
            return None;
        }

        Some(candidate.to_string())
    }

    fn parse_nostr_connect_secret_override(result_value: &str) -> Option<String> {
        let candidate = result_value.trim();
        if candidate.eq_ignore_ascii_case("ack")
            || candidate.eq_ignore_ascii_case("ok")
            || candidate.eq_ignore_ascii_case("success")
        {
            return None;
        }

        // Interop: some signers return their currently bound app secret instead
        // of echoing ours when reconnecting.
        Self::normalize_nostr_connect_secret(candidate)
    }

    fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }

        std::fs::write(path, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    fn nostr_connect_client_nsec_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.data_dir).join("nostr_connect_client_nsec.txt")
    }

    fn nostr_connect_secret_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.data_dir).join("nostr_connect_secret.txt")
    }

    fn nostr_connect_pending_login_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.data_dir).join("nostr_connect_pending_login.json")
    }

    fn nostr_connect_scoped_keyring_account(&self, account: &str) -> String {
        let canonical_dir = std::path::Path::new(&self.data_dir)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(&self.data_dir));
        let digest = <nostr_sdk::hashes::sha256::Hash as nostr_sdk::hashes::Hash>::hash(
            canonical_dir.to_string_lossy().as_bytes(),
        );
        format!("{account}.{digest}")
    }

    fn nostr_connect_keyring_entry(&self, account: &str) -> Option<keyring_core::Entry> {
        if let Err(e) = crate::mdk_support::init_keyring_once(&self.keychain_group) {
            tracing::warn!(%e, "nostr_connect: keyring init failed");
            return None;
        }
        let scoped_account = self.nostr_connect_scoped_keyring_account(account);
        match keyring_core::Entry::new(crate::mdk_support::SERVICE_ID, &scoped_account) {
            Ok(entry) => Some(entry),
            Err(e) => {
                tracing::warn!(%e, account, scoped_account, "nostr_connect: keyring entry unavailable");
                None
            }
        }
    }

    fn get_nostr_connect_keyring_value(&self, account: &str) -> Option<String> {
        let entry = self.nostr_connect_keyring_entry(account)?;
        match entry.get_password() {
            Ok(v) => Some(v),
            Err(keyring_core::Error::NoEntry) => None,
            Err(e) => {
                tracing::warn!(%e, account, "nostr_connect: keyring read failed");
                None
            }
        }
    }

    fn set_nostr_connect_keyring_value(&self, account: &str, value: &str) -> bool {
        let Some(entry) = self.nostr_connect_keyring_entry(account) else {
            return false;
        };
        match entry.set_password(value) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(%e, account, "nostr_connect: keyring write failed");
                false
            }
        }
    }

    fn clear_nostr_connect_keyring_value(&self, account: &str) {
        let Some(entry) = self.nostr_connect_keyring_entry(account) else {
            return;
        };
        match entry.delete_credential() {
            Ok(()) => {}
            Err(keyring_core::Error::NoEntry) => {}
            Err(e) => {
                tracing::warn!(%e, account, "nostr_connect: keyring delete failed");
            }
        }
    }

    fn load_nostr_connect_pairing_from_keyring(&self) -> Option<(Keys, String)> {
        let raw = self.get_nostr_connect_keyring_value(NOSTR_CONNECT_PAIRING_KEYRING_ACCOUNT)?;
        let parsed = serde_json::from_str::<PersistedNostrConnectPairing>(&raw).ok()?;
        let keys = Keys::parse(parsed.client_nsec.trim()).ok()?;
        let secret = Self::normalize_nostr_connect_secret(&parsed.secret)?;
        Some((keys, secret))
    }

    fn load_nostr_connect_pairing_from_files(&self) -> Option<(Keys, String)> {
        let existing_keys = std::fs::read_to_string(self.nostr_connect_client_nsec_path())
            .ok()
            .and_then(|raw| Keys::parse(raw.trim()).ok());
        let existing_secret = std::fs::read_to_string(self.nostr_connect_secret_path())
            .ok()
            .and_then(|raw| Self::normalize_nostr_connect_secret(&raw));
        match (existing_keys, existing_secret) {
            (Some(keys), Some(secret)) => Some((keys, secret)),
            _ => None,
        }
    }

    fn persist_nostr_connect_pairing_to_keyring(&self, client_keys: &Keys, secret: &str) -> bool {
        let Some(secret) = Self::normalize_nostr_connect_secret(secret) else {
            return false;
        };
        let payload = PersistedNostrConnectPairing {
            client_nsec: client_keys.secret_key().to_bech32().expect("infallible"),
            secret,
        };
        let json = match serde_json::to_string(&payload) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "nostr_connect: pairing serialization failed");
                return false;
            }
        };
        self.set_nostr_connect_keyring_value(NOSTR_CONNECT_PAIRING_KEYRING_ACCOUNT, &json)
    }

    fn remove_nostr_connect_pairing_files(&self) {
        for path in [
            self.nostr_connect_client_nsec_path(),
            self.nostr_connect_secret_path(),
        ] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(
                        %e,
                        path = %path.display(),
                        "nostr_connect: failed to remove pairing file"
                    );
                }
            }
        }
    }

    fn load_or_create_nostr_connect_pairing(&self) -> (Keys, String) {
        if let Some(pairing) = self.load_nostr_connect_pairing_from_keyring() {
            return pairing;
        }
        if let Some(pairing) = self.load_nostr_connect_pairing_from_files() {
            return pairing;
        }

        let keys = Keys::generate();
        let secret = Self::make_nostr_connect_secret();
        self.maybe_persist_nostr_connect_pairing(&keys, &secret);
        (keys, secret)
    }

    fn maybe_persist_nostr_connect_pairing(&self, client_keys: &Keys, secret: &str) {
        let Some(secret) = Self::normalize_nostr_connect_secret(secret) else {
            return;
        };

        if self.persist_nostr_connect_pairing_to_keyring(client_keys, &secret) {
            self.remove_nostr_connect_pairing_files();
            return;
        }

        let client_nsec = client_keys.secret_key().to_bech32().expect("infallible");
        let client_nsec_path = self.nostr_connect_client_nsec_path();
        if let Err(e) = Self::write_private_file(&client_nsec_path, client_nsec.as_bytes()) {
            tracing::warn!(
                %e,
                path = %client_nsec_path.display(),
                "nostr_connect: failed to persist client nsec"
            );
        }

        let path = self.nostr_connect_secret_path();
        if let Err(e) = Self::write_private_file(&path, secret.as_bytes()) {
            tracing::warn!(%e, path = %path.display(), "nostr_connect: failed to persist client secret");
        }
    }

    fn clear_nostr_connect_pairing_data(&self) {
        self.clear_nostr_connect_keyring_value(NOSTR_CONNECT_PAIRING_KEYRING_ACCOUNT);
        self.remove_nostr_connect_pairing_files();
    }

    fn load_pending_nostr_connect_login_snapshot(
        &self,
    ) -> Option<PersistedPendingNostrConnectLogin> {
        if let Some(raw) =
            self.get_nostr_connect_keyring_value(NOSTR_CONNECT_PENDING_KEYRING_ACCOUNT)
        {
            if let Ok(snapshot) = serde_json::from_str::<PersistedPendingNostrConnectLogin>(&raw) {
                return Some(snapshot);
            }
        }

        let raw = std::fs::read_to_string(self.nostr_connect_pending_login_path()).ok()?;
        serde_json::from_str::<PersistedPendingNostrConnectLogin>(&raw).ok()
    }

    fn persist_pending_nostr_connect_login_snapshot(&self, pending: &PendingNostrConnectLogin) {
        let payload = PersistedPendingNostrConnectLogin {
            started_at_unix: pending.started_at_unix,
            client_nsec: pending.client_nsec.clone(),
            relays: pending
                .relays
                .iter()
                .map(|relay| relay.as_str_without_trailing_slash().to_string())
                .collect(),
            secret: pending.secret.clone(),
            callback_received: pending.callback_received,
        };
        let path = self.nostr_connect_pending_login_path();
        let bytes = match serde_json::to_vec(&payload) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "nostr_connect: failed to serialize pending login snapshot");
                return;
            }
        };

        if let Ok(json) = String::from_utf8(bytes.clone()) {
            if self.set_nostr_connect_keyring_value(NOSTR_CONNECT_PENDING_KEYRING_ACCOUNT, &json) {
                self.clear_pending_nostr_connect_login_file_snapshot();
                return;
            }
        }

        if let Err(e) = Self::write_private_file(&path, &bytes) {
            tracing::warn!(
                %e,
                path = %path.display(),
                "nostr_connect: failed to persist pending login snapshot"
            );
        }
    }

    fn clear_pending_nostr_connect_login_file_snapshot(&self) {
        let path = self.nostr_connect_pending_login_path();
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(
                    %e,
                    path = %path.display(),
                    "nostr_connect: failed to remove pending login snapshot"
                );
            }
        }
    }

    fn clear_pending_nostr_connect_login_snapshot(&self) {
        self.clear_nostr_connect_keyring_value(NOSTR_CONNECT_PENDING_KEYRING_ACCOUNT);
        self.clear_pending_nostr_connect_login_file_snapshot();
    }

    fn spawn_nostr_connect_timeout(&self, attempt_id: u64, delay: Duration) {
        let core_sender = self.core_sender.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = core_sender.send(CoreMsg::Internal(Box::new(
                InternalEvent::NostrConnectTimeout { attempt_id },
            )));
        });
    }

    fn start_pending_nostr_connect_login(
        &mut self,
        client_keys: Keys,
        relays: Vec<RelayUrl>,
        secret: String,
        started_at_unix: i64,
        callback_received: bool,
    ) {
        self.maybe_persist_nostr_connect_pairing(&client_keys, &secret);
        let connect_response_result = Arc::new(Mutex::new(
            None::<Result<NostrConnectConnectResponse, String>>,
        ));
        self.spawn_nostr_connect_connect_response_waiter(
            client_keys.clone(),
            relays.clone(),
            secret.clone(),
            connect_response_result.clone(),
        );

        let attempt_id = self.next_nostr_connect_attempt_id();
        let pending = PendingNostrConnectLogin {
            attempt_id,
            started_at_unix,
            client_nsec: client_keys.secret_key().to_bech32().expect("infallible"),
            relays,
            secret,
            callback_received,
            connect_response_result,
        };
        self.persist_pending_nostr_connect_login_snapshot(&pending);

        let elapsed_secs = now_seconds().saturating_sub(started_at_unix).max(0) as u64;
        let remaining_secs = NOSTR_CONNECT_LOGIN_TIMEOUT_SECS.saturating_sub(elapsed_secs);
        let timeout_delay = if remaining_secs == 0 {
            Duration::from_millis(1)
        } else {
            Duration::from_secs(remaining_secs)
        };
        self.spawn_nostr_connect_timeout(attempt_id, timeout_delay);
        self.pending_nostr_connect_login = Some(pending);
    }

    fn clear_pending_nostr_connect_login(&mut self) {
        self.pending_nostr_connect_login = None;
        self.clear_pending_nostr_connect_login_snapshot();
    }

    fn resume_pending_nostr_connect_login_from_disk(&mut self) {
        let Some(pending) = self.load_pending_nostr_connect_login_snapshot() else {
            return;
        };

        let Some(secret) = Self::normalize_nostr_connect_secret(&pending.secret) else {
            tracing::warn!("nostr_connect: pending login snapshot missing valid secret; removing");
            self.clear_pending_nostr_connect_login_snapshot();
            return;
        };

        let client_keys = match Keys::parse(pending.client_nsec.trim()) {
            Ok(keys) => keys,
            Err(e) => {
                tracing::warn!(
                    %e,
                    "nostr_connect: pending login snapshot has invalid client key; removing"
                );
                self.clear_pending_nostr_connect_login_snapshot();
                return;
            }
        };

        let relays: Vec<RelayUrl> = pending
            .relays
            .iter()
            .filter_map(|raw_url| RelayUrl::parse(raw_url).ok())
            .collect();
        if relays.is_empty() {
            tracing::warn!("nostr_connect: pending login snapshot has no valid relays; removing");
            self.clear_pending_nostr_connect_login_snapshot();
            return;
        }

        let elapsed_secs = now_seconds().saturating_sub(pending.started_at_unix).max(0) as u64;
        if elapsed_secs >= NOSTR_CONNECT_LOGIN_TIMEOUT_SECS {
            tracing::info!("nostr_connect: dropping stale pending login snapshot");
            self.clear_pending_nostr_connect_login_snapshot();
            return;
        }

        tracing::info!("nostr_connect: resuming pending login from disk");
        self.set_busy(|b| {
            b.logging_in = true;
            b.creating_account = false;
        });
        self.start_pending_nostr_connect_login(
            client_keys,
            relays,
            secret,
            pending.started_at_unix,
            pending.callback_received,
        );
    }

    fn start_bunker_signer_session(
        &mut self,
        bunker_uri_raw: String,
        client_keys: Keys,
    ) -> anyhow::Result<()> {
        let bunker_uri = bunker_uri_raw.trim();
        if bunker_uri.is_empty() {
            anyhow::bail!("Invalid bunker URI");
        }
        tracing::info!("nostr_connect: connecting to bunker");
        let client_nsec = client_keys.secret_key().to_bech32().expect("infallible");
        let connector = self.bunker_signer_connector();
        let connect_once = |uri: &str| connector.connect(&self.runtime, uri, client_keys.clone());
        let output = match connect_once(bunker_uri) {
            Ok(output) => output,
            Err(primary_error) => {
                let primary_msg = self.bunker_login_error_message(&primary_error);
                let should_retry_without_secret =
                    Self::is_bunker_new_secret_rejection(&primary_error);
                if !should_retry_without_secret {
                    tracing::error!(%primary_msg, "nostr_connect: bunker connect failed");
                    return Err(anyhow::anyhow!(primary_msg));
                }

                let Some(uri_without_secret) = Self::bunker_uri_without_secret(bunker_uri) else {
                    tracing::error!(
                        %primary_msg,
                        "nostr_connect: bunker connect failed (no secret parameter to drop)"
                    );
                    return Err(anyhow::anyhow!(primary_msg));
                };

                tracing::warn!(
                    "nostr_connect: bunker connect rejected new secret; retrying without secret query parameter"
                );
                match connect_once(&uri_without_secret) {
                    Ok(output) => output,
                    Err(retry_error) => {
                        let retry_msg = self.bunker_login_error_message(&retry_error);
                        tracing::error!(
                            %retry_msg,
                            "nostr_connect: bunker connect retry without secret failed"
                        );
                        return Err(anyhow::anyhow!(retry_msg));
                    }
                }
            }
        };
        tracing::info!(user_pubkey = %output.user_pubkey.to_hex(), "nostr_connect: bunker connected, starting session");
        let mode = SessionAuthMode::BunkerSigner {
            bunker_uri: output.canonical_bunker_uri.clone(),
        };
        self.start_session_with_signer(output.user_pubkey, output.signer, None, mode)?;
        self.emit_bunker_session_descriptor(output.canonical_bunker_uri, client_nsec);
        Ok(())
    }

    fn begin_bunker_login(&mut self, bunker_uri: String) {
        if !self.external_signer_enabled() {
            self.toast("External signer is disabled");
            return;
        }

        self.set_busy(|b| {
            b.logging_in = true;
            b.creating_account = false;
        });

        if let Err(e) = self.start_bunker_signer_session(bunker_uri, Keys::generate()) {
            self.clear_busy();
            self.toast(format!("{e:#}"));
            return;
        }

        self.clear_busy();
    }

    fn begin_nostr_connect_login(&mut self) {
        self.clear_pending_nostr_connect_login();
        tracing::info!("nostr_connect: begin_nostr_connect_login");

        if !self.external_signer_enabled() {
            tracing::warn!("nostr_connect: external signer disabled");
            self.toast("External signer is disabled");
            return;
        }

        self.set_busy(|b| {
            b.logging_in = true;
            b.creating_account = false;
        });

        let (client_keys, secret) = self.load_or_create_nostr_connect_pairing();
        let relays = self.default_relays();
        let client_uri = match self.make_nostr_connect_client_uri(&client_keys, &relays, &secret) {
            Ok(uri) => uri,
            Err(e) => {
                tracing::error!(%e, "nostr_connect: make_client_uri failed");
                self.clear_busy();
                self.toast(format!("{e:#}"));
                return;
            }
        };

        tracing::info!(
            uri = %Self::nostr_connect_redacted_uri_for_log(&client_uri),
            "nostr_connect: opening external URL"
        );
        self.maybe_write_nostr_connect_debug_snapshot(
            &client_uri,
            &client_keys.secret_key().to_bech32().expect("infallible"),
            &secret,
            &relays,
        );
        // Persist/arm pending state before app switch so callback can always resume,
        // even if iOS suspends us immediately after opening Primal.
        self.start_pending_nostr_connect_login(client_keys, relays, secret, now_seconds(), false);
        if let Err(e) = self.open_external_url(client_uri.clone()) {
            tracing::error!(%e, "nostr_connect: open_external_url failed");
            self.clear_pending_nostr_connect_login();
            self.clear_busy();
            self.toast(format!("{e:#}"));
            return;
        }
        tracing::info!("nostr_connect: external URL opened, waiting for callback");
    }

    fn progress_pending_nostr_connect_login(
        &mut self,
        callback_url: Option<&str>,
        mark_callback_received: bool,
        trigger: &'static str,
    ) {
        let Some(mut pending) = self.pending_nostr_connect_login.take() else {
            tracing::warn!(
                trigger,
                "nostr_connect: callback/continue but no pending login, ignoring"
            );
            return;
        };
        if mark_callback_received {
            pending.callback_received = true;
        } else if !pending.callback_received {
            tracing::info!(
                trigger,
                "nostr_connect: pending login resumed before callback; still waiting for callback"
            );
            self.persist_pending_nostr_connect_login_snapshot(&pending);
            self.pending_nostr_connect_login = Some(pending);
            return;
        }
        let client_keys = match Keys::parse(&pending.client_nsec) {
            Ok(keys) => keys,
            Err(e) => {
                tracing::error!(%e, "nostr_connect: invalid client key");
                self.clear_pending_nostr_connect_login_snapshot();
                self.clear_busy();
                self.toast(format!("Invalid bunker client key: {e}"));
                return;
            }
        };

        let connect_response =
            match self.wait_for_pending_nostr_connect_signer(&mut pending, callback_url) {
                Ok(Some(connect_response)) => connect_response,
                Ok(None) => {
                    // Foreground retry path: response not received yet.
                    tracing::info!(
                        trigger,
                        "nostr_connect: signer response not ready yet; still waiting"
                    );
                    self.persist_pending_nostr_connect_login_snapshot(&pending);
                    self.pending_nostr_connect_login = Some(pending);
                    return;
                }
                Err(e) => {
                    tracing::error!(%e, "nostr_connect: connect response validation failed");
                    self.clear_pending_nostr_connect_login_snapshot();
                    self.clear_busy();
                    self.toast(format!("{e:#}"));
                    return;
                }
            };
        self.clear_pending_nostr_connect_login_snapshot();
        self.maybe_persist_nostr_connect_pairing(&client_keys, &connect_response.agreed_secret);
        let bunker_uri = self.make_bunker_uri(
            connect_response.remote_signer_pubkey,
            &pending.relays,
            &connect_response.agreed_secret,
        );

        tracing::info!("nostr_connect: starting bunker signer session");
        if let Err(e) = self.start_bunker_signer_session(bunker_uri, client_keys) {
            tracing::error!(%e, "nostr_connect: bunker session failed");
            self.clear_busy();
            self.toast(format!("{e:#}"));
            return;
        }

        tracing::info!("nostr_connect: login complete");
        self.clear_busy();
    }

    fn on_nostr_connect_callback(&mut self, url: String) {
        tracing::info!(%url, pending = self.pending_nostr_connect_login.is_some(), "nostr_connect: callback received");
        self.progress_pending_nostr_connect_login(Some(url.as_str()), true, "callback");
    }

    fn continue_pending_nostr_connect_login(&mut self) {
        if self.pending_nostr_connect_login.is_some() {
            tracing::info!("nostr_connect: foregrounded with pending login, continuing");
            self.progress_pending_nostr_connect_login(None, false, "foreground");
        }
    }

    fn reset_nostr_connect_pairing(&mut self) {
        self.clear_pending_nostr_connect_login();
        self.clear_nostr_connect_pairing_data();
        self.clear_busy();
        self.toast("Nostr Connect pairing reset");
    }

    fn restore_bunker_session(&mut self, bunker_uri: String, client_nsec: String) {
        self.set_busy(|b| {
            b.logging_in = true;
            b.creating_account = false;
        });

        if !self.external_signer_enabled() {
            self.clear_busy();
            self.toast("External signer is disabled");
            return;
        }

        let keys = match Keys::parse(client_nsec.trim()) {
            Ok(keys) => keys,
            Err(e) => {
                self.clear_busy();
                self.toast(format!("Invalid bunker client key: {e}"));
                return;
            }
        };

        if let Err(e) = self.start_bunker_signer_session(bunker_uri, keys) {
            self.clear_busy();
            self.toast(format!("Login failed: {e:#}"));
            return;
        }

        self.clear_busy();
    }

    fn begin_external_signer_login(&mut self, current_user_hint: Option<String>) {
        if !self.external_signer_enabled() {
            self.toast("External signer is disabled");
            return;
        }
        let Some(bridge) = self.external_signer_bridge() else {
            self.toast("External signer bridge unavailable");
            return;
        };

        self.set_busy(|b| {
            b.logging_in = true;
            b.creating_account = false;
        });

        let hint = current_user_hint.and_then(|v| {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });

        let ExternalSignerHandshakeResult {
            ok,
            pubkey,
            signer_package,
            current_user,
            error_kind,
            error_message,
        } = bridge.request_public_key(hint);

        if !ok {
            self.clear_busy();
            self.toast(self.external_signer_handshake_error(error_kind, error_message));
            return;
        }

        let pubkey = pubkey.unwrap_or_default();
        let signer_package = signer_package.unwrap_or_default();
        let current_user = current_user.unwrap_or_else(|| pubkey.clone());
        if pubkey.trim().is_empty() || signer_package.trim().is_empty() {
            self.clear_busy();
            self.toast("External signer returned an invalid response");
            return;
        }

        if let Err(e) = self.start_external_signer_session(pubkey, signer_package, current_user) {
            self.clear_busy();
            self.toast(format!("{e:#}"));
            return;
        }

        self.clear_busy();
    }

    fn push_screen(&mut self, screen: Screen) {
        self.state.router.screen_stack.push(screen);
    }

    fn open_chat_screen(&mut self, chat_id: &str) {
        // UX: creating a chat from "NewChat" or "NewGroupChat" should land you in the chat,
        // with back returning to the chat list (not back to the compose screen).
        if matches!(
            self.state.router.screen_stack.last(),
            Some(Screen::NewChat) | Some(Screen::NewGroupChat)
        ) {
            self.state.router.screen_stack.pop();
        }

        let screen = Screen::Chat {
            chat_id: chat_id.to_string(),
        };
        if self.state.router.screen_stack.last() != Some(&screen) {
            self.push_screen(screen);
        }
    }

    fn handle_auth_transition(&mut self, logged_in: bool) {
        if logged_in {
            self.call_runtime.stop_all();
            self.state.router.default_screen = Screen::ChatList;
            self.state.router.screen_stack.clear();
            self.state.active_call = None;
            self.call_session_params = None;
            self.emit_router();
        } else {
            self.call_runtime.stop_all();
            self.state.router.default_screen = Screen::Login;
            self.state.router.screen_stack.clear();
            self.state.current_chat = None;
            self.state.active_call = None;
            self.state.call_timeline = vec![];
            self.state.chat_list = vec![];
            self.state.busy = BusyState::idle();
            self.loaded_count.clear();
            self.unread_counts.clear();
            self.delivery_overrides.clear();
            self.pending_sends.clear();
            self.pending_media_sends.clear();
            self.pending_media_downloads.clear();
            self.local_outbox.clear();
            self.profiles.clear();
            if let Some(conn) = self.profile_db.as_ref() {
                profile_db::clear_all(conn);
            }
            profile_pics::clear_cache(&self.data_dir);
            self.state.my_profile = MyProfileState::empty();
            self.state.follow_list = vec![];
            self.state.peer_profile = None;
            self.call_session_params = None;
            self.call_timeline_logged_keys.clear();
            self.save_call_timeline();
            self.last_outgoing_ts = 0;
            self.emit_router();
            self.emit_busy();
            self.emit_chat_list();
            self.emit_current_chat();
            self.emit_call_state();
        }
    }

    fn wipe_local_data(&mut self) {
        // Drop SQLite handles before deleting files.
        self.profile_db = None;
        self.archived_chats.clear();
        self.push_subscribed_chat_ids.clear();
        self.push_apns_token = None;
        self.state.toast = None;

        let root = std::path::Path::new(&self.data_dir);
        match std::fs::read_dir(root) {
            Ok(entries) => {
                for entry in entries {
                    let Ok(entry) = entry else {
                        continue;
                    };
                    let path = entry.path();
                    if entry.file_name() == OsStr::new(IOS_MIGRATION_SENTINEL) {
                        // Keep iOS migration sentinel so next launch won't pull legacy data
                        // from the old non-app-group container.
                        continue;
                    }
                    let file_type = match entry.file_type() {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(%e, path = %path.display(), "wipe: failed to read file type");
                            continue;
                        }
                    };
                    let res = if file_type.is_dir() {
                        std::fs::remove_dir_all(&path)
                    } else {
                        std::fs::remove_file(&path)
                    };
                    if let Err(e) = res {
                        tracing::warn!(%e, path = %path.display(), "wipe: failed to delete path");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(%e, path = %root.display(), "wipe: failed to enumerate data dir");
            }
        }

        if let Err(e) = std::fs::create_dir_all(root) {
            tracing::warn!(%e, path = %root.display(), "wipe: failed to recreate data dir");
        }

        // Wipe removed config files; reset in-memory config for same-process logins.
        self.config = config::load_app_config(&self.data_dir);

        profile_pics::ensure_dir(&self.data_dir);
        self.profile_db = match profile_db::open_profile_db(&self.data_dir) {
            Ok(conn) => Some(conn),
            Err(e) => {
                tracing::warn!(%e, "wipe: failed to reopen profile cache db");
                None
            }
        };
        self.push_device_id = Self::load_or_create_push_device_id(&self.data_dir);
    }

    fn set_busy(&mut self, f: impl FnOnce(&mut BusyState)) {
        let mut next = self.state.busy.clone();
        f(&mut next);
        if next != self.state.busy {
            self.state.busy = next;
            self.emit_busy();
        }
    }

    fn clear_busy(&mut self) {
        self.set_busy(|b| *b = BusyState::idle());
    }

    fn sync_current_chat_to_router(&mut self) {
        let top = self.state.router.screen_stack.last().cloned();
        match top {
            Some(Screen::Chat { chat_id }) | Some(Screen::GroupInfo { chat_id }) => {
                let needs_refresh = self
                    .state
                    .current_chat
                    .as_ref()
                    .map(|c| c.chat_id != chat_id)
                    .unwrap_or(true);
                if needs_refresh {
                    self.unread_counts.insert(chat_id.clone(), 0);
                    self.refresh_chat_list_from_storage();
                    self.refresh_current_chat(&chat_id);
                }
            }
            _ => {
                if self.state.current_chat.is_some() {
                    self.state.current_chat = None;
                    self.emit_current_chat();
                }
            }
        }
    }

    pub fn handle_message(&mut self, msg: CoreMsg) {
        match msg {
            CoreMsg::Action(ref action) => {
                // Never log `?action` directly: it can contain secrets (e.g. `nsec`).
                tracing::info!(action = action.tag(), "dispatch");
                self.handle_action(action.clone());
            }
            CoreMsg::Internal(internal) => self.handle_internal(*internal),
        }
    }

    fn handle_internal(&mut self, internal: InternalEvent) {
        match internal {
            InternalEvent::SubscriptionsRecomputed {
                token,
                giftwrap_sub,
                group_sub,
            } => {
                // Ignore stale results (e.g., logout/login during recompute).
                if token != self.subs_recompute_token {
                    return;
                }

                self.subs_recompute_in_flight = false;
                if let Some(sess) = self.session.as_mut() {
                    sess.giftwrap_sub = giftwrap_sub;
                    sess.group_sub = group_sub;
                }

                if self.subs_recompute_dirty {
                    self.subs_recompute_dirty = false;
                    self.recompute_subscriptions();
                }
            }
            InternalEvent::Toast(ref msg) => {
                tracing::info!(msg, "toast");
                self.toast(msg.clone());
            }
            InternalEvent::NostrConnectConnectResponseReady => {
                if self.pending_nostr_connect_login.is_some() {
                    tracing::info!(
                        "nostr_connect: connect response ready, continuing pending login"
                    );
                    self.progress_pending_nostr_connect_login(
                        None,
                        false,
                        "connect-response-ready",
                    );
                }
            }
            InternalEvent::NostrConnectTimeout { attempt_id } => {
                let pending_attempt = self
                    .pending_nostr_connect_login
                    .as_ref()
                    .map(|pending| pending.attempt_id);
                if pending_attempt != Some(attempt_id) {
                    return;
                }
                tracing::warn!(attempt_id, "nostr_connect: pending login timed out");
                self.clear_pending_nostr_connect_login();
                self.clear_busy();
                self.toast("Signer connect response timed out");
            }
            InternalEvent::NostrConnectInjectConnectResponseForTests {
                remote_signer_pubkey,
            } => {
                let parsed = match PublicKey::parse(remote_signer_pubkey.trim()) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(
                            %e,
                            "nostr_connect: test injection had invalid signer pubkey"
                        );
                        return;
                    }
                };
                let mut should_continue = false;
                if let Some(pending) = self.pending_nostr_connect_login.as_mut() {
                    let injected = NostrConnectConnectResponse {
                        remote_signer_pubkey: parsed,
                        agreed_secret: pending.secret.clone(),
                    };
                    match pending.connect_response_result.lock() {
                        Ok(mut slot) => {
                            *slot = Some(Ok(injected));
                        }
                        Err(poison) => {
                            *poison.into_inner() = Some(Ok(injected));
                        }
                    }
                    should_continue = true;
                }
                if should_continue {
                    self.progress_pending_nostr_connect_login(
                        None,
                        false,
                        "connect-response-ready",
                    );
                }
            }
            InternalEvent::CallRuntimeConnected { call_id } => {
                if let Some(call) = self.state.active_call.as_ref() {
                    if call.call_id == call_id && matches!(call.status, CallStatus::Connecting) {
                        let previous = self.state.active_call.clone();
                        let call = self.state.active_call.as_mut().unwrap();
                        call.set_status(CallStatus::Active);
                        if call.started_at.is_none() {
                            call.started_at = Some(now_seconds());
                        }
                        self.emit_call_state_with_previous(previous);
                    }
                }
            }
            InternalEvent::CallRuntimeStats {
                call_id,
                tx_frames,
                rx_frames,
                rx_dropped,
                jitter_buffer_ms,
                last_rtt_ms,
                video_tx,
                video_rx,
                video_rx_decrypt_fail,
            } => {
                if let Some(call) = self.state.active_call.as_ref() {
                    if call.call_id == call_id {
                        let previous = self.state.active_call.clone();
                        let call = self.state.active_call.as_mut().unwrap();
                        if matches!(call.status, CallStatus::Connecting) {
                            call.set_status(CallStatus::Active);
                            if call.started_at.is_none() {
                                call.started_at = Some(now_seconds());
                            }
                        }
                        call.debug = Some(CallDebugStats {
                            tx_frames,
                            rx_frames,
                            rx_dropped,
                            jitter_buffer_ms,
                            last_rtt_ms,
                            video_tx,
                            video_rx,
                            video_rx_decrypt_fail,
                        });
                        self.emit_call_state_with_previous(previous);
                    }
                }
            }
            InternalEvent::VideoFrameFromPlatform { payload } => {
                if let Some(call) = self.state.active_call.as_ref() {
                    if call.is_video_call && call.is_camera_enabled {
                        self.call_runtime.send_video_frame(&call.call_id, payload);
                    }
                }
            }
            InternalEvent::KeyPackagePublished { ok, ref error } => {
                tracing::info!(ok, ?error, "key_package_published");
                if !ok {
                    let msg = error.clone().unwrap_or_else(|| "unknown error".into());
                    if msg.contains("no relays")
                        || msg.contains("not ready")
                        || msg.contains("not connected")
                    {
                        self.toast("Key package publish delayed: relay connection is not ready");
                    } else {
                        self.toast(format!("Key package publish failed: {msg}"));
                    }
                }
            }
            InternalEvent::PushSubscriptionsSynced { groups } => {
                self.handle_push_subscriptions_synced(groups);
            }
            InternalEvent::PushUnsubscriptionsSynced { groups } => {
                self.handle_push_unsubscriptions_synced(groups);
            }
            InternalEvent::PublishMessageResult {
                chat_id,
                rumor_id,
                ok,
                error,
            } => {
                tracing::info!(
                    ok,
                    ?error,
                    %chat_id,
                    %rumor_id,
                    "message_publish_result"
                );
                let per_chat = self.delivery_overrides.entry(chat_id.clone()).or_default();
                if ok {
                    per_chat.insert(rumor_id.clone(), MessageDeliveryState::Sent);
                    if let Some(m) = self.pending_sends.get_mut(&chat_id) {
                        m.remove(&rumor_id);
                    }
                } else {
                    per_chat.insert(
                        rumor_id.clone(),
                        MessageDeliveryState::Failed {
                            reason: error.unwrap_or_else(|| "publish failed".into()),
                        },
                    );
                }
                self.refresh_chat_list_from_storage();
                self.refresh_current_chat_if_open(&chat_id);
            }
            InternalEvent::ChatMediaUploadCompleted {
                request_id,
                uploaded_url,
                descriptor_sha256_hex,
                error,
            } => {
                self.handle_chat_media_upload_completed(
                    request_id,
                    uploaded_url,
                    descriptor_sha256_hex,
                    error,
                );
            }
            InternalEvent::ChatMediaDownloadFetched {
                request_id,
                encrypted_data,
                error,
            } => {
                self.handle_chat_media_download_fetched(request_id, encrypted_data, error);
            }
            InternalEvent::PeerKeyPackageFetched {
                peer_pubkey,
                key_package_event,
                error,
            } => {
                let network_enabled = self.network_enabled();
                tracing::info!(
                    peer = %peer_pubkey.to_hex(),
                    kp_found = key_package_event.is_some(),
                    ?error,
                    "peer_key_package_fetched"
                );
                if let Some(err) = error {
                    self.set_busy(|b| b.creating_chat = false);
                    self.toast(err);
                    return;
                }
                let Some(kp_event) = key_package_event else {
                    self.set_busy(|b| b.creating_chat = false);
                    self.toast("Could not find peer key package (kind 443). The peer must run Pika/MDK once (publish a key package) and you must share at least one relay.".to_string());
                    return;
                };
                let kp_event = normalize_peer_key_package_event_for_mdk(&kp_event);

                // Merge our default relays with any relays the peer advertised in their key package.
                let peer_relays =
                    extract_relays_from_key_package_event(&kp_event).unwrap_or_default();
                let mut group_relays = self.default_relays();
                for r in peer_relays.iter().cloned() {
                    if !group_relays.contains(&r) {
                        group_relays.push(r);
                    }
                }
                let group_result = {
                    let Some(sess) = self.session.as_mut() else {
                        self.set_busy(|b| b.creating_chat = false);
                        return;
                    };

                    // Validate peer key package before use (spec-v2).
                    if let Err(e) = sess.mdk.parse_key_package(&kp_event) {
                        self.set_busy(|b| b.creating_chat = false);
                        self.toast(format!(
                            "Invalid peer key package: {e}. If this is a Marmot/WhiteNoise interop peer, ensure it publishes MIP-00 compliant tags (mls_protocol_version=1.0, encoding=base64)."
                        ));
                        return;
                    }

                    // Create group (1:1 DM).
                    let admins = vec![sess.pubkey, peer_pubkey];
                    let config = NostrGroupConfigData {
                        name: DEFAULT_GROUP_NAME.to_string(),
                        description: DEFAULT_GROUP_DESCRIPTION.to_string(),
                        image_hash: None,
                        image_key: None,
                        image_nonce: None,
                        relays: group_relays.clone(),
                        admins,
                    };

                    let group_result =
                        match sess
                            .mdk
                            .create_group(&sess.pubkey, vec![kp_event.clone()], config)
                        {
                            Ok(r) => r,
                            Err(e) => {
                                self.set_busy(|b| b.creating_chat = false);
                                self.toast(format!("Create group failed: {e}"));
                                return;
                            }
                        };

                    group_result
                };

                // Deliver welcomes (gift-wrapped kind 444) to the peer.
                if network_enabled {
                    self.publish_welcomes_to_peer(
                        peer_pubkey,
                        group_result.welcome_rumors,
                        group_relays.clone(),
                    );
                }

                // Refresh state + subscriptions + navigate.
                self.refresh_all_from_storage();

                let chat_id = hex::encode(group_result.group.nostr_group_id);
                self.open_chat_screen(&chat_id);
                self.refresh_current_chat(&chat_id);
                self.emit_router();
                self.set_busy(|b| b.creating_chat = false);
            }
            InternalEvent::GiftWrapReceived { wrapper, rumor } => {
                tracing::info!(
                    wrapper_id = %wrapper.id.to_hex(),
                    rumor_kind = rumor.kind.as_u16(),
                    "giftwrap_received"
                );
                let Some(sess) = self.session.as_mut() else {
                    tracing::warn!("giftwrap_received but no session");
                    return;
                };

                if rumor.kind != Kind::MlsWelcome {
                    tracing::debug!(
                        kind = rumor.kind.as_u16(),
                        "giftwrap ignored (not MlsWelcome)"
                    );
                    return;
                }

                let welcome = match sess.mdk.process_welcome(&wrapper.id, &rumor) {
                    Ok(w) => w,
                    Err(e) => {
                        tracing::error!(%e, "process_welcome failed");
                        return;
                    }
                };

                // Skip if we already joined this group (e.g. Welcome re-delivered
                // from relays after an app restart).  Reprocessing the Welcome
                // would reset the MLS ratchet state and break message decryption.
                let nostr_group_hex = hex::encode(welcome.nostr_group_id);
                // Check both the in-memory index and MDK storage to catch
                // duplicates even before refresh_all_from_storage() runs.
                // Only skip if the group is Active (fully joined). Pending
                // groups from a prior process_welcome haven't been accepted
                // yet and should not block the accept flow.
                let already_joined = sess.groups.contains_key(&nostr_group_hex)
                    || sess.mdk.get_groups().unwrap_or_default().iter().any(|g| {
                        hex::encode(g.nostr_group_id) == nostr_group_hex
                            && g.state == mdk_storage_traits::groups::types::GroupState::Active
                    });
                if already_joined {
                    tracing::debug!(
                        nostr_group_id = %nostr_group_hex,
                        "welcome skipped (group already exists)"
                    );
                    return;
                }

                tracing::info!(
                    nostr_group_id = %nostr_group_hex,
                    group_name = %welcome.group_name,
                    "welcome_accepted"
                );

                if let Err(e) = sess.mdk.accept_welcome(&welcome) {
                    tracing::error!(%e, "accept_welcome failed");
                    self.toast(format!("Welcome accept failed: {e}"));
                    return;
                }

                // Rotate the referenced key package: delete best-effort, publish fresh.
                if self.network_enabled() {
                    if let Some(kp_event_id) = referenced_key_package_event_id(&rumor) {
                        self.delete_event_best_effort(kp_event_id);
                    }
                    self.ensure_key_package_published_best_effort();
                }

                self.refresh_all_from_storage();
            }
            InternalEvent::ProfilesFetched { profiles } => {
                let now = now_seconds();
                for (hex_pubkey, metadata_json, event_created_at) in profiles {
                    self.upsert_profile(
                        hex_pubkey,
                        ProfileCache::from_metadata_json(metadata_json, event_created_at, now),
                    );
                }

                self.refresh_chat_list_from_storage();
                if let Some(chat) = self.state.current_chat.as_ref() {
                    let chat_id = chat.chat_id.clone();
                    self.refresh_current_chat(&chat_id);
                }
            }
            InternalEvent::MyProfileFetched { metadata } => {
                self.apply_my_profile_metadata(metadata);
            }
            InternalEvent::MyProfileSaved { metadata } => {
                self.apply_my_profile_metadata(Some(metadata));
                self.toast("Profile updated");
            }
            InternalEvent::MyProfileError { message, toast } => {
                if toast {
                    self.toast(message);
                } else {
                    tracing::debug!(%message, "profile action failed");
                }
            }
            InternalEvent::GroupKeyPackagesFetched {
                peer_pubkeys,
                group_name,
                key_package_events,
                failed_peers,
                candidate_kp_relays,
            } => {
                let network_enabled = self.network_enabled();

                if key_package_events.is_empty() {
                    self.set_busy(|b| b.creating_chat = false);
                    let names: Vec<String> = failed_peers
                        .iter()
                        .map(|(pk, e)| format!("{}: {e}", &pk.to_hex()[..8]))
                        .collect();
                    self.toast(format!("No key packages found: {}", names.join(", ")));
                    return;
                }

                if !failed_peers.is_empty() {
                    let names: Vec<String> = failed_peers
                        .iter()
                        .map(|(pk, _)| pk.to_hex()[..8].to_string())
                        .collect();
                    self.toast(format!(
                        "Could not add {} peer(s): {}",
                        failed_peers.len(),
                        names.join(", ")
                    ));
                }

                // Check if this is an add-members-to-existing-group operation.
                // We repurpose group_name: if it's a hex chat_id of an existing group, it's an add-member op.
                let is_add_members = self.chat_exists(&group_name);

                if is_add_members {
                    let chat_id = group_name;
                    let Some(sess) = self.session.as_mut() else {
                        self.set_busy(|b| b.creating_chat = false);
                        return;
                    };
                    let Some(entry) = sess.groups.get(&chat_id).cloned() else {
                        self.set_busy(|b| b.creating_chat = false);
                        self.toast("Chat not found");
                        return;
                    };

                    let kp_events: Vec<Event> = key_package_events
                        .iter()
                        .map(normalize_peer_key_package_event_for_mdk)
                        .collect();

                    for ev in &kp_events {
                        if let Err(e) = sess.mdk.parse_key_package(ev) {
                            self.set_busy(|b| b.creating_chat = false);
                            self.toast(format!("Invalid key package: {e}"));
                            return;
                        }
                    }

                    let result = match sess.mdk.add_members(&entry.mls_group_id, &kp_events) {
                        Ok(r) => r,
                        Err(e) => {
                            self.set_busy(|b| b.creating_chat = false);
                            self.toast(format!("Add members failed: {e}"));
                            return;
                        }
                    };

                    let added: Vec<PublicKey> = kp_events.iter().map(|e| e.pubkey).collect();
                    self.publish_evolution_event(
                        &chat_id,
                        entry.mls_group_id,
                        result.evolution_event,
                        result.welcome_rumors,
                        added,
                    );
                    // Clear busy immediately — relay confirmation, merge, and
                    // welcome delivery continue in the background via
                    // GroupEvolutionPublished handler.
                    self.set_busy(|b| b.creating_chat = false);
                } else {
                    // Create new group chat.
                    let kp_events: Vec<Event> = key_package_events
                        .iter()
                        .map(normalize_peer_key_package_event_for_mdk)
                        .collect();

                    let peer_relays: Vec<RelayUrl> = kp_events
                        .iter()
                        .flat_map(|e| extract_relays_from_key_package_event(e).unwrap_or_default())
                        .collect();
                    let mut group_relays = self.default_relays();
                    for r in candidate_kp_relays.iter().cloned() {
                        if !group_relays.contains(&r) {
                            group_relays.push(r);
                        }
                    }
                    for r in peer_relays.iter().cloned() {
                        if !group_relays.contains(&r) {
                            group_relays.push(r);
                        }
                    }

                    let Some(sess) = self.session.as_mut() else {
                        self.set_busy(|b| b.creating_chat = false);
                        return;
                    };

                    for ev in &kp_events {
                        if let Err(e) = sess.mdk.parse_key_package(ev) {
                            self.set_busy(|b| b.creating_chat = false);
                            self.toast(format!("Invalid key package: {e}"));
                            return;
                        }
                    }

                    let admins = vec![sess.pubkey];

                    let config = NostrGroupConfigData {
                        name: group_name.clone(),
                        description: DEFAULT_GROUP_DESCRIPTION.to_string(),
                        image_hash: None,
                        image_key: None,
                        image_nonce: None,
                        relays: group_relays.clone(),
                        admins,
                    };

                    let group_result =
                        match sess
                            .mdk
                            .create_group(&sess.pubkey, kp_events.clone(), config)
                        {
                            Ok(r) => r,
                            Err(e) => {
                                self.set_busy(|b| b.creating_chat = false);
                                self.toast(format!("Create group failed: {e}"));
                                return;
                            }
                        };

                    // Deliver welcomes to all peers.
                    if network_enabled {
                        let mut welcome_relays = peer_relays;
                        for r in candidate_kp_relays {
                            if !welcome_relays.contains(&r) {
                                welcome_relays.push(r);
                            }
                        }
                        for r in group_relays {
                            if !welcome_relays.contains(&r) {
                                welcome_relays.push(r);
                            }
                        }
                        for pk in &peer_pubkeys {
                            self.publish_welcomes_to_peer(
                                *pk,
                                group_result.welcome_rumors.clone(),
                                welcome_relays.clone(),
                            );
                        }
                    }

                    self.refresh_all_from_storage();
                    let chat_id = hex::encode(group_result.group.nostr_group_id);
                    self.open_chat_screen(&chat_id);
                    self.refresh_current_chat(&chat_id);
                    self.emit_router();
                    self.set_busy(|b| b.creating_chat = false);
                }
            }
            InternalEvent::GroupEvolutionPublished {
                chat_id: _,
                mls_group_id,
                welcome_rumors,
                added_pubkeys,
                ok,
                error,
            } => {
                if !ok {
                    self.toast(format!(
                        "Group update failed: {}",
                        error.unwrap_or_else(|| "unknown".into())
                    ));
                    return;
                }

                // Merge the pending commit now that relay confirmed.
                if let Some(sess) = self.session.as_mut() {
                    if let Err(e) = sess.mdk.merge_pending_commit(&mls_group_id) {
                        tracing::error!(%e, "merge_pending_commit failed");
                    }
                }

                // Send welcomes to newly added members.
                if let Some(rumors) = welcome_rumors {
                    if !rumors.is_empty() && self.network_enabled() {
                        let fallback_relays = self.default_relays();
                        let relays: Vec<RelayUrl> = self
                            .session
                            .as_ref()
                            .and_then(|s| s.mdk.get_relays(&mls_group_id).ok())
                            .map(|s| s.into_iter().collect())
                            .filter(|v: &Vec<RelayUrl>| !v.is_empty())
                            .unwrap_or(fallback_relays);
                        for pk in added_pubkeys {
                            self.publish_welcomes_to_peer(pk, rumors.clone(), relays.clone());
                        }
                    }
                }

                self.refresh_all_from_storage();
            }
            InternalEvent::FollowListFetched {
                followed_pubkeys,
                fetched_profiles,
                checked_pubkeys,
            } => {
                // Upsert freshly fetched profiles.
                let now = now_seconds();
                for (hex_pubkey, metadata_json, event_created_at) in fetched_profiles {
                    self.upsert_profile(
                        hex_pubkey,
                        ProfileCache::from_metadata_json(metadata_json, event_created_at, now),
                    );
                }
                // Mark checked-but-not-fetched pubkeys so we don't re-fetch them.
                for pk in &checked_pubkeys {
                    if !self.profiles.contains_key(pk) {
                        self.upsert_profile(
                            pk.clone(),
                            ProfileCache::from_metadata_json(None, 0, now),
                        );
                    }
                }

                // Build follow list entries from the shared profile cache.
                let mut follow_list: Vec<crate::state::FollowListEntry> = followed_pubkeys
                    .into_iter()
                    .map(|hex_pubkey| {
                        let npub = PublicKey::from_hex(&hex_pubkey)
                            .ok()
                            .and_then(|pk| pk.to_bech32().ok())
                            .unwrap_or_else(|| hex_pubkey.clone());
                        let cached = self.profiles.get(&hex_pubkey);
                        let name = cached.and_then(|p| p.name.clone());
                        let username = cached.and_then(|p| p.username.clone());
                        let picture_url =
                            cached.and_then(|p| p.display_picture_url(&self.data_dir, &hex_pubkey));
                        crate::state::FollowListEntry {
                            pubkey: hex_pubkey,
                            npub,
                            name,
                            username,
                            picture_url,
                        }
                    })
                    .collect();
                // Sort: names first (alphabetical), then npub-only entries.
                follow_list.sort_by(|a, b| match (&a.name, &b.name) {
                    (Some(na), Some(nb)) => na.to_lowercase().cmp(&nb.to_lowercase()),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a.npub.cmp(&b.npub),
                });
                self.state.follow_list = follow_list;
                // Persist to cache (guard against empty relay responses wiping the DB).
                if self.state.follow_list.len() >= 2 {
                    if let Some(conn) = self.profile_db.as_ref() {
                        let pubkeys: Vec<String> = self
                            .state
                            .follow_list
                            .iter()
                            .map(|f| f.pubkey.clone())
                            .collect();
                        profile_db::save_follows(conn, &pubkeys);
                    }
                }
                self.set_busy(|b| b.fetching_follow_list = false);
                // Update peer_profile.is_followed if the sheet is open.
                if let Some(ref mut pp) = self.state.peer_profile {
                    pp.is_followed = self.state.follow_list.iter().any(|f| f.pubkey == pp.pubkey);
                }
                // Refresh chat list too since profiles were updated.
                self.refresh_chat_list_from_storage();
                if let Some(chat) = self.state.current_chat.as_ref() {
                    let chat_id = chat.chat_id.clone();
                    self.refresh_current_chat(&chat_id);
                }
            }
            InternalEvent::PeerProfileFetched {
                pubkey,
                metadata_json,
                event_created_at,
            } => {
                let now = now_seconds();
                self.upsert_profile(
                    pubkey.clone(),
                    ProfileCache::from_metadata_json(metadata_json, event_created_at, now),
                );

                // Update peer_profile if it's still showing this pubkey.
                if let Some(ref mut pp) = self.state.peer_profile {
                    if pp.pubkey == pubkey {
                        let cached = self.profiles.get(&pubkey);
                        pp.name = cached.and_then(|p| p.name.clone());
                        pp.about = cached.and_then(|p| p.about.clone());
                        pp.picture_url =
                            cached.and_then(|p| p.display_picture_url(&self.data_dir, &pubkey));
                        self.emit_state();
                    }
                }
            }
            InternalEvent::ProfilePicCached { pubkey, url } => {
                // Only refresh if the profile still has the same picture_url
                // (guards against mid-download URL changes).
                let url_matches = self
                    .profiles
                    .get(&pubkey)
                    .and_then(|c| c.picture_url.as_deref())
                    == Some(&url);
                if !url_matches {
                    return;
                }
                let file_url = self
                    .profiles
                    .get(&pubkey)
                    .and_then(|p| p.display_picture_url(&self.data_dir, &pubkey));
                let mut changed = false;

                // Update own profile.
                let is_me =
                    self.session.as_ref().map(|s| s.pubkey.to_hex()).as_deref() == Some(&pubkey);
                if is_me {
                    let next = self.my_profile_state();
                    if next != self.state.my_profile {
                        self.state.my_profile = next;
                        changed = true;
                    }
                }

                // Patch picture URLs in chat list members.
                for chat in &mut self.state.chat_list {
                    for member in &mut chat.members {
                        if member.pubkey == pubkey {
                            member.picture_url = file_url.clone();
                            changed = true;
                        }
                    }
                }

                // Patch picture URLs in current chat members.
                if let Some(ref mut chat) = self.state.current_chat {
                    for member in &mut chat.members {
                        if member.pubkey == pubkey {
                            member.picture_url = file_url.clone();
                            changed = true;
                        }
                    }
                }

                // Patch peer profile if open.
                if let Some(ref mut pp) = self.state.peer_profile {
                    if pp.pubkey == pubkey {
                        pp.picture_url = file_url.clone();
                        changed = true;
                    }
                }

                // Patch follow list.
                for entry in &mut self.state.follow_list {
                    if entry.pubkey == pubkey {
                        entry.picture_url = file_url.clone();
                        changed = true;
                    }
                }

                if changed {
                    self.emit_state();
                }
            }
            InternalEvent::ContactListModifyFailed { pubkey, revert_to } => {
                // Revert the optimistic DB update.
                if let Some(conn) = self.profile_db.as_ref() {
                    if revert_to {
                        profile_db::add_follow(conn, &pubkey);
                    } else {
                        profile_db::remove_follow(conn, &pubkey);
                    }
                }
                if let Some(ref mut pp) = self.state.peer_profile {
                    if pp.pubkey == pubkey {
                        pp.is_followed = revert_to;
                    }
                }
                self.toast("Failed to update follow list".to_string());
                self.emit_state();
            }
            InternalEvent::GroupMessageReceived { event } => {
                tracing::debug!(event_id = %event.id.to_hex(), "group_message_received");
                let result = {
                    let Some(sess) = self.session.as_mut() else {
                        tracing::warn!("group_message but no session");
                        return;
                    };
                    match sess.mdk.process_message(&event) {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!(event_id = %event.id.to_hex(), %e, "process_message failed");
                            self.toast(format!("Message decrypt failed: {e}"));
                            return;
                        }
                    }
                };
                let is_app_message =
                    matches!(result, MessageProcessingResult::ApplicationMessage(_));
                let mut app_sender: Option<PublicKey> = None;
                let mut app_content: Option<String> = None;
                let mut is_typing_indicator = false;
                let mut is_call_signal_kind = false;
                let mut is_reaction = false;

                let mls_group_id: Option<GroupId> = match &result {
                    MessageProcessingResult::ApplicationMessage(msg) => {
                        match msg.kind {
                            Kind::Custom(20_067)
                                if msg.content == "typing"
                                    && msg.tags.iter().any(|t| {
                                        t.kind() == TagKind::d()
                                            && t.content().map(|c| c == "pika").unwrap_or(false)
                                    }) =>
                            {
                                is_typing_indicator = true;
                                app_sender = Some(msg.pubkey);
                            }
                            Kind::Custom(10) => {
                                is_call_signal_kind = true;
                                app_sender = Some(msg.pubkey);
                                app_content = Some(msg.content.clone());
                            }
                            Kind::ChatMessage | Kind::Reaction => {
                                if msg.kind == Kind::Reaction {
                                    is_reaction = true;
                                }
                                app_sender = Some(msg.pubkey);
                                app_content = Some(msg.content.clone());
                            }
                            kind => {
                                tracing::warn!(?kind, "ignoring app message with unknown kind");
                                return;
                            }
                        }
                        Some(msg.mls_group_id.clone())
                    }
                    MessageProcessingResult::Proposal(update) => Some(update.mls_group_id.clone()),
                    MessageProcessingResult::PendingProposal { mls_group_id } => {
                        Some(mls_group_id.clone())
                    }
                    MessageProcessingResult::IgnoredProposal { mls_group_id, .. } => {
                        Some(mls_group_id.clone())
                    }
                    MessageProcessingResult::ExternalJoinProposal { mls_group_id } => {
                        Some(mls_group_id.clone())
                    }
                    MessageProcessingResult::Commit { mls_group_id } => Some(mls_group_id.clone()),
                    MessageProcessingResult::Unprocessable { mls_group_id } => {
                        Some(mls_group_id.clone())
                    }
                    MessageProcessingResult::PreviouslyFailed => None,
                };

                if let Some(group_id) = mls_group_id {
                    let chat_id = {
                        let Some(sess) = self.session.as_mut() else {
                            self.refresh_all_from_storage();
                            return;
                        };
                        match sess.mdk.get_group(&group_id) {
                            Ok(Some(group)) => Some(hex::encode(group.nostr_group_id)),
                            _ => None,
                        }
                    };
                    if let Some(chat_id) = chat_id {
                        if is_typing_indicator {
                            // Handle typing indicator: update in-memory state, refresh chat.
                            if let Some(sender) = app_sender {
                                let sender_hex = sender.to_hex();
                                let my_hex = self.session.as_ref().map(|s| s.pubkey.to_hex());
                                // Ignore our own typing indicators.
                                if my_hex.as_deref() != Some(sender_hex.as_str()) {
                                    self.update_typing(&chat_id, &sender_hex, now_seconds() + 10);
                                    self.refresh_current_chat_if_open(&chat_id);
                                }
                            }
                        } else {
                            // Normal message: clear typing indicator for sender.
                            if let Some(sender) = app_sender {
                                self.update_typing(&chat_id, &sender.to_hex(), 0);
                            }

                            if is_call_signal_kind {
                                if let (Some(sender), Some(content)) =
                                    (app_sender, app_content.as_deref())
                                {
                                    if let Some(signal) =
                                        self.maybe_parse_call_signal(&sender, content)
                                    {
                                        self.handle_incoming_call_signal(&chat_id, &sender, signal);
                                    }
                                }
                            }

                            let current =
                                self.state.current_chat.as_ref().map(|c| c.chat_id.as_str());
                            let is_call_signal = is_call_signal_kind;
                            if current != Some(chat_id.as_str()) && !is_call_signal && !is_reaction
                            {
                                *self.unread_counts.entry(chat_id.clone()).or_insert(0) += 1;
                            } else if is_app_message && !is_call_signal {
                                self.loaded_count
                                    .entry(chat_id.clone())
                                    .and_modify(|n| *n += 1)
                                    .or_insert(51);
                            }
                            self.refresh_chat_list_from_storage();
                            self.refresh_current_chat_if_open(&chat_id);
                        }
                    } else {
                        // Fallback: refresh everything if metadata lookup fails.
                        self.refresh_all_from_storage();
                    }
                } else {
                    self.refresh_all_from_storage();
                }
            }
        }
    }

    fn handle_action(&mut self, action: AppAction) {
        match action {
            // Auth
            AppAction::CreateAccount => {
                self.set_busy(|b| {
                    b.creating_account = true;
                    b.logging_in = false;
                });
                let keys = Keys::generate();
                let nsec = keys.secret_key().to_bech32().expect("infallible");
                let pubkey = keys.public_key().to_hex();
                let npub = keys.public_key().to_bech32().unwrap_or(pubkey.clone());

                self.emit_account_created(nsec, pubkey.clone(), npub.clone());

                if let Err(e) = self.start_session(keys) {
                    // Include the full anyhow context chain; this is critical for diagnosing
                    // keyring/SQLCipher issues on iOS.
                    self.clear_busy();
                    self.toast(format!("Create account failed: {e:#}"));
                } else {
                    self.clear_busy();
                }
            }
            AppAction::Login { nsec } | AppAction::RestoreSession { nsec } => {
                self.set_busy(|b| {
                    b.logging_in = true;
                    b.creating_account = false;
                });
                let nsec = nsec.trim();
                if nsec.is_empty() {
                    self.clear_busy();
                    self.toast("Enter an nsec");
                    return;
                }
                let keys = match Keys::parse(nsec) {
                    Ok(k) => k,
                    Err(e) => {
                        self.clear_busy();
                        self.toast(format!("Invalid nsec: {e}"));
                        return;
                    }
                };
                if let Err(e) = self.start_session(keys) {
                    self.clear_busy();
                    self.toast(format!("Login failed: {e:#}"));
                } else {
                    self.clear_busy();
                }
            }
            AppAction::BeginExternalSignerLogin { current_user_hint } => {
                self.begin_external_signer_login(current_user_hint);
            }
            AppAction::BeginBunkerLogin { bunker_uri } => {
                self.begin_bunker_login(bunker_uri);
            }
            AppAction::BeginNostrConnectLogin => {
                self.begin_nostr_connect_login();
            }
            AppAction::ResetNostrConnectPairing => {
                self.reset_nostr_connect_pairing();
            }
            AppAction::NostrConnectCallback { url } => {
                self.on_nostr_connect_callback(url);
            }
            AppAction::RestoreSessionExternalSigner {
                pubkey,
                signer_package,
                current_user,
            } => {
                self.set_busy(|b| {
                    b.logging_in = true;
                    b.creating_account = false;
                });

                if !self.external_signer_enabled() {
                    self.clear_busy();
                    self.toast("External signer is disabled");
                    return;
                }

                if let Err(e) =
                    self.start_external_signer_session(pubkey, signer_package, current_user)
                {
                    self.clear_busy();
                    let detail = format!("{e:#}");
                    if let Some(msg) = user_visible_signer_error(&detail) {
                        self.toast(msg);
                    } else {
                        self.toast(format!("Login failed: {detail}"));
                    }
                } else {
                    self.clear_busy();
                }
            }
            AppAction::RestoreSessionBunker {
                bunker_uri,
                client_nsec,
            } => {
                self.restore_bunker_session(bunker_uri, client_nsec);
            }
            AppAction::Logout => {
                self.clear_pending_nostr_connect_login();
                // Delete the MLS database before tearing down the session so stale
                // ratchet state doesn't persist across logins.
                if let Some(sess) = self.session.as_ref() {
                    let db_path =
                        crate::mdk_support::mdk_db_path(&self.data_dir, &sess.pubkey.to_hex());
                    if let Err(e) = std::fs::remove_file(&db_path) {
                        tracing::warn!(%e, path = %db_path.display(), "failed to delete mdk db on logout");
                    } else {
                        tracing::info!(path = %db_path.display(), "deleted mdk db on logout");
                    }
                }
                self.clear_push_subscriptions();
                self.stop_session();
                self.state.auth = AuthState::LoggedOut;
                self.emit_auth();
                self.handle_auth_transition(false);
            }
            AppAction::WipeLocalData => {
                self.clear_push_subscriptions();
                self.stop_session();
                self.state.auth = AuthState::LoggedOut;
                self.emit_auth();
                self.wipe_local_data();
                self.handle_auth_transition(false);
            }
            AppAction::ArchiveChat { chat_id } => {
                self.archived_chats.insert(chat_id.clone());
                self.save_archived_chats();
                // If we're viewing this chat, navigate back.
                prune_chat_routes(&mut self.state.router.screen_stack, &chat_id);
                self.state.current_chat = None;
                self.refresh_chat_list_from_storage();
                self.emit_router();
            }
            AppAction::ReactToMessage {
                chat_id,
                message_id,
                emoji,
            } => {
                if !self.is_logged_in() {
                    return;
                }
                let Some(sess) = self.session.as_mut() else {
                    return;
                };
                let Some(group) = sess.groups.get(&chat_id).cloned() else {
                    return;
                };

                let msg_event_id = match nostr_sdk::prelude::EventId::parse(&message_id) {
                    Ok(id) => id,
                    Err(_) => return,
                };

                let rumor = UnsignedEvent::new(
                    sess.pubkey,
                    Timestamp::now(),
                    Kind::Reaction,
                    [Tag::event(msg_event_id)],
                    emoji,
                );

                let wrapper = match sess.mdk.create_message(&group.mls_group_id, rumor) {
                    Ok(ev) => ev,
                    Err(e) => {
                        tracing::warn!(err = %e, "reaction create_message failed");
                        return;
                    }
                };

                // Fire-and-forget publish.
                let client = sess.client.clone();
                self.runtime.spawn(async move {
                    let _ = client.send_event(&wrapper).await;
                });

                // Refresh chat to pick up the reaction from storage.
                self.refresh_current_chat(&chat_id);
            }
            AppAction::TypingStarted { chat_id } => {
                if !self.is_logged_in() {
                    return;
                }

                // Debounce: don't send more than once every 5 seconds per chat.
                let now = now_seconds();
                if let Some(&last) = self.last_typing_sent.get(&chat_id) {
                    if now - last < 5 {
                        return;
                    }
                }
                self.last_typing_sent.insert(chat_id.clone(), now);

                let Some(sess) = self.session.as_mut() else {
                    return;
                };
                let Some(group) = sess.groups.get(&chat_id).cloned() else {
                    return;
                };

                let expires_at = now as u64 + 10;
                let rumor = UnsignedEvent::new(
                    sess.pubkey,
                    Timestamp::now(),
                    TYPING_INDICATOR_KIND,
                    [
                        Tag::custom(TagKind::d(), ["pika"]),
                        Tag::expiration(Timestamp::from_secs(expires_at)),
                    ],
                    "typing",
                );

                let wrapper = match sess.mdk.create_message(&group.mls_group_id, rumor) {
                    Ok(ev) => ev,
                    Err(e) => {
                        tracing::warn!(err = %e, "typing indicator create_message failed");
                        return;
                    }
                };

                let client = sess.client.clone();
                self.runtime.spawn(async move {
                    let _ = client.send_event(&wrapper).await;
                });
            }
            AppAction::ClearToast => {
                if self.state.toast.is_some() {
                    self.state.toast = None;
                    self.emit_toast();
                }
            }
            AppAction::SetPushToken { token } => {
                self.set_push_token(token);
            }
            AppAction::ReregisterPush => {
                self.reregister_push();
            }
            AppAction::Foregrounded => {
                // Native should send lifecycle signals as actions. Rust owns all state changes.
                if self.is_logged_in() {
                    self.reopen_mdk(); // Pick up NSE's ratchet changes
                    self.refresh_all_from_storage();
                    self.refresh_my_profile(false);
                    self.refresh_follow_list();
                } else {
                    tracing::info!(
                        pending_nostr_connect = self.pending_nostr_connect_login.is_some(),
                        "foregrounded while logged out"
                    );
                    // Some signers fail to invoke callback URLs reliably.
                    // If a nostr-connect login is pending, continue it on foreground.
                    self.continue_pending_nostr_connect_login();
                }
            }
            AppAction::ReloadConfig => {
                self.config = config::load_app_config(&self.data_dir);

                if !self.network_enabled() {
                    self.toast("Config reloaded (network disabled)");
                    return;
                }

                if self.is_logged_in() {
                    self.publish_key_package_relays_best_effort();
                    self.ensure_key_package_published_best_effort();
                    self.recompute_subscriptions();
                    self.refresh_follow_list();
                }

                self.toast("Relay config reloaded");
            }
            AppAction::OpenPeerProfile { pubkey } => {
                if !self.is_logged_in() {
                    return;
                }
                let npub = PublicKey::from_hex(&pubkey)
                    .ok()
                    .and_then(|pk| pk.to_bech32().ok())
                    .unwrap_or_else(|| pubkey.clone());
                let cached = self.profiles.get(&pubkey);
                let is_followed = self.state.follow_list.iter().any(|f| f.pubkey == pubkey);
                self.state.peer_profile = Some(crate::state::PeerProfileState {
                    pubkey: pubkey.clone(),
                    npub,
                    name: cached.and_then(|p| p.name.clone()),
                    about: cached.and_then(|p| p.about.clone()),
                    picture_url: cached
                        .and_then(|p| p.display_picture_url(&self.data_dir, &pubkey)),
                    is_followed,
                });
                self.emit_state();
                self.fetch_peer_profile(&pubkey);
                self.refresh_follow_list();
            }
            AppAction::ClosePeerProfile => {
                self.state.peer_profile = None;
                self.emit_state();
            }
            AppAction::RefreshFollowList => {
                if !self.is_logged_in() {
                    return;
                }
                self.refresh_follow_list();
            }
            AppAction::FollowUser { pubkey } => {
                if !self.is_logged_in() {
                    return;
                }
                self.follow_user(&pubkey);
            }
            AppAction::UnfollowUser { pubkey } => {
                if !self.is_logged_in() {
                    return;
                }
                self.unfollow_user(&pubkey);
            }
            AppAction::RefreshMyProfile => {
                if !self.is_logged_in() {
                    self.toast("Please log in first");
                    return;
                }
                self.refresh_my_profile(true);
            }
            AppAction::SaveMyProfile { name, about } => {
                self.save_my_profile(name, about);
            }
            AppAction::UploadMyProfileImage {
                image_base64,
                mime_type,
            } => {
                self.upload_my_profile_image(image_base64, mime_type);
            }

            // Navigation
            AppAction::PushScreen { screen } => {
                if !self.is_logged_in() && screen != Screen::Login {
                    self.toast("Please log in first");
                    return;
                }
                self.push_screen(screen);
                self.sync_current_chat_to_router();
                self.emit_router();
            }
            AppAction::UpdateScreenStack { stack } => {
                self.state.router.screen_stack = stack;

                self.sync_current_chat_to_router();

                self.emit_router();
            }

            // Chat
            AppAction::CreateChat { peer_npub } => {
                if !self.is_logged_in() {
                    self.toast("Please log in first");
                    return;
                }

                let network_enabled = self.network_enabled();
                let group_relays = self.default_relays();

                let peer_npub = peer_npub.trim().to_string();
                if peer_npub.is_empty() {
                    self.toast("Enter a peer npub");
                    return;
                }

                let peer_pubkey = match PublicKey::parse(&peer_npub) {
                    Ok(p) => p,
                    Err(e) => {
                        self.toast(format!("Invalid npub: {e}"));
                        return;
                    }
                };

                self.set_busy(|b| b.creating_chat = true);

                // Allow "note to self" flow for local/offline testing.
                let my_pubkey = match self.session.as_ref() {
                    Some(s) => s.pubkey,
                    None => {
                        self.set_busy(|b| b.creating_chat = false);
                        return;
                    }
                };
                if peer_pubkey == my_pubkey {
                    let Some(sess) = self.session.as_mut() else {
                        self.set_busy(|b| b.creating_chat = false);
                        return;
                    };
                    let config = NostrGroupConfigData {
                        name: "Note to self".to_string(),
                        description: DEFAULT_GROUP_DESCRIPTION.to_string(),
                        image_hash: None,
                        image_key: None,
                        image_nonce: None,
                        relays: group_relays.clone(),
                        admins: vec![my_pubkey],
                    };

                    let group_result = match sess.mdk.create_group(&sess.pubkey, vec![], config) {
                        Ok(r) => r,
                        Err(e) => {
                            self.set_busy(|b| b.creating_chat = false);
                            self.toast(format!("Create chat failed: {e}"));
                            return;
                        }
                    };

                    self.refresh_all_from_storage();
                    let chat_id = hex::encode(group_result.group.nostr_group_id);
                    self.open_chat_screen(&chat_id);
                    self.refresh_current_chat(&chat_id);
                    self.emit_router();
                    self.set_busy(|b| b.creating_chat = false);
                    return;
                }

                if !network_enabled {
                    self.set_busy(|b| b.creating_chat = false);
                    self.toast("Network disabled (set PIKA_DISABLE_NETWORK=0)");
                    return;
                }

                // Fetch peer key package asynchronously; actor will create the group on completion.
                // The user stays on the NewChat screen with a loading indicator until the
                // operation completes (success navigates to the chat; failure toasts an error).
                let (client, tx) = {
                    let Some(sess) = self.session.as_ref() else {
                        return;
                    };
                    (sess.client.clone(), self.core_sender.clone())
                };
                tracing::info!(peer = %peer_pubkey.to_hex(), "create_chat: fetching peer key package");
                self.runtime.spawn(async move {
                    // Fetch peer key package (kind 443) from connected relays.
                    let kp_filter = Filter::new()
                        .author(peer_pubkey)
                        .kind(Kind::MlsKeyPackage)
                        .limit(10);

                    match client.fetch_events(kp_filter, Duration::from_secs(8)).await {
                        Ok(events) => {
                            let best = events.into_iter().max_by_key(|e| e.created_at);
                            let _ = tx.send(CoreMsg::Internal(Box::new(
                                InternalEvent::PeerKeyPackageFetched {
                                    peer_pubkey,
                                    key_package_event: best,
                                    error: None,
                                },
                            )));
                        }
                        Err(e) => {
                            let _ = tx.send(CoreMsg::Internal(Box::new(
                                InternalEvent::PeerKeyPackageFetched {
                                    peer_pubkey,
                                    key_package_event: None,
                                    error: Some(format!("Fetch peer key package failed: {e}")),
                                },
                            )));
                        }
                    }
                });
            }
            AppAction::OpenChat { chat_id } => {
                if !self.is_logged_in() {
                    self.toast("Please log in first");
                    return;
                }
                if !self.chat_exists(&chat_id) {
                    self.toast("Chat not found");
                    return;
                }
                self.unread_counts.insert(chat_id.clone(), 0);
                self.refresh_chat_list_from_storage();
                self.open_chat_screen(&chat_id);
                self.refresh_current_chat(&chat_id);
                self.emit_router();
            }
            AppAction::SendMessage {
                chat_id,
                content,
                kind,
                reply_to_message_id,
            } => {
                if !self.is_logged_in() {
                    self.toast("Please log in first");
                    return;
                }

                let kind = kind.map(Kind::from).unwrap_or(Kind::ChatMessage);

                let content = content.trim().to_string();
                if content.is_empty() {
                    return;
                }
                let reply_to_message_id = reply_to_message_id
                    .map(|id| id.trim().to_string())
                    .filter(|id| !id.is_empty());

                // Build reply tags if replying to an existing message.
                let mut tags = Vec::new();
                let effective_reply_to = {
                    let Some(sess) = self.session.as_ref() else {
                        return;
                    };
                    reply_to_message_id.as_ref().and_then(|reply_to_id| {
                        let reply_event_id = EventId::parse(reply_to_id).ok()?;
                        let group = sess.groups.get(&chat_id)?;
                        let reply_target = sess
                            .mdk
                            .get_message(&group.mls_group_id, &reply_event_id)
                            .ok()
                            .flatten()?;
                        let p_tag = Tag::parse(vec![
                            "p".to_string(),
                            reply_target.pubkey.to_hex(),
                            String::new(),
                        ])
                        .ok()?;
                        let k_tag = Tag::parse(vec![
                            "k".to_string(),
                            reply_target.kind.as_u16().to_string(),
                        ])
                        .ok()?;
                        tags.push(Tag::event(reply_event_id));
                        tags.push(p_tag);
                        tags.push(k_tag);
                        Some(reply_event_id.to_hex())
                    })
                };

                self.publish_chat_message_with_tags(
                    chat_id,
                    content,
                    kind,
                    tags,
                    effective_reply_to,
                    vec![],
                );
            }
            AppAction::SendChatMedia {
                chat_id,
                data_base64,
                mime_type,
                filename,
                caption,
            } => {
                self.send_chat_media(chat_id, data_base64, mime_type, filename, caption);
            }
            AppAction::DownloadChatMedia {
                chat_id,
                message_id,
                original_hash_hex,
            } => {
                self.download_chat_media(chat_id, message_id, original_hash_hex);
            }
            AppAction::RetryMessage {
                chat_id,
                message_id,
            } => {
                if !self.is_logged_in() {
                    self.toast("Please log in first");
                    return;
                }
                let network_enabled = self.network_enabled();
                let fallback_relays = self.default_relays();

                let (client, relays, ps) = {
                    let Some(sess) = self.session.as_mut() else {
                        return;
                    };
                    let Some(ps) = self
                        .pending_sends
                        .get(&chat_id)
                        .and_then(|m| m.get(&message_id))
                        .cloned()
                    else {
                        self.toast("Nothing to retry");
                        return;
                    };

                    if !network_enabled {
                        (sess.client.clone(), vec![], ps)
                    } else {
                        let Some(group) = sess.groups.get(&chat_id).cloned() else {
                            self.toast("Chat not found");
                            return;
                        };
                        let relays: Vec<RelayUrl> = sess
                            .mdk
                            .get_relays(&group.mls_group_id)
                            .ok()
                            .map(|s| s.into_iter().collect())
                            .filter(|v: &Vec<RelayUrl>| !v.is_empty())
                            .unwrap_or_else(|| fallback_relays.clone());
                        (sess.client.clone(), relays, ps)
                    }
                };

                self.delivery_overrides
                    .entry(chat_id.clone())
                    .or_default()
                    .insert(message_id.clone(), MessageDeliveryState::Pending);
                self.refresh_current_chat_if_open(&chat_id);
                self.refresh_chat_list_from_storage();

                if !network_enabled {
                    let _ = self.core_sender.send(CoreMsg::Internal(Box::new(
                        InternalEvent::PublishMessageResult {
                            chat_id,
                            rumor_id: message_id,
                            ok: true,
                            error: None,
                        },
                    )));
                    return;
                }
                let _ = self.core_sender.send(CoreMsg::Internal(Box::new(
                    InternalEvent::PublishMessageResult {
                        chat_id,
                        rumor_id: ps.rumor_id_hex,
                        ok: true,
                        error: None,
                    },
                )));
                self.runtime.spawn(async move {
                    if let Err(e) = client.send_event_to(relays, &ps.wrapper_event).await {
                        tracing::warn!(%e, "message retry broadcast failed");
                    }
                });
            }
            AppAction::StartCall { chat_id } => {
                self.handle_start_call_action(&chat_id);
            }
            AppAction::AcceptCall { chat_id } => {
                self.handle_accept_call_action(&chat_id);
            }
            AppAction::RejectCall { chat_id } => {
                self.handle_reject_call_action(&chat_id);
            }
            AppAction::StartVideoCall { chat_id } => {
                self.handle_start_video_call_action(&chat_id);
            }
            AppAction::EndCall => {
                self.handle_end_call_action();
            }
            AppAction::ToggleMute => {
                self.handle_toggle_mute_action();
            }
            AppAction::ToggleCamera => {
                self.handle_toggle_camera_action();
            }
            AppAction::LoadOlderMessages {
                chat_id,
                before_message_id,
                limit,
            } => {
                if !self.is_logged_in() {
                    return;
                }
                if !self.chat_exists(&chat_id) {
                    return;
                }

                // Sanity check only (spec-v2).
                if let Some(cur) = &self.state.current_chat {
                    if cur.chat_id == chat_id {
                        if let Some(oldest) = cur.messages.first() {
                            if oldest.id != before_message_id {
                                self.refresh_current_chat(&chat_id);
                                return;
                            }
                        }
                    }
                }

                self.load_older_messages(&chat_id, limit as usize);
            }

            // Group chat actions
            AppAction::CreateGroupChat {
                peer_npubs,
                group_name,
            } => {
                if !self.is_logged_in() {
                    self.toast("Please log in first");
                    return;
                }
                if peer_npubs.is_empty() {
                    self.toast("Add at least one member");
                    return;
                }
                let group_name = group_name.trim().to_string();
                if group_name.is_empty() {
                    self.toast("Enter a group name");
                    return;
                }

                let mut peer_pubkeys: Vec<PublicKey> = Vec::new();
                for npub in &peer_npubs {
                    match PublicKey::parse(npub.trim()) {
                        Ok(p) => peer_pubkeys.push(p),
                        Err(e) => {
                            self.toast(format!("Invalid npub: {e}"));
                            return;
                        }
                    }
                }

                self.set_busy(|b| b.creating_chat = true);

                if !self.network_enabled() {
                    self.set_busy(|b| b.creating_chat = false);
                    self.toast("Network disabled");
                    return;
                }

                let (client, tx) = {
                    let Some(sess) = self.session.as_ref() else {
                        self.set_busy(|b| b.creating_chat = false);
                        return;
                    };
                    (sess.client.clone(), self.core_sender.clone())
                };
                let fallback_kp_relays = self.key_package_relays();
                let fallback_popular_relays = self.default_relays();

                self.runtime.spawn(async move {
                    // Ensure default relays are connected before any fetches.
                    for r in fallback_kp_relays
                        .iter()
                        .chain(fallback_popular_relays.iter())
                    {
                        let _ = client.add_relay(r.clone()).await;
                    }
                    client.connect().await;
                    client.wait_for_connection(Duration::from_secs(5)).await;

                    let mut all_kp_events: Vec<Event> = Vec::new();
                    let mut failed: Vec<(PublicKey, String)> = Vec::new();
                    let mut all_candidate_relays: Vec<RelayUrl> = Vec::new();

                    for pk in &peer_pubkeys {
                        let kp_relay_filter = Filter::new()
                            .author(*pk)
                            .kind(Kind::MlsKeyPackageRelays)
                            .limit(5);
                        let mut candidate_relays: Vec<RelayUrl> = Vec::new();
                        if let Ok(events) = client
                            .fetch_events(kp_relay_filter, Duration::from_secs(6))
                            .await
                        {
                            let mut newest: Option<Event> = None;
                            for e in events.into_iter() {
                                if newest
                                    .as_ref()
                                    .map(|b| e.created_at > b.created_at)
                                    .unwrap_or(true)
                                {
                                    newest = Some(e);
                                }
                            }
                            if let Some(ev) = newest.as_ref() {
                                candidate_relays = extract_relays_from_key_package_relays_event(ev);
                            }
                        }
                        if candidate_relays.is_empty() {
                            let mut s: BTreeSet<RelayUrl> = BTreeSet::new();
                            for r in fallback_kp_relays.iter().cloned() {
                                s.insert(r);
                            }
                            for r in fallback_popular_relays.iter().cloned() {
                                s.insert(r);
                            }
                            candidate_relays = s.into_iter().collect();
                        }
                        for r in candidate_relays.iter().cloned() {
                            let _ = client.add_relay(r).await;
                        }
                        client.connect().await;
                        client.wait_for_connection(Duration::from_secs(4)).await;

                        let kp_filter = Filter::new()
                            .author(*pk)
                            .kind(Kind::MlsKeyPackage)
                            .limit(10);
                        let res = match client
                            .fetch_events_from(
                                candidate_relays.clone(),
                                kp_filter.clone(),
                                Duration::from_secs(8),
                            )
                            .await
                        {
                            Ok(v) => Ok(v),
                            Err(_) => client.fetch_events(kp_filter, Duration::from_secs(8)).await,
                        };
                        match res {
                            Ok(events) => {
                                let best = events.into_iter().max_by_key(|e| e.created_at);
                                if let Some(ev) = best {
                                    all_kp_events.push(ev);
                                } else {
                                    failed.push((*pk, "No key package found".into()));
                                }
                            }
                            Err(e) => {
                                failed.push((*pk, format!("Fetch failed: {e}")));
                            }
                        }
                        for r in candidate_relays {
                            if !all_candidate_relays.contains(&r) {
                                all_candidate_relays.push(r);
                            }
                        }
                    }

                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::GroupKeyPackagesFetched {
                            peer_pubkeys,
                            group_name,
                            key_package_events: all_kp_events,
                            failed_peers: failed,
                            candidate_kp_relays: all_candidate_relays,
                        },
                    )));
                });
            }
            AppAction::AddGroupMembers {
                chat_id,
                peer_npubs,
            } => {
                if !self.is_logged_in() {
                    self.toast("Please log in first");
                    return;
                }
                if !self.network_enabled() {
                    self.toast("Network disabled");
                    return;
                }
                let mut peer_pubkeys: Vec<PublicKey> = Vec::new();
                for npub in &peer_npubs {
                    match PublicKey::parse(npub.trim()) {
                        Ok(p) => peer_pubkeys.push(p),
                        Err(e) => {
                            self.toast(format!("Invalid npub: {e}"));
                            return;
                        }
                    }
                }
                self.set_busy(|b| b.creating_chat = true);

                let (client, tx) = {
                    let Some(sess) = self.session.as_ref() else {
                        self.set_busy(|b| b.creating_chat = false);
                        return;
                    };
                    (sess.client.clone(), self.core_sender.clone())
                };
                let fallback_kp_relays = self.key_package_relays();
                let fallback_popular_relays = self.default_relays();
                let chat_id_clone = chat_id.clone();

                // Fetch key packages then add members.
                self.runtime.spawn(async move {
                    // Ensure relays are connected before fetches.
                    for r in fallback_kp_relays
                        .iter()
                        .chain(fallback_popular_relays.iter())
                    {
                        let _ = client.add_relay(r.clone()).await;
                    }
                    client.connect().await;
                    client.wait_for_connection(Duration::from_secs(5)).await;

                    let mut kp_events: Vec<Event> = Vec::new();
                    let mut failed: Vec<(PublicKey, String)> = Vec::new();
                    let mut all_candidate_relays: Vec<RelayUrl> = Vec::new();

                    for pk in &peer_pubkeys {
                        let kp_relay_filter = Filter::new()
                            .author(*pk)
                            .kind(Kind::MlsKeyPackageRelays)
                            .limit(5);
                        let mut candidate_relays: Vec<RelayUrl> = Vec::new();
                        if let Ok(events) = client
                            .fetch_events(kp_relay_filter, Duration::from_secs(6))
                            .await
                        {
                            let newest = events.into_iter().max_by_key(|e| e.created_at);
                            if let Some(ev) = newest.as_ref() {
                                candidate_relays = extract_relays_from_key_package_relays_event(ev);
                            }
                        }
                        if candidate_relays.is_empty() {
                            let mut s: BTreeSet<RelayUrl> = BTreeSet::new();
                            for r in fallback_kp_relays.iter().cloned() {
                                s.insert(r);
                            }
                            for r in fallback_popular_relays.iter().cloned() {
                                s.insert(r);
                            }
                            candidate_relays = s.into_iter().collect();
                        }
                        for r in candidate_relays.iter().cloned() {
                            let _ = client.add_relay(r).await;
                        }
                        client.connect().await;
                        client.wait_for_connection(Duration::from_secs(4)).await;

                        let kp_filter = Filter::new()
                            .author(*pk)
                            .kind(Kind::MlsKeyPackage)
                            .limit(10);
                        let res = match client
                            .fetch_events_from(
                                candidate_relays.clone(),
                                kp_filter.clone(),
                                Duration::from_secs(8),
                            )
                            .await
                        {
                            Ok(v) => Ok(v),
                            Err(_) => client.fetch_events(kp_filter, Duration::from_secs(8)).await,
                        };
                        match res {
                            Ok(events) => {
                                if let Some(ev) = events.into_iter().max_by_key(|e| e.created_at) {
                                    kp_events.push(ev);
                                } else {
                                    failed.push((*pk, "No key package found".into()));
                                }
                            }
                            Err(e) => failed.push((*pk, format!("Fetch failed: {e}"))),
                        }
                        for r in candidate_relays {
                            if !all_candidate_relays.contains(&r) {
                                all_candidate_relays.push(r);
                            }
                        }
                    }

                    if !failed.is_empty() {
                        let names: Vec<String> = failed
                            .iter()
                            .map(|(pk, e)| format!("{}: {e}", &pk.to_hex()[..8]))
                            .collect();
                        let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::Toast(
                            format!("Failed to fetch key packages for: {}", names.join(", ")),
                        ))));
                    }

                    if kp_events.is_empty() {
                        let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::Toast(
                            "No key packages found for any peer".into(),
                        ))));
                        return;
                    }

                    // Send fetched KP events back to the actor thread for MDK mutation.
                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::GroupKeyPackagesFetched {
                            peer_pubkeys,
                            group_name: chat_id_clone, // repurpose: signals this is an add-member op
                            key_package_events: kp_events,
                            failed_peers: failed,
                            candidate_kp_relays: all_candidate_relays,
                        },
                    )));
                });
            }
            AppAction::RemoveGroupMembers {
                chat_id,
                member_pubkeys,
            } => {
                if !self.is_logged_in() {
                    self.toast("Please log in first");
                    return;
                }
                let Some(sess) = self.session.as_mut() else {
                    return;
                };
                let Some(entry) = sess.groups.get(&chat_id).cloned() else {
                    self.toast("Chat not found");
                    return;
                };

                let mut pubkeys: Vec<PublicKey> = Vec::new();
                for hex in &member_pubkeys {
                    match PublicKey::from_hex(hex) {
                        Ok(p) => pubkeys.push(p),
                        Err(e) => {
                            self.toast(format!("Invalid pubkey: {e}"));
                            return;
                        }
                    }
                }

                let result = match sess.mdk.remove_members(&entry.mls_group_id, &pubkeys) {
                    Ok(r) => r,
                    Err(e) => {
                        self.toast(format!("Remove members failed: {e}"));
                        return;
                    }
                };

                self.publish_evolution_event(
                    &chat_id,
                    entry.mls_group_id.clone(),
                    result.evolution_event,
                    None,
                    vec![],
                );
            }
            AppAction::LeaveGroup { chat_id } => {
                if !self.is_logged_in() {
                    self.toast("Please log in first");
                    return;
                }
                let Some(sess) = self.session.as_mut() else {
                    return;
                };
                let Some(entry) = sess.groups.get(&chat_id).cloned() else {
                    self.toast("Chat not found");
                    return;
                };

                let result = match sess.mdk.leave_group(&entry.mls_group_id) {
                    Ok(r) => r,
                    Err(e) => {
                        self.toast(format!("Leave group failed: {e}"));
                        return;
                    }
                };

                self.publish_evolution_event(
                    &chat_id,
                    entry.mls_group_id.clone(),
                    result.evolution_event,
                    None,
                    vec![],
                );

                // Navigate back to chat list.
                prune_chat_routes(&mut self.state.router.screen_stack, &chat_id);
                self.state.current_chat = None;
                self.refresh_all_from_storage();
                self.emit_router();
            }
            AppAction::RenameGroup { chat_id, name } => {
                if !self.is_logged_in() {
                    self.toast("Please log in first");
                    return;
                }
                let Some(sess) = self.session.as_mut() else {
                    return;
                };
                let Some(entry) = sess.groups.get(&chat_id).cloned() else {
                    self.toast("Chat not found");
                    return;
                };

                let update = mdk_core::prelude::NostrGroupDataUpdate::new().name(name);
                let result = match sess.mdk.update_group_data(&entry.mls_group_id, update) {
                    Ok(r) => r,
                    Err(e) => {
                        self.toast(format!("Rename failed: {e}"));
                        return;
                    }
                };

                self.publish_evolution_event(
                    &chat_id,
                    entry.mls_group_id.clone(),
                    result.evolution_event,
                    None,
                    vec![],
                );
            }
        }
    }

    /// Publish a group evolution event (commit) to relays in the background.
    ///
    /// Returns immediately after spawning — callers should clear any busy/loading
    /// state right after calling this. Relay confirmation, `merge_pending_commit`,
    /// and welcome delivery happen asynchronously via the `GroupEvolutionPublished`
    /// internal event handler.
    ///
    /// Ordering (MIP-02 / MIP-03):
    ///   1. Relay confirmation (retries with exponential backoff)
    ///   2. `merge_pending_commit` (only after relay ack)
    ///   3. Welcome delivery (only after merge)
    ///
    /// TODO: A second group mutation (add/remove member) before the background
    /// merge completes will fail because OpenMLS rejects new commits while one
    /// is pending. This surfaces as an error toast but doesn't corrupt state.
    /// Consider adding a per-group operation lock if this becomes a UX problem.
    fn publish_evolution_event(
        &mut self,
        chat_id: &str,
        mls_group_id: GroupId,
        event: Event,
        welcome_rumors: Option<Vec<UnsignedEvent>>,
        added_pubkeys: Vec<PublicKey>,
    ) {
        let fallback_relays = self.default_relays();
        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let relays: Vec<RelayUrl> = sess
            .mdk
            .get_relays(&mls_group_id)
            .ok()
            .map(|s| s.into_iter().collect())
            .filter(|v: &Vec<RelayUrl>| !v.is_empty())
            .unwrap_or_else(|| fallback_relays.clone());

        let client = sess.client.clone();
        let tx = self.core_sender.clone();
        let chat_id = chat_id.to_string();
        let mls_group_id_clone = mls_group_id.clone();

        self.runtime.spawn(async move {
            // Retry with exponential backoff, matching key-package publish pattern.
            // Some relays require NIP-42 auth before accepting protected events.
            let mut last_err: Option<String> = None;
            for attempt in 0..5u8 {
                match client.send_event_to(&relays, &event).await {
                    Ok(output) if !output.success.is_empty() => {
                        let _ = tx.send(CoreMsg::Internal(Box::new(
                            InternalEvent::GroupEvolutionPublished {
                                chat_id,
                                mls_group_id: mls_group_id_clone,
                                welcome_rumors,
                                added_pubkeys,
                                ok: true,
                                error: None,
                            },
                        )));
                        return;
                    }
                    Ok(output) => {
                        let errors: Vec<&str> =
                            output.failed.values().map(|s| s.as_str()).collect();
                        let summary = if errors.is_empty() {
                            "no relay accepted event".to_string()
                        } else {
                            errors.join("; ")
                        };
                        let any_retryable = errors.iter().any(|e| {
                            e.contains("protected")
                                || e.contains("auth")
                                || e.contains("AUTH")
                        });
                        last_err = Some(summary);
                        if !any_retryable {
                            break;
                        }
                    }
                    Err(e) => {
                        last_err = Some(e.to_string());
                    }
                }
                let delay_ms = 250u64.saturating_mul(1u64 << attempt);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            tracing::warn!(error = ?last_err, chat_id, "evolution event broadcast failed after retries");
            let _ = tx.send(CoreMsg::Internal(Box::new(
                InternalEvent::GroupEvolutionPublished {
                    chat_id,
                    mls_group_id: mls_group_id_clone,
                    welcome_rumors,
                    added_pubkeys,
                    ok: false,
                    error: last_err,
                },
            )));
        });
    }
}

fn call_timeline_ended_text(
    reason: &str,
    previous_status: Option<&CallStatus>,
    started_at: Option<i64>,
) -> String {
    if matches!(previous_status, Some(CallStatus::Ringing)) && reason == "busy" {
        return "Missed call".to_string();
    }
    if reason == "declined" {
        return "Call declined".to_string();
    }

    let mut text = "Call ended".to_string();
    if reason != "user_hangup" {
        let display = reason.replace('_', " ");
        let capitalized = display
            .split_whitespace()
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().to_string() + c.as_str(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        text.push_str(&format!(": {capitalized}"));
    }

    if let Some(start) = started_at {
        if start > 0 {
            let elapsed = now_seconds() - start;
            if elapsed > 0 {
                let hours = elapsed / 3600;
                let minutes = (elapsed % 3600) / 60;
                let secs = elapsed % 60;
                let duration = if hours > 0 {
                    format!("{hours}:{minutes:02}:{secs:02}")
                } else {
                    format!("{minutes:02}:{secs:02}")
                };
                text.push_str(&format!(" ({duration})"));
            }
        }
    }
    text
}

fn prune_chat_routes(stack: &mut Vec<Screen>, chat_id: &str) {
    stack.retain(|screen| {
        !matches!(
            screen,
            Screen::Chat { chat_id: id } | Screen::GroupInfo { chat_id: id } if id == chat_id
        )
    });
}

// (Config + interop helpers live in `config.rs` and `interop.rs`.)

#[cfg(test)]
mod tests {
    use super::{prune_chat_routes, AppCore};
    use crate::bunker_signer::{NostrConnectBunkerSignerConnector, SharedBunkerSignerConnector};
    use crate::external_signer::SharedExternalSignerBridge;
    use crate::Screen;
    use std::sync::{Arc, RwLock};

    fn make_core(data_dir: String) -> AppCore {
        let (update_tx, _update_rx) = flume::unbounded();
        let (core_tx, _core_rx) = flume::unbounded();
        let external_signer_bridge: SharedExternalSignerBridge = Arc::new(RwLock::new(None));
        let bunker_signer_connector: SharedBunkerSignerConnector = Arc::new(RwLock::new(Arc::new(
            NostrConnectBunkerSignerConnector::default(),
        )));
        AppCore::new(
            update_tx,
            core_tx,
            data_dir,
            String::new(),
            Arc::new(RwLock::new(crate::state::AppState::empty())),
            external_signer_bridge,
            bunker_signer_connector,
        )
    }

    #[test]
    fn call_timeline_persists_across_core_restart() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().to_string_lossy().into_owned();

        let mut first = make_core(data_dir.clone());
        first.append_call_timeline_event(
            "call-1:started".to_string(),
            "chat-1".to_string(),
            "Call started".to_string(),
            1_700_000_000,
        );
        assert_eq!(first.state.call_timeline.len(), 1);
        drop(first);

        let mut second = make_core(data_dir);
        second.load_call_timeline();
        assert_eq!(second.state.call_timeline.len(), 1);
        assert_eq!(second.state.call_timeline[0].id, "call-1:started");
        assert_eq!(second.state.call_timeline[0].chat_id, "chat-1");

        // The logged-key cache must be restored so duplicate transition keys
        // don't append duplicate timeline entries after restart.
        second.append_call_timeline_event(
            "call-1:started".to_string(),
            "chat-1".to_string(),
            "Call started".to_string(),
            1_700_000_001,
        );
        assert_eq!(second.state.call_timeline.len(), 1);
    }

    #[test]
    fn prune_chat_routes_removes_chat_and_group_info_for_target_chat() {
        let mut stack = vec![
            Screen::NewChat,
            Screen::Chat {
                chat_id: "chat-a".into(),
            },
            Screen::GroupInfo {
                chat_id: "chat-a".into(),
            },
            Screen::GroupInfo {
                chat_id: "chat-b".into(),
            },
        ];

        prune_chat_routes(&mut stack, "chat-a");

        assert_eq!(
            stack,
            vec![
                Screen::NewChat,
                Screen::GroupInfo {
                    chat_id: "chat-b".into()
                }
            ]
        );
    }

    #[test]
    fn prune_chat_routes_keeps_stack_when_chat_not_present() {
        let mut stack = vec![
            Screen::Chat {
                chat_id: "chat-a".into(),
            },
            Screen::GroupInfo {
                chat_id: "chat-b".into(),
            },
        ];

        prune_chat_routes(&mut stack, "chat-z");

        assert_eq!(
            stack,
            vec![
                Screen::Chat {
                    chat_id: "chat-a".into()
                },
                Screen::GroupInfo {
                    chat_id: "chat-b".into()
                }
            ]
        );
    }
}
