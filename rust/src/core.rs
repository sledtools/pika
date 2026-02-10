use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use base64::Engine as _;
use flume::Sender;

use crate::actions::AppAction;
use crate::mdk_support::{open_mdk, PikaMdk};
use crate::state::now_seconds;
use crate::state::{
    AuthState, ChatMessage, ChatSummary, ChatViewState, MessageDeliveryState, Router, Screen,
};
use crate::updates::{AppUpdate, CoreMsg, InternalEvent};

use mdk_core::prelude::{GroupId, MessageProcessingResult, NostrGroupConfigData};
use mdk_storage_traits::groups::Pagination;
use nostr_sdk::prelude::*;

const DEFAULT_GROUP_NAME: &str = "DM";
const DEFAULT_GROUP_DESCRIPTION: &str = "";

// "Popular ones" per user request; keep small for MVP.
const DEFAULT_RELAY_URLS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://relay.primal.net",
    "wss://nostr.wine",
];

// Key packages (kind 443) are NIP-70 "protected" in modern MDK.
// Many popular relays (incl. Damus/Primal/nos.lol) currently reject protected events.
// Default these to relays that accept protected kind 443 publishes (manual probe).
const DEFAULT_KEY_PACKAGE_RELAY_URLS: &[&str] = &[
    "wss://nostr-pub.wellorder.net",
    "wss://nostr-01.yakihonne.com",
    "wss://nostr-02.yakihonne.com",
    "wss://relay.satlantis.io",
];

const LOCAL_OUTBOX_MAX_PER_CHAT: usize = 8;

#[derive(Debug, Clone)]
struct GroupIndexEntry {
    mls_group_id: GroupId,
    peer_npub: String,
    peer_name: Option<String>,
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
    seq: u64,
}

struct Session {
    keys: Keys,
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

    data_dir: String,
    config: AppConfig,
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
}

impl AppCore {
    pub fn new(
        update_sender: Sender<AppUpdate>,
        core_sender: Sender<CoreMsg>,
        data_dir: String,
        shared_state: Arc<RwLock<crate::state::AppState>>,
    ) -> Self {
        let config = load_app_config(&data_dir);
        let router = Router {
            default_screen: Screen::Login,
            screen_stack: vec![],
        };
        let state = crate::state::AppState {
            rev: 0,
            router,
            auth: AuthState::LoggedOut,
            chat_list: vec![],
            current_chat: None,
            toast: None,
        };

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_time()
            .enable_io()
            .build()
            .expect("tokio runtime");

        let this = Self {
            state,
            rev: 0,
            outbox_seq: 0,
            last_outgoing_ts: 0,
            update_sender,
            core_sender,
            shared_state,
            data_dir,
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
        };

        // Ensure FfiApp.state() has an immediately-available snapshot.
        this.commit_state();
        this
    }

    fn network_enabled(&self) -> bool {
        // Used to keep Rust tests deterministic and offline.
        if let Some(disable) = self.config.disable_network {
            return !disable;
        }
        std::env::var("PIKA_DISABLE_NETWORK").ok().as_deref() != Some("1")
    }

    fn default_relays(&self) -> Vec<RelayUrl> {
        if let Some(urls) = &self.config.relay_urls {
            let parsed: Vec<RelayUrl> = urls
                .iter()
                .filter_map(|u| RelayUrl::parse(u).ok())
                .collect();
            if !parsed.is_empty() {
                return parsed;
            }
        }
        DEFAULT_RELAY_URLS
            .iter()
            .filter_map(|u| RelayUrl::parse(u).ok())
            .collect()
    }

    fn key_package_relays(&self) -> Vec<RelayUrl> {
        if let Some(urls) = &self.config.key_package_relay_urls {
            let parsed: Vec<RelayUrl> = urls
                .iter()
                .filter_map(|u| RelayUrl::parse(u).ok())
                .collect();
            if !parsed.is_empty() {
                return parsed;
            }
        }
        DEFAULT_KEY_PACKAGE_RELAY_URLS
            .iter()
            .filter_map(|u| RelayUrl::parse(u).ok())
            .collect()
    }

    fn all_session_relays(&self) -> Vec<RelayUrl> {
        // Ensure the single nostr-sdk client can publish/fetch both:
        // - normal traffic on general relays
        // - key packages (kind 443) on key-package relays
        let mut set: BTreeSet<RelayUrl> = BTreeSet::new();
        for r in self.default_relays() {
            set.insert(r);
        }
        for r in self.key_package_relays() {
            set.insert(r);
        }
        set.into_iter().collect()
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

    fn emit(&mut self, update: AppUpdate) {
        self.commit_state();
        let _ = self.update_sender.send(update);
    }

    fn commit_state(&self) {
        let snapshot = self.state.clone();
        match self.shared_state.write() {
            Ok(mut g) => *g = snapshot,
            Err(poison) => *poison.into_inner() = snapshot,
        }
    }

    fn emit_auth(&mut self) {
        let rev = self.next_rev();
        self.emit(AppUpdate::AuthChanged {
            rev,
            auth: self.state.auth.clone(),
        });
    }

    fn emit_router(&mut self) {
        let rev = self.next_rev();
        self.emit(AppUpdate::RouterChanged {
            rev,
            router: self.state.router.clone(),
        });
    }

    fn emit_chat_list(&mut self) {
        let rev = self.next_rev();
        self.emit(AppUpdate::ChatListChanged {
            rev,
            chat_list: self.state.chat_list.clone(),
        });
    }

    fn emit_current_chat(&mut self) {
        let rev = self.next_rev();
        self.emit(AppUpdate::CurrentChatChanged {
            rev,
            current_chat: self.state.current_chat.clone(),
        });
    }

    fn emit_toast(&mut self) {
        let rev = self.next_rev();
        self.emit(AppUpdate::ToastChanged {
            rev,
            toast: self.state.toast.clone(),
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

    fn push_screen(&mut self, screen: Screen) {
        self.state.router.screen_stack.push(screen);
    }

    fn open_chat_screen(&mut self, chat_id: &str) {
        // UX: creating a chat from "NewChat" should land you in the chat, with back returning to
        // the chat list (not back to the compose screen).
        if matches!(self.state.router.screen_stack.last(), Some(Screen::NewChat)) {
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
            self.state.router.default_screen = Screen::ChatList;
            self.state.router.screen_stack.clear();
            self.emit_router();
        } else {
            self.state.router.default_screen = Screen::Login;
            self.state.router.screen_stack.clear();
            self.state.current_chat = None;
            self.state.chat_list = vec![];
            self.loaded_count.clear();
            self.unread_counts.clear();
            self.delivery_overrides.clear();
            self.pending_sends.clear();
            self.local_outbox.clear();
            self.last_outgoing_ts = 0;
            self.emit_router();
            self.emit_chat_list();
            self.emit_current_chat();
        }
    }

    pub fn handle_message(&mut self, msg: CoreMsg) {
        match msg {
            CoreMsg::Action(ref action) => {
                tracing::info!(?action, "dispatch");
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
            InternalEvent::KeyPackagePublished { ok, ref error } => {
                tracing::info!(ok, ?error, "key_package_published");
                if !ok {
                    self.toast(format!(
                        "Key package publish failed: {}",
                        error.clone().unwrap_or_else(|| "unknown error".into())
                    ));
                }
            }
            InternalEvent::PublishMessageResult {
                chat_id,
                rumor_id,
                ok,
                error,
            } => {
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
            InternalEvent::PeerKeyPackageFetched {
                peer_pubkey,
                candidate_kp_relays,
                key_package_event,
                error,
            } => {
                let network_enabled = self.network_enabled();
                tracing::info!(
                    peer = %peer_pubkey.to_hex(),
                    kp_found = key_package_event.is_some(),
                    ?error,
                    kp_relays = ?candidate_kp_relays.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                    "peer_key_package_fetched"
                );
                if let Some(err) = error {
                    self.toast(err);
                    return;
                }
                let Some(kp_event) = key_package_event else {
                    self.toast("Could not find peer key package (kind 443). The peer must run Pika/MDK once (publish a key package) and you must share at least one relay.".to_string());
                    return;
                };
                let kp_event = normalize_peer_key_package_event_for_mdk(&kp_event);

                // Prefer using relays the peer advertised in their key package, but keep our
                // defaults too so we remain reachable from our existing pool.
                //
                // Interop note: some peers only advertise relays via kind 10051 (MLS Key Package
                // Relays) and do not duplicate relay tags onto the kind 443 itself. Preserve the
                // relays we used to fetch the key package so we can include them in the group.
                let peer_relays =
                    extract_relays_from_key_package_event(&kp_event).unwrap_or_default();
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
                let group_result = {
                    let Some(sess) = self.session.as_mut() else {
                        return;
                    };

                    // Validate peer key package before use (spec-v2).
                    if let Err(e) = sess.mdk.parse_key_package(&kp_event) {
                        self.toast(format!(
                            "Invalid peer key package: {e}. If this is a Marmot/WhiteNoise interop peer, ensure it publishes MIP-00 compliant tags (mls_protocol_version=1.0, encoding=base64)."
                        ));
                        return;
                    }

                    // Create group (1:1 DM).
                    let admins = vec![sess.keys.public_key(), peer_pubkey];
                    let config = NostrGroupConfigData {
                        name: DEFAULT_GROUP_NAME.to_string(),
                        description: DEFAULT_GROUP_DESCRIPTION.to_string(),
                        image_hash: None,
                        image_key: None,
                        image_nonce: None,
                        relays: group_relays.clone(),
                        admins,
                    };

                    let group_result = match sess.mdk.create_group(
                        &sess.keys.public_key(),
                        vec![kp_event.clone()],
                        config,
                    ) {
                        Ok(r) => r,
                        Err(e) => {
                            self.toast(format!("Create group failed: {e}"));
                            return;
                        }
                    };

                    group_result
                };

                // Deliver welcomes (gift-wrapped kind 444) to the peer.
                if network_enabled {
                    let mut welcome_relays = peer_relays;
                    for r in candidate_kp_relays {
                        if !welcome_relays.contains(&r) {
                            welcome_relays.push(r);
                        }
                    }
                    for r in group_relays.clone() {
                        if !welcome_relays.contains(&r) {
                            welcome_relays.push(r);
                        }
                    }
                    self.publish_welcomes_to_peer(
                        peer_pubkey,
                        group_result.welcome_rumors,
                        welcome_relays,
                    );
                }

                // Refresh state + subscriptions + navigate.
                self.refresh_all_from_storage();

                let chat_id = hex::encode(group_result.group.nostr_group_id);
                self.open_chat_screen(&chat_id);
                self.refresh_current_chat(&chat_id);
                self.emit_router();
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
                        self.toast(format!("Welcome processing failed: {e}"));
                        return;
                    }
                };

                tracing::info!(
                    nostr_group_id = %hex::encode(welcome.nostr_group_id),
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
            InternalEvent::GroupMessageReceived { event } => {
                tracing::debug!(event_id = %event.id.to_hex(), "group_message_received");
                let Some(sess) = self.session.as_mut() else {
                    tracing::warn!("group_message but no session");
                    return;
                };
                let result = match sess.mdk.process_message(&event) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!(event_id = %event.id.to_hex(), %e, "process_message failed");
                        self.toast(format!("Message decrypt failed: {e}"));
                        return;
                    }
                };
                let is_app_message =
                    matches!(result, MessageProcessingResult::ApplicationMessage(_));

                let mls_group_id: Option<GroupId> = match &result {
                    MessageProcessingResult::ApplicationMessage(msg) => {
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
                    if let Ok(Some(group)) = sess.mdk.get_group(&group_id) {
                        let chat_id = hex::encode(group.nostr_group_id);
                        let current = self.state.current_chat.as_ref().map(|c| c.chat_id.as_str());
                        if current != Some(chat_id.as_str()) {
                            *self.unread_counts.entry(chat_id.clone()).or_insert(0) += 1;
                        } else if is_app_message {
                            self.loaded_count
                                .entry(chat_id.clone())
                                .and_modify(|n| *n += 1)
                                .or_insert(51);
                        }
                        self.refresh_chat_list_from_storage();
                        self.refresh_current_chat_if_open(&chat_id);
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
                let keys = Keys::generate();
                let nsec = keys.secret_key().to_bech32().expect("infallible");
                let pubkey = keys.public_key().to_hex();
                let npub = keys.public_key().to_bech32().unwrap_or(pubkey.clone());

                let rev = self.next_rev();
                self.emit(AppUpdate::AccountCreated {
                    rev,
                    nsec,
                    pubkey: pubkey.clone(),
                    npub: npub.clone(),
                });

                if let Err(e) = self.start_session(keys) {
                    // Include the full anyhow context chain; this is critical for diagnosing
                    // keyring/SQLCipher issues on iOS.
                    self.toast(format!("Create account failed: {e:#}"));
                }
            }
            AppAction::Login { nsec } | AppAction::RestoreSession { nsec } => {
                let nsec = nsec.trim();
                if nsec.is_empty() {
                    self.toast("Enter an nsec");
                    return;
                }
                let keys = match Keys::parse(nsec) {
                    Ok(k) => k,
                    Err(e) => {
                        self.toast(format!("Invalid nsec: {e}"));
                        return;
                    }
                };
                if let Err(e) = self.start_session(keys) {
                    self.toast(format!("Login failed: {e:#}"));
                }
            }
            AppAction::Logout => {
                self.stop_session();
                self.state.auth = AuthState::LoggedOut;
                self.emit_auth();
                self.handle_auth_transition(false);
            }
            AppAction::ClearToast => {
                if self.state.toast.is_some() {
                    self.state.toast = None;
                    self.emit_toast();
                }
            }
            AppAction::Foregrounded => {
                // Native should send lifecycle signals as actions. Rust owns all state changes.
                if self.is_logged_in() {
                    self.refresh_all_from_storage();
                }
            }

            // Navigation
            AppAction::PushScreen { screen } => {
                if !self.is_logged_in() && screen != Screen::Login {
                    self.toast("Please log in first");
                    return;
                }
                self.push_screen(screen);
                self.emit_router();
            }
            AppAction::UpdateScreenStack { stack } => {
                self.state.router.screen_stack = stack;

                // If the top screen is a chat, show it. Otherwise clear current_chat.
                let top = self.state.router.screen_stack.last().cloned();
                match top {
                    Some(Screen::Chat { chat_id }) => {
                        self.unread_counts.insert(chat_id.clone(), 0);
                        self.refresh_chat_list_from_storage();
                        self.refresh_current_chat(&chat_id);
                    }
                    _ => {
                        self.state.current_chat = None;
                        self.emit_current_chat();
                    }
                }

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

                let Some(sess) = self.session.as_mut() else {
                    return;
                };

                let peer_pubkey = match PublicKey::parse(&peer_npub) {
                    Ok(p) => p,
                    Err(e) => {
                        self.toast(format!("Invalid npub: {e}"));
                        return;
                    }
                };

                // Allow "note to self" flow for local/offline testing.
                if peer_pubkey == sess.keys.public_key() {
                    let config = NostrGroupConfigData {
                        name: "Note to self".to_string(),
                        description: DEFAULT_GROUP_DESCRIPTION.to_string(),
                        image_hash: None,
                        image_key: None,
                        image_nonce: None,
                        relays: group_relays.clone(),
                        admins: vec![sess.keys.public_key()],
                    };

                    let group_result =
                        match sess
                            .mdk
                            .create_group(&sess.keys.public_key(), vec![], config)
                        {
                            Ok(r) => r,
                            Err(e) => {
                                self.toast(format!("Create chat failed: {e}"));
                                return;
                            }
                        };

                    self.refresh_all_from_storage();
                    let chat_id = hex::encode(group_result.group.nostr_group_id);
                    self.open_chat_screen(&chat_id);
                    self.refresh_current_chat(&chat_id);
                    self.emit_router();
                    return;
                }

                if !network_enabled {
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
                // Fallback relays for discovering peer key packages:
                // - key-package relays (protected kind 443 publishes for modern clients)
                // - plus our "popular" relays to support peers that only publish key packages there.
                let fallback_kp_relays = self.key_package_relays();
                let fallback_popular_relays = self.default_relays();
                tracing::info!(peer = %peer_pubkey.to_hex(), "create_chat: fetching peer key package");
                self.runtime.spawn(async move {
                    // 1) Discover the peer's key-package relays (kind 10051), per NIP-104.
                    // These events are not protected, so they can live on "popular" relays.
                    tracing::info!(peer = %peer_pubkey.to_hex(), "fetching kind 10051 (MlsKeyPackageRelays)");
                    let kp_relay_list_filter = Filter::new()
                        .author(peer_pubkey)
                        .kind(Kind::MlsKeyPackageRelays)
                        .limit(5);

                    let mut candidate_kp_relays: Vec<RelayUrl> = Vec::new();
                    if let Ok(events) = client
                        .fetch_events(kp_relay_list_filter, Duration::from_secs(6))
                        .await
                    {
                        // Choose newest.
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
                            candidate_kp_relays = extract_relays_from_key_package_relays_event(ev);
                        }
                    }

                    if candidate_kp_relays.is_empty() {
                        tracing::info!("no kind 10051 found, using fallback relays");
                        let mut s: BTreeSet<RelayUrl> = BTreeSet::new();
                        for r in fallback_kp_relays.iter().cloned() {
                            s.insert(r);
                        }
                        for r in fallback_popular_relays.iter().cloned() {
                            s.insert(r);
                        }
                        candidate_kp_relays = s.into_iter().collect();
                    }

                    tracing::info!(
                        kp_relays = ?candidate_kp_relays.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                        "fetching kind 443 from relays"
                    );

                    // Ensure these relays exist in the pool and are connected before requesting from them.
                    for r in candidate_kp_relays.iter().cloned() {
                        let _ = client.add_relay(r).await;
                    }
                    client.connect().await;
                    client.wait_for_connection(Duration::from_secs(4)).await;

                    // 2) Fetch peer key package (kind 443) from the discovered relays.
                    let kp_filter = Filter::new()
                        .author(peer_pubkey)
                        .kind(Kind::MlsKeyPackage)
                        .limit(10);

                    let res = match client
                        .fetch_events_from(
                            candidate_kp_relays.clone(),
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
                            let mut best: Option<Event> = None;
                            for e in events.into_iter() {
                                if best
                                    .as_ref()
                                    .map(|b| e.created_at > b.created_at)
                                    .unwrap_or(true)
                                {
                                    best = Some(e);
                                }
                            }
                            let _ = tx.send(CoreMsg::Internal(Box::new(
                                InternalEvent::PeerKeyPackageFetched {
                                    peer_pubkey,
                                    candidate_kp_relays: candidate_kp_relays.clone(),
                                    key_package_event: best,
                                    error: None,
                                },
                            )));
                        }
                        Err(e) => {
                            let _ = tx.send(CoreMsg::Internal(Box::new(
                                InternalEvent::PeerKeyPackageFetched {
                                    peer_pubkey,
                                    candidate_kp_relays: candidate_kp_relays.clone(),
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
            AppAction::SendMessage { chat_id, content } => {
                if !self.is_logged_in() {
                    self.toast("Please log in first");
                    return;
                }
                let network_enabled = self.network_enabled();
                let fallback_relays = self.default_relays();
                let content = content.trim().to_string();
                if content.is_empty() {
                    return;
                }

                // Nostr timestamps are second-granularity; rapid sends can share the same second.
                // Keep outgoing timestamps monotonic to avoid tie-related paging nondeterminism.
                let ts = {
                    let now = now_seconds();
                    if now <= self.last_outgoing_ts {
                        self.last_outgoing_ts += 1;
                    } else {
                        self.last_outgoing_ts = now;
                    }
                    self.last_outgoing_ts
                };
                let (client, wrapper, relays, rumor_id_hex) = {
                    let Some(sess) = self.session.as_mut() else {
                        return;
                    };
                    let Some(group) = sess.groups.get(&chat_id).cloned() else {
                        self.toast("Chat not found");
                        return;
                    };

                    // Build rumor and ensure stable id for optimistic UI.
                    let mut rumor = UnsignedEvent::new(
                        sess.keys.public_key(),
                        Timestamp::from(ts as u64),
                        Kind::Custom(9),
                        [],
                        content.clone(),
                    );
                    rumor.ensure_id();
                    let rumor_id_hex = rumor.id().to_hex();

                    // Optimistic UI: mark as pending immediately.
                    self.delivery_overrides
                        .entry(chat_id.clone())
                        .or_default()
                        .insert(rumor_id_hex.clone(), MessageDeliveryState::Pending);

                    // Ensure UI can render the message even if MDK storage doesn't immediately
                    // surface it in get_messages().
                    self.outbox_seq = self.outbox_seq.wrapping_add(1);
                    let seq = self.outbox_seq;
                    self.local_outbox
                        .entry(chat_id.clone())
                        .or_default()
                        .insert(
                            rumor_id_hex.clone(),
                            LocalOutgoing {
                                content: content.clone(),
                                timestamp: ts,
                                sender_pubkey: sess.keys.public_key().to_hex(),
                                seq,
                            },
                        );

                    let wrapper = match sess.mdk.create_message(&group.mls_group_id, rumor) {
                        Ok(e) => e,
                        Err(e) => {
                            self.toast(format!("Encrypt failed: {e}"));
                            self.delivery_overrides
                                .entry(chat_id.clone())
                                .or_default()
                                .insert(
                                    rumor_id_hex.clone(),
                                    MessageDeliveryState::Failed {
                                        reason: format!("encrypt failed: {e}"),
                                    },
                                );
                            // Reflect failure immediately in the UI.
                            self.refresh_current_chat_if_open(&chat_id);
                            self.refresh_chat_list_from_storage();
                            return;
                        }
                    };

                    // Save wrapper for retries.
                    self.pending_sends
                        .entry(chat_id.clone())
                        .or_default()
                        .insert(
                            rumor_id_hex.clone(),
                            PendingSend {
                                wrapper_event: wrapper.clone(),
                                rumor_id_hex: rumor_id_hex.clone(),
                            },
                        );

                    let relays: Vec<RelayUrl> = if network_enabled {
                        sess.mdk
                            .get_relays(&group.mls_group_id)
                            .ok()
                            .map(|s| s.into_iter().collect())
                            .filter(|v: &Vec<RelayUrl>| !v.is_empty())
                            .unwrap_or_else(|| fallback_relays.clone())
                    } else {
                        vec![]
                    };

                    (sess.client.clone(), wrapper, relays, rumor_id_hex)
                };

                // Update slices from storage (includes the new message).
                self.prune_local_outbox(&chat_id);
                self.refresh_chat_list_from_storage();
                self.refresh_current_chat_if_open(&chat_id);

                if !network_enabled {
                    // Deterministic tests: treat as immediate success.
                    let _ = self.core_sender.send(CoreMsg::Internal(Box::new(
                        InternalEvent::PublishMessageResult {
                            chat_id,
                            rumor_id: rumor_id_hex,
                            ok: true,
                            error: None,
                        },
                    )));
                    return;
                }

                let tx = self.core_sender.clone();
                self.runtime.spawn(async move {
                    let out = client.send_event_to(relays, &wrapper).await;
                    match out {
                        Ok(output) => {
                            let ok = !output.success.is_empty();
                            let err = if ok {
                                None
                            } else {
                                Some(
                                    output
                                        .failed
                                        .values()
                                        .next()
                                        .cloned()
                                        .unwrap_or_else(|| "no relay accepted event".into()),
                                )
                            };
                            let _ = tx.send(CoreMsg::Internal(Box::new(
                                InternalEvent::PublishMessageResult {
                                    chat_id,
                                    rumor_id: rumor_id_hex,
                                    ok,
                                    error: err,
                                },
                            )));
                        }
                        Err(e) => {
                            let _ = tx.send(CoreMsg::Internal(Box::new(
                                InternalEvent::PublishMessageResult {
                                    chat_id,
                                    rumor_id: rumor_id_hex,
                                    ok: false,
                                    error: Some(e.to_string()),
                                },
                            )));
                        }
                    }
                });
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
                let tx = self.core_sender.clone();
                self.runtime.spawn(async move {
                    let out = client.send_event_to(relays, &ps.wrapper_event).await;
                    match out {
                        Ok(output) => {
                            let ok = !output.success.is_empty();
                            let err = if ok {
                                None
                            } else {
                                Some(
                                    output
                                        .failed
                                        .values()
                                        .next()
                                        .cloned()
                                        .unwrap_or_else(|| "no relay accepted event".into()),
                                )
                            };
                            let _ = tx.send(CoreMsg::Internal(Box::new(
                                InternalEvent::PublishMessageResult {
                                    chat_id,
                                    rumor_id: ps.rumor_id_hex,
                                    ok,
                                    error: err,
                                },
                            )));
                        }
                        Err(e) => {
                            let _ = tx.send(CoreMsg::Internal(Box::new(
                                InternalEvent::PublishMessageResult {
                                    chat_id,
                                    rumor_id: ps.rumor_id_hex,
                                    ok: false,
                                    error: Some(e.to_string()),
                                },
                            )));
                        }
                    }
                });
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
        }
    }

    fn start_session(&mut self, keys: Keys) -> anyhow::Result<()> {
        // Tear down any existing session first.
        self.stop_session();

        let pubkey = keys.public_key();
        let pubkey_hex = pubkey.to_hex();
        let npub = pubkey.to_bech32().unwrap_or(pubkey_hex.clone());

        tracing::info!(pubkey = %pubkey_hex, npub = %npub, "start_session");

        // MDK per-identity encrypted sqlite DB.
        let mdk = open_mdk(&self.data_dir, &pubkey)?;
        tracing::info!("mdk opened");

        let client = Client::new(keys.clone());

        if self.network_enabled() {
            let relays = self.all_session_relays();
            tracing::info!(relays = ?relays.iter().map(|r| r.to_string()).collect::<Vec<_>>(), "connecting_relays");
            let c = client.clone();
            self.runtime.spawn(async move {
                for r in relays {
                    let _ = c.add_relay(r).await;
                }
                c.connect().await;
            });
            tracing::info!("relays connected");
        }

        let sess = Session {
            keys: keys.clone(),
            mdk,
            client: client.clone(),
            alive: Arc::new(AtomicBool::new(true)),
            giftwrap_sub: None,
            group_sub: None,
            groups: HashMap::new(),
        };

        self.session = Some(sess);

        self.state.auth = AuthState::LoggedIn {
            npub,
            pubkey: pubkey_hex,
        };
        self.emit_auth();
        self.handle_auth_transition(true);

        // Start notifications processing (async -> internal events).
        if self.network_enabled() {
            self.start_notifications_loop();
        }

        self.refresh_all_from_storage();

        if self.network_enabled() {
            self.publish_key_package_relays_best_effort();
            self.ensure_key_package_published_best_effort();
            self.recompute_subscriptions();
        }

        Ok(())
    }

    fn stop_session(&mut self) {
        // Invalidate/stop any in-flight subscription recompute tasks.
        self.subs_recompute_token = self.subs_recompute_token.wrapping_add(1);
        self.subs_recompute_in_flight = false;
        self.subs_recompute_dirty = false;

        if let Some(sess) = self.session.take() {
            sess.alive.store(false, Ordering::SeqCst);
            if self.network_enabled() {
                let client = sess.client.clone();
                self.runtime.spawn(async move {
                    client.unsubscribe_all().await;
                    client.shutdown().await;
                });
            }
        }
    }

    fn start_notifications_loop(&mut self) {
        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let mut rx = sess.client.notifications();
        let client = sess.client.clone();
        let tx = self.core_sender.clone();
        self.runtime.spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(RelayPoolNotification::Message { relay_url, message }) => {
                        // NIP-42 auth is required by many relays to publish NIP-70 "protected" events.
                        // MDK marks key packages (kind 443) as protected, so we must respond to AUTH
                        // challenges or publishing will be rejected ("blocked: event marked as protected").
                        if let RelayMessage::Auth { challenge } = message {
                            // nostr-sdk 0.44 doesn't expose a `Client::auth` helper; build/sign/send.
                            if let Ok(event) = client
                                .sign_event_builder(EventBuilder::auth(
                                    challenge,
                                    relay_url.clone(),
                                ))
                                .await
                            {
                                let _ = client
                                    .send_msg_to([relay_url], ClientMessage::auth(event))
                                    .await;
                            }
                        }
                    }
                    Ok(RelayPoolNotification::Event { event, .. }) => {
                        let ev: Event = (*event).clone();
                        match ev.kind {
                            Kind::GiftWrap => {
                                match client.unwrap_gift_wrap(&ev).await {
                                    Ok(unwrapped) => {
                                        let _ = tx.send(CoreMsg::Internal(Box::new(
                                            InternalEvent::GiftWrapReceived {
                                                wrapper: ev,
                                                rumor: unwrapped.rumor,
                                            },
                                        )));
                                    }
                                    Err(_) => {
                                        // Ignore malformed/unreadable giftwrap.
                                    }
                                }
                            }
                            Kind::MlsGroupMessage => {
                                let _ = tx.send(CoreMsg::Internal(Box::new(
                                    InternalEvent::GroupMessageReceived { event: ev },
                                )));
                            }
                            _ => {}
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    fn ensure_key_package_published_best_effort(&mut self) {
        let relays = self.key_package_relays();
        let Some(sess) = self.session.as_mut() else {
            return;
        };
        let (content, tags) = match sess
            .mdk
            .create_key_package_for_event(&sess.keys.public_key(), relays.clone())
        {
            Ok(v) => v,
            Err(e) => {
                self.toast(format!("Key package create failed: {e}"));
                return;
            }
        };

        let builder = EventBuilder::new(Kind::MlsKeyPackage, content).tags(tags);
        let event = match builder.sign_with_keys(&sess.keys) {
            Ok(e) => e,
            Err(e) => {
                self.toast(format!("Key package sign failed: {e}"));
                return;
            }
        };

        let client = sess.client.clone();
        let tx = self.core_sender.clone();
        self.runtime.spawn(async move {
            // Ensure these relays exist in the pool. (Session startup adds defaults, but config can change.)
            for r in relays.iter().cloned() {
                let _ = client.add_relay(r).await;
            }
            client.connect().await;
            client.wait_for_connection(Duration::from_secs(4)).await;

            // Best-effort with retries: some relays require NIP-42 auth before accepting
            // protected events. They will emit an AUTH challenge; we respond in the
            // notifications loop, then retry publishing.
            let mut last_err: Option<String> = None;
            for attempt in 0..5u8 {
                let out = client.send_event_to(&relays, &event).await;
                match out {
                    Ok(output) => {
                        if !output.success.is_empty() {
                            let _ = tx.send(CoreMsg::Internal(Box::new(
                                InternalEvent::KeyPackagePublished {
                                    ok: true,
                                    error: None,
                                },
                            )));
                            return;
                        }
                        // Aggregate a representative error string.
                        let err = output
                            .failed
                            .values()
                            .next()
                            .cloned()
                            .unwrap_or_else(|| "no relay accepted event".into());
                        last_err = Some(err.clone());

                        // Retry on common transient causes (auth-required / policy blocks).
                        // This keeps v2 usable on relays that require a NIP-42 AUTH handshake
                        // before accepting NIP-70 protected events.
                        let should_retry = err.contains("event marked as protected")
                            || err.contains("protected")
                            || err.contains("auth")
                            || err.contains("AUTH");
                        if !should_retry {
                            break;
                        }
                    }
                    Err(e) => {
                        last_err = Some(e.to_string());
                    }
                }

                // Backoff: 250ms, 500ms, 1s, 2s, 4s (bounded).
                let delay_ms = 250u64.saturating_mul(1u64 << attempt);
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }

            let _ = tx.send(CoreMsg::Internal(Box::new(
                InternalEvent::KeyPackagePublished {
                    ok: false,
                    error: last_err,
                },
            )));
        });
    }

    fn publish_key_package_relays_best_effort(&mut self) {
        let general_relays = self.default_relays();
        let kp_relays = self.key_package_relays();
        let Some(sess) = self.session.as_ref() else {
            return;
        };

        if general_relays.is_empty() || kp_relays.is_empty() {
            return;
        }

        let tags: Vec<Tag> = kp_relays.iter().cloned().map(Tag::relay).collect();

        let builder = EventBuilder::new(Kind::MlsKeyPackageRelays, "").tags(tags);
        let event = match builder.sign_with_keys(&sess.keys) {
            Ok(e) => e,
            Err(_) => return,
        };

        let client = sess.client.clone();
        self.runtime.spawn(async move {
            // Ensure general relays exist.
            for r in general_relays.iter().cloned() {
                let _ = client.add_relay(r).await;
            }
            client.connect().await;
            client.wait_for_connection(Duration::from_secs(4)).await;
            let _ = client.send_event_to(general_relays, &event).await;
        });
    }

    fn recompute_subscriptions(&mut self) {
        let network_enabled = self.network_enabled();
        if !network_enabled {
            return;
        }
        if self.subs_recompute_in_flight {
            self.subs_recompute_dirty = true;
            return;
        }
        // Ensure the client is connected to all relays referenced by joined groups.
        // Without this, we may subscribe to #h filters but never actually see events because
        // the relay URLs were never added to the client pool.
        let mut needed_relays: Vec<RelayUrl> = self.all_session_relays();
        if let Some(sess) = self.session.as_ref() {
            for entry in sess.groups.values() {
                if let Ok(set) = sess.mdk.get_relays(&entry.mls_group_id) {
                    for r in set.into_iter() {
                        if !needed_relays.contains(&r) {
                            needed_relays.push(r);
                        }
                    }
                }
            }
        }

        let Some(sess) = self.session.as_mut() else {
            return;
        };

        self.subs_recompute_in_flight = true;
        self.subs_recompute_dirty = false;
        self.subs_recompute_token = self.subs_recompute_token.wrapping_add(1);
        let token = self.subs_recompute_token;

        let client = sess.client.clone();
        let tx = self.core_sender.clone();
        let my_hex = sess.keys.public_key().to_hex();
        let prev_giftwrap_sub = sess.giftwrap_sub.clone();
        let prev_group_sub = sess.group_sub.clone();
        let h_values: Vec<String> = sess.groups.keys().cloned().collect();
        let alive = sess.alive.clone();

        self.runtime.spawn(async move {
            // Session lifecycle guard: if the user logs out while this task is in-flight, avoid
            // side effects like reconnecting or re-subscribing for a dead session.
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            for r in needed_relays {
                let _ = client.add_relay(r).await;
            }
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            client.connect().await;
            client.wait_for_connection(Duration::from_secs(4)).await;
            if !alive.load(Ordering::SeqCst) {
                return;
            }

            // Tear down previous subscriptions for a clean recompute.
            if let Some(id) = prev_giftwrap_sub {
                let _ = client.unsubscribe(&id).await;
            }
            if let Some(id) = prev_group_sub {
                let _ = client.unsubscribe(&id).await;
            }
            if !alive.load(Ordering::SeqCst) {
                return;
            }

            // GiftWrap inbox subscription (kind GiftWrap, #p = me).
            // NOTE: Filter `pubkey` matches the event author; GiftWraps can be authored by anyone,
            // so we must filter by the recipient `p` tag (spec-v2).
            let gift_filter = Filter::new()
                .kind(Kind::GiftWrap)
                .custom_tags(SingleLetterTag::lowercase(Alphabet::P), vec![my_hex]);
            let giftwrap_sub = client
                .subscribe(gift_filter, None)
                .await
                .ok()
                .map(|o| o.val);

            // Group subscription: kind 445 filtered by #h for all joined groups.
            let group_sub = if h_values.is_empty() {
                None
            } else {
                let group_filter = Filter::new()
                    .kind(Kind::MlsGroupMessage)
                    .custom_tags(SingleLetterTag::lowercase(Alphabet::H), h_values);
                client
                    .subscribe(group_filter, None)
                    .await
                    .ok()
                    .map(|o| o.val)
            };

            let _ = tx.send(CoreMsg::Internal(Box::new(
                InternalEvent::SubscriptionsRecomputed {
                    token,
                    giftwrap_sub,
                    group_sub,
                },
            )));
        });
    }

    fn publish_welcomes_to_peer(
        &mut self,
        peer_pubkey: PublicKey,
        welcome_rumors: Vec<UnsignedEvent>,
        relays: Vec<RelayUrl>,
    ) {
        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let client = sess.client.clone();
        self.runtime.spawn(async move {
            for r in relays.iter().cloned() {
                let _ = client.add_relay(r).await;
            }
            client.connect().await;
            client.wait_for_connection(Duration::from_secs(4)).await;

            let expires = Timestamp::from_secs(Timestamp::now().as_secs() + 30 * 24 * 60 * 60);
            let tags = vec![Tag::expiration(expires)];
            for rumor in welcome_rumors {
                let _ = client
                    .gift_wrap_to(relays.clone(), &peer_pubkey, rumor, tags.clone())
                    .await;
            }
        });
    }

    fn delete_event_best_effort(&mut self, id: EventId) {
        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let client = sess.client.clone();
        let keys = sess.keys.clone();
        let relays = self.default_relays();
        self.runtime.spawn(async move {
            let req = EventDeletionRequest::new()
                .id(id)
                .reason("rotated key package");
            if let Ok(ev) = EventBuilder::delete(req).sign_with_keys(&keys) {
                let _ = client.send_event_to(relays, &ev).await;
            }
        });
    }

    fn refresh_all_from_storage(&mut self) {
        self.refresh_chat_list_from_storage();
        if let Some(Screen::Chat { chat_id }) = self.state.router.screen_stack.last().cloned() {
            self.refresh_current_chat(&chat_id);
        }
        if self.network_enabled() {
            self.recompute_subscriptions();
        }
    }

    fn refresh_chat_list_from_storage(&mut self) {
        let Some(sess) = self.session.as_mut() else {
            self.state.chat_list = vec![];
            self.emit_chat_list();
            return;
        };

        let groups = match sess.mdk.get_groups() {
            Ok(gs) => gs,
            Err(e) => {
                self.toast(format!("Storage error: {e}"));
                return;
            }
        };

        let my_pubkey = sess.keys.public_key();
        let mut index: HashMap<String, GroupIndexEntry> = HashMap::new();
        let mut list: Vec<ChatSummary> = Vec::new();

        for g in groups {
            // Map to chat_id = hex(nostr_group_id)
            let chat_id = hex::encode(g.nostr_group_id);

            // Determine peer for 1:1 (or note-to-self).
            let peer_pubkey = sess.mdk.get_members(&g.mls_group_id).ok().and_then(
                |members: BTreeSet<PublicKey>| members.into_iter().find(|p| p != &my_pubkey),
            );
            let peer_npub = peer_pubkey
                .as_ref()
                .and_then(|p| p.to_bech32().ok())
                .unwrap_or_else(|| my_pubkey.to_bech32().unwrap_or_else(|_| my_pubkey.to_hex()));

            // Do not rely on `last_message_id` being populated in all MDK flows.
            // For MVP scale, fetching the newest message per group is cheap and robust.
            let newest = sess
                .mdk
                .get_messages(&g.mls_group_id, Some(Pagination::new(Some(1), Some(0))))
                .ok()
                .and_then(|v| v.into_iter().next());

            let stored_last_message = newest.as_ref().map(|m| m.content.clone());
            let stored_last_message_at = newest
                .as_ref()
                .map(|m| m.created_at.as_secs() as i64)
                .or_else(|| g.last_message_at.map(|t| t.as_secs() as i64));

            // Merge with local optimistic outbox (if any). If storage doesn't show the new message
            // yet, we still want chat list previews to update immediately.
            let local_last = self.local_outbox.get(&chat_id).and_then(|m| {
                m.values()
                    .max_by(|a, b| {
                        a.timestamp
                            .cmp(&b.timestamp)
                            .then_with(|| a.seq.cmp(&b.seq))
                    })
                    .cloned()
            });
            let local_last_at = local_last.as_ref().map(|m| m.timestamp);

            let (last_message, last_message_at) = match (stored_last_message_at, local_last_at) {
                (Some(a), Some(b)) if b > a => {
                    (local_last.as_ref().map(|m| m.content.clone()), Some(b))
                }
                (None, Some(b)) => (local_last.as_ref().map(|m| m.content.clone()), Some(b)),
                _ => (stored_last_message, stored_last_message_at),
            };

            let unread_count = *self.unread_counts.get(&chat_id).unwrap_or(&0);

            list.push(ChatSummary {
                chat_id: chat_id.clone(),
                peer_npub: peer_npub.clone(),
                peer_name: None,
                last_message,
                last_message_at,
                unread_count,
            });

            index.insert(
                chat_id,
                GroupIndexEntry {
                    mls_group_id: g.mls_group_id,
                    peer_npub,
                    peer_name: None,
                },
            );
        }

        list.sort_by_key(|c| std::cmp::Reverse(c.last_message_at.unwrap_or(0)));
        sess.groups = index;
        self.state.chat_list = list;
        self.emit_chat_list();
    }

    fn chat_exists(&self, chat_id: &str) -> bool {
        self.session
            .as_ref()
            .map(|s| s.groups.contains_key(chat_id))
            .unwrap_or(false)
    }

    fn refresh_current_chat_if_open(&mut self, chat_id: &str) {
        if self.state.current_chat.as_ref().map(|c| c.chat_id.as_str()) == Some(chat_id) {
            self.refresh_current_chat(chat_id);
        }
    }

    fn refresh_current_chat(&mut self, chat_id: &str) {
        let Some(sess) = self.session.as_mut() else {
            self.state.current_chat = None;
            self.emit_current_chat();
            return;
        };
        let Some(entry) = sess.groups.get(chat_id).cloned() else {
            self.state.current_chat = None;
            self.emit_current_chat();
            return;
        };

        // Default initial load: newest 50, and preserve paging by reloading the already-loaded count.
        let desired = *self.loaded_count.get(chat_id).unwrap_or(&50usize);
        let limit = desired.max(50);
        let messages = sess
            .mdk
            .get_messages(
                &entry.mls_group_id,
                Some(Pagination::new(Some(limit), Some(0))),
            )
            .unwrap_or_default();

        let storage_len = messages.len();
        // MDK returns descending by created_at; UI wants ascending.
        let mut msgs: Vec<ChatMessage> = messages
            .into_iter()
            .rev()
            .map(|m| {
                let id = m.id.to_hex();
                let is_mine = m.pubkey == sess.keys.public_key();
                let delivery = self
                    .delivery_overrides
                    .get(chat_id)
                    .and_then(|map| map.get(&id))
                    .cloned()
                    .unwrap_or(MessageDeliveryState::Sent);
                ChatMessage {
                    id,
                    sender_pubkey: m.pubkey.to_hex(),
                    content: m.content,
                    timestamp: m.created_at.as_secs() as i64,
                    is_mine,
                    delivery,
                }
            })
            .collect();

        // Add optimistic local messages not yet visible through MDK storage.
        //
        // Important: do not inject messages older than the oldest storage-backed message in the
        // current window, or we'd break paging by showing older content "for free".
        let oldest_loaded_ts = msgs.first().map(|m| m.timestamp).unwrap_or(i64::MIN);
        let present_ids: std::collections::HashSet<String> =
            msgs.iter().map(|m| m.id.clone()).collect();
        if let Some(local) = self.local_outbox.get(chat_id).cloned() {
            for (id, lm) in local.into_iter() {
                if present_ids.contains(&id) {
                    continue;
                }
                if lm.timestamp < oldest_loaded_ts {
                    continue;
                }
                let delivery = self
                    .delivery_overrides
                    .get(chat_id)
                    .and_then(|map| map.get(&id))
                    .cloned()
                    .unwrap_or(MessageDeliveryState::Pending);
                msgs.push(ChatMessage {
                    id,
                    sender_pubkey: lm.sender_pubkey,
                    content: lm.content,
                    timestamp: lm.timestamp,
                    is_mine: true,
                    delivery,
                });
            }
            msgs.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));
        }

        // Prune optimistic outbox entries once they show up in storage-backed messages, and also
        // drop anything older than the current loaded storage window (paging correctness).
        if let Some(local) = self.local_outbox.get_mut(chat_id) {
            local.retain(|id, lm| !present_ids.contains(id) && lm.timestamp >= oldest_loaded_ts);
        }

        let can_load_older = storage_len == limit;
        // loaded_count tracks the number of storage-backed messages loaded (used for paging offsets).
        self.loaded_count.insert(chat_id.to_string(), storage_len);

        self.state.current_chat = Some(ChatViewState {
            chat_id: chat_id.to_string(),
            peer_npub: entry.peer_npub,
            peer_name: entry.peer_name,
            messages: msgs,
            can_load_older,
        });
        self.emit_current_chat();
    }

    fn load_older_messages(&mut self, chat_id: &str, limit: usize) {
        let Some(sess) = self.session.as_mut() else {
            return;
        };
        let Some(entry) = sess.groups.get(chat_id).cloned() else {
            return;
        };

        let offset = *self.loaded_count.get(chat_id).unwrap_or(&0);
        let page = sess
            .mdk
            .get_messages(
                &entry.mls_group_id,
                Some(Pagination::new(Some(limit), Some(offset))),
            )
            .unwrap_or_default();

        if page.is_empty() {
            if let Some(cur) = self.state.current_chat.as_mut() {
                if cur.chat_id == chat_id {
                    cur.can_load_older = false;
                    self.emit_current_chat();
                }
            }
            return;
        }

        let fetched_len = page.len();

        // Reverse page to ascending.
        let mut older: Vec<ChatMessage> = page
            .into_iter()
            .rev()
            .map(|m| {
                let id = m.id.to_hex();
                let is_mine = m.pubkey == sess.keys.public_key();
                let delivery = self
                    .delivery_overrides
                    .get(chat_id)
                    .and_then(|map| map.get(&id))
                    .cloned()
                    .unwrap_or(MessageDeliveryState::Sent);
                ChatMessage {
                    id,
                    sender_pubkey: m.pubkey.to_hex(),
                    content: m.content,
                    timestamp: m.created_at.as_secs() as i64,
                    is_mine,
                    delivery,
                }
            })
            .collect();

        if let Some(cur) = self.state.current_chat.as_mut() {
            if cur.chat_id == chat_id {
                older.append(&mut cur.messages);
                cur.messages = older;
                cur.can_load_older = fetched_len == limit;
                self.loaded_count
                    .insert(chat_id.to_string(), offset + fetched_len);
                self.emit_current_chat();
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
struct AppConfig {
    disable_network: Option<bool>,
    relay_urls: Option<Vec<String>>,
    key_package_relay_urls: Option<Vec<String>>,
}

fn load_app_config(data_dir: &str) -> AppConfig {
    let path = Path::new(data_dir).join("pika_config.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return AppConfig::default();
    };

    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return AppConfig::default();
    };
    let obj = v.as_object();
    if obj.is_none() {
        return AppConfig::default();
    }
    let obj = obj.unwrap();

    let disable_network = obj.get("disable_network").and_then(|v| v.as_bool());
    let relay_urls = obj
        .get("relay_urls")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty());

    let key_package_relay_urls = obj
        .get("key_package_relay_urls")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty());

    AppConfig {
        disable_network,
        relay_urls,
        key_package_relay_urls,
    }
}

fn extract_relays_from_key_package_relays_event(event: &Event) -> Vec<RelayUrl> {
    if event.kind != Kind::MlsKeyPackageRelays {
        return vec![];
    }
    let mut out: Vec<RelayUrl> = Vec::new();
    for t in event.tags.iter() {
        if let Some(TagStandard::Relay(url)) = t.as_standardized() {
            out.push(url.clone());
        }
    }
    out
}

fn extract_relays_from_key_package_event(event: &Event) -> Option<Vec<RelayUrl>> {
    for t in event.tags.iter() {
        if t.kind() == TagKind::Relays {
            let mut out = Vec::new();
            for s in t.as_slice().iter().skip(1) {
                if let Ok(u) = RelayUrl::parse(s) {
                    out.push(u);
                }
            }
            if !out.is_empty() {
                return Some(out);
            }
        }
    }
    None
}

// Best-effort compatibility for peers publishing legacy/interop keypackages:
// - protocol version "1" instead of "1.0"
// - ciphersuite "1" instead of "0x0001"
// - missing encoding tag + hex-encoded content instead of base64
//
// This does NOT re-sign the event; MDK doesn't require Nostr signature verification for
// keypackage parsing, but it does validate the credential identity matches `event.pubkey`.
fn normalize_peer_key_package_event_for_mdk(event: &Event) -> Event {
    let mut out = event.clone();

    // Determine if content looks like hex. Some interop stacks omit the encoding tag and use hex.
    let content_is_hex = {
        let s = out.content.trim();
        !s.is_empty() && s.len().is_multiple_of(2) && s.bytes().all(|b| b.is_ascii_hexdigit())
    };

    let mut encoding_value: Option<String> = None;
    for t in out.tags.iter() {
        if t.kind() == TagKind::Custom("encoding".into()) {
            if let Some(v) = t.as_slice().get(1) {
                encoding_value = Some(v.to_string());
            }
        }
    }

    let mut tags: Vec<Tag> = Vec::new();
    let mut saw_encoding = false;
    for t in out.tags.iter() {
        let kind = t.kind();
        if kind == TagKind::MlsProtocolVersion {
            let v = t.as_slice().get(1).map(|s| s.as_str()).unwrap_or("");
            if v == "1" {
                tags.push(Tag::custom(TagKind::MlsProtocolVersion, ["1.0"]));
                continue;
            }
        }
        if kind == TagKind::MlsCiphersuite {
            let v = t.as_slice().get(1).map(|s| s.as_str()).unwrap_or("");
            if v == "1" {
                tags.push(Tag::custom(TagKind::MlsCiphersuite, ["0x0001"]));
                continue;
            }
        }
        if kind == TagKind::Custom("encoding".into()) {
            saw_encoding = true;
            // We'll rewrite to base64 if we convert from hex below.
            // Otherwise keep the original tag.
            tags.push(t.clone());
            continue;
        }
        tags.push(t.clone());
    }

    // Convert legacy hex -> base64 and force encoding tag.
    // Prefer explicit encoding=hex, but also accept missing encoding when content looks hex.
    let encoding_is_hex = encoding_value
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("hex"))
        .unwrap_or(false);
    if encoding_is_hex || (!saw_encoding && content_is_hex) {
        if let Ok(bytes) = hex::decode(out.content.trim()) {
            out.content = base64::engine::general_purpose::STANDARD.encode(bytes);

            // Replace/insert encoding tag to base64.
            tags.retain(|t| t.kind() != TagKind::Custom("encoding".into()));
            tags.push(Tag::custom(TagKind::Custom("encoding".into()), ["base64"]));
        }
    } else if !saw_encoding {
        // MDK requires an explicit encoding tag; default to base64 for modern clients.
        tags.push(Tag::custom(TagKind::Custom("encoding".into()), ["base64"]));
    }

    out.tags = tags.into_iter().collect();
    out
}

fn referenced_key_package_event_id(rumor: &UnsignedEvent) -> Option<EventId> {
    rumor
        .tags
        .find(TagKind::e())
        .and_then(|t| t.content())
        .and_then(|s| EventId::from_hex(s).ok())
}
