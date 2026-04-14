use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::protocol::{
    AppendRoomEventRequest, CreateRoomRequest, DeviceRecord, RegisterDeviceRequest, RoomEvent,
    RoomSummary,
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
pub struct ChatStore {
    devices_by_owner: BTreeMap<String, BTreeMap<String, DeviceRecord>>,
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
        let mut members = BTreeSet::new();
        members.insert(creator_npub.to_string());
        members.extend(
            request
                .member_npubs
                .into_iter()
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty()),
        );

        let summary = RoomSummary {
            room_id: new_prefixed_id("room"),
            created_by: creator_npub.to_string(),
            members: members.into_iter().collect(),
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

    pub fn append_room_event(
        &mut self,
        sender_npub: &str,
        room_id: &str,
        request: AppendRoomEventRequest,
        now: u64,
    ) -> Result<RoomEvent, StoreError> {
        let room = self
            .rooms
            .get_mut(room_id)
            .ok_or(StoreError::RoomNotFound)?;
        if !room
            .summary
            .members
            .iter()
            .any(|member| member == sender_npub)
        {
            return Err(StoreError::NotRoomMember);
        }

        if let Some(device_id) = request.sender_device_id.as_deref() {
            let devices = self
                .devices_by_owner
                .get(sender_npub)
                .ok_or(StoreError::DeviceNotFound)?;
            if !devices.contains_key(device_id) {
                return Err(StoreError::DeviceOwnerMismatch);
            }
        }

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
    use crate::protocol::{AppendRoomEventRequest, CreateRoomRequest, RoomEventType};

    #[test]
    fn create_room_deduplicates_members() {
        let mut store = ChatStore::default();
        let room = store.create_room(
            "npub1alice",
            CreateRoomRequest {
                member_npubs: vec!["npub1bob".to_string(), "npub1alice".to_string()],
            },
            100,
        );
        assert_eq!(
            room.members,
            vec!["npub1alice".to_string(), "npub1bob".to_string()]
        );
    }

    #[test]
    fn append_requires_membership() {
        let mut store = ChatStore::default();
        let room = store.create_room(
            "npub1alice",
            CreateRoomRequest {
                member_npubs: vec![],
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
