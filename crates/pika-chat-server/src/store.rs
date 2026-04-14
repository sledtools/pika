use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::protocol::{
    AppendRoomEventRequest, ClaimKeyPackageRequest, CreateRoomRequest, DeviceRecord,
    KeyPackageRecord, RegisterDeviceRequest, RoomEvent, RoomEventType, RoomSummary,
    SubmitMembershipCommitRequest, UploadKeyPackageRequest, UploadWelcomeRequest, WelcomeRecord,
};

const MAX_SYNC_LIMIT: usize = 200;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("room not found")]
    RoomNotFound,
    #[error("device not found")]
    DeviceNotFound,
    #[error("not a room member")]
    NotRoomMember,
    #[error("device does not belong to caller")]
    DeviceOwnerMismatch,
    #[error("event content must not be empty")]
    EmptyEventContent,
    #[error("key package payload must not be empty")]
    EmptyKeyPackagePayload,
    #[error("key package not found")]
    KeyPackageNotFound,
    #[error("room epoch mismatch: expected {expected}, actual {actual}")]
    RoomEpochMismatch { expected: u64, actual: u64 },
}

#[derive(Debug, Error)]
pub enum StoreHandleError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("persist chat store: {0}")]
    Persist(#[from] anyhow::Error),
}

#[derive(Clone)]
pub struct StoreHandle {
    inner: Arc<RwLock<ChatStore>>,
    state_path: Option<PathBuf>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatStore {
    devices_by_owner: BTreeMap<String, BTreeMap<String, DeviceRecord>>,
    key_packages_by_owner: BTreeMap<String, Vec<KeyPackageRecord>>,
    welcomes_by_recipient: BTreeMap<String, Vec<WelcomeRecord>>,
    rooms: BTreeMap<String, StoredRoom>,
}

#[derive(Serialize, Deserialize)]
struct StoredRoom {
    summary: RoomSummary,
    events: Vec<RoomEvent>,
}

impl StoreHandle {
    pub fn in_memory() -> Self {
        Self {
            inner: Arc::new(RwLock::new(ChatStore::default())),
            state_path: None,
        }
    }

    pub fn load_or_create(state_path: PathBuf) -> Result<Self, StoreHandleError> {
        let store = if state_path.exists() {
            let bytes = std::fs::read(&state_path)
                .with_context(|| format!("read chat store {}", state_path.display()))?;
            serde_json::from_slice::<ChatStore>(&bytes)
                .with_context(|| format!("decode chat store {}", state_path.display()))?
        } else {
            ChatStore::default()
        };

        Ok(Self {
            inner: Arc::new(RwLock::new(store)),
            state_path: Some(state_path),
        })
    }

    pub async fn register_device(
        &self,
        owner_npub: &str,
        request: RegisterDeviceRequest,
        now: u64,
    ) -> Result<DeviceRecord, StoreHandleError> {
        let mut store = self.inner.write().await;
        let device = store.register_device(owner_npub, request, now);
        self.persist_locked(&store)?;
        Ok(device)
    }

    pub async fn create_room(
        &self,
        creator_npub: &str,
        request: CreateRoomRequest,
        now: u64,
    ) -> Result<RoomSummary, StoreHandleError> {
        let mut store = self.inner.write().await;
        let room = store.create_room(creator_npub, request, now);
        self.persist_locked(&store)?;
        Ok(room)
    }

    pub async fn upload_key_package(
        &self,
        owner_npub: &str,
        request: UploadKeyPackageRequest,
        now: u64,
    ) -> Result<KeyPackageRecord, StoreHandleError> {
        let mut store = self.inner.write().await;
        let key_package = store.upload_key_package(owner_npub, request, now)?;
        self.persist_locked(&store)?;
        Ok(key_package)
    }

    pub async fn enqueue_welcome(
        &self,
        sender_npub: &str,
        request: UploadWelcomeRequest,
        now: u64,
    ) -> Result<WelcomeRecord, StoreHandleError> {
        let mut store = self.inner.write().await;
        let welcome = store.enqueue_welcome(sender_npub, request, now)?;
        self.persist_locked(&store)?;
        Ok(welcome)
    }

    pub async fn claim_welcomes(
        &self,
        recipient_npub: &str,
    ) -> Result<Vec<WelcomeRecord>, StoreHandleError> {
        let mut store = self.inner.write().await;
        let welcomes = store.claim_welcomes(recipient_npub);
        self.persist_locked(&store)?;
        Ok(welcomes)
    }

    pub async fn claim_key_package(
        &self,
        claimer_npub: &str,
        request: ClaimKeyPackageRequest,
        now: u64,
    ) -> Result<KeyPackageRecord, StoreHandleError> {
        let mut store = self.inner.write().await;
        let key_package = store.claim_key_package(claimer_npub, request, now)?;
        self.persist_locked(&store)?;
        Ok(key_package)
    }

    pub async fn append_room_event(
        &self,
        sender_npub: &str,
        room_id: &str,
        request: AppendRoomEventRequest,
        now: u64,
    ) -> Result<RoomEvent, StoreHandleError> {
        let mut store = self.inner.write().await;
        let event = store.append_room_event(sender_npub, room_id, request, now)?;
        self.persist_locked(&store)?;
        Ok(event)
    }

    pub async fn submit_membership_commit(
        &self,
        sender_npub: &str,
        room_id: &str,
        request: SubmitMembershipCommitRequest,
        now: u64,
    ) -> Result<(RoomSummary, RoomEvent), StoreHandleError> {
        let mut store = self.inner.write().await;
        let result = store.submit_membership_commit(sender_npub, room_id, request, now)?;
        self.persist_locked(&store)?;
        Ok(result)
    }

    pub async fn room_summary_for_member(
        &self,
        member_npub: &str,
        room_id: &str,
    ) -> Result<RoomSummary, StoreError> {
        self.inner
            .read()
            .await
            .room_summary_for_member(member_npub, room_id)
    }

    pub async fn sync_room_events(
        &self,
        member_npub: &str,
        room_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<RoomEvent>, StoreError> {
        self.inner
            .read()
            .await
            .sync_room_events(member_npub, room_id, after_seq, limit)
    }

    fn persist_locked(&self, store: &ChatStore) -> Result<(), StoreHandleError> {
        let Some(path) = &self.state_path else {
            return Ok(());
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create chat store dir {}", parent.display()))?;
        }

        let bytes = serde_json::to_vec_pretty(store).context("serialize chat store")?;
        let tmp_path = tmp_path(path);
        std::fs::write(&tmp_path, bytes)
            .with_context(|| format!("write chat store {}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, path).with_context(|| {
            format!(
                "move persisted chat store from {} to {}",
                tmp_path.display(),
                path.display()
            )
        })?;
        Ok(())
    }
}

impl ChatStore {
    pub fn register_device(
        &mut self,
        owner_npub: &str,
        request: RegisterDeviceRequest,
        now: u64,
    ) -> DeviceRecord {
        let device = DeviceRecord {
            device_id: new_prefixed_id("dev"),
            owner_npub: owner_npub.to_string(),
            platform: clean_optional_field(request.platform),
            push_token: clean_optional_field(request.push_token),
            created_at: now,
        };
        self.devices_by_owner
            .entry(owner_npub.to_string())
            .or_default()
            .insert(device.device_id.clone(), device.clone());
        device
    }

    pub fn create_room(
        &mut self,
        creator_npub: &str,
        request: CreateRoomRequest,
        now: u64,
    ) -> RoomSummary {
        let mut members = normalized_member_npubs(request.member_npubs);
        members.insert(creator_npub.to_string());

        let summary = RoomSummary {
            room_id: new_prefixed_id("room"),
            created_by: creator_npub.to_string(),
            members: members.into_iter().collect(),
            epoch: 0,
            last_seq: 0,
            created_at: now,
        };
        self.rooms.insert(
            summary.room_id.clone(),
            StoredRoom {
                summary: summary.clone(),
                events: Vec::new(),
            },
        );
        summary
    }

    pub fn upload_key_package(
        &mut self,
        owner_npub: &str,
        request: UploadKeyPackageRequest,
        now: u64,
    ) -> Result<KeyPackageRecord, StoreError> {
        let devices = self
            .devices_by_owner
            .get(owner_npub)
            .ok_or(StoreError::DeviceNotFound)?;
        if !devices.contains_key(&request.device_id) {
            return Err(StoreError::DeviceOwnerMismatch);
        }

        let payload = request.payload.trim().to_string();
        if payload.is_empty() {
            return Err(StoreError::EmptyKeyPackagePayload);
        }

        let key_package = KeyPackageRecord {
            key_package_id: new_prefixed_id("kp"),
            owner_npub: owner_npub.to_string(),
            device_id: request.device_id,
            ciphersuite: clean_optional_field(request.ciphersuite),
            payload,
            created_at: now,
            claimed_at: None,
            claimed_by_npub: None,
            claimed_by_room_id: None,
        };
        self.key_packages_by_owner
            .entry(owner_npub.to_string())
            .or_default()
            .push(key_package.clone());
        Ok(key_package)
    }

    fn ensure_device_owner(
        &self,
        owner_npub: &str,
        device_id: Option<&str>,
    ) -> Result<(), StoreError> {
        let Some(device_id) = device_id else {
            return Ok(());
        };
        let devices = self
            .devices_by_owner
            .get(owner_npub)
            .ok_or(StoreError::DeviceNotFound)?;
        if devices.contains_key(device_id) {
            Ok(())
        } else {
            Err(StoreError::DeviceOwnerMismatch)
        }
    }

    pub fn enqueue_welcome(
        &mut self,
        sender_npub: &str,
        request: UploadWelcomeRequest,
        now: u64,
    ) -> Result<WelcomeRecord, StoreError> {
        let welcome = prepare_welcome_record(
            sender_npub,
            request.recipient_npub,
            request.wrapper_event_json,
            request.server_url,
            request.room_id,
            now,
        )?;
        self.welcomes_by_recipient
            .entry(welcome.recipient_npub.clone())
            .or_default()
            .push(welcome.clone());
        Ok(welcome)
    }

    pub fn claim_welcomes(&mut self, recipient_npub: &str) -> Vec<WelcomeRecord> {
        self.welcomes_by_recipient
            .remove(recipient_npub)
            .unwrap_or_default()
    }

    pub fn claim_key_package(
        &mut self,
        claimer_npub: &str,
        request: ClaimKeyPackageRequest,
        now: u64,
    ) -> Result<KeyPackageRecord, StoreError> {
        let owner_npub = request.owner_npub.trim().to_ascii_lowercase();
        if owner_npub.is_empty() {
            return Err(StoreError::KeyPackageNotFound);
        }

        if let Some(room_id) = request.room_id.as_deref() {
            let room = self.rooms.get(room_id).ok_or(StoreError::RoomNotFound)?;
            if !room
                .summary
                .members
                .iter()
                .any(|member| member == claimer_npub)
            {
                return Err(StoreError::NotRoomMember);
            }
        }

        let key_packages = self
            .key_packages_by_owner
            .get_mut(&owner_npub)
            .ok_or(StoreError::KeyPackageNotFound)?;
        let Some(key_package) = key_packages
            .iter_mut()
            .find(|record| record.claimed_at.is_none())
        else {
            return Err(StoreError::KeyPackageNotFound);
        };

        key_package.claimed_at = Some(now);
        key_package.claimed_by_npub = Some(claimer_npub.to_string());
        key_package.claimed_by_room_id = request.room_id;
        Ok(key_package.clone())
    }

    pub fn append_room_event(
        &mut self,
        sender_npub: &str,
        room_id: &str,
        request: AppendRoomEventRequest,
        now: u64,
    ) -> Result<RoomEvent, StoreError> {
        self.ensure_device_owner(sender_npub, request.sender_device_id.as_deref())?;
        let room = self
            .rooms
            .get_mut(room_id)
            .ok_or(StoreError::RoomNotFound)?;
        ensure_room_member(room, sender_npub)?;

        let content = request.content.trim().to_string();
        if content.is_empty() {
            return Err(StoreError::EmptyEventContent);
        }

        let next_seq = room.summary.last_seq.saturating_add(1);
        let event = RoomEvent {
            event_id: new_prefixed_id("evt"),
            room_id: room.summary.room_id.clone(),
            seq: next_seq,
            event_type: request.event_type,
            epoch: request.epoch,
            sender_npub: sender_npub.to_string(),
            sender_device_id: request.sender_device_id,
            content,
            created_at: now,
        };
        room.summary.last_seq = next_seq;
        room.events.push(event.clone());
        Ok(event)
    }

    pub fn submit_membership_commit(
        &mut self,
        sender_npub: &str,
        room_id: &str,
        request: SubmitMembershipCommitRequest,
        now: u64,
    ) -> Result<(RoomSummary, RoomEvent), StoreError> {
        let SubmitMembershipCommitRequest {
            expected_epoch,
            sender_device_id,
            member_npubs,
            wrapper_event_json,
            welcomes,
        } = request;
        self.ensure_device_owner(sender_npub, sender_device_id.as_deref())?;
        let content = wrapper_event_json.trim().to_string();
        if content.is_empty() {
            return Err(StoreError::EmptyEventContent);
        }

        let welcomes = welcomes
            .into_iter()
            .map(|welcome| {
                prepare_welcome_record(
                    sender_npub,
                    welcome.recipient_npub,
                    welcome.wrapper_event_json,
                    welcome.server_url,
                    welcome.room_id,
                    now,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        let (summary, event) = {
            let room = self
                .rooms
                .get_mut(room_id)
                .ok_or(StoreError::RoomNotFound)?;
            ensure_room_member(room, sender_npub)?;

            if expected_epoch != room.summary.epoch {
                return Err(StoreError::RoomEpochMismatch {
                    expected: expected_epoch,
                    actual: room.summary.epoch,
                });
            }

            let next_seq = room.summary.last_seq.saturating_add(1);
            let next_epoch = room.summary.epoch.saturating_add(1);
            let event = RoomEvent {
                event_id: new_prefixed_id("evt"),
                room_id: room.summary.room_id.clone(),
                seq: next_seq,
                event_type: RoomEventType::Commit,
                epoch: next_epoch,
                sender_npub: sender_npub.to_string(),
                sender_device_id,
                content,
                created_at: now,
            };

            room.summary.last_seq = next_seq;
            room.summary.epoch = next_epoch;
            room.summary.members = normalized_member_npubs(member_npubs).into_iter().collect();
            room.events.push(event.clone());
            (room.summary.clone(), event)
        };

        for welcome in welcomes {
            self.welcomes_by_recipient
                .entry(welcome.recipient_npub.clone())
                .or_default()
                .push(welcome);
        }

        Ok((summary, event))
    }

    pub fn room_summary_for_member(
        &self,
        member_npub: &str,
        room_id: &str,
    ) -> Result<RoomSummary, StoreError> {
        let room = self.rooms.get(room_id).ok_or(StoreError::RoomNotFound)?;
        if !room
            .summary
            .members
            .iter()
            .any(|member| member == member_npub)
        {
            return Err(StoreError::NotRoomMember);
        }
        Ok(room.summary.clone())
    }

    pub fn sync_room_events(
        &self,
        member_npub: &str,
        room_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<RoomEvent>, StoreError> {
        let room = self.rooms.get(room_id).ok_or(StoreError::RoomNotFound)?;
        if !room
            .summary
            .members
            .iter()
            .any(|member| member == member_npub)
        {
            return Err(StoreError::NotRoomMember);
        }
        let limit = limit.clamp(1, MAX_SYNC_LIMIT);
        Ok(room
            .events
            .iter()
            .filter(|event| event.seq > after_seq)
            .take(limit)
            .cloned()
            .collect())
    }
}

fn clean_optional_field(value: Option<String>) -> Option<String> {
    value
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

fn prepare_welcome_record(
    sender_npub: &str,
    recipient_npub: String,
    wrapper_event_json: String,
    server_url: Option<String>,
    room_id: Option<String>,
    now: u64,
) -> Result<WelcomeRecord, StoreError> {
    let recipient_npub = recipient_npub.trim().to_ascii_lowercase();
    let wrapper_event_json = wrapper_event_json.trim().to_string();
    if wrapper_event_json.is_empty() {
        return Err(StoreError::EmptyEventContent);
    }
    let server_url = clean_optional_field(server_url);
    let room_id = clean_optional_field(room_id);
    let (server_url, room_id) = match (server_url, room_id) {
        (Some(server_url), Some(room_id)) => (Some(server_url), Some(room_id)),
        _ => (None, None),
    };

    Ok(WelcomeRecord {
        welcome_id: new_prefixed_id("welcome"),
        recipient_npub,
        sender_npub: sender_npub.to_string(),
        wrapper_event_json,
        server_url,
        room_id,
        created_at: now,
    })
}

fn normalized_member_npubs(values: Vec<String>) -> BTreeSet<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

fn ensure_room_member(room: &StoredRoom, sender_npub: &str) -> Result<(), StoreError> {
    if room
        .summary
        .members
        .iter()
        .any(|member| member == sender_npub)
    {
        Ok(())
    } else {
        Err(StoreError::NotRoomMember)
    }
}

fn new_prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    match tmp.extension() {
        Some(extension) => {
            let mut with_tmp = extension.to_os_string();
            with_tmp.push(".tmp");
            tmp.set_extension(with_tmp);
        }
        None => {
            tmp.set_extension("tmp");
        }
    }
    tmp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AppendRoomEventRequest, ClaimKeyPackageRequest, CreateRoomRequest, RegisterDeviceRequest,
        RoomEventType, SubmitMembershipCommitRequest, UploadKeyPackageRequest, WelcomeEnvelope,
    };

    #[test]
    fn create_room_deduplicates_members() {
        let mut store = ChatStore::default();
        let room = store.create_room(
            "npub1alice",
            CreateRoomRequest {
                member_npubs: vec!["npub1bob".to_string(), "npub1alice".to_string()],
                epoch: None,
            },
            100,
        );
        assert_eq!(
            room.members,
            vec!["npub1alice".to_string(), "npub1bob".to_string()]
        );
        assert_eq!(room.epoch, 0);
    }

    #[test]
    fn append_requires_membership() {
        let mut store = ChatStore::default();
        let room = store.create_room(
            "npub1alice",
            CreateRoomRequest {
                member_npubs: vec![],
                epoch: None,
            },
            100,
        );
        let err = store
            .append_room_event(
                "npub1mallory",
                &room.room_id,
                AppendRoomEventRequest {
                    event_type: RoomEventType::ApplicationMessage,
                    epoch: 1,
                    sender_device_id: None,
                    content: "abc".to_string(),
                },
                101,
            )
            .expect_err("outsider should fail");
        assert!(matches!(err, StoreError::NotRoomMember));
    }

    #[test]
    fn key_package_upload_and_claim_round_trip() {
        let mut store = ChatStore::default();
        let device = store.register_device(
            "npub1alice",
            RegisterDeviceRequest {
                platform: Some("ios".to_string()),
                push_token: None,
            },
            100,
        );
        let key_package = store
            .upload_key_package(
                "npub1alice",
                UploadKeyPackageRequest {
                    device_id: device.device_id.clone(),
                    ciphersuite: Some("mls128".to_string()),
                    payload: "opaque-key-package".to_string(),
                },
                101,
            )
            .expect("upload key package");
        assert_eq!(key_package.claimed_at, None);

        let claimed = store
            .claim_key_package(
                "npub1bob",
                ClaimKeyPackageRequest {
                    owner_npub: "npub1alice".to_string(),
                    room_id: None,
                },
                102,
            )
            .expect("claim key package");
        assert_eq!(claimed.claimed_by_npub.as_deref(), Some("npub1bob"));
        assert_eq!(claimed.payload, "opaque-key-package");

        let err = store
            .claim_key_package(
                "npub1carol",
                ClaimKeyPackageRequest {
                    owner_npub: "npub1alice".to_string(),
                    room_id: None,
                },
                103,
            )
            .expect_err("single uploaded package should be exhausted");
        assert!(matches!(err, StoreError::KeyPackageNotFound));
    }

    #[test]
    fn membership_commit_updates_room_epoch_members_and_welcomes() {
        let mut store = ChatStore::default();
        let device = store.register_device(
            "npub1alice",
            RegisterDeviceRequest {
                platform: Some("ios".to_string()),
                push_token: None,
            },
            100,
        );
        let room = store.create_room(
            "npub1alice",
            CreateRoomRequest {
                member_npubs: vec!["npub1bob".to_string()],
                epoch: None,
            },
            101,
        );

        let (summary, event) = store
            .submit_membership_commit(
                "npub1alice",
                &room.room_id,
                SubmitMembershipCommitRequest {
                    expected_epoch: 0,
                    sender_device_id: Some(device.device_id),
                    member_npubs: vec![
                        "npub1alice".to_string(),
                        "npub1bob".to_string(),
                        "npub1carol".to_string(),
                    ],
                    wrapper_event_json: "{\"kind\":1059,\"content\":\"commit\"}".to_string(),
                    welcomes: vec![WelcomeEnvelope {
                        recipient_npub: "npub1carol".to_string(),
                        wrapper_event_json: "{\"kind\":1059,\"content\":\"welcome\"}".to_string(),
                        server_url: Some("https://chat.example".to_string()),
                        room_id: Some(room.room_id.clone()),
                    }],
                },
                102,
            )
            .expect("submit membership commit");

        assert_eq!(event.event_type, RoomEventType::Commit);
        assert_eq!(event.seq, 1);
        assert_eq!(event.epoch, 1);
        assert_eq!(summary.epoch, 1);
        assert_eq!(summary.last_seq, 1);
        assert!(summary.members.contains(&"npub1carol".to_string()));

        let claimed = store.claim_welcomes("npub1carol");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].sender_npub, "npub1alice");
        assert_eq!(
            claimed[0].server_url.as_deref(),
            Some("https://chat.example")
        );
        assert_eq!(claimed[0].room_id.as_deref(), Some(room.room_id.as_str()));

        let synced_summary = store
            .room_summary_for_member("npub1carol", &room.room_id)
            .expect("new member should see room");
        assert_eq!(synced_summary.epoch, 1);
    }

    #[test]
    fn membership_commit_epoch_mismatch_is_atomic() {
        let mut store = ChatStore::default();
        let device = store.register_device(
            "npub1alice",
            RegisterDeviceRequest {
                platform: Some("ios".to_string()),
                push_token: None,
            },
            100,
        );
        let room = store.create_room(
            "npub1alice",
            CreateRoomRequest {
                member_npubs: vec!["npub1bob".to_string()],
                epoch: None,
            },
            101,
        );

        let err = store
            .submit_membership_commit(
                "npub1alice",
                &room.room_id,
                SubmitMembershipCommitRequest {
                    expected_epoch: 1,
                    sender_device_id: Some(device.device_id),
                    member_npubs: vec!["npub1alice".to_string(), "npub1carol".to_string()],
                    wrapper_event_json: "{\"kind\":1059,\"content\":\"commit\"}".to_string(),
                    welcomes: vec![WelcomeEnvelope {
                        recipient_npub: "npub1carol".to_string(),
                        wrapper_event_json: "{\"kind\":1059,\"content\":\"welcome\"}".to_string(),
                        server_url: Some("https://chat.example".to_string()),
                        room_id: Some(room.room_id.clone()),
                    }],
                },
                102,
            )
            .expect_err("stale epoch should fail");
        assert!(matches!(
            err,
            StoreError::RoomEpochMismatch {
                expected: 1,
                actual: 0
            }
        ));

        let summary = store
            .room_summary_for_member("npub1alice", &room.room_id)
            .expect("room should still exist");
        assert_eq!(summary.epoch, 0);
        assert_eq!(summary.last_seq, 0);
        assert_eq!(
            summary.members,
            vec!["npub1alice".to_string(), "npub1bob".to_string()]
        );
        assert!(store.claim_welcomes("npub1carol").is_empty());
        assert!(store
            .sync_room_events("npub1alice", &room.room_id, 0, 10)
            .expect("sync events")
            .is_empty());
    }

    #[test]
    fn persistent_store_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("chat-store.json");
        let handle = StoreHandle::load_or_create(path.clone()).expect("create persistent store");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let room_id = rt.block_on(async {
            let room = handle
                .create_room(
                    "npub1alice",
                    CreateRoomRequest {
                        member_npubs: vec!["npub1bob".to_string()],
                        epoch: None,
                    },
                    100,
                )
                .await
                .expect("create room");
            let _ = handle
                .append_room_event(
                    "npub1alice",
                    &room.room_id,
                    AppendRoomEventRequest {
                        event_type: RoomEventType::ApplicationMessage,
                        epoch: 1,
                        sender_device_id: None,
                        content: "ciphertext-1".to_string(),
                    },
                    101,
                )
                .await
                .expect("append event");
            room.room_id
        });

        let reloaded = StoreHandle::load_or_create(path).expect("reload persistent store");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let room = reloaded
                .room_summary_for_member("npub1alice", &room_id)
                .await
                .expect("room should reload");
            assert_eq!(room.last_seq, 1);

            let events = reloaded
                .sync_room_events("npub1alice", &room_id, 0, 10)
                .await
                .expect("events should reload");
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].content, "ciphertext-1");
        });
    }
}
