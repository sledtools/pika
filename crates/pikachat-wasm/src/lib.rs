use std::collections::BTreeMap;

use base64::Engine;
use mdk_core::prelude::{GroupId, MessageProcessingResult, MDK};
use mdk_memory_storage::MdkMemoryStorage;
use nostr::{
    Event, EventBuilder, EventId, JsonUtil, Keys, Kind, RelayUrl, SecretKey, UnsignedEvent,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SNAPSHOT_SCHEMA_VERSION: u32 = 2;
const DEFAULT_KEYPACKAGE_RELAY: &str = "wss://relay.damus.io";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdentityState {
    pub pubkey_hex: String,
    pub secret_key_hex: String,
    pub key_package_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdentityStateV1 {
    pub pubkey_hint: String,
    pub key_package_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GroupState {
    pub group_id: String,
    #[serde(default)]
    pub mls_group_id_hex: String,
    #[serde(default)]
    pub nostr_group_id_hex: String,
    pub processed_welcomes: u64,
    pub applied_messages: u64,
    pub outbound_messages: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub schema_version: u32,
    pub identity: Option<IdentityState>,
    pub groups: BTreeMap<String, GroupState>,
    pub outbound_counter: u64,
    pub mdk_storage_snapshot_b64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSnapshotV1 {
    pub schema_version: u32,
    pub identity: Option<IdentityStateV1>,
    pub groups: BTreeMap<String, GroupState>,
    pub outbound_counter: u64,
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            identity: None,
            groups: BTreeMap::new(),
            outbound_counter: 0,
            mdk_storage_snapshot_b64: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InitOrLoadIdentityResult {
    pub created: bool,
    pub pubkey_hint: String,
    pub key_package_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyPackagePublishPayload {
    pub pubkey_hint: String,
    pub key_package_hint: String,
    pub payload_json: String,
    pub tags_json: String,
    pub event_json: String,
    pub event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessWelcomeResult {
    pub group_id: String,
    pub created_group: bool,
    pub processed_welcomes: u64,
    pub mls_group_id_hex: String,
    pub nostr_group_id_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessGroupMessageResult {
    pub group_id: String,
    pub event_id: String,
    pub applied_messages: u64,
    pub message_kind: String,
    pub plaintext: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateOutboundGroupMessageResult {
    pub group_id: String,
    pub event_id: String,
    pub sequence: u64,
    pub ciphertext_b64: String,
    pub event_json: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("identity is not initialized")]
    IdentityMissing,
    #[error("snapshot schema mismatch: expected {expected}, got {got}")]
    UnsupportedSnapshotSchema { expected: u32, got: u32 },
    #[error("group_id is required")]
    EmptyGroupId,
    #[error("content is required")]
    EmptyContent,
    #[error("group not found: {0}")]
    GroupMissing(String),
    #[error("group metadata missing mls_group_id_hex for group {0}")]
    GroupMissingMlsGroupId(String),
    #[error("legacy process_welcome requires welcome event payload")]
    LegacyProcessWelcomeUnsupported,
    #[error("invalid snapshot json: {0}")]
    InvalidSnapshotJson(#[from] serde_json::Error),
    #[error("invalid secret key: {0}")]
    InvalidSecretKey(String),
    #[error("invalid event id: {0}")]
    InvalidEventId(String),
    #[error("invalid welcome event json: {0}")]
    InvalidWelcomeEvent(String),
    #[error("invalid event json: {0}")]
    InvalidEvent(String),
    #[error("invalid group id hex for {group_id}: {reason}")]
    InvalidGroupIdHex { group_id: String, reason: String },
    #[error("storage snapshot error: {0}")]
    StorageSnapshot(String),
    #[error("mdk error: {0}")]
    Mdk(String),
}

#[derive(Debug)]
struct RealRuntime {
    keys: Keys,
    mdk: MDK<MdkMemoryStorage>,
}

#[derive(Debug, Default)]
pub struct PikachatWasmRuntime {
    snapshot: RuntimeSnapshot,
    runtime: Option<RealRuntime>,
}

impl PikachatWasmRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_snapshot_json(snapshot_json: &str) -> Result<Self, RuntimeError> {
        let mut runtime = Self::new();
        runtime.load_snapshot_json(snapshot_json)?;
        Ok(runtime)
    }

    pub fn snapshot(&self) -> &RuntimeSnapshot {
        &self.snapshot
    }

    pub fn snapshot_json(&self) -> Result<String, RuntimeError> {
        Ok(serde_json::to_string_pretty(&self.snapshot)?)
    }

    pub fn load_snapshot_json(&mut self, snapshot_json: &str) -> Result<(), RuntimeError> {
        let raw: serde_json::Value = serde_json::from_str(snapshot_json)?;
        let schema_version = raw
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .map(|v| v as u32)
            .unwrap_or(1);

        match schema_version {
            SNAPSHOT_SCHEMA_VERSION => {
                let snapshot: RuntimeSnapshot = serde_json::from_value(raw)?;
                self.snapshot = snapshot;
            }
            1 => {
                let snapshot_v1: RuntimeSnapshotV1 = serde_json::from_value(raw)?;
                self.snapshot = RuntimeSnapshot {
                    schema_version: SNAPSHOT_SCHEMA_VERSION,
                    identity: None,
                    groups: snapshot_v1.groups,
                    outbound_counter: snapshot_v1.outbound_counter,
                    mdk_storage_snapshot_b64: None,
                };
            }
            other => {
                return Err(RuntimeError::UnsupportedSnapshotSchema {
                    expected: SNAPSHOT_SCHEMA_VERSION,
                    got: other,
                });
            }
        }

        self.runtime = None;
        Ok(())
    }

    pub fn init_or_load_identity(
        &mut self,
        secret_seed_hint: Option<&str>,
    ) -> Result<InitOrLoadIdentityResult, RuntimeError> {
        if self.snapshot.identity.is_some() {
            self.ensure_runtime_loaded()?;
            let identity = self
                .snapshot
                .identity
                .as_ref()
                .ok_or(RuntimeError::IdentityMissing)?;
            return Ok(InitOrLoadIdentityResult {
                created: false,
                pubkey_hint: identity.pubkey_hex.clone(),
                key_package_hint: identity.key_package_hint.clone(),
            });
        }

        let secret_key = resolve_secret_key(secret_seed_hint)?;
        let keys = Keys::new(secret_key);
        let pubkey_hex = keys.public_key().to_hex();
        let secret_key_hex = keys.secret_key().to_secret_hex();

        self.snapshot.identity = Some(IdentityState {
            pubkey_hex: pubkey_hex.clone(),
            secret_key_hex: secret_key_hex.clone(),
            key_package_hint: String::new(),
        });
        self.runtime = Some(build_runtime_from_snapshot(&self.snapshot)?);
        self.sync_storage_snapshot_blob()?;

        Ok(InitOrLoadIdentityResult {
            created: true,
            pubkey_hint: pubkey_hex,
            key_package_hint: String::new(),
        })
    }

    pub fn publish_keypackage_payload(&mut self) -> Result<KeyPackagePublishPayload, RuntimeError> {
        let (pubkey_hint, payload_json, tags, key_package_hint, event_json, event_id) = {
            let runtime = self.runtime_mut()?;
            let relay = RelayUrl::parse(DEFAULT_KEYPACKAGE_RELAY)
                .map_err(|e| RuntimeError::Mdk(format!("parse default relay: {e}")))?;
            let (payload_json, tags, hash_ref) = runtime
                .mdk
                .create_key_package_for_event(&runtime.keys.public_key(), vec![relay])
                .map_err(map_mdk_err)?;
            let event = EventBuilder::new(Kind::MlsKeyPackage, payload_json.clone())
                .tags(tags.clone())
                .sign_with_keys(&runtime.keys)
                .map_err(|e| RuntimeError::Mdk(format!("sign key package event: {e}")))?;
            (
                runtime.keys.public_key().to_hex(),
                payload_json,
                tags,
                hex::encode(hash_ref),
                event.as_json(),
                event.id.to_hex(),
            )
        };

        if let Some(identity) = self.snapshot.identity.as_mut() {
            identity.key_package_hint = key_package_hint.clone();
        }

        self.sync_storage_snapshot_blob()?;

        Ok(KeyPackagePublishPayload {
            pubkey_hint,
            key_package_hint,
            payload_json,
            tags_json: serde_json::to_string(&tags)?,
            event_json,
            event_id,
        })
    }

    pub fn process_welcome(
        &mut self,
        _group_id: &str,
    ) -> Result<ProcessWelcomeResult, RuntimeError> {
        Err(RuntimeError::LegacyProcessWelcomeUnsupported)
    }

    pub fn process_welcome_event_json(
        &mut self,
        group_id: &str,
        wrapper_event_id_hex: &str,
        welcome_event_json: &str,
    ) -> Result<ProcessWelcomeResult, RuntimeError> {
        let normalized_group = group_id.trim();
        if normalized_group.is_empty() {
            return Err(RuntimeError::EmptyGroupId);
        }

        let wrapper_event_id = EventId::from_hex(wrapper_event_id_hex.trim())
            .map_err(|e| RuntimeError::InvalidEventId(e.to_string()))?;
        let welcome_event = UnsignedEvent::from_json(welcome_event_json)
            .map_err(|e| RuntimeError::InvalidWelcomeEvent(e.to_string()))?;

        let (mls_group_id_hex, nostr_group_id_hex) = {
            let runtime = self.runtime_mut()?;
            let welcome = runtime
                .mdk
                .process_welcome(&wrapper_event_id, &welcome_event)
                .map_err(map_mdk_err)?;
            runtime.mdk.accept_welcome(&welcome).map_err(map_mdk_err)?;
            (
                hex::encode(welcome.mls_group_id.as_slice()),
                hex::encode(welcome.nostr_group_id),
            )
        };

        let created_group = !self.snapshot.groups.contains_key(normalized_group);
        let (processed_welcomes, stored_mls_group_id_hex, stored_nostr_group_id_hex) = {
            let group = self
                .snapshot
                .groups
                .entry(normalized_group.to_string())
                .or_insert_with(|| GroupState {
                    group_id: normalized_group.to_string(),
                    ..GroupState::default()
                });
            group.processed_welcomes += 1;
            group.mls_group_id_hex = mls_group_id_hex;
            group.nostr_group_id_hex = nostr_group_id_hex;
            (
                group.processed_welcomes,
                group.mls_group_id_hex.clone(),
                group.nostr_group_id_hex.clone(),
            )
        };

        self.sync_storage_snapshot_blob()?;

        Ok(ProcessWelcomeResult {
            group_id: normalized_group.to_string(),
            created_group,
            processed_welcomes,
            mls_group_id_hex: stored_mls_group_id_hex,
            nostr_group_id_hex: stored_nostr_group_id_hex,
        })
    }

    pub fn process_group_message(
        &mut self,
        group_id: &str,
        event_id: &str,
        ciphertext_b64: &str,
    ) -> Result<ProcessGroupMessageResult, RuntimeError> {
        let normalized_group = group_id.trim();
        if normalized_group.is_empty() {
            return Err(RuntimeError::EmptyGroupId);
        }
        let normalized_event = event_id.trim();
        if normalized_event.is_empty() {
            return Err(RuntimeError::EmptyContent);
        }
        if ciphertext_b64.trim().is_empty() {
            return Err(RuntimeError::EmptyContent);
        }

        let group = self
            .snapshot
            .groups
            .entry(normalized_group.to_string())
            .or_insert_with(|| GroupState {
                group_id: normalized_group.to_string(),
                ..GroupState::default()
            });
        group.applied_messages += 1;
        let plaintext = decode_scaffold_ciphertext_content(ciphertext_b64);
        let message_kind = if plaintext.is_some() {
            "application"
        } else {
            "unknown"
        };

        Ok(ProcessGroupMessageResult {
            group_id: normalized_group.to_string(),
            event_id: normalized_event.to_string(),
            applied_messages: group.applied_messages,
            message_kind: message_kind.to_string(),
            plaintext,
        })
    }

    pub fn process_group_message_event_json(
        &mut self,
        group_id: &str,
        event_json: &str,
    ) -> Result<ProcessGroupMessageResult, RuntimeError> {
        let normalized_group = group_id.trim();
        if normalized_group.is_empty() {
            return Err(RuntimeError::EmptyGroupId);
        }

        if !self.snapshot.groups.contains_key(normalized_group) {
            return Err(RuntimeError::GroupMissing(normalized_group.to_string()));
        }

        let event =
            Event::from_json(event_json).map_err(|e| RuntimeError::InvalidEvent(e.to_string()))?;
        let event_id = event.id.to_hex();

        let runtime = self.runtime_mut()?;
        let message_result = runtime.mdk.process_message(&event).map_err(map_mdk_err)?;
        let (message_kind, plaintext) = map_message_result(&message_result);

        if let Some(group) = self.snapshot.groups.get_mut(normalized_group) {
            group.applied_messages += 1;
        }

        self.sync_storage_snapshot_blob()?;

        Ok(ProcessGroupMessageResult {
            group_id: normalized_group.to_string(),
            event_id,
            applied_messages: self
                .snapshot
                .groups
                .get(normalized_group)
                .map(|group| group.applied_messages)
                .unwrap_or(0),
            message_kind,
            plaintext,
        })
    }

    pub fn create_outbound_group_message(
        &mut self,
        group_id: &str,
        plaintext: &str,
    ) -> Result<CreateOutboundGroupMessageResult, RuntimeError> {
        let normalized_group = group_id.trim();
        if normalized_group.is_empty() {
            return Err(RuntimeError::EmptyGroupId);
        }
        let normalized_content = plaintext.trim();
        if normalized_content.is_empty() {
            return Err(RuntimeError::EmptyContent);
        }

        let group_state = self
            .snapshot
            .groups
            .get(normalized_group)
            .cloned()
            .ok_or_else(|| RuntimeError::GroupMissing(normalized_group.to_string()))?;
        if group_state.mls_group_id_hex.trim().is_empty() {
            return Err(RuntimeError::GroupMissingMlsGroupId(
                normalized_group.to_string(),
            ));
        }

        let group_id_bytes = hex::decode(group_state.mls_group_id_hex.trim()).map_err(|e| {
            RuntimeError::InvalidGroupIdHex {
                group_id: normalized_group.to_string(),
                reason: e.to_string(),
            }
        })?;
        let mls_group_id = GroupId::from_slice(&group_id_bytes);

        let runtime = self.runtime_mut()?;
        let rumor = EventBuilder::new(Kind::ChatMessage, normalized_content)
            .build(runtime.keys.public_key());
        let event = runtime
            .mdk
            .create_message(&mls_group_id, rumor)
            .map_err(map_mdk_err)?;

        self.snapshot.outbound_counter += 1;
        let sequence = self.snapshot.outbound_counter;

        if let Some(group) = self.snapshot.groups.get_mut(normalized_group) {
            group.outbound_messages += 1;
        }

        self.sync_storage_snapshot_blob()?;

        Ok(CreateOutboundGroupMessageResult {
            group_id: normalized_group.to_string(),
            event_id: event.id.to_hex(),
            sequence,
            ciphertext_b64: event.content.clone(),
            event_json: event.as_json(),
        })
    }

    fn runtime_mut(&mut self) -> Result<&mut RealRuntime, RuntimeError> {
        self.ensure_runtime_loaded()?;
        self.runtime.as_mut().ok_or(RuntimeError::IdentityMissing)
    }

    fn ensure_runtime_loaded(&mut self) -> Result<(), RuntimeError> {
        if self.runtime.is_none() {
            self.runtime = Some(build_runtime_from_snapshot(&self.snapshot)?);
        }
        Ok(())
    }

    fn sync_storage_snapshot_blob(&mut self) -> Result<(), RuntimeError> {
        if let Some(runtime) = self.runtime.as_ref() {
            let snapshot_bytes = runtime
                .mdk
                .storage()
                .create_snapshot_bytes()
                .map_err(|e| RuntimeError::StorageSnapshot(e.to_string()))?;
            self.snapshot.mdk_storage_snapshot_b64 =
                Some(base64::engine::general_purpose::STANDARD.encode(snapshot_bytes));
        }
        Ok(())
    }
}

fn build_runtime_from_snapshot(snapshot: &RuntimeSnapshot) -> Result<RealRuntime, RuntimeError> {
    let identity = snapshot
        .identity
        .as_ref()
        .ok_or(RuntimeError::IdentityMissing)?;
    let secret_bytes = hex::decode(identity.secret_key_hex.trim())
        .map_err(|e| RuntimeError::InvalidSecretKey(e.to_string()))?;
    let secret = SecretKey::from_slice(&secret_bytes)
        .map_err(|e| RuntimeError::InvalidSecretKey(e.to_string()))?;
    let keys = Keys::new(secret);

    let storage = MdkMemoryStorage::default();
    if let Some(snapshot_b64) = snapshot.mdk_storage_snapshot_b64.as_ref() {
        let encoded = snapshot_b64.trim();
        if !encoded.is_empty() {
            let snapshot_bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded.as_bytes())
                .map_err(|e| {
                    RuntimeError::StorageSnapshot(format!("decode snapshot bytes: {e}"))
                })?;
            storage
                .restore_snapshot_bytes(&snapshot_bytes)
                .map_err(|e| RuntimeError::StorageSnapshot(e.to_string()))?;
        }
    }

    Ok(RealRuntime {
        keys,
        mdk: MDK::new(storage),
    })
}

fn map_mdk_err(error: mdk_core::Error) -> RuntimeError {
    RuntimeError::Mdk(error.to_string())
}

fn map_message_result(result: &MessageProcessingResult) -> (String, Option<String>) {
    match result {
        MessageProcessingResult::ApplicationMessage(msg) => {
            ("application".to_string(), Some(msg.content.clone()))
        }
        MessageProcessingResult::Proposal(_) | MessageProcessingResult::PendingProposal { .. } => {
            ("proposal".to_string(), None)
        }
        MessageProcessingResult::IgnoredProposal { .. } => ("ignored_proposal".to_string(), None),
        MessageProcessingResult::ExternalJoinProposal { .. } => {
            ("external_join_proposal".to_string(), None)
        }
        MessageProcessingResult::Commit { .. } => ("commit".to_string(), None),
        MessageProcessingResult::Unprocessable { .. } => ("unprocessable".to_string(), None),
        MessageProcessingResult::PreviouslyFailed => ("previously_failed".to_string(), None),
    }
}

fn resolve_secret_key(seed_hint: Option<&str>) -> Result<SecretKey, RuntimeError> {
    let seed = seed_hint
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("generated-seed");

    if let Ok(bytes) = hex::decode(seed) {
        if bytes.len() == 32 {
            if let Ok(secret) = SecretKey::from_slice(&bytes) {
                return Ok(secret);
            }
        }
    }

    // Derive a deterministic fallback secret key from the seed string.
    let digest = Sha256::digest(seed.as_bytes());
    let mut candidate = digest.to_vec();
    for tweak in 0u8..=255u8 {
        candidate[31] = candidate[31].wrapping_add(tweak);
        if let Ok(secret) = SecretKey::from_slice(&candidate) {
            return Ok(secret);
        }
    }

    Err(RuntimeError::InvalidSecretKey(
        "failed to derive a valid secret key".to_string(),
    ))
}

fn decode_scaffold_ciphertext_content(ciphertext_b64: &str) -> Option<String> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(ciphertext_b64.as_bytes())
        .ok()?;
    let payload: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let content = payload.get("content")?.as_str()?.trim().to_string();
    if content.is_empty() {
        return None;
    }
    Some(content)
}

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub struct WasmRuntime {
    inner: PikachatWasmRuntime,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
impl WasmRuntime {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: PikachatWasmRuntime::new(),
        }
    }

    pub fn load_snapshot_json(&mut self, snapshot_json: &str) -> Result<(), JsValue> {
        self.inner
            .load_snapshot_json(snapshot_json)
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn snapshot_json(&self) -> Result<String, JsValue> {
        self.inner
            .snapshot_json()
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn init_or_load_identity_json(
        &mut self,
        secret_seed_hint: Option<String>,
    ) -> Result<String, JsValue> {
        let result = self
            .inner
            .init_or_load_identity(secret_seed_hint.as_deref())
            .map_err(|err| JsValue::from_str(&err.to_string()))?;
        serde_json::to_string(&result).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn publish_keypackage_payload_json(&mut self) -> Result<String, JsValue> {
        let result = self
            .inner
            .publish_keypackage_payload()
            .map_err(|err| JsValue::from_str(&err.to_string()))?;
        serde_json::to_string(&result).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn process_welcome_json(&mut self, group_id: &str) -> Result<String, JsValue> {
        let result = self
            .inner
            .process_welcome(group_id)
            .map_err(|err| JsValue::from_str(&err.to_string()))?;
        serde_json::to_string(&result).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn process_welcome_event_json(
        &mut self,
        group_id: &str,
        wrapper_event_id_hex: &str,
        welcome_event_json: &str,
    ) -> Result<String, JsValue> {
        let result = self
            .inner
            .process_welcome_event_json(group_id, wrapper_event_id_hex, welcome_event_json)
            .map_err(|err| JsValue::from_str(&err.to_string()))?;
        serde_json::to_string(&result).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn process_group_message_json(
        &mut self,
        group_id: &str,
        event_id: &str,
        ciphertext_b64: &str,
    ) -> Result<String, JsValue> {
        let result = self
            .inner
            .process_group_message(group_id, event_id, ciphertext_b64)
            .map_err(|err| JsValue::from_str(&err.to_string()))?;
        serde_json::to_string(&result).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn process_group_message_event_json(
        &mut self,
        group_id: &str,
        event_json: &str,
    ) -> Result<String, JsValue> {
        let result = self
            .inner
            .process_group_message_event_json(group_id, event_json)
            .map_err(|err| JsValue::from_str(&err.to_string()))?;
        serde_json::to_string(&result).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn create_outbound_group_message_json(
        &mut self,
        group_id: &str,
        plaintext: &str,
    ) -> Result<String, JsValue> {
        let result = self
            .inner
            .create_outbound_group_message(group_id, plaintext)
            .map_err(|err| JsValue::from_str(&err.to_string()))?;
        serde_json::to_string(&result).map_err(|err| JsValue::from_str(&err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdk_core::groups::NostrGroupConfigData;
    use nostr::{Alphabet, SingleLetterTag, Timestamp};

    fn relay_url() -> RelayUrl {
        RelayUrl::parse("wss://relay.damus.io").expect("relay")
    }

    #[test]
    fn rejects_unsupported_snapshot_schema() {
        let mut runtime = PikachatWasmRuntime::new();
        let bad = serde_json::json!({
            "schema_version": 99,
            "identity": null,
            "groups": {},
            "outbound_counter": 0,
            "mdk_storage_snapshot_b64": null,
        })
        .to_string();

        let err = runtime.load_snapshot_json(&bad).expect_err("should fail");
        match err {
            RuntimeError::UnsupportedSnapshotSchema { expected, got } => {
                assert_eq!(expected, SNAPSHOT_SCHEMA_VERSION);
                assert_eq!(got, 99);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn loads_v1_snapshot_by_migrating_to_v2() {
        let v1 = serde_json::json!({
            "schema_version": 1,
            "identity": {
                "pubkey_hint": "pk_seed",
                "key_package_hint": "kp_seed",
            },
            "groups": {
                "group-a": {
                    "group_id": "group-a",
                    "processed_welcomes": 1,
                    "applied_messages": 2,
                    "outbound_messages": 3
                }
            },
            "outbound_counter": 7
        })
        .to_string();

        let runtime = PikachatWasmRuntime::from_snapshot_json(&v1).expect("load v1");
        assert_eq!(runtime.snapshot().schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(runtime.snapshot().outbound_counter, 7);
        assert!(runtime.snapshot().identity.is_none());
        assert!(runtime.snapshot().mdk_storage_snapshot_b64.is_none());
    }

    #[test]
    fn real_mdk_roundtrip_processes_welcome_decrypts_and_encrypts() {
        let mut alice = PikachatWasmRuntime::new();
        let mut bob = PikachatWasmRuntime::new();

        let alice_id = alice
            .init_or_load_identity(Some(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ))
            .expect("alice init");
        let bob_id = bob
            .init_or_load_identity(Some(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ))
            .expect("bob init");

        let bob_keypackage = bob.publish_keypackage_payload().expect("bob keypackage");
        let bob_keypackage_event =
            Event::from_json(&bob_keypackage.event_json).expect("parse bob keypackage event");

        let alice_runtime = alice.runtime_mut().expect("alice runtime");
        let alice_pubkey = alice_runtime.keys.public_key();
        let bob_pubkey = bob_runtime_pubkey(&bob);

        let config = NostrGroupConfigData::new(
            "Agent Chat".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![relay_url()],
            vec![alice_pubkey, bob_pubkey],
        );

        let create_result = alice_runtime
            .mdk
            .create_group(&alice_pubkey, vec![bob_keypackage_event], config)
            .expect("alice create group");

        let welcome_rumor = create_result
            .welcome_rumors
            .first()
            .expect("welcome rumor")
            .clone();
        let runtime_group_id = hex::encode(create_result.group.nostr_group_id);
        let wrapper_event_id = EventId::all_zeros().to_hex();

        bob.process_welcome_event_json(
            &runtime_group_id,
            &wrapper_event_id,
            &welcome_rumor.as_json(),
        )
        .expect("bob process welcome");

        let alice_msg_rumor =
            EventBuilder::new(Kind::ChatMessage, "hello from alice").build(alice_pubkey);
        let alice_msg_event = alice_runtime
            .mdk
            .create_message(&create_result.group.mls_group_id, alice_msg_rumor)
            .expect("alice create message");

        let bob_processed = bob
            .process_group_message_event_json(&runtime_group_id, &alice_msg_event.as_json())
            .expect("bob process group message");
        assert_eq!(bob_processed.message_kind, "application");
        assert_eq!(bob_processed.plaintext.as_deref(), Some("hello from alice"));

        let bob_outbound = bob
            .create_outbound_group_message(&runtime_group_id, "hello from bob")
            .expect("bob create outbound");
        let bob_outbound_event =
            Event::from_json(&bob_outbound.event_json).expect("parse bob outbound event");

        match alice_runtime
            .mdk
            .process_message(&bob_outbound_event)
            .expect("alice process bob outbound")
        {
            MessageProcessingResult::ApplicationMessage(msg) => {
                assert_eq!(msg.pubkey.to_hex(), bob_id.pubkey_hint);
                assert_eq!(msg.content, "hello from bob");
            }
            other => panic!("unexpected message processing result: {other:?}"),
        }

        assert!(!bob
            .snapshot
            .mdk_storage_snapshot_b64
            .as_deref()
            .unwrap_or_default()
            .is_empty());

        // Snapshot restore should preserve state enough to continue sending in the same group.
        let snapshot_json = bob.snapshot_json().expect("snapshot json");
        let mut restored =
            PikachatWasmRuntime::from_snapshot_json(&snapshot_json).expect("restore");
        let restored_outbound = restored
            .create_outbound_group_message(&runtime_group_id, "restored runtime message")
            .expect("restored outbound");
        let restored_event =
            Event::from_json(&restored_outbound.event_json).expect("parse restored event");
        match alice_runtime
            .mdk
            .process_message(&restored_event)
            .expect("alice process restored outbound")
        {
            MessageProcessingResult::ApplicationMessage(msg) => {
                assert_eq!(msg.pubkey.to_hex(), bob_id.pubkey_hint);
                assert_eq!(msg.content, "restored runtime message");
            }
            other => panic!("unexpected restored message result: {other:?}"),
        }

        // Ensure group tag matches runtime group id mapping semantics.
        let h_tag = restored_event
            .tags
            .iter()
            .find(|tag| {
                tag.kind() == nostr::TagKind::SingleLetter(SingleLetterTag::lowercase(Alphabet::H))
            })
            .and_then(|tag| tag.content())
            .expect("h tag");
        assert_eq!(h_tag, runtime_group_id);

        let now = Timestamp::now();
        assert!(now.as_u64() > 0);
        assert_eq!(alice_id.created, true);
    }

    #[test]
    fn legacy_process_group_message_surfaces_scaffold_plaintext() {
        let mut runtime = PikachatWasmRuntime::new();
        runtime
            .init_or_load_identity(Some("seed"))
            .expect("init identity");

        let payload = serde_json::json!({ "content": "hello" }).to_string();
        let ciphertext = base64::engine::general_purpose::STANDARD.encode(payload.as_bytes());
        let processed = runtime
            .process_group_message("group-a", "evt-test", &ciphertext)
            .expect("process");

        assert_eq!(processed.message_kind, "application");
        assert_eq!(processed.plaintext.as_deref(), Some("hello"));
    }

    fn bob_runtime_pubkey(runtime: &PikachatWasmRuntime) -> nostr::PublicKey {
        runtime
            .snapshot
            .identity
            .as_ref()
            .expect("bob identity")
            .pubkey_hex
            .parse()
            .expect("bob pubkey")
    }
}
