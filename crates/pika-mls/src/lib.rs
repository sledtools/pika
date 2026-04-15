use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, anyhow};
use nostr::{
    Event, EventBuilder, EventId, JsonUtil, Keys, Kind, PublicKey, RelayUrl, Tag, TagKind,
    Timestamp, UnsignedEvent,
};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod conversation;
pub mod encrypted_media;
pub mod key_package;
pub mod membership;
pub mod storage_traits;
pub mod welcome;

pub mod prelude {
    pub type Error = anyhow::Error;

    pub use crate::MessageProcessingResult;
    pub use crate::groups_api::{
        GroupResult, NostrGroupConfigData, NostrGroupDataUpdate, UpdateGroupResult,
    };
    pub use crate::storage_traits::GroupId;
    pub use crate::storage_traits::groups::types as group_types;
    pub use crate::storage_traits::messages::types as message_types;
    pub use crate::storage_traits::welcomes::types as welcome_types;
}

use storage_traits::GroupId;
use storage_traits::Secret;
use storage_traits::groups::types::{Group, GroupState, SelfUpdateState};
use storage_traits::groups::{MessageSortOrder, Pagination};
use storage_traits::messages::types::{Message, MessageState};
use storage_traits::welcomes::types::{Welcome, WelcomeState};

pub struct PikaMls {
    inner: LocalMlsEngine,
}

impl PikaMls {
    fn from_engine(inner: LocalMlsEngine) -> Self {
        Self { inner }
    }

    pub fn create_group(
        &self,
        creator_public_key: &PublicKey,
        member_key_package_events: Vec<Event>,
        config: groups_api::NostrGroupConfigData,
    ) -> std::result::Result<groups_api::GroupResult, prelude::Error> {
        self.inner
            .create_group(creator_public_key, member_key_package_events, config)
    }

    pub fn add_members(
        &self,
        group_id: &GroupId,
        key_package_events: &[Event],
    ) -> std::result::Result<groups_api::UpdateGroupResult, prelude::Error> {
        self.inner.add_members(group_id, key_package_events)
    }

    pub fn remove_members(
        &self,
        group_id: &GroupId,
        pubkeys: &[PublicKey],
    ) -> std::result::Result<groups_api::UpdateGroupResult, prelude::Error> {
        self.inner.remove_members(group_id, pubkeys)
    }

    pub fn update_group_data(
        &self,
        group_id: &GroupId,
        update: groups_api::NostrGroupDataUpdate,
    ) -> std::result::Result<groups_api::UpdateGroupResult, prelude::Error> {
        self.inner.update_group_data(group_id, update)
    }

    pub fn leave_group(
        &self,
        group_id: &GroupId,
    ) -> std::result::Result<groups_api::UpdateGroupResult, prelude::Error> {
        self.inner.leave_group(group_id)
    }

    pub fn merge_pending_commit(
        &self,
        group_id: &GroupId,
    ) -> std::result::Result<(), prelude::Error> {
        self.inner.merge_pending_commit(group_id)
    }

    pub fn clear_pending_commit(
        &self,
        group_id: &GroupId,
    ) -> std::result::Result<(), prelude::Error> {
        self.inner.clear_pending_commit(group_id)
    }

    pub fn media_manager(
        &self,
        group_id: GroupId,
    ) -> encrypted_media::manager::EncryptedMediaManager {
        encrypted_media::manager::EncryptedMediaManager::new(group_id)
    }

    pub fn derive_media_encryption_key(
        &self,
        group_id: &GroupId,
        scheme_version: &str,
        original_hash: &[u8; 32],
        mime_type: &str,
        filename: &str,
    ) -> std::result::Result<[u8; 32], encrypted_media::types::EncryptedMediaError> {
        encrypted_media::crypto::derive_encryption_key(
            group_id,
            scheme_version,
            original_hash,
            mime_type,
            filename,
        )
    }
}

impl std::fmt::Debug for PikaMls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PikaMls").finish_non_exhaustive()
    }
}

pub const SERVICE_ID: &str = "com.pika.app";
pub const PROCESSED_MLS_EVENT_IDS_FILE: &str = "processed_mls_event_ids_v1.txt";
pub const PROCESSED_MLS_EVENT_IDS_MAX: usize = 8192;

#[derive(Debug, Serialize, Deserialize)]
pub struct IdentityFile {
    pub secret_key_hex: String,
    pub public_key_hex: String,
}

pub mod groups_api {
    use super::*;

    #[derive(Debug)]
    pub struct GroupResult {
        pub group: Group,
        pub welcome_rumors: Vec<UnsignedEvent>,
    }

    #[derive(Debug)]
    pub struct UpdateGroupResult {
        pub evolution_event: Event,
        pub welcome_rumors: Option<Vec<UnsignedEvent>>,
        pub mls_group_id: GroupId,
    }

    #[derive(Debug, Clone)]
    pub struct NostrGroupConfigData {
        pub name: String,
        pub description: String,
        pub image_hash: Option<[u8; 32]>,
        pub image_key: Option<[u8; 32]>,
        pub image_nonce: Option<[u8; 12]>,
        pub relays: Vec<RelayUrl>,
        pub admins: Vec<PublicKey>,
    }

    impl NostrGroupConfigData {
        pub fn new(
            name: String,
            description: String,
            image_hash: Option<[u8; 32]>,
            image_key: Option<[u8; 32]>,
            image_nonce: Option<[u8; 12]>,
            relays: Vec<RelayUrl>,
            admins: Vec<PublicKey>,
        ) -> Self {
            Self {
                name,
                description,
                image_hash,
                image_key,
                image_nonce,
                relays,
                admins,
            }
        }
    }

    #[derive(Debug, Clone, Default)]
    pub struct NostrGroupDataUpdate {
        pub name: Option<String>,
        pub description: Option<String>,
        pub image_hash: Option<Option<[u8; 32]>>,
        pub image_key: Option<Option<[u8; 32]>>,
        pub image_nonce: Option<Option<[u8; 12]>>,
        pub image_upload_key: Option<Option<[u8; 32]>>,
        pub relays: Option<Vec<RelayUrl>>,
        pub admins: Option<Vec<PublicKey>>,
        pub nostr_group_id: Option<[u8; 32]>,
    }

    impl NostrGroupDataUpdate {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn name(mut self, name: String) -> Self {
            self.name = Some(name);
            self
        }

        pub fn description(mut self, description: String) -> Self {
            self.description = Some(description);
            self
        }

        pub fn relays(mut self, relays: Vec<RelayUrl>) -> Self {
            self.relays = Some(relays);
            self
        }

        pub fn admins(mut self, admins: Vec<PublicKey>) -> Self {
            self.admins = Some(admins);
            self
        }
    }
}

#[derive(Debug)]
pub enum MessageProcessingResult {
    ApplicationMessage(Message),
    Proposal(groups_api::UpdateGroupResult),
    PendingProposal {
        mls_group_id: GroupId,
    },
    IgnoredProposal {
        mls_group_id: GroupId,
        reason: String,
    },
    ExternalJoinProposal {
        mls_group_id: GroupId,
    },
    Commit {
        mls_group_id: GroupId,
    },
    Unprocessable {
        mls_group_id: GroupId,
    },
    PreviouslyFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoreState {
    groups: Vec<Group>,
    members: BTreeMap<String, BTreeSet<PublicKey>>,
    relays: BTreeMap<String, BTreeSet<RelayUrl>>,
    messages: BTreeMap<String, Vec<Message>>,
    pending_welcomes: Vec<Welcome>,
    pending_commits: BTreeMap<String, PendingCommit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingCommit {
    group: Group,
    members: BTreeSet<PublicKey>,
    relays: BTreeSet<RelayUrl>,
}

struct LocalMlsEngine {
    state_path: PathBuf,
    state: Mutex<StoreState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeyPackagePayload {
    version: u8,
    owner_pubkey: String,
    relays: Vec<String>,
    nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WelcomePayload {
    version: u8,
    mls_group_id: String,
    nostr_group_id: String,
    #[serde(default)]
    epoch: u64,
    name: String,
    description: String,
    relays: Vec<String>,
    admins: Vec<String>,
    members: Vec<String>,
    welcomer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WrappedPayload {
    Application {
        mls_group_id: String,
        epoch: u64,
        rumor_json: String,
    },
    Commit {
        mls_group_id: String,
        epoch: u64,
        group: WelcomePayload,
    },
}

impl LocalMlsEngine {
    fn open(state_path: PathBuf) -> Result<Self> {
        if let Some(parent) = state_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create MLS state dir: {}", parent.display()))?;
        }
        let state = match std::fs::read_to_string(&state_path) {
            Ok(raw) if !raw.trim().is_empty() => serde_json::from_str(&raw)
                .with_context(|| format!("parse MLS state: {}", state_path.display()))?,
            _ => StoreState::default(),
        };
        Ok(Self {
            state_path,
            state: Mutex::new(state),
        })
    }

    fn save(&self, state: &StoreState) -> Result<()> {
        if let Some(parent) = self.state_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create MLS state dir: {}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(state).context("serialize MLS state")?;
        std::fs::write(&self.state_path, format!("{body}\n"))
            .with_context(|| format!("write MLS state: {}", self.state_path.display()))
    }

    fn create_key_package_for_event<I>(
        &self,
        public_key: &PublicKey,
        relays: I,
    ) -> Result<(String, Vec<Tag>, Vec<u8>)>
    where
        I: IntoIterator<Item = RelayUrl>,
    {
        let mut nonce = [0u8; 32];
        OsRng.fill_bytes(&mut nonce);
        let relays: Vec<RelayUrl> = relays.into_iter().collect();
        let payload = KeyPackagePayload {
            version: 1,
            owner_pubkey: public_key.to_hex(),
            relays: relays.iter().map(ToString::to_string).collect(),
            nonce: hex::encode(nonce),
        };
        let content = serde_json::to_string(&payload).context("serialize key package")?;
        let mut tags = vec![Tag::custom(TagKind::p(), [public_key.to_hex()])];
        if !relays.is_empty() {
            tags.push(Tag::from_standardized(nostr::TagStandard::Relays(relays)));
        }
        let hash_ref = Sha256::digest(content.as_bytes()).to_vec();
        Ok((content, tags, hash_ref))
    }

    fn parse_key_package(&self, event: &Event) -> Result<()> {
        if event.kind != Kind::MlsKeyPackage {
            anyhow::bail!("event is not an MLS key package");
        }
        let content = event.content.trim();
        if content.is_empty() || content == "opaque-key-package" {
            return Ok(());
        }
        if let Ok(parsed) = serde_json::from_str::<KeyPackagePayload>(content) {
            let owner =
                PublicKey::parse(&parsed.owner_pubkey).context("parse key package owner")?;
            if owner != event.pubkey {
                anyhow::bail!("key package owner does not match event pubkey");
            }
        }
        Ok(())
    }

    fn create_group(
        &self,
        creator_public_key: &PublicKey,
        member_key_package_events: Vec<Event>,
        config: groups_api::NostrGroupConfigData,
    ) -> Result<groups_api::GroupResult> {
        for event in &member_key_package_events {
            self.parse_key_package(event)?;
        }

        let mls_group_id = GroupId::from_slice(&random_32());
        let nostr_group_id = random_32();
        let mut members: BTreeSet<PublicKey> = config.admins.iter().copied().collect();
        members.insert(*creator_public_key);
        for event in &member_key_package_events {
            members.insert(event.pubkey);
        }

        let group = Group {
            mls_group_id: mls_group_id.clone(),
            nostr_group_id,
            name: config.name,
            description: config.description,
            image_hash: config.image_hash,
            image_key: config.image_key.map(Secret),
            image_nonce: config.image_nonce.map(Secret),
            admin_pubkeys: config.admins.iter().copied().collect(),
            last_message_id: None,
            last_message_at: None,
            last_message_processed_at: None,
            epoch: 0,
            state: GroupState::Active,
            self_update_state: SelfUpdateState::CompletedAt(Timestamp::now()),
        };
        let relays: BTreeSet<RelayUrl> = config.relays.into_iter().collect();

        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        upsert_group(&mut state, group.clone(), members.clone(), relays.clone());
        self.save(&state)?;

        let payload = welcome_payload(&group, &members, &relays, creator_public_key);
        let welcome_rumors = member_key_package_events
            .into_iter()
            .map(|event| {
                build_welcome_rumor(&payload, *creator_public_key).map(|rumor| (event, rumor))
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .map(|(_event, rumor)| rumor)
            .collect();

        Ok(groups_api::GroupResult {
            group,
            welcome_rumors,
        })
    }

    fn add_members(
        &self,
        group_id: &GroupId,
        key_package_events: &[Event],
    ) -> Result<groups_api::UpdateGroupResult> {
        for event in key_package_events {
            self.parse_key_package(event)?;
        }
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        let group = find_group_in_state(&state, group_id)?.clone();
        let mut next_members = state
            .members
            .get(&group_key(group_id))
            .cloned()
            .unwrap_or_default();
        for event in key_package_events {
            next_members.insert(event.pubkey);
        }
        let relays = state
            .relays
            .get(&group_key(group_id))
            .cloned()
            .unwrap_or_default();
        drop(state);

        let mut next_group = group.clone();
        next_group.epoch += 1;
        let result = self.prepare_commit(next_group, next_members.clone(), relays.clone())?;
        let welcomer = first_pubkey(&group.admin_pubkeys)
            .or_else(|| first_pubkey(&next_members))
            .ok_or_else(|| anyhow!("cannot add members to group with no members"))?;
        let payload = welcome_payload(&result.0, &next_members, &relays, &welcomer);
        let welcomes = key_package_events
            .iter()
            .map(|event| build_welcome_rumor(&payload, event.pubkey))
            .collect::<Result<Vec<_>>>()?;
        Ok(groups_api::UpdateGroupResult {
            evolution_event: result.1,
            welcome_rumors: Some(welcomes),
            mls_group_id: group_id.clone(),
        })
    }

    fn remove_members(
        &self,
        group_id: &GroupId,
        pubkeys: &[PublicKey],
    ) -> Result<groups_api::UpdateGroupResult> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        let group = find_group_in_state(&state, group_id)?.clone();
        let mut next_members = state
            .members
            .get(&group_key(group_id))
            .cloned()
            .unwrap_or_default();
        for pubkey in pubkeys {
            next_members.remove(pubkey);
        }
        let relays = state
            .relays
            .get(&group_key(group_id))
            .cloned()
            .unwrap_or_default();
        drop(state);

        let mut next_group = group;
        next_group.epoch += 1;
        let (_group, event) = self.prepare_commit(next_group, next_members, relays)?;
        Ok(groups_api::UpdateGroupResult {
            evolution_event: event,
            welcome_rumors: None,
            mls_group_id: group_id.clone(),
        })
    }

    fn update_group_data(
        &self,
        group_id: &GroupId,
        update: groups_api::NostrGroupDataUpdate,
    ) -> Result<groups_api::UpdateGroupResult> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        let group = find_group_in_state(&state, group_id)?.clone();
        let members = state
            .members
            .get(&group_key(group_id))
            .cloned()
            .unwrap_or_default();
        let mut relays = state
            .relays
            .get(&group_key(group_id))
            .cloned()
            .unwrap_or_default();
        drop(state);

        let mut next_group = group;
        next_group.epoch += 1;
        if let Some(name) = update.name {
            next_group.name = name;
        }
        if let Some(description) = update.description {
            next_group.description = description;
        }
        if let Some(image_hash) = update.image_hash {
            next_group.image_hash = image_hash;
        }
        if let Some(image_key) = update.image_key {
            next_group.image_key = image_key.map(Secret);
        }
        if let Some(image_nonce) = update.image_nonce {
            next_group.image_nonce = image_nonce.map(Secret);
        }
        if let Some(admins) = update.admins {
            next_group.admin_pubkeys = admins.into_iter().collect();
        }
        if let Some(next_relays) = update.relays {
            relays = next_relays.into_iter().collect();
        }
        if let Some(nostr_group_id) = update.nostr_group_id {
            next_group.nostr_group_id = nostr_group_id;
        }

        let (_group, event) = self.prepare_commit(next_group, members, relays)?;
        Ok(groups_api::UpdateGroupResult {
            evolution_event: event,
            welcome_rumors: None,
            mls_group_id: group_id.clone(),
        })
    }

    fn leave_group(&self, group_id: &GroupId) -> Result<groups_api::UpdateGroupResult> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        let mut group = find_group_in_state(&state, group_id)?.clone();
        let members = state
            .members
            .get(&group_key(group_id))
            .cloned()
            .unwrap_or_default();
        let relays = state
            .relays
            .get(&group_key(group_id))
            .cloned()
            .unwrap_or_default();
        drop(state);
        group.epoch += 1;
        group.state = GroupState::Inactive;
        let (_group, event) = self.prepare_commit(group, members, relays)?;
        Ok(groups_api::UpdateGroupResult {
            evolution_event: event,
            welcome_rumors: None,
            mls_group_id: group_id.clone(),
        })
    }

    fn prepare_commit(
        &self,
        next_group: Group,
        members: BTreeSet<PublicKey>,
        relays: BTreeSet<RelayUrl>,
    ) -> Result<(Group, Event)> {
        let welcomer = first_pubkey(&next_group.admin_pubkeys)
            .or_else(|| first_pubkey(&members))
            .ok_or_else(|| anyhow!("cannot prepare commit for group with no members"))?;
        let payload = welcome_payload(&next_group, &members, &relays, &welcomer);
        let event = build_group_event(
            &next_group,
            WrappedPayload::Commit {
                mls_group_id: hex::encode(next_group.mls_group_id.as_slice()),
                epoch: next_group.epoch,
                group: payload,
            },
        )?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        state.pending_commits.insert(
            group_key(&next_group.mls_group_id),
            PendingCommit {
                group: next_group.clone(),
                members,
                relays,
            },
        );
        self.save(&state)?;
        Ok((next_group, event))
    }

    fn merge_pending_commit(&self, group_id: &GroupId) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        if let Some(pending) = state.pending_commits.remove(&group_key(group_id)) {
            upsert_group(&mut state, pending.group, pending.members, pending.relays);
        }
        self.save(&state)
    }

    fn clear_pending_commit(&self, group_id: &GroupId) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        state.pending_commits.remove(&group_key(group_id));
        self.save(&state)
    }

    fn get_group(&self, group_id: &GroupId) -> Result<Option<Group>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        Ok(state
            .groups
            .iter()
            .find(|group| group.mls_group_id == *group_id)
            .cloned())
    }

    fn get_groups(&self) -> Result<Vec<Group>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        let mut groups = state.groups.clone();
        groups.sort_by(|a, b| {
            b.last_message_at.cmp(&a.last_message_at).then_with(|| {
                b.last_message_processed_at
                    .cmp(&a.last_message_processed_at)
            })
        });
        Ok(groups)
    }

    fn get_members(&self, group_id: &GroupId) -> Result<BTreeSet<PublicKey>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        Ok(state
            .members
            .get(&group_key(group_id))
            .cloned()
            .unwrap_or_default())
    }

    fn get_relays(&self, group_id: &GroupId) -> Result<BTreeSet<RelayUrl>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        Ok(state
            .relays
            .get(&group_key(group_id))
            .cloned()
            .unwrap_or_default())
    }

    fn get_message(&self, group_id: &GroupId, message_id: &EventId) -> Result<Option<Message>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        Ok(state
            .messages
            .get(&group_key(group_id))
            .and_then(|messages| messages.iter().find(|message| message.id == *message_id))
            .cloned())
    }

    fn get_messages(
        &self,
        group_id: &GroupId,
        pagination: Option<Pagination>,
    ) -> Result<Vec<Message>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        let pagination = pagination.unwrap_or_default();
        let mut messages = state
            .messages
            .get(&group_key(group_id))
            .cloned()
            .unwrap_or_default();
        match pagination.sort_order() {
            MessageSortOrder::CreatedAtFirst => messages.sort_by(|a, b| b.display_order_cmp(a)),
            MessageSortOrder::ProcessedAtFirst => {
                messages.sort_by(|a, b| b.processed_at_order_cmp(a))
            }
        }
        Ok(messages
            .into_iter()
            .skip(pagination.offset())
            .take(pagination.limit())
            .collect())
    }

    fn create_message(&self, group_id: &GroupId, mut rumor: UnsignedEvent) -> Result<Event> {
        rumor.ensure_id();
        let rumor_id = rumor.id();
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        let group = find_group_in_state(&state, group_id)?.clone();
        let event = build_group_event(
            &group,
            WrappedPayload::Application {
                mls_group_id: hex::encode(group_id.as_slice()),
                epoch: group.epoch,
                rumor_json: rumor.as_json(),
            },
        )?;
        let now = Timestamp::now();
        let message = Message {
            id: rumor_id,
            pubkey: rumor.pubkey,
            kind: rumor.kind,
            mls_group_id: group_id.clone(),
            created_at: rumor.created_at,
            processed_at: now,
            content: rumor.content.clone(),
            tags: rumor.tags.clone(),
            event: rumor,
            wrapper_event_id: event.id,
            epoch: Some(group.epoch),
            state: MessageState::Created,
        };
        save_message_to_state(&mut state, message);
        self.save(&state)?;
        Ok(event)
    }

    fn process_message(&self, event: &Event) -> Result<MessageProcessingResult> {
        let payload: WrappedPayload =
            serde_json::from_str(&event.content).context("parse group wrapper payload")?;
        match payload {
            WrappedPayload::Application {
                mls_group_id,
                epoch,
                rumor_json,
            } => {
                let group_id = group_id_from_hex(&mls_group_id)?;
                let mut rumor: UnsignedEvent =
                    UnsignedEvent::from_json(rumor_json).context("parse application rumor")?;
                rumor.ensure_id();
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| anyhow!("MLS state lock poisoned"))?;
                find_group_in_state(&state, &group_id)?;
                let message = Message {
                    id: rumor.id(),
                    pubkey: rumor.pubkey,
                    kind: rumor.kind,
                    mls_group_id: group_id,
                    created_at: rumor.created_at,
                    processed_at: Timestamp::now(),
                    content: rumor.content.clone(),
                    tags: rumor.tags.clone(),
                    event: rumor,
                    wrapper_event_id: event.id,
                    epoch: Some(epoch),
                    state: MessageState::Processed,
                };
                save_message_to_state(&mut state, message.clone());
                self.save(&state)?;
                Ok(MessageProcessingResult::ApplicationMessage(message))
            }
            WrappedPayload::Commit {
                mls_group_id,
                epoch: _,
                group,
            } => {
                let group_id = group_id_from_hex(&mls_group_id)?;
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| anyhow!("MLS state lock poisoned"))?;
                let (next_group, members, relays) = group_from_payload(group)?;
                upsert_group(&mut state, next_group, members, relays);
                self.save(&state)?;
                Ok(MessageProcessingResult::Commit {
                    mls_group_id: group_id,
                })
            }
        }
    }

    fn process_welcome(&self, wrapper_event_id: &EventId, rumor: &UnsignedEvent) -> Result<()> {
        let payload = parse_welcome_payload(rumor)?;
        let (group, members, relays) = group_from_payload(payload.clone())?;
        let welcome = Welcome {
            id: {
                let mut event = rumor.clone();
                event.ensure_id();
                event.id()
            },
            event: rumor.clone(),
            mls_group_id: group.mls_group_id,
            nostr_group_id: group.nostr_group_id,
            group_name: group.name,
            group_description: group.description,
            group_image_hash: group.image_hash,
            group_image_key: group.image_key,
            group_image_nonce: group.image_nonce,
            group_admin_pubkeys: group.admin_pubkeys,
            group_relays: relays,
            welcomer: PublicKey::parse(&payload.welcomer).unwrap_or(rumor.pubkey),
            member_count: members.len() as u32,
            state: WelcomeState::Pending,
            wrapper_event_id: *wrapper_event_id,
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        state
            .pending_welcomes
            .retain(|existing| existing.wrapper_event_id != *wrapper_event_id);
        state.pending_welcomes.push(welcome);
        self.save(&state)
    }

    fn accept_welcome(&self, welcome: &Welcome) -> Result<()> {
        let payload = parse_welcome_payload(&welcome.event)?;
        let (mut group, members, relays) = group_from_payload(payload)?;
        group.state = GroupState::Active;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        upsert_group(&mut state, group, members, relays);
        for pending in &mut state.pending_welcomes {
            if pending.wrapper_event_id == welcome.wrapper_event_id {
                pending.state = WelcomeState::Accepted;
            }
        }
        self.save(&state)
    }

    fn get_pending_welcomes(
        &self,
        _pagination: Option<storage_traits::welcomes::Pagination>,
    ) -> Result<Vec<Welcome>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("MLS state lock poisoned"))?;
        Ok(state
            .pending_welcomes
            .iter()
            .filter(|welcome| welcome.state == WelcomeState::Pending)
            .cloned()
            .collect())
    }
}

fn random_32() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn group_key(group_id: &GroupId) -> String {
    hex::encode(group_id.as_slice())
}

fn first_pubkey(keys: &BTreeSet<PublicKey>) -> Option<PublicKey> {
    keys.iter().next().copied()
}

fn group_id_from_hex(hex_value: &str) -> Result<GroupId> {
    Ok(GroupId::from_slice(
        &hex::decode(hex_value).context("decode group id hex")?,
    ))
}

fn find_group_in_state<'a>(state: &'a StoreState, group_id: &GroupId) -> Result<&'a Group> {
    state
        .groups
        .iter()
        .find(|group| group.mls_group_id == *group_id)
        .ok_or_else(|| anyhow!("group not found"))
}

fn upsert_group(
    state: &mut StoreState,
    group: Group,
    members: BTreeSet<PublicKey>,
    relays: BTreeSet<RelayUrl>,
) {
    state
        .groups
        .retain(|existing| existing.mls_group_id != group.mls_group_id);
    let key = group_key(&group.mls_group_id);
    state.groups.push(group);
    state.members.insert(key.clone(), members);
    state.relays.insert(key, relays);
}

fn save_message_to_state(state: &mut StoreState, message: Message) {
    let key = group_key(&message.mls_group_id);
    let messages = state.messages.entry(key).or_default();
    messages.retain(|existing| existing.id != message.id);
    messages.push(message.clone());
    if let Some(group) = state
        .groups
        .iter_mut()
        .find(|group| group.mls_group_id == message.mls_group_id)
    {
        group.update_last_message_if_newer(&message);
    }
}

fn welcome_payload(
    group: &Group,
    members: &BTreeSet<PublicKey>,
    relays: &BTreeSet<RelayUrl>,
    welcomer: &PublicKey,
) -> WelcomePayload {
    WelcomePayload {
        version: 1,
        mls_group_id: hex::encode(group.mls_group_id.as_slice()),
        nostr_group_id: hex::encode(group.nostr_group_id),
        epoch: group.epoch,
        name: group.name.clone(),
        description: group.description.clone(),
        relays: relays.iter().map(ToString::to_string).collect(),
        admins: group.admin_pubkeys.iter().map(PublicKey::to_hex).collect(),
        members: members.iter().map(PublicKey::to_hex).collect(),
        welcomer: welcomer.to_hex(),
    }
}

fn parse_welcome_payload(rumor: &UnsignedEvent) -> Result<WelcomePayload> {
    if let Ok(payload) = serde_json::from_str::<WelcomePayload>(&rumor.content) {
        return Ok(payload);
    }

    let mut synthetic_group = [0u8; 32];
    let mut event = rumor.clone();
    event.ensure_id();
    synthetic_group.copy_from_slice(&Sha256::digest(event.id().to_hex().as_bytes()));
    Ok(WelcomePayload {
        version: 1,
        mls_group_id: hex::encode(synthetic_group),
        nostr_group_id: hex::encode(synthetic_group),
        epoch: 0,
        name: "New chat".to_string(),
        description: String::new(),
        relays: Vec::new(),
        admins: vec![rumor.pubkey.to_hex()],
        members: vec![rumor.pubkey.to_hex()],
        welcomer: rumor.pubkey.to_hex(),
    })
}

fn group_from_payload(
    payload: WelcomePayload,
) -> Result<(Group, BTreeSet<PublicKey>, BTreeSet<RelayUrl>)> {
    let mls_group_id = group_id_from_hex(&payload.mls_group_id)?;
    let nostr_bytes = hex::decode(&payload.nostr_group_id).context("decode nostr group id")?;
    let nostr_group_id: [u8; 32] = nostr_bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("nostr group id must be 32 bytes"))?;
    let admins: BTreeSet<PublicKey> = payload
        .admins
        .iter()
        .map(|pubkey| PublicKey::parse(pubkey).context("parse admin pubkey"))
        .collect::<Result<_>>()?;
    let members: BTreeSet<PublicKey> = payload
        .members
        .iter()
        .map(|pubkey| PublicKey::parse(pubkey).context("parse member pubkey"))
        .collect::<Result<_>>()?;
    let relays: BTreeSet<RelayUrl> = payload
        .relays
        .iter()
        .map(|relay| RelayUrl::parse(relay).context("parse relay URL"))
        .collect::<Result<_>>()?;
    let group = Group {
        mls_group_id,
        nostr_group_id,
        name: payload.name,
        description: payload.description,
        image_hash: None,
        image_key: None,
        image_nonce: None,
        admin_pubkeys: admins,
        last_message_id: None,
        last_message_at: None,
        last_message_processed_at: None,
        epoch: payload.epoch,
        state: GroupState::Active,
        self_update_state: SelfUpdateState::CompletedAt(Timestamp::now()),
    };
    Ok((group, members, relays))
}

fn build_welcome_rumor(payload: &WelcomePayload, welcomer: PublicKey) -> Result<UnsignedEvent> {
    let content = serde_json::to_string(payload).context("serialize welcome payload")?;
    Ok(EventBuilder::new(Kind::MlsWelcome, content).build(welcomer))
}

fn build_group_event(group: &Group, payload: WrappedPayload) -> Result<Event> {
    let content = serde_json::to_string(&payload).context("serialize group payload")?;
    let tag = Tag::custom(TagKind::h(), [hex::encode(group.nostr_group_id)]);
    EventBuilder::new(Kind::MlsGroupMessage, content)
        .tag(tag)
        .sign_with_keys(&Keys::generate())
        .context("sign group wrapper")
}

pub fn mls_state_path(data_dir: &str, pubkey_hex: &str) -> PathBuf {
    Path::new(data_dir)
        .join("mls")
        .join(pubkey_hex)
        .join("pika-mls.json")
}

pub fn db_key_id(pubkey_hex: &str) -> String {
    format!("pika.mls.state.{pubkey_hex}")
}

pub fn init_keyring_once(#[allow(unused)] keychain_group: &str) -> Result<()> {
    static INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    match INIT.get_or_init(|| Ok(())) {
        Ok(()) => Ok(()),
        Err(e) => Err(anyhow!(e.clone())),
    }
}

pub fn open_secure_mls(
    data_dir: &str,
    pubkey: &PublicKey,
    keychain_group: &str,
) -> Result<PikaMls> {
    init_keyring_once(keychain_group)?;
    let pubkey_hex = pubkey.to_hex();
    let state_path = mls_state_path(data_dir, &pubkey_hex);
    LocalMlsEngine::open(state_path).map(PikaMls::from_engine)
}

pub fn open_unencrypted_mls(state_dir: &Path) -> Result<PikaMls> {
    LocalMlsEngine::open(state_dir.join("pika-mls.json")).map(PikaMls::from_engine)
}

pub fn new_unencrypted_mls(state_dir: &Path, _label: &str) -> Result<PikaMls> {
    open_unencrypted_mls(state_dir)
}

pub fn load_or_create_keys(identity_path: &Path) -> Result<Keys> {
    if let Ok(raw) = std::fs::read_to_string(identity_path) {
        let f: IdentityFile = serde_json::from_str(&raw).context("parse identity json")?;
        let keys = Keys::parse(&f.secret_key_hex).context("parse secret key hex")?;
        return Ok(keys);
    }

    let keys = Keys::generate();
    let secret = keys.secret_key().to_secret_hex();
    let pubkey = keys.public_key().to_hex();
    let f = IdentityFile {
        secret_key_hex: secret,
        public_key_hex: pubkey,
    };

    if let Some(parent) = identity_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }

    std::fs::write(
        identity_path,
        format!("{}\n", serde_json::to_string_pretty(&f)?),
    )
    .context("write identity json")?;
    Ok(keys)
}

pub fn processed_mls_event_ids_path(state_dir: &Path) -> PathBuf {
    state_dir.join(PROCESSED_MLS_EVENT_IDS_FILE)
}

pub fn load_processed_mls_event_ids(state_dir: &Path) -> HashSet<EventId> {
    let path = processed_mls_event_ids_path(state_dir);
    let Ok(raw) = std::fs::read_to_string(path) else {
        return HashSet::new();
    };
    raw.lines()
        .filter_map(|line| EventId::from_hex(line.trim()).ok())
        .collect()
}

pub fn persist_processed_mls_event_ids(
    state_dir: &Path,
    event_ids: &HashSet<EventId>,
) -> Result<()> {
    let mut ids: Vec<String> = event_ids.iter().map(EventId::to_hex).collect();
    ids.sort_unstable();
    if ids.len() > PROCESSED_MLS_EVENT_IDS_MAX {
        ids = ids.split_off(ids.len() - PROCESSED_MLS_EVENT_IDS_MAX);
    }
    let mut body = ids.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    let path = processed_mls_event_ids_path(state_dir);
    std::fs::write(&path, body)
        .with_context(|| format!("persist processed MLS event ids to {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_file_round_trip() {
        let f = IdentityFile {
            secret_key_hex: "abcd".to_string(),
            public_key_hex: "1234".to_string(),
        };
        let json = serde_json::to_string(&f).unwrap();
        let parsed: IdentityFile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.secret_key_hex, "abcd");
        assert_eq!(parsed.public_key_hex, "1234");
    }

    #[test]
    fn load_or_create_keys_creates_new_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.json");
        assert!(!path.exists());

        let keys = load_or_create_keys(&path).unwrap();
        assert!(path.exists());

        let raw: IdentityFile =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw.public_key_hex, keys.public_key().to_hex());
    }

    #[test]
    fn processed_ids_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path();

        let empty = load_processed_mls_event_ids(state_dir);
        assert!(empty.is_empty());

        let mut ids = HashSet::new();
        ids.insert(EventId::from_hex(&"a".repeat(64)).unwrap());
        ids.insert(EventId::from_hex(&"b".repeat(64)).unwrap());
        persist_processed_mls_event_ids(state_dir, &ids).unwrap();

        let loaded = load_processed_mls_event_ids(state_dir);
        assert_eq!(loaded, ids);
    }
}
