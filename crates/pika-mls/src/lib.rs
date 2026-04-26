use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use anyhow::{Context, Result, anyhow};
use base64::prelude::*;
use nostr::{
    Event, EventBuilder, EventId, JsonUtil, Keys, Kind, PublicKey, RelayUrl, Tag, TagKind,
    Timestamp, UnsignedEvent,
};
use openmls::prelude::{
    BasicCredential, Capabilities, Ciphersuite, ContentType, CredentialType, CredentialWithKey,
    ExtensionType, KeyPackage, KeyPackageIn, MlsGroup, MlsGroupCreateConfig, MlsGroupJoinConfig,
    MlsMessageBodyIn, MlsMessageIn, MlsMessageOut, ProcessedMessageContent, ProtocolMessage,
    ProtocolVersion, SenderRatchetConfiguration, StagedWelcome,
    tls_codec::{Deserialize as TlsDeserializeTrait, Serialize as TlsSerializeTrait},
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_memory_storage::MemoryStorage;
use openmls_rust_crypto::RustCrypto;
use openmls_traits::OpenMlsProvider;
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

const ENCRYPTED_STATE_SCHEME_VERSION: &str = "pika-local-openmls-state-v1";
const KEY_PACKAGE_ENVELOPE_VERSION: u8 = 1;
const WELCOME_ENVELOPE_VERSION: u8 = 1;
const MLS_MESSAGE_ENVELOPE_VERSION: u8 = 1;
const APPLICATION_PAYLOAD_VERSION: u8 = 1;
const MEDIA_EXPORTER_LABEL: &str = "pika-media-exporter-v1";
const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

pub struct PikaMls {
    inner: OpenMlsEngine,
}

impl PikaMls {
    fn from_engine(inner: OpenMlsEngine) -> Self {
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
        let key_context = self
            .inner
            .export_media_context(&group_id)
            .expect("media manager requires an active OpenMLS group");
        encrypted_media::manager::EncryptedMediaManager::new(group_id, key_context)
    }

    pub fn derive_media_encryption_key(
        &self,
        group_id: &GroupId,
        scheme_version: &str,
        original_hash: &[u8; 32],
        mime_type: &str,
        filename: &str,
    ) -> std::result::Result<[u8; 32], encrypted_media::types::EncryptedMediaError> {
        let key_context = self.inner.export_media_context(group_id).map_err(|err| {
            encrypted_media::types::EncryptedMediaError::EncryptionFailed {
                reason: err.to_string(),
            }
        })?;
        encrypted_media::crypto::derive_encryption_key(
            group_id,
            &key_context,
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
    #[serde(default)]
    account_signers: BTreeMap<String, Vec<u8>>,
    #[serde(default)]
    openmls_storage: BTreeMap<String, String>,
    #[serde(default)]
    outbound_wrappers: BTreeSet<EventId>,
    #[serde(default)]
    local_pubkey: Option<PublicKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EncryptedStoreState {
    scheme_version: String,
    nonce: String,
    ciphertext: String,
}

struct LoadedStoreState {
    state: StoreState,
    was_plaintext: bool,
}

#[derive(Debug, Clone, Copy)]
enum StateCodec {
    Plaintext,
    #[allow(dead_code)]
    Encrypted {
        key: [u8; 32],
    },
}

#[derive(Debug, Default)]
struct PikaOpenMlsProvider {
    crypto: RustCrypto,
    storage: MemoryStorage,
}

impl PikaOpenMlsProvider {
    fn with_storage(storage: MemoryStorage) -> Self {
        Self {
            crypto: RustCrypto::default(),
            storage,
        }
    }
}

impl OpenMlsProvider for PikaOpenMlsProvider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}

struct OpenMlsEngine {
    state_path: PathBuf,
    codec: StateCodec,
    provider: PikaOpenMlsProvider,
    state: Mutex<StoreState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeyPackageEnvelope {
    version: u8,
    owner_pubkey: String,
    ciphersuite: String,
    relays: Vec<String>,
    key_package: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WelcomeEnvelope {
    version: u8,
    mls_group_id: String,
    nostr_group_id: String,
    name: String,
    description: String,
    image_hash: Option<[u8; 32]>,
    image_key: Option<[u8; 32]>,
    image_nonce: Option<[u8; 12]>,
    relays: Vec<String>,
    admins: Vec<String>,
    welcomer: String,
    member_count: u32,
    welcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MlsMessageEnvelope {
    version: u8,
    mls_group_id: String,
    epoch: u64,
    message_kind: MlsMessageKind,
    mls_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum MlsMessageKind {
    Application,
    Commit,
    Proposal,
}

impl MlsMessageKind {
    fn from_content_type(content_type: ContentType) -> Self {
        match content_type {
            ContentType::Application => Self::Application,
            ContentType::Proposal => Self::Proposal,
            ContentType::Commit => Self::Commit,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PikaApplicationPayload {
    Rumor {
        version: u8,
        rumor_json: String,
    },
    GroupData {
        version: u8,
        update: WireGroupDataUpdate,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct WireGroupDataUpdate {
    name: Option<String>,
    description: Option<String>,
    image_hash: Option<Option<[u8; 32]>>,
    image_key: Option<Option<[u8; 32]>>,
    image_nonce: Option<Option<[u8; 12]>>,
    relays: Option<Vec<String>>,
    admins: Option<Vec<String>>,
    nostr_group_id: Option<[u8; 32]>,
}

impl OpenMlsEngine {
    fn open(state_path: PathBuf) -> Result<Self> {
        Self::open_with_codec(state_path, StateCodec::Plaintext)
    }

    fn open_with_codec(state_path: PathBuf, codec: StateCodec) -> Result<Self> {
        if let Some(parent) = state_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create MLS state dir: {}", parent.display()))?;
        }
        let loaded = codec
            .load(&state_path)
            .with_context(|| format!("load OpenMLS state: {}", state_path.display()))?;
        if looks_like_legacy_fake_mls_state(&loaded.state) {
            anyhow::bail!(
                "legacy fake MLS state detected; reset local pika-mls state before using real OpenMLS"
            );
        }
        let storage = memory_storage_from_snapshot(&loaded.state.openmls_storage)
            .context("restore OpenMLS storage snapshot")?;
        let engine = Self {
            state_path,
            codec,
            provider: PikaOpenMlsProvider::with_storage(storage),
            state: Mutex::new(loaded.state.clone()),
        };
        if matches!(codec, StateCodec::Encrypted { .. }) && loaded.was_plaintext {
            engine.save_snapshot()?;
        }
        Ok(engine)
    }

    fn save_locked(&self, state: &mut StoreState) -> Result<()> {
        state.openmls_storage = memory_storage_snapshot(&self.provider.storage)?;
        if let Some(parent) = self.state_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create MLS state dir: {}", parent.display()))?;
        }
        let body = self
            .codec
            .encode(state)
            .context("serialize OpenMLS state")?;
        write_private_file(&self.state_path, body)
            .with_context(|| format!("write OpenMLS state: {}", self.state_path.display()))
    }

    fn save_snapshot(&self) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        self.save_locked(&mut state)
    }

    fn ensure_account_signer(&self, public_key: &PublicKey) -> Result<SignatureKeyPair> {
        let account_key = public_key.to_hex();
        let existing = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
            state.local_pubkey = Some(*public_key);
            state.account_signers.get(&account_key).cloned()
        };
        if let Some(public) = existing
            && let Some(signer) = SignatureKeyPair::read(
                self.provider.storage(),
                &public,
                CIPHERSUITE.signature_algorithm(),
            )
        {
            return Ok(signer);
        }

        let signer = SignatureKeyPair::new(CIPHERSUITE.signature_algorithm())
            .map_err(|err| anyhow!("generate OpenMLS signer: {err:?}"))?;
        signer
            .store(self.provider.storage())
            .map_err(|err| anyhow!("store OpenMLS signer: {err:?}"))?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        state
            .account_signers
            .insert(account_key, signer.to_public_vec());
        state.local_pubkey = Some(*public_key);
        self.save_locked(&mut state)?;
        Ok(signer)
    }

    fn credential_with_key(
        &self,
        public_key: &PublicKey,
        signer: &SignatureKeyPair,
    ) -> CredentialWithKey {
        CredentialWithKey {
            credential: BasicCredential::new(public_key.to_bytes().to_vec()).into(),
            signature_key: signer.to_public_vec().into(),
        }
    }

    fn signer_for_group(&self, group: &MlsGroup) -> Result<SignatureKeyPair> {
        let own_leaf = group
            .own_leaf_node()
            .ok_or_else(|| anyhow!("OpenMLS group has no own leaf"))?;
        SignatureKeyPair::read(
            self.provider.storage(),
            own_leaf.signature_key().as_slice(),
            CIPHERSUITE.signature_algorithm(),
        )
        .ok_or_else(|| anyhow!("missing OpenMLS signer for group"))
    }

    fn group_config() -> MlsGroupCreateConfig {
        MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .sender_ratchet_configuration(SenderRatchetConfiguration::new(32, 2000))
            .capabilities(Capabilities::new(
                None,
                Some(&[CIPHERSUITE]),
                Some(&[ExtensionType::RatchetTree]),
                None,
                Some(&[CredentialType::Basic]),
            ))
            .use_ratchet_tree_extension(true)
            .build()
    }

    fn join_config() -> MlsGroupJoinConfig {
        MlsGroupJoinConfig::builder()
            .sender_ratchet_configuration(SenderRatchetConfiguration::new(32, 2000))
            .use_ratchet_tree_extension(true)
            .build()
    }

    fn create_key_package_for_event<I>(
        &self,
        public_key: &PublicKey,
        relays: I,
    ) -> Result<(String, Vec<Tag>, Vec<u8>)>
    where
        I: IntoIterator<Item = RelayUrl>,
    {
        let relays: Vec<RelayUrl> = relays.into_iter().collect();
        let signer = self.ensure_account_signer(public_key)?;
        let credential = self.credential_with_key(public_key, &signer);
        let key_package_bundle = KeyPackage::builder()
            .build(CIPHERSUITE, &self.provider, &signer, credential)
            .map_err(|err| anyhow!("build OpenMLS key package: {err:?}"))?;
        let key_package_bytes = key_package_bundle
            .key_package()
            .tls_serialize_detached()
            .context("serialize OpenMLS key package")?;
        let hash_ref = key_package_bundle
            .key_package()
            .hash_ref(self.provider.crypto())
            .map_err(|err| anyhow!("hash OpenMLS key package: {err:?}"))?
            .as_slice()
            .to_vec();
        let envelope = KeyPackageEnvelope {
            version: KEY_PACKAGE_ENVELOPE_VERSION,
            owner_pubkey: public_key.to_hex(),
            ciphersuite: format!("{CIPHERSUITE:?}"),
            relays: relays.iter().map(ToString::to_string).collect(),
            key_package: BASE64_STANDARD.encode(key_package_bytes),
        };
        let content = serde_json::to_string(&envelope).context("serialize key package envelope")?;
        let mut tags = vec![Tag::custom(TagKind::p(), [public_key.to_hex()])];
        if !relays.is_empty() {
            tags.push(Tag::from_standardized(nostr::TagStandard::Relays(relays)));
        }
        self.save_snapshot()?;
        Ok((content, tags, hash_ref))
    }

    fn parse_key_package(&self, event: &Event) -> Result<KeyPackage> {
        if event.kind != Kind::MlsKeyPackage {
            anyhow::bail!("event is not an MLS key package");
        }
        event
            .verify()
            .context("verify key package event signature")?;
        let envelope: KeyPackageEnvelope =
            serde_json::from_str(&event.content).context("parse key package envelope")?;
        if envelope.version != KEY_PACKAGE_ENVELOPE_VERSION {
            anyhow::bail!(
                "unsupported key package envelope version: {}",
                envelope.version
            );
        }
        let owner = PublicKey::parse(&envelope.owner_pubkey).context("parse key package owner")?;
        if owner != event.pubkey {
            anyhow::bail!("key package owner does not match event pubkey");
        }
        let key_package_bytes = BASE64_STANDARD
            .decode(envelope.key_package)
            .context("decode key package")?;
        let key_package_in = KeyPackageIn::tls_deserialize_exact(key_package_bytes)
            .context("deserialize OpenMLS key package")?;
        let key_package = key_package_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|err| anyhow!("validate OpenMLS key package: {err:?}"))?;
        if key_package.ciphersuite() != CIPHERSUITE {
            anyhow::bail!(
                "unsupported key package ciphersuite: {:?}",
                key_package.ciphersuite()
            );
        }
        let credential_pubkey = pubkey_from_credential(key_package.leaf_node().credential())?;
        if credential_pubkey != event.pubkey {
            anyhow::bail!("key package credential does not match event pubkey");
        }
        Ok(key_package)
    }

    fn create_group(
        &self,
        creator_public_key: &PublicKey,
        member_key_package_events: Vec<Event>,
        config: groups_api::NostrGroupConfigData,
    ) -> Result<groups_api::GroupResult> {
        let mut member_packages = Vec::with_capacity(member_key_package_events.len());
        for event in &member_key_package_events {
            member_packages.push((event.pubkey, self.parse_key_package(event)?));
        }

        let signer = self.ensure_account_signer(creator_public_key)?;
        let credential = self.credential_with_key(creator_public_key, &signer);
        let mls_group_id = GroupId::from_slice(&random_32());
        let nostr_group_id = random_32();
        let create_config = Self::group_config();
        let mut open_group = MlsGroup::new_with_group_id(
            &self.provider,
            &signer,
            &create_config,
            open_group_id(&mls_group_id),
            credential,
        )
        .map_err(|err| anyhow!("create OpenMLS group: {err:?}"))?;

        let relays: BTreeSet<RelayUrl> = config.relays.into_iter().collect();
        let mut app_group = Group {
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
            epoch: open_group.epoch().as_u64(),
            state: GroupState::Active,
            self_update_state: SelfUpdateState::CompletedAt(Timestamp::now()),
        };

        let mut welcome_rumors = Vec::new();
        if !member_packages.is_empty() {
            let key_packages: Vec<KeyPackage> = member_packages
                .iter()
                .map(|(_, key_package)| key_package.clone())
                .collect();
            let (_commit, welcome, _group_info) = open_group
                .add_members(&self.provider, &signer, &key_packages)
                .map_err(|err| anyhow!("add initial OpenMLS members: {err:?}"))?;
            open_group
                .merge_pending_commit(&self.provider)
                .map_err(|err| anyhow!("merge initial OpenMLS commit: {err:?}"))?;
            app_group.epoch = open_group.epoch().as_u64();
            let members = members_from_mls_group(&open_group)?;
            let welcome_envelope = self.welcome_envelope(
                &app_group,
                &relays,
                creator_public_key,
                members.len(),
                welcome,
            )?;
            welcome_rumors = member_packages
                .iter()
                .map(|_| build_welcome_rumor(&welcome_envelope, *creator_public_key))
                .collect::<Result<Vec<_>>>()?;
        }

        let members = members_from_mls_group(&open_group)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        state.local_pubkey = Some(*creator_public_key);
        upsert_group(&mut state, app_group.clone(), members, relays);
        self.save_locked(&mut state)?;

        Ok(groups_api::GroupResult {
            group: app_group,
            welcome_rumors,
        })
    }

    fn add_members(
        &self,
        group_id: &GroupId,
        key_package_events: &[Event],
    ) -> Result<groups_api::UpdateGroupResult> {
        if key_package_events.is_empty() {
            anyhow::bail!("no key packages provided");
        }
        let mut open_group = self.load_group(group_id)?;
        let signer = self.signer_for_group(&open_group)?;
        let mut member_packages = Vec::with_capacity(key_package_events.len());
        for event in key_package_events {
            member_packages.push((event.pubkey, self.parse_key_package(event)?));
        }
        let app_group = self.app_group(group_id)?;
        let local_pubkey = self.local_pubkey_for_wire()?;
        ensure_group_admin(&app_group, &local_pubkey)?;
        let key_packages: Vec<KeyPackage> = member_packages
            .iter()
            .map(|(_, key_package)| key_package.clone())
            .collect();
        let old_epoch = open_group.epoch().as_u64();
        let (commit, welcome, _group_info) = open_group
            .add_members(&self.provider, &signer, &key_packages)
            .map_err(|err| anyhow!("add OpenMLS members: {err:?}"))?;
        let relays = self.group_relays(group_id)?;
        let next_member_count = self
            .group_members(group_id)?
            .len()
            .saturating_add(member_packages.len());
        let welcome_envelope = self.welcome_envelope(
            &app_group,
            &relays,
            &local_pubkey,
            next_member_count,
            welcome,
        )?;
        let welcome_rumors = member_packages
            .iter()
            .map(|_| build_welcome_rumor(&welcome_envelope, local_pubkey))
            .collect::<Result<Vec<_>>>()?;
        let event =
            self.build_group_event(&app_group, commit, old_epoch, MlsMessageKind::Commit)?;
        self.save_snapshot()?;

        Ok(groups_api::UpdateGroupResult {
            evolution_event: event,
            welcome_rumors: Some(welcome_rumors),
            mls_group_id: group_id.clone(),
        })
    }

    fn remove_members(
        &self,
        group_id: &GroupId,
        pubkeys: &[PublicKey],
    ) -> Result<groups_api::UpdateGroupResult> {
        if pubkeys.is_empty() {
            anyhow::bail!("no members provided");
        }
        let app_group = self.app_group(group_id)?;
        let local_pubkey = self.local_pubkey_for_wire()?;
        ensure_group_admin(&app_group, &local_pubkey)?;
        let mut open_group = self.load_group(group_id)?;
        let signer = self.signer_for_group(&open_group)?;
        let mut leaf_indices = Vec::with_capacity(pubkeys.len());
        for target in pubkeys {
            let member = open_group
                .members()
                .find(|member| pubkey_from_credential(&member.credential).ok() == Some(*target))
                .ok_or_else(|| anyhow!("member not found: {}", target.to_hex()))?;
            leaf_indices.push(member.index);
        }
        let old_epoch = open_group.epoch().as_u64();
        let (commit, _welcome, _group_info) = open_group
            .remove_members(&self.provider, &signer, &leaf_indices)
            .map_err(|err| anyhow!("remove OpenMLS members: {err:?}"))?;
        let event =
            self.build_group_event(&app_group, commit, old_epoch, MlsMessageKind::Commit)?;
        self.save_snapshot()?;

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
        let app_group = self.app_group(group_id)?;
        let epoch = app_group.epoch;
        let open_group = self.load_group(group_id)?;
        let own_pubkey = open_group
            .own_leaf_node()
            .ok_or_else(|| anyhow!("OpenMLS group has no own leaf"))
            .and_then(|leaf| pubkey_from_credential(leaf.credential()))?;
        ensure_group_admin(&app_group, &own_pubkey)?;
        let wire_update = WireGroupDataUpdate::try_from_update(update.clone())?;
        self.apply_group_data_update(group_id, &wire_update, &own_pubkey)?;
        let payload = PikaApplicationPayload::GroupData {
            version: APPLICATION_PAYLOAD_VERSION,
            update: wire_update,
        };
        let event = self.create_application_wrapper(
            group_id,
            &serde_json::to_vec(&payload).context("serialize group data payload")?,
            epoch,
        )?;

        Ok(groups_api::UpdateGroupResult {
            evolution_event: event,
            welcome_rumors: None,
            mls_group_id: group_id.clone(),
        })
    }

    fn leave_group(&self, group_id: &GroupId) -> Result<groups_api::UpdateGroupResult> {
        let mut open_group = self.load_group(group_id)?;
        let signer = self.signer_for_group(&open_group)?;
        let old_epoch = open_group.epoch().as_u64();
        let proposal = open_group
            .leave_group(&self.provider, &signer)
            .map_err(|err| anyhow!("create OpenMLS leave proposal: {err:?}"))?;
        let app_group = self.app_group(group_id)?;
        let event =
            self.build_group_event(&app_group, proposal, old_epoch, MlsMessageKind::Proposal)?;
        self.save_snapshot()?;

        Ok(groups_api::UpdateGroupResult {
            evolution_event: event,
            welcome_rumors: None,
            mls_group_id: group_id.clone(),
        })
    }

    fn merge_pending_commit(&self, group_id: &GroupId) -> Result<()> {
        let mut open_group = self.load_group(group_id)?;
        open_group
            .merge_pending_commit(&self.provider)
            .map_err(|err| anyhow!("merge OpenMLS pending commit: {err:?}"))?;
        let members = members_from_mls_group(&open_group)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        if let Some(group) = state
            .groups
            .iter_mut()
            .find(|group| group.mls_group_id == *group_id)
        {
            group.epoch = open_group.epoch().as_u64();
            group.state = if open_group.is_active() {
                GroupState::Active
            } else {
                GroupState::Inactive
            };
        }
        state.members.insert(group_key(group_id), members);
        self.save_locked(&mut state)
    }

    fn clear_pending_commit(&self, group_id: &GroupId) -> Result<()> {
        let mut open_group = self.load_group(group_id)?;
        open_group
            .clear_pending_commit(self.provider.storage())
            .map_err(|err| anyhow!("clear OpenMLS pending commit: {err:?}"))?;
        self.save_snapshot()
    }

    fn get_group(&self, group_id: &GroupId) -> Result<Option<Group>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
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
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
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
        self.group_members(group_id)
    }

    fn get_relays(&self, group_id: &GroupId) -> Result<BTreeSet<RelayUrl>> {
        self.group_relays(group_id)
    }

    fn get_message(&self, group_id: &GroupId, message_id: &EventId) -> Result<Option<Message>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
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
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
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
        let group = self.app_group(group_id)?;
        let epoch = group.epoch;
        let payload = PikaApplicationPayload::Rumor {
            version: APPLICATION_PAYLOAD_VERSION,
            rumor_json: rumor.as_json(),
        };
        let event = self.create_application_wrapper(
            group_id,
            &serde_json::to_vec(&payload).context("serialize application payload")?,
            epoch,
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
            epoch: Some(epoch),
            state: MessageState::Created,
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        save_message_to_state(&mut state, message);
        state.outbound_wrappers.insert(event.id);
        self.save_locked(&mut state)?;
        Ok(event)
    }

    fn process_message(&self, event: &Event) -> Result<MessageProcessingResult> {
        let envelope: MlsMessageEnvelope =
            serde_json::from_str(&event.content).context("parse MLS message envelope")?;
        if envelope.version != MLS_MESSAGE_ENVELOPE_VERSION {
            anyhow::bail!(
                "unsupported MLS message envelope version: {}",
                envelope.version
            );
        }
        let group_id = group_id_from_hex(&envelope.mls_group_id)?;
        if self.outbound_wrapper_seen(&event.id)? {
            return Ok(MessageProcessingResult::Unprocessable {
                mls_group_id: group_id,
            });
        }
        let message_bytes = BASE64_STANDARD
            .decode(&envelope.mls_message)
            .context("decode MLS message")?;
        let message_in = MlsMessageIn::tls_deserialize_exact(message_bytes)
            .context("deserialize MLS message")?;
        let protocol_message: ProtocolMessage = message_in
            .try_into_protocol_message()
            .map_err(|err| anyhow!("decode MLS protocol message: {err:?}"))?;
        if protocol_message.group_id().as_slice() != group_id.as_slice() {
            anyhow::bail!("MLS envelope group id does not match protocol message");
        }
        if protocol_message.epoch().as_u64() != envelope.epoch {
            anyhow::bail!(
                "MLS envelope epoch {} does not match protocol message epoch {}",
                envelope.epoch,
                protocol_message.epoch().as_u64()
            );
        }
        let actual_message_kind =
            MlsMessageKind::from_content_type(protocol_message.content_type());
        if envelope.message_kind != actual_message_kind {
            anyhow::bail!(
                "MLS envelope kind {:?} does not match protocol message kind {:?}",
                envelope.message_kind,
                actual_message_kind
            );
        }
        let mut open_group = self.load_group(&group_id)?;
        let processed = open_group
            .process_message(&self.provider, protocol_message)
            .map_err(|err| anyhow!("process OpenMLS message: {err:?}"))?;
        let authenticated_sender = pubkey_from_credential(processed.credential())
            .context("parse MLS sender credential")?;

        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(application_message) => {
                let bytes = application_message.into_bytes();
                match serde_json::from_slice::<PikaApplicationPayload>(&bytes) {
                    Ok(PikaApplicationPayload::Rumor {
                        version,
                        rumor_json,
                    }) => {
                        ensure_application_payload_version(version)?;
                        self.process_application_rumor(
                            &group_id,
                            event,
                            envelope.epoch,
                            &rumor_json,
                            &authenticated_sender,
                        )
                    }
                    Ok(PikaApplicationPayload::GroupData { version, update }) => {
                        ensure_application_payload_version(version)?;
                        self.apply_group_data_update(&group_id, &update, &authenticated_sender)?;
                        Ok(MessageProcessingResult::Commit {
                            mls_group_id: group_id,
                        })
                    }
                    Err(_) => {
                        let rumor_json =
                            String::from_utf8(bytes).context("application message is not UTF-8")?;
                        self.process_application_rumor(
                            &group_id,
                            event,
                            envelope.epoch,
                            &rumor_json,
                            &authenticated_sender,
                        )
                    }
                }
            }
            ProcessedMessageContent::ProposalMessage(proposal) => {
                open_group
                    .store_pending_proposal(self.provider.storage(), *proposal)
                    .map_err(|err| anyhow!("store OpenMLS proposal: {err:?}"))?;
                self.save_snapshot()?;
                Ok(MessageProcessingResult::PendingProposal {
                    mls_group_id: group_id,
                })
            }
            ProcessedMessageContent::ExternalJoinProposalMessage(proposal) => {
                open_group
                    .store_pending_proposal(self.provider.storage(), *proposal)
                    .map_err(|err| anyhow!("store OpenMLS external join proposal: {err:?}"))?;
                self.save_snapshot()?;
                Ok(MessageProcessingResult::ExternalJoinProposal {
                    mls_group_id: group_id,
                })
            }
            ProcessedMessageContent::StagedCommitMessage(staged_commit) => {
                let app_group = self.app_group(&group_id)?;
                ensure_group_admin(&app_group, &authenticated_sender)?;
                open_group
                    .merge_staged_commit(&self.provider, *staged_commit)
                    .map_err(|err| anyhow!("merge OpenMLS staged commit: {err:?}"))?;
                let members = members_from_mls_group(&open_group)?;
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
                if let Some(group) = state
                    .groups
                    .iter_mut()
                    .find(|group| group.mls_group_id == group_id)
                {
                    group.epoch = open_group.epoch().as_u64();
                    group.state = if open_group.is_active() {
                        GroupState::Active
                    } else {
                        GroupState::Inactive
                    };
                }
                state.members.insert(group_key(&group_id), members);
                self.save_locked(&mut state)?;
                Ok(MessageProcessingResult::Commit {
                    mls_group_id: group_id,
                })
            }
        }
    }

    fn process_application_rumor(
        &self,
        group_id: &GroupId,
        wrapper: &Event,
        epoch: u64,
        rumor_json: &str,
        authenticated_sender: &PublicKey,
    ) -> Result<MessageProcessingResult> {
        let mut rumor = UnsignedEvent::from_json(rumor_json).context("parse application rumor")?;
        rumor.ensure_id();
        if rumor.pubkey != *authenticated_sender {
            anyhow::bail!("application rumor pubkey does not match MLS sender credential");
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        find_group_in_state(&state, group_id)?;
        let message = Message {
            id: rumor.id(),
            pubkey: rumor.pubkey,
            kind: rumor.kind,
            mls_group_id: group_id.clone(),
            created_at: rumor.created_at,
            processed_at: Timestamp::now(),
            content: rumor.content.clone(),
            tags: rumor.tags.clone(),
            event: rumor,
            wrapper_event_id: wrapper.id,
            epoch: Some(epoch),
            state: MessageState::Processed,
        };
        save_message_to_state(&mut state, message.clone());
        self.save_locked(&mut state)?;
        Ok(MessageProcessingResult::ApplicationMessage(message))
    }

    fn process_welcome(&self, wrapper_event_id: &EventId, rumor: &UnsignedEvent) -> Result<()> {
        let welcome = self.welcome_from_rumor(wrapper_event_id, rumor, WelcomeState::Pending)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        state
            .pending_welcomes
            .retain(|existing| existing.wrapper_event_id != *wrapper_event_id);
        let already_active = state.groups.iter().any(|group| {
            group.state == GroupState::Active
                && (group.mls_group_id == welcome.mls_group_id
                    || group.nostr_group_id == welcome.nostr_group_id)
        });
        if already_active {
            return self.save_locked(&mut state);
        }
        state.pending_welcomes.push(welcome);
        self.save_locked(&mut state)
    }

    fn welcome_from_rumor(
        &self,
        wrapper_event_id: &EventId,
        rumor: &UnsignedEvent,
        state: WelcomeState,
    ) -> Result<Welcome> {
        let envelope = parse_welcome_envelope(rumor)?;
        Ok(Welcome {
            id: {
                let mut event = rumor.clone();
                event.ensure_id();
                event.id()
            },
            event: rumor.clone(),
            mls_group_id: group_id_from_hex(&envelope.mls_group_id)?,
            nostr_group_id: decode_hex_array::<32>(&envelope.nostr_group_id)
                .context("decode nostr group id")?,
            group_name: envelope.name,
            group_description: envelope.description,
            group_image_hash: envelope.image_hash,
            group_image_key: envelope.image_key.map(Secret),
            group_image_nonce: envelope.image_nonce.map(Secret),
            group_admin_pubkeys: parse_pubkey_set(&envelope.admins)?,
            group_relays: parse_relay_set(&envelope.relays)?,
            welcomer: PublicKey::parse(&envelope.welcomer).context("parse welcome welcomer")?,
            member_count: envelope.member_count,
            state,
            wrapper_event_id: *wrapper_event_id,
        })
    }

    fn accept_welcome(&self, welcome: &Welcome) -> Result<()> {
        let envelope = parse_welcome_envelope(&welcome.event)?;
        let envelope_group_id =
            group_id_from_hex(&envelope.mls_group_id).context("decode welcome MLS group id")?;
        let welcomer = PublicKey::parse(&envelope.welcomer).context("parse welcome welcomer")?;
        let welcome_bytes = BASE64_STANDARD
            .decode(&envelope.welcome)
            .context("decode OpenMLS welcome")?;
        let message_in = MlsMessageIn::tls_deserialize_exact(welcome_bytes)
            .context("deserialize OpenMLS welcome")?;
        let open_welcome = match message_in.extract() {
            MlsMessageBodyIn::Welcome(welcome) => welcome,
            other => anyhow::bail!("expected OpenMLS welcome, got {other:?}"),
        };
        let staged = StagedWelcome::new_from_welcome(
            &self.provider,
            &Self::join_config(),
            open_welcome,
            None,
        )
        .map_err(|err| anyhow!("stage OpenMLS welcome: {err:?}"))?;
        let open_group = staged
            .into_group(&self.provider)
            .map_err(|err| anyhow!("join OpenMLS group: {err:?}"))?;
        if open_group.group_id().as_slice() != envelope_group_id.as_slice() {
            anyhow::bail!("welcome MLS group id does not match staged OpenMLS group");
        }
        let members = members_from_mls_group(&open_group)?;
        if !members.contains(&welcomer) {
            anyhow::bail!("welcome welcomer is not an MLS group member");
        }
        let relays = parse_relay_set(&envelope.relays)?;
        let admins = parse_pubkey_set(&envelope.admins)?;
        let mut group = Group {
            mls_group_id: GroupId::from_slice(open_group.group_id().as_slice()),
            nostr_group_id: decode_hex_array::<32>(&envelope.nostr_group_id)
                .context("decode nostr group id")?,
            name: envelope.name,
            description: envelope.description,
            image_hash: envelope.image_hash,
            image_key: envelope.image_key.map(Secret),
            image_nonce: envelope.image_nonce.map(Secret),
            admin_pubkeys: admins,
            last_message_id: None,
            last_message_at: None,
            last_message_processed_at: None,
            epoch: open_group.epoch().as_u64(),
            state: GroupState::Active,
            self_update_state: SelfUpdateState::CompletedAt(Timestamp::now()),
        };
        group.state = if open_group.is_active() {
            GroupState::Active
        } else {
            GroupState::Inactive
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        upsert_group(&mut state, group, members, relays);
        for pending in &mut state.pending_welcomes {
            if pending.wrapper_event_id == welcome.wrapper_event_id {
                pending.state = WelcomeState::Accepted;
            }
        }
        self.save_locked(&mut state)
    }

    fn get_pending_welcomes(
        &self,
        _pagination: Option<storage_traits::welcomes::Pagination>,
    ) -> Result<Vec<Welcome>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        Ok(state
            .pending_welcomes
            .iter()
            .filter(|welcome| welcome.state == WelcomeState::Pending)
            .cloned()
            .collect())
    }

    fn export_media_context(&self, group_id: &GroupId) -> Result<[u8; 32]> {
        let open_group = self.load_group(group_id)?;
        let context = Sha256::digest(group_id.as_slice());
        let secret = open_group
            .export_secret(self.provider.crypto(), MEDIA_EXPORTER_LABEL, &context, 32)
            .map_err(|err| anyhow!("export OpenMLS media secret: {err:?}"))?;
        secret
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("OpenMLS media secret must be 32 bytes"))
    }

    fn load_group(&self, group_id: &GroupId) -> Result<MlsGroup> {
        MlsGroup::load(self.provider.storage(), &open_group_id(group_id))
            .map_err(|err| anyhow!("load OpenMLS group: {err:?}"))?
            .ok_or_else(|| anyhow!("OpenMLS group not found"))
    }

    fn app_group(&self, group_id: &GroupId) -> Result<Group> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        Ok(find_group_in_state(&state, group_id)?.clone())
    }

    fn group_members(&self, group_id: &GroupId) -> Result<BTreeSet<PublicKey>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        Ok(state
            .members
            .get(&group_key(group_id))
            .cloned()
            .unwrap_or_default())
    }

    fn group_relays(&self, group_id: &GroupId) -> Result<BTreeSet<RelayUrl>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        Ok(state
            .relays
            .get(&group_key(group_id))
            .cloned()
            .unwrap_or_default())
    }

    fn local_pubkey_for_wire(&self) -> Result<PublicKey> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        state
            .local_pubkey
            .ok_or_else(|| anyhow!("local pubkey missing from OpenMLS state"))
    }

    fn outbound_wrapper_seen(&self, event_id: &EventId) -> Result<bool> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        Ok(state.outbound_wrappers.contains(event_id))
    }

    fn welcome_envelope(
        &self,
        group: &Group,
        relays: &BTreeSet<RelayUrl>,
        welcomer: &PublicKey,
        member_count: usize,
        welcome: MlsMessageOut,
    ) -> Result<WelcomeEnvelope> {
        Ok(WelcomeEnvelope {
            version: WELCOME_ENVELOPE_VERSION,
            mls_group_id: hex::encode(group.mls_group_id.as_slice()),
            nostr_group_id: hex::encode(group.nostr_group_id),
            name: group.name.clone(),
            description: group.description.clone(),
            image_hash: group.image_hash,
            image_key: group.image_key.as_ref().map(|secret| secret.0),
            image_nonce: group.image_nonce.as_ref().map(|secret| secret.0),
            relays: relays.iter().map(ToString::to_string).collect(),
            admins: group.admin_pubkeys.iter().map(PublicKey::to_hex).collect(),
            welcomer: welcomer.to_hex(),
            member_count: member_count as u32,
            welcome: BASE64_STANDARD.encode(welcome.to_bytes().context("serialize welcome")?),
        })
    }

    fn create_application_wrapper(
        &self,
        group_id: &GroupId,
        plaintext: &[u8],
        epoch: u64,
    ) -> Result<Event> {
        let mut open_group = self.load_group(group_id)?;
        let signer = self.signer_for_group(&open_group)?;
        let mls_message = open_group
            .create_message(&self.provider, &signer, plaintext)
            .map_err(|err| anyhow!("create OpenMLS application message: {err:?}"))?;
        let app_group = self.app_group(group_id)?;
        let event =
            self.build_group_event(&app_group, mls_message, epoch, MlsMessageKind::Application)?;
        self.save_snapshot()?;
        Ok(event)
    }

    fn build_group_event(
        &self,
        group: &Group,
        mls_message: MlsMessageOut,
        epoch: u64,
        message_kind: MlsMessageKind,
    ) -> Result<Event> {
        let envelope = MlsMessageEnvelope {
            version: MLS_MESSAGE_ENVELOPE_VERSION,
            mls_group_id: hex::encode(group.mls_group_id.as_slice()),
            epoch,
            message_kind,
            mls_message: BASE64_STANDARD
                .encode(mls_message.to_bytes().context("serialize MLS message")?),
        };
        let content = serde_json::to_string(&envelope).context("serialize MLS message envelope")?;
        let tag = Tag::custom(TagKind::h(), [hex::encode(group.nostr_group_id)]);
        let event = EventBuilder::new(Kind::MlsGroupMessage, content)
            .tag(tag)
            .sign_with_keys(&Keys::generate())
            .context("sign group wrapper")?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        state.outbound_wrappers.insert(event.id);
        self.save_locked(&mut state)?;
        Ok(event)
    }

    fn apply_group_data_update(
        &self,
        group_id: &GroupId,
        update: &WireGroupDataUpdate,
        authenticated_sender: &PublicKey,
    ) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        let group = state
            .groups
            .iter_mut()
            .find(|group| group.mls_group_id == *group_id)
            .ok_or_else(|| anyhow!("group not found"))?;
        ensure_group_admin(group, authenticated_sender)?;
        apply_wire_update_to_group(group, update)?;
        if let Some(relays) = &update.relays {
            state
                .relays
                .insert(group_key(group_id), parse_relay_set(relays)?);
        }
        self.save_locked(&mut state)
    }
}

impl WireGroupDataUpdate {
    fn try_from_update(update: groups_api::NostrGroupDataUpdate) -> Result<Self> {
        Ok(Self {
            name: update.name,
            description: update.description,
            image_hash: update.image_hash,
            image_key: update.image_key,
            image_nonce: update.image_nonce,
            relays: update
                .relays
                .map(|relays| relays.into_iter().map(|relay| relay.to_string()).collect()),
            admins: update
                .admins
                .map(|admins| admins.into_iter().map(|pubkey| pubkey.to_hex()).collect()),
            nostr_group_id: update.nostr_group_id,
        })
    }
}

impl StateCodec {
    fn load(self, state_path: &Path) -> Result<LoadedStoreState> {
        let raw = match std::fs::read_to_string(state_path) {
            Ok(raw) if !raw.trim().is_empty() => raw,
            _ => {
                return Ok(LoadedStoreState {
                    state: StoreState::default(),
                    was_plaintext: false,
                });
            }
        };
        let trimmed = raw.trim();
        if let Ok(envelope) = serde_json::from_str::<EncryptedStoreState>(trimmed)
            && envelope.scheme_version == ENCRYPTED_STATE_SCHEME_VERSION
        {
            let StateCodec::Encrypted { key } = self else {
                anyhow::bail!("encrypted OpenMLS state requires open_secure_mls");
            };
            return Ok(LoadedStoreState {
                state: decrypt_store_state(&key, &envelope)
                    .context("decrypt encrypted OpenMLS state")?,
                was_plaintext: false,
            });
        }

        let state = serde_json::from_str(trimmed).context("parse plaintext OpenMLS state")?;
        Ok(LoadedStoreState {
            state,
            was_plaintext: true,
        })
    }

    fn encode(self, state: &StoreState) -> Result<String> {
        let plaintext = serde_json::to_vec(state).context("serialize plaintext OpenMLS state")?;
        let body = match self {
            StateCodec::Plaintext => {
                serde_json::to_string_pretty(state).context("serialize plaintext OpenMLS state")?
            }
            StateCodec::Encrypted { key } => {
                let envelope = encrypt_store_state(&key, &plaintext)?;
                serde_json::to_string_pretty(&envelope)
                    .context("serialize encrypted OpenMLS state")?
            }
        };
        Ok(format!("{body}\n"))
    }
}

fn encrypt_store_state(key: &[u8; 32], plaintext: &[u8]) -> Result<EncryptedStoreState> {
    let nonce = random_12();
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("invalid OpenMLS state key"))?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: ENCRYPTED_STATE_SCHEME_VERSION.as_bytes(),
            },
        )
        .map_err(|_| anyhow!("AES-GCM seal failed"))?;
    Ok(EncryptedStoreState {
        scheme_version: ENCRYPTED_STATE_SCHEME_VERSION.to_string(),
        nonce: hex::encode(nonce),
        ciphertext: hex::encode(ciphertext),
    })
}

fn decrypt_store_state(key: &[u8; 32], envelope: &EncryptedStoreState) -> Result<StoreState> {
    if envelope.scheme_version != ENCRYPTED_STATE_SCHEME_VERSION {
        anyhow::bail!(
            "unknown encrypted OpenMLS state scheme version: {}",
            envelope.scheme_version
        );
    }
    let nonce = decode_hex_array::<12>(&envelope.nonce).context("decode OpenMLS state nonce")?;
    let ciphertext =
        hex::decode(&envelope.ciphertext).context("decode OpenMLS state ciphertext")?;
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("invalid OpenMLS state key"))?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: ciphertext.as_slice(),
                aad: ENCRYPTED_STATE_SCHEME_VERSION.as_bytes(),
            },
        )
        .map_err(|_| anyhow!("AES-GCM open failed"))?;
    serde_json::from_slice(&plaintext).context("parse decrypted OpenMLS state")
}

fn looks_like_legacy_fake_mls_state(state: &StoreState) -> bool {
    state.openmls_storage.is_empty()
        && (!state.groups.is_empty()
            || !state.members.is_empty()
            || !state.relays.is_empty()
            || state.messages.values().any(|messages| !messages.is_empty())
            || !state.pending_welcomes.is_empty())
}

fn memory_storage_snapshot(storage: &MemoryStorage) -> Result<BTreeMap<String, String>> {
    let values = storage
        .values
        .read()
        .map_err(|_| anyhow!("OpenMLS storage lock poisoned"))?;
    Ok(values
        .iter()
        .map(|(key, value)| (BASE64_STANDARD.encode(key), BASE64_STANDARD.encode(value)))
        .collect())
}

fn memory_storage_from_snapshot(snapshot: &BTreeMap<String, String>) -> Result<MemoryStorage> {
    let mut values = HashMap::new();
    for (key, value) in snapshot {
        values.insert(
            BASE64_STANDARD
                .decode(key)
                .context("decode OpenMLS storage key")?,
            BASE64_STANDARD
                .decode(value)
                .context("decode OpenMLS storage value")?,
        );
    }
    Ok(MemoryStorage {
        values: RwLock::new(values),
    })
}

fn write_private_file(path: &Path, body: String) -> Result<()> {
    let tmp_path = private_tmp_path(path);
    let write_result = (|| -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp_path)
            .with_context(|| format!("create temp state file {}", tmp_path.display()))?;
        use std::io::Write;
        file.write_all(body.as_bytes())
            .with_context(|| format!("write {}", tmp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("sync {}", tmp_path.display()))?;
        drop(file);
        std::fs::rename(&tmp_path, path)
            .with_context(|| format!("rename {} to {}", tmp_path.display(), path.display()))?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            let dir = std::fs::File::open(parent)
                .with_context(|| format!("open state dir {}", parent.display()))?;
            dir.sync_all()
                .with_context(|| format!("sync state dir {}", parent.display()))?;
        }
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    write_result?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("set private permissions on {}", path.display()))?;
    }
    Ok(())
}

fn private_tmp_path(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let suffix = format!("tmp-{}-{}", std::process::id(), hex::encode(random_12()));
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if !extension.is_empty() => {
            tmp.set_extension(format!("{extension}.{suffix}"));
        }
        _ => {
            tmp.set_extension(suffix);
        }
    }
    tmp
}

fn random_32() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn random_12() -> [u8; 12] {
    let mut bytes = [0u8; 12];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn open_group_id(group_id: &GroupId) -> openmls::prelude::GroupId {
    openmls::prelude::GroupId::from_slice(group_id.as_slice())
}

fn group_key(group_id: &GroupId) -> String {
    hex::encode(group_id.as_slice())
}

fn group_id_from_hex(hex_value: &str) -> Result<GroupId> {
    let bytes = hex::decode(hex_value).context("decode group id hex")?;
    if bytes.len() != 32 {
        anyhow::bail!("group id must be 32 bytes hex");
    }
    Ok(GroupId::from_slice(&bytes))
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

fn pubkey_from_credential(credential: &openmls::prelude::Credential) -> Result<PublicKey> {
    let basic = BasicCredential::try_from(credential.clone())
        .map_err(|err| anyhow!("expected OpenMLS basic credential: {err:?}"))?;
    PublicKey::from_slice(basic.identity()).context("parse Nostr pubkey from MLS credential")
}

fn members_from_mls_group(group: &MlsGroup) -> Result<BTreeSet<PublicKey>> {
    group
        .members()
        .map(|member| pubkey_from_credential(&member.credential))
        .collect()
}

fn parse_pubkey_set(values: &[String]) -> Result<BTreeSet<PublicKey>> {
    values
        .iter()
        .map(|value| PublicKey::parse(value).context("parse pubkey"))
        .collect()
}

fn parse_relay_set(values: &[String]) -> Result<BTreeSet<RelayUrl>> {
    values
        .iter()
        .map(|value| RelayUrl::parse(value).context("parse relay URL"))
        .collect()
}

fn apply_wire_update_to_group(group: &mut Group, update: &WireGroupDataUpdate) -> Result<()> {
    if let Some(name) = &update.name {
        group.name = name.clone();
    }
    if let Some(description) = &update.description {
        group.description = description.clone();
    }
    if let Some(image_hash) = update.image_hash {
        group.image_hash = image_hash;
    }
    if let Some(image_key) = update.image_key {
        group.image_key = image_key.map(Secret);
    }
    if let Some(image_nonce) = update.image_nonce {
        group.image_nonce = image_nonce.map(Secret);
    }
    if let Some(admins) = &update.admins {
        group.admin_pubkeys = parse_pubkey_set(admins)?;
    }
    if let Some(nostr_group_id) = update.nostr_group_id {
        group.nostr_group_id = nostr_group_id;
    }
    Ok(())
}

fn ensure_group_admin(group: &Group, pubkey: &PublicKey) -> Result<()> {
    if group.admin_pubkeys.contains(pubkey) {
        Ok(())
    } else {
        anyhow::bail!("group update requires an admin sender")
    }
}

fn decode_hex_array<const N: usize>(hex_value: &str) -> Result<[u8; N]> {
    let bytes = hex::decode(hex_value)?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("expected {N} bytes"))
}

fn build_welcome_rumor(payload: &WelcomeEnvelope, welcomer: PublicKey) -> Result<UnsignedEvent> {
    let content = serde_json::to_string(payload).context("serialize welcome envelope")?;
    Ok(EventBuilder::new(Kind::MlsWelcome, content).build(welcomer))
}

fn ensure_application_payload_version(version: u8) -> Result<()> {
    if version != APPLICATION_PAYLOAD_VERSION {
        anyhow::bail!("unsupported application payload version: {version}");
    }
    Ok(())
}

fn parse_welcome_envelope(rumor: &UnsignedEvent) -> Result<WelcomeEnvelope> {
    if rumor.kind != Kind::MlsWelcome {
        anyhow::bail!("rumor is not an MLS welcome");
    }
    let payload: WelcomeEnvelope =
        serde_json::from_str(&rumor.content).context("parse welcome envelope")?;
    if payload.version != WELCOME_ENVELOPE_VERSION {
        anyhow::bail!("unsupported welcome envelope version: {}", payload.version);
    }
    group_id_from_hex(&payload.mls_group_id).context("validate welcome MLS group id")?;
    decode_hex_array::<32>(&payload.nostr_group_id).context("validate welcome Nostr group id")?;
    let welcomer = PublicKey::parse(&payload.welcomer).context("parse welcome welcomer")?;
    if welcomer != rumor.pubkey {
        anyhow::bail!("welcome welcomer does not match rumor pubkey");
    }
    Ok(payload)
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

pub fn init_keyring_once(keychain_group: &str) -> Result<()> {
    static INIT: OnceLock<(String, std::result::Result<(), String>)> = OnceLock::new();
    let (configured_group, result) = INIT.get_or_init(|| {
        (
            keychain_group.to_string(),
            install_keyring_store(keychain_group),
        )
    });
    #[cfg(target_os = "ios")]
    if result.is_ok() && configured_group != keychain_group {
        anyhow::bail!(
            "keyring already initialized with a different access group: {configured_group}"
        );
    }
    #[cfg(not(target_os = "ios"))]
    let _ = configured_group;
    match result {
        Ok(()) => Ok(()),
        Err(e) => Err(anyhow!(e.clone())),
    }
}

fn install_keyring_store(#[allow(unused)] keychain_group: &str) -> std::result::Result<(), String> {
    install_platform_keyring_store(keychain_group).map_err(|err| err.to_string())
}

#[cfg(target_os = "ios")]
fn install_platform_keyring_store(keychain_group: &str) -> Result<()> {
    let store = if keychain_group.trim().is_empty() {
        apple_native_keyring_store::protected::Store::new()
    } else {
        let config = std::collections::HashMap::from([("access-group", keychain_group)]);
        apple_native_keyring_store::protected::Store::new_with_configuration(&config)
    }
    .context("create iOS keyring store")?;
    keyring_core::set_default_store(store);
    Ok(())
}

#[cfg(target_os = "android")]
fn install_platform_keyring_store(_keychain_group: &str) -> Result<()> {
    let store =
        android_native_keyring_store::Store::new().context("create Android keyring store")?;
    keyring_core::set_default_store(store);
    Ok(())
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn install_platform_keyring_store(_keychain_group: &str) -> Result<()> {
    Ok(())
}

#[cfg(any(target_os = "ios", target_os = "android"))]
fn secure_state_codec(pubkey_hex: &str) -> Result<StateCodec> {
    let entry = keyring_core::Entry::new(SERVICE_ID, &db_key_id(pubkey_hex))
        .context("create OpenMLS state keyring entry")?;
    match entry.get_secret() {
        Ok(secret) => {
            let key = secret
                .as_slice()
                .try_into()
                .map_err(|_| anyhow!("stored OpenMLS state key must be 32 bytes"))?;
            Ok(StateCodec::Encrypted { key })
        }
        Err(keyring_core::Error::NoEntry) => {
            let key = random_32();
            entry
                .set_secret(&key)
                .context("persist OpenMLS state key to keyring")?;
            Ok(StateCodec::Encrypted { key })
        }
        Err(err) => Err(anyhow!(err)).context("read OpenMLS state key from keyring"),
    }
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn secure_state_codec(_pubkey_hex: &str) -> Result<StateCodec> {
    Ok(StateCodec::Plaintext)
}

pub fn open_secure_mls(
    data_dir: &str,
    pubkey: &PublicKey,
    keychain_group: &str,
) -> Result<PikaMls> {
    init_keyring_once(keychain_group)?;
    let pubkey_hex = pubkey.to_hex();
    let state_path = mls_state_path(data_dir, &pubkey_hex);
    let codec = secure_state_codec(&pubkey_hex)?;
    let mls = OpenMlsEngine::open_with_codec(state_path, codec).map(PikaMls::from_engine)?;
    {
        let mut state = mls
            .inner
            .state
            .lock()
            .map_err(|_| anyhow!("OpenMLS state lock poisoned"))?;
        if state.local_pubkey.is_none() {
            state.local_pubkey = Some(*pubkey);
            mls.inner.save_locked(&mut state)?;
        }
    }
    Ok(mls)
}

pub fn open_unencrypted_mls(state_dir: &Path) -> Result<PikaMls> {
    OpenMlsEngine::open(state_dir.join("pika-mls.json")).map(PikaMls::from_engine)
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

    #[test]
    fn encrypted_state_file_hides_local_state_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("pika-mls.json");
        let state_key = [7u8; 32];
        let mls = PikaMls::from_engine(
            OpenMlsEngine::open_with_codec(
                state_path.clone(),
                StateCodec::Encrypted { key: state_key },
            )
            .unwrap(),
        );
        let keys = Keys::generate();
        let config = groups_api::NostrGroupConfigData::new(
            "state secret chat".to_string(),
            "state secret description".to_string(),
            None,
            None,
            None,
            Vec::new(),
            vec![keys.public_key()],
        );
        let created = mls
            .create_group(&keys.public_key(), Vec::new(), config)
            .unwrap();

        let raw = std::fs::read_to_string(&state_path).unwrap();
        assert!(raw.contains(ENCRYPTED_STATE_SCHEME_VERSION));
        assert!(raw.contains("\"ciphertext\""));
        assert!(!raw.contains("state secret chat"));
        assert!(!raw.contains("OpenMLS"));
        drop(mls);

        let reopened = PikaMls::from_engine(
            OpenMlsEngine::open_with_codec(state_path, StateCodec::Encrypted { key: state_key })
                .unwrap(),
        );
        let group = crate::conversation::ConversationQueries::new(&reopened)
            .get_group(&created.group.mls_group_id)
            .unwrap()
            .unwrap();
        assert_eq!(group.name, "state secret chat");
        assert!(
            reopened
                .inner
                .load_group(&created.group.mls_group_id)
                .is_ok()
        );
    }

    #[test]
    fn encrypted_open_migrates_plaintext_state_file() {
        let dir = tempfile::tempdir().unwrap();
        let mls = open_unencrypted_mls(dir.path()).unwrap();
        let keys = Keys::generate();
        let config = groups_api::NostrGroupConfigData::new(
            "migrated state chat".to_string(),
            String::new(),
            None,
            None,
            None,
            Vec::new(),
            vec![keys.public_key()],
        );
        let created = mls
            .create_group(&keys.public_key(), Vec::new(), config)
            .unwrap();
        let state_path = dir.path().join("pika-mls.json");
        let plaintext = std::fs::read_to_string(&state_path).unwrap();
        assert!(plaintext.contains("migrated state chat"));
        drop(mls);

        let state_key = [9u8; 32];
        let encrypted = PikaMls::from_engine(
            OpenMlsEngine::open_with_codec(
                state_path.clone(),
                StateCodec::Encrypted { key: state_key },
            )
            .unwrap(),
        );
        let raw = std::fs::read_to_string(&state_path).unwrap();
        assert!(raw.contains(ENCRYPTED_STATE_SCHEME_VERSION));
        assert!(!raw.contains("migrated state chat"));
        assert!(
            crate::conversation::ConversationQueries::new(&encrypted)
                .get_group(&created.group.mls_group_id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn legacy_fake_state_requires_reset() {
        let dir = tempfile::tempdir().unwrap();
        let mls = open_unencrypted_mls(dir.path()).unwrap();
        let keys = Keys::generate();
        let config = groups_api::NostrGroupConfigData::new(
            "legacy fake state".to_string(),
            String::new(),
            None,
            None,
            None,
            Vec::new(),
            vec![keys.public_key()],
        );
        mls.create_group(&keys.public_key(), Vec::new(), config)
            .unwrap();
        drop(mls);

        let state_path = dir.path().join("pika-mls.json");
        let raw = std::fs::read_to_string(&state_path).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        value["openmls_storage"] = serde_json::json!({});
        std::fs::write(&state_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();

        let err = open_unencrypted_mls(dir.path()).expect_err("legacy fake state should not open");
        assert!(format!("{err:#}").contains("legacy fake MLS state detected"));
    }

    #[test]
    fn key_package_event_signature_must_verify() {
        let dir = tempfile::tempdir().unwrap();
        let mls = open_unencrypted_mls(dir.path()).unwrap();
        let keys = Keys::generate();
        let (content, tags, _hash_ref) = crate::key_package::create_key_package_for_event(
            &mls,
            &keys.public_key(),
            Vec::<RelayUrl>::new(),
        )
        .unwrap();
        let event = EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(&keys)
            .unwrap();

        let mut value = serde_json::to_value(&event).unwrap();
        value["sig"] = serde_json::Value::String("00".repeat(64));
        let tampered: Event = serde_json::from_value(value).unwrap();

        let err = crate::key_package::parse_key_package(&mls, &tampered)
            .expect_err("tampered key package signature should fail");
        assert!(format!("{err:#}").contains("verify key package event signature"));
    }

    #[test]
    fn encrypted_state_file_rejects_wrong_key() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("pika-mls.json");
        let mls = PikaMls::from_engine(
            OpenMlsEngine::open_with_codec(
                state_path.clone(),
                StateCodec::Encrypted { key: [1; 32] },
            )
            .unwrap(),
        );
        let keys = Keys::generate();
        let config = groups_api::NostrGroupConfigData::new(
            "wrong key chat".to_string(),
            String::new(),
            None,
            None,
            None,
            Vec::new(),
            vec![keys.public_key()],
        );
        mls.create_group(&keys.public_key(), Vec::new(), config)
            .unwrap();
        drop(mls);

        let err = match OpenMlsEngine::open_with_codec(
            state_path,
            StateCodec::Encrypted { key: [2; 32] },
        ) {
            Ok(_) => panic!("wrong state key unexpectedly opened encrypted OpenMLS state"),
            Err(err) => err,
        };
        assert!(format!("{err:#}").contains("decrypt encrypted OpenMLS state"));
    }

    struct TwoMemberOpenMlsFixture {
        _alice_dir: tempfile::TempDir,
        _bob_dir: tempfile::TempDir,
        alice_mls: PikaMls,
        bob_mls: PikaMls,
        alice_keys: Keys,
        created: groups_api::GroupResult,
    }

    fn two_member_openmls_fixture() -> TwoMemberOpenMlsFixture {
        let alice_dir = tempfile::tempdir().unwrap();
        let bob_dir = tempfile::tempdir().unwrap();
        let alice_mls = open_unencrypted_mls(alice_dir.path()).unwrap();
        let bob_mls = open_unencrypted_mls(bob_dir.path()).unwrap();
        let alice_keys = Keys::generate();
        let bob_keys = Keys::generate();
        let relay = RelayUrl::parse("wss://example.test").unwrap();

        let (kp_content, kp_tags, _hash_ref) = crate::key_package::create_key_package_for_event(
            &bob_mls,
            &bob_keys.public_key(),
            vec![relay.clone()],
        )
        .unwrap();
        let bob_key_package = EventBuilder::new(Kind::MlsKeyPackage, kp_content)
            .tags(kp_tags)
            .sign_with_keys(&bob_keys)
            .unwrap();
        let config = groups_api::NostrGroupConfigData::new(
            "private chat".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![relay],
            vec![alice_keys.public_key()],
        );

        let created = alice_mls
            .create_group(&alice_keys.public_key(), vec![bob_key_package], config)
            .unwrap();
        let wrapper_id = EventId::from_hex(&"1".repeat(64)).unwrap();
        let staged = crate::welcome::stage_pending_welcome(
            &bob_mls,
            &wrapper_id,
            &created.welcome_rumors[0],
        )
        .unwrap();
        crate::welcome::accept_pending_welcome(&bob_mls, &staged).unwrap();

        TwoMemberOpenMlsFixture {
            _alice_dir: alice_dir,
            _bob_dir: bob_dir,
            alice_mls,
            bob_mls,
            alice_keys,
            created,
        }
    }

    #[test]
    fn application_messages_are_openmls_encrypted_and_shared_via_welcome() {
        let fixture = two_member_openmls_fixture();

        let plaintext = "server must not see this plaintext";
        let rumor =
            EventBuilder::new(Kind::ChatMessage, plaintext).build(fixture.alice_keys.public_key());
        let wrapped = crate::conversation::wrap_rumor(
            &fixture.alice_mls,
            &fixture.created.group.mls_group_id,
            rumor,
        )
        .unwrap();
        assert!(!wrapped.wrapper.content.contains(plaintext));
        assert!(wrapped.wrapper.content.contains("\"mls_message\""));

        let processed =
            crate::conversation::process_group_message_event(&fixture.bob_mls, &wrapped.wrapper)
                .unwrap()
                .unwrap();
        match processed {
            MessageProcessingResult::ApplicationMessage(message) => {
                assert_eq!(message.content, plaintext);
                assert_eq!(message.pubkey, fixture.alice_keys.public_key());
            }
            other => panic!("expected application message, got {other:?}"),
        }
    }

    #[test]
    fn application_rumor_pubkey_must_match_mls_sender_credential() {
        let fixture = two_member_openmls_fixture();
        let charlie_keys = Keys::generate();
        let rumor = EventBuilder::new(Kind::ChatMessage, "spoof").build(charlie_keys.public_key());
        let wrapped = crate::conversation::wrap_rumor(
            &fixture.alice_mls,
            &fixture.created.group.mls_group_id,
            rumor,
        )
        .unwrap();

        let err =
            crate::conversation::process_group_message_event(&fixture.bob_mls, &wrapped.wrapper)
                .expect_err("spoofed application rumor should fail");
        assert!(format!("{err:#}").contains("application rumor pubkey"));
    }

    #[test]
    fn non_admin_cannot_prepare_membership_commit() {
        let fixture = two_member_openmls_fixture();
        let charlie_dir = tempfile::tempdir().unwrap();
        let charlie_mls = open_unencrypted_mls(charlie_dir.path()).unwrap();
        let charlie_keys = Keys::generate();
        let (kp_content, kp_tags, _hash_ref) = crate::key_package::create_key_package_for_event(
            &charlie_mls,
            &charlie_keys.public_key(),
            Vec::<RelayUrl>::new(),
        )
        .unwrap();
        let charlie_key_package = EventBuilder::new(Kind::MlsKeyPackage, kp_content)
            .tags(kp_tags)
            .sign_with_keys(&charlie_keys)
            .unwrap();

        let err = fixture
            .bob_mls
            .add_members(&fixture.created.group.mls_group_id, &[charlie_key_package])
            .expect_err("non-admin member should not prepare membership commit");
        assert!(format!("{err:#}").contains("group update requires an admin sender"));
    }

    #[test]
    fn mls_message_envelope_kind_must_match_protocol_message() {
        let fixture = two_member_openmls_fixture();
        let rumor = EventBuilder::new(Kind::ChatMessage, "kind mismatch")
            .build(fixture.alice_keys.public_key());
        let wrapped = crate::conversation::wrap_rumor(
            &fixture.alice_mls,
            &fixture.created.group.mls_group_id,
            rumor,
        )
        .unwrap();
        let mut envelope: MlsMessageEnvelope =
            serde_json::from_str(&wrapped.wrapper.content).unwrap();
        envelope.message_kind = MlsMessageKind::Commit;
        let tampered = EventBuilder::new(
            Kind::MlsGroupMessage,
            serde_json::to_string(&envelope).unwrap(),
        )
        .sign_with_keys(&Keys::generate())
        .unwrap();

        let err = crate::conversation::process_group_message_event(&fixture.bob_mls, &tampered)
            .expect_err("mismatched envelope kind should fail");
        assert!(format!("{err:#}").contains("MLS envelope kind"));
    }

    #[test]
    fn media_encryption_rejects_tampered_ciphertext() {
        let dir = tempfile::tempdir().unwrap();
        let mls = open_unencrypted_mls(dir.path()).unwrap();
        let keys = Keys::generate();
        let config = groups_api::NostrGroupConfigData::new(
            "media chat".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://example.test").unwrap()],
            vec![keys.public_key()],
        );
        let created = mls
            .create_group(&keys.public_key(), Vec::new(), config)
            .unwrap();
        let manager = mls.media_manager(created.group.mls_group_id);
        let upload = manager
            .encrypt_for_upload_with_options(
                b"hello media",
                "text/plain",
                "hello.txt",
                &encrypted_media::types::MediaProcessingOptions::validation_only(),
            )
            .unwrap();
        let reference = manager.create_media_reference(&upload, "https://example.test/blob".into());
        let mut tampered = upload.encrypted_data.clone();
        let last = tampered.last_mut().unwrap();
        *last ^= 0x01;

        let err = manager
            .decrypt_from_download(&tampered, &reference)
            .unwrap_err();
        assert!(matches!(
            err,
            encrypted_media::types::EncryptedMediaError::DecryptionFailed { .. }
        ));
    }
}
