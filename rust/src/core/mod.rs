mod config;
mod interop;
mod profile;
mod session;
mod storage;

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use flume::Sender;

use crate::actions::AppAction;
use crate::mdk_support::{open_mdk, PikaMdk};
use crate::state::now_seconds;
use crate::state::{
    AuthState, BusyState, ChatMessage, ChatSummary, ChatViewState, MessageDeliveryState,
    MyProfileState, Screen,
};
use crate::updates::{AppUpdate, CoreMsg, InternalEvent};

use mdk_core::prelude::{GroupId, MessageProcessingResult, NostrGroupConfigData};
use mdk_storage_traits::groups::Pagination;
use nostr_sdk::prelude::*;

use interop::{
    extract_relays_from_key_package_event, extract_relays_from_key_package_relays_event,
    normalize_peer_key_package_event_for_mdk, referenced_key_package_event_id,
};

const DEFAULT_GROUP_NAME: &str = "DM";
const DEFAULT_GROUP_DESCRIPTION: &str = "";

const LOCAL_OUTBOX_MAX_PER_CHAT: usize = 8;

#[derive(Debug, Clone)]
struct GroupIndexEntry {
    mls_group_id: GroupId,
    is_group: bool,
    group_name: Option<String>,
    // (pubkey, name, picture_url) for each member except self
    members: Vec<(PublicKey, Option<String>, Option<String>)>,
    admin_pubkeys: Vec<String>,
}

#[derive(Debug, Clone)]
struct ProfileCache {
    name: Option<String>,
    picture_url: Option<String>,
    fetched_at: i64,
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

    // Nostr kind:0 profile cache (survives across session refreshes).
    profiles: HashMap<String, ProfileCache>, // hex pubkey -> cached profile
    my_metadata: Option<Metadata>,
}

impl AppCore {
    pub fn new(
        update_sender: Sender<AppUpdate>,
        core_sender: Sender<CoreMsg>,
        data_dir: String,
        shared_state: Arc<RwLock<crate::state::AppState>>,
    ) -> Self {
        let config = config::load_app_config(&data_dir);
        let state = crate::state::AppState::empty();

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
            profiles: HashMap::new(),
            my_metadata: None,
        };

        // Ensure FfiApp.state() has an immediately-available snapshot.
        let snapshot = this.state.clone();
        this.commit_state_snapshot(&snapshot);
        this
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
            self.state.router.default_screen = Screen::ChatList;
            self.state.router.screen_stack.clear();
            self.emit_router();
        } else {
            self.state.router.default_screen = Screen::Login;
            self.state.router.screen_stack.clear();
            self.state.current_chat = None;
            self.state.chat_list = vec![];
            self.state.busy = BusyState::idle();
            self.loaded_count.clear();
            self.unread_counts.clear();
            self.delivery_overrides.clear();
            self.pending_sends.clear();
            self.local_outbox.clear();
            self.profiles.clear();
            self.my_metadata = None;
            self.state.my_profile = MyProfileState::empty();
            self.state.follow_list = vec![];
            self.last_outgoing_ts = 0;
            self.emit_router();
            self.emit_busy();
            self.emit_chat_list();
            self.emit_current_chat();
        }
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
                            self.set_busy(|b| b.creating_chat = false);
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
            InternalEvent::ProfilesFetched { profiles } => {
                let now = now_seconds();
                for (hex_pubkey, name, picture_url) in profiles {
                    self.profiles.insert(
                        hex_pubkey,
                        ProfileCache {
                            name,
                            picture_url,
                            fetched_at: now,
                        },
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

                    let admins = vec![sess.keys.public_key()];

                    let config = NostrGroupConfigData {
                        name: group_name.clone(),
                        description: DEFAULT_GROUP_DESCRIPTION.to_string(),
                        image_hash: None,
                        image_key: None,
                        image_nonce: None,
                        relays: group_relays.clone(),
                        admins,
                    };

                    let group_result = match sess.mdk.create_group(
                        &sess.keys.public_key(),
                        kp_events.clone(),
                        config,
                    ) {
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
                    self.set_busy(|b| b.creating_chat = false);
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

                self.set_busy(|b| b.creating_chat = false);
                self.refresh_all_from_storage();
            }
            InternalEvent::FollowListFetched { entries } => {
                let now = now_seconds();
                // Update profile cache with newly fetched data.
                for (hex_pubkey, name, picture_url) in &entries {
                    self.profiles.insert(
                        hex_pubkey.clone(),
                        ProfileCache {
                            name: name.clone(),
                            picture_url: picture_url.clone(),
                            fetched_at: now,
                        },
                    );
                }
                // Build follow list entries.
                let mut follow_list: Vec<crate::state::FollowListEntry> = entries
                    .into_iter()
                    .map(|(hex_pubkey, name, picture_url)| {
                        let npub = PublicKey::from_hex(&hex_pubkey)
                            .ok()
                            .and_then(|pk| pk.to_bech32().ok())
                            .unwrap_or_else(|| hex_pubkey.clone());
                        crate::state::FollowListEntry {
                            pubkey: hex_pubkey,
                            npub,
                            name,
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
                self.set_busy(|b| b.fetching_follow_list = false);
                // Refresh chat list too since profiles were updated.
                self.refresh_chat_list_from_storage();
                if let Some(chat) = self.state.current_chat.as_ref() {
                    let chat_id = chat.chat_id.clone();
                    self.refresh_current_chat(&chat_id);
                }
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
            AppAction::Logout => {
                // Delete the MLS database before tearing down the session so stale
                // ratchet state doesn't persist across logins.
                if let Some(sess) = self.session.as_ref() {
                    let db_path = crate::mdk_support::mdk_db_path(
                        &self.data_dir,
                        &sess.keys.public_key().to_hex(),
                    );
                    if let Err(e) = std::fs::remove_file(&db_path) {
                        tracing::warn!(%e, path = %db_path.display(), "failed to delete mdk db on logout");
                    } else {
                        tracing::info!(path = %db_path.display(), "deleted mdk db on logout");
                    }
                }
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
                    self.refresh_my_profile(false);
                    self.refresh_follow_list();
                }
            }
            AppAction::RefreshFollowList => {
                if !self.is_logged_in() {
                    return;
                }
                self.refresh_follow_list();
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
                    Some(s) => s.keys.public_key(),
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

                    let group_result =
                        match sess
                            .mdk
                            .create_group(&sess.keys.public_key(), vec![], config)
                        {
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
                self.state
                    .router
                    .screen_stack
                    .retain(|s| !matches!(s, Screen::Chat { chat_id: id } if id == &chat_id));
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
            let out = client.send_event_to(&relays, &event).await;
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
                                .unwrap_or_else(|| "no relay accepted".into()),
                        )
                    };
                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::GroupEvolutionPublished {
                            chat_id,
                            mls_group_id: mls_group_id_clone,
                            welcome_rumors,
                            added_pubkeys,
                            ok,
                            error: err,
                        },
                    )));
                }
                Err(e) => {
                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::GroupEvolutionPublished {
                            chat_id,
                            mls_group_id: mls_group_id_clone,
                            welcome_rumors: None,
                            added_pubkeys: vec![],
                            ok: false,
                            error: Some(e.to_string()),
                        },
                    )));
                }
            }
        });
    }
}

// (Config + interop helpers live in `config.rs` and `interop.rs`.)
