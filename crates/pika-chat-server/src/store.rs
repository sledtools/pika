use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use nostr_sdk::prelude::{Event, Kind};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::protocol::{
    AckWelcomeRequest, AppendRoomEventRequest, ClaimKeyPackageRequest, CreateRoomRequest,
    FinalizeKeyPackageRequest, KeyPackageRecord, ReleaseKeyPackageRequest, ReleaseWelcomeRequest,
    RoomEvent, RoomEventType, RoomSummary, SubmitMembershipCommitRequest, SubmitRoomCommitRequest,
    UploadKeyPackageRequest, UploadWelcomeRequest, WelcomeRecord,
};

const MAX_SYNC_LIMIT: usize = 200;
const LEASE_TTL_SECS: u64 = 300;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("room not found")]
    RoomNotFound,
    #[error("not a room member")]
    NotRoomMember,
    #[error("event content must not be empty")]
    EmptyEventContent,
    #[error("key package payload must not be empty")]
    EmptyKeyPackagePayload,
    #[error("key package not found")]
    KeyPackageNotFound,
    #[error("welcome not found")]
    WelcomeNotFound,
    #[error("lease is not active")]
    LeaseNotActive,
    #[error("lease token does not match")]
    LeaseTokenMismatch,
    #[error("room epoch mismatch: expected {expected}, actual {actual}")]
    RoomEpochMismatch { expected: u64, actual: u64 },
    #[error("commit events must use the room commit endpoint")]
    CommitRequiresCommitEndpoint,
    #[error("room event envelope invalid: {0}")]
    InvalidRoomEventEnvelope(String),
    #[error("room event type {event_type:?} does not match MLS message kind {message_kind}")]
    RoomEventKindMismatch {
        event_type: RoomEventType,
        message_kind: String,
    },
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

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatStore {
    key_packages_by_owner: BTreeMap<String, Vec<KeyPackageRecord>>,
    welcomes_by_recipient: BTreeMap<String, Vec<WelcomeRecord>>,
    rooms: BTreeMap<String, StoredRoom>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredRoom {
    summary: RoomSummary,
    events: Vec<StoredRoomEvent>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredRoomEvent {
    #[serde(flatten)]
    event: RoomEvent,
    #[serde(default)]
    visible_to_npubs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum MlsMessageKind {
    Application,
    Commit,
    Proposal,
}

#[derive(Debug, Clone, Deserialize)]
struct MlsMessageEnvelope {
    version: u8,
    mls_group_id: String,
    epoch: u64,
    message_kind: MlsMessageKind,
}

#[derive(Debug, Clone)]
struct ValidatedRoomEventWrapper {
    wrapper_event_id: String,
    envelope: MlsMessageEnvelope,
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

    pub async fn create_room(
        &self,
        creator_npub: &str,
        request: CreateRoomRequest,
        now: u64,
    ) -> Result<RoomSummary, StoreHandleError> {
        self.update_store(|store| Ok(store.create_room(creator_npub, request, now)))
            .await
    }

    pub async fn upload_key_package(
        &self,
        owner_npub: &str,
        request: UploadKeyPackageRequest,
        now: u64,
    ) -> Result<KeyPackageRecord, StoreHandleError> {
        self.update_store(|store| store.upload_key_package(owner_npub, request, now))
            .await
    }

    pub async fn enqueue_welcome(
        &self,
        sender_npub: &str,
        request: UploadWelcomeRequest,
        now: u64,
    ) -> Result<WelcomeRecord, StoreHandleError> {
        self.update_store(|store| store.enqueue_welcome(sender_npub, request, now))
            .await
    }

    pub async fn claim_welcomes(
        &self,
        now: u64,
        recipient_npub: &str,
    ) -> Result<Vec<WelcomeRecord>, StoreHandleError> {
        self.update_store(|store| Ok(store.claim_welcomes(recipient_npub, now)))
            .await
    }

    pub async fn ack_welcome(
        &self,
        recipient_npub: &str,
        request: AckWelcomeRequest,
        now: u64,
    ) -> Result<(), StoreHandleError> {
        self.update_store(|store| store.ack_welcome(recipient_npub, request, now))
            .await
    }

    pub async fn release_welcome(
        &self,
        recipient_npub: &str,
        request: ReleaseWelcomeRequest,
    ) -> Result<(), StoreHandleError> {
        self.update_store(|store| store.release_welcome(recipient_npub, request))
            .await
    }

    pub async fn claim_key_package(
        &self,
        claimer_npub: &str,
        request: ClaimKeyPackageRequest,
        now: u64,
    ) -> Result<KeyPackageRecord, StoreHandleError> {
        self.update_store(|store| store.claim_key_package(claimer_npub, request, now))
            .await
    }

    pub async fn finalize_key_package(
        &self,
        claimer_npub: &str,
        request: FinalizeKeyPackageRequest,
        now: u64,
    ) -> Result<KeyPackageRecord, StoreHandleError> {
        self.update_store(|store| store.finalize_key_package(claimer_npub, request, now))
            .await
    }

    pub async fn release_key_package(
        &self,
        claimer_npub: &str,
        request: ReleaseKeyPackageRequest,
    ) -> Result<(), StoreHandleError> {
        self.update_store(|store| store.release_key_package(claimer_npub, request))
            .await
    }

    pub async fn append_room_event(
        &self,
        sender_npub: &str,
        room_id: &str,
        request: AppendRoomEventRequest,
        now: u64,
    ) -> Result<RoomEvent, StoreHandleError> {
        self.update_store(|store| store.append_room_event(sender_npub, room_id, request, now))
            .await
    }

    pub async fn submit_membership_commit(
        &self,
        sender_npub: &str,
        room_id: &str,
        request: SubmitMembershipCommitRequest,
        now: u64,
    ) -> Result<(RoomSummary, RoomEvent), StoreHandleError> {
        self.update_store(|store| {
            store.submit_membership_commit(sender_npub, room_id, request, now)
        })
        .await
    }

    pub async fn submit_room_commit(
        &self,
        sender_npub: &str,
        room_id: &str,
        request: SubmitRoomCommitRequest,
        now: u64,
    ) -> Result<(RoomSummary, RoomEvent), StoreHandleError> {
        self.update_store(|store| store.submit_room_commit(sender_npub, room_id, request, now))
            .await
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

    pub async fn sync_room(
        &self,
        member_npub: &str,
        room_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<(RoomSummary, Vec<RoomEvent>), StoreError> {
        self.inner
            .read()
            .await
            .sync_room(member_npub, room_id, after_seq, limit)
    }

    async fn update_store<T, F>(&self, apply: F) -> Result<T, StoreHandleError>
    where
        F: FnOnce(&mut ChatStore) -> Result<T, StoreError>,
    {
        let mut store = self.inner.write().await;
        let mut next = store.clone();
        let result = apply(&mut next)?;
        self.persist_locked(&next)?;
        *store = next;
        Ok(result)
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
    pub fn create_room(
        &mut self,
        creator_npub: &str,
        request: CreateRoomRequest,
        now: u64,
    ) -> RoomSummary {
        let mut members = normalized_member_npubs(request.member_npubs);
        members.insert(creator_npub.to_string());
        let initial_epoch = request.initial_epoch.unwrap_or(0);

        let summary = RoomSummary {
            room_id: new_prefixed_id("room"),
            created_by: creator_npub.to_string(),
            members: members.into_iter().collect(),
            mls_group_id: clean_optional_field(request.mls_group_id)
                .map(|group_id| group_id.to_ascii_lowercase()),
            epoch: initial_epoch,
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
        let payload = request.payload.trim().to_string();
        if payload.is_empty() {
            return Err(StoreError::EmptyKeyPackagePayload);
        }

        let key_package = KeyPackageRecord {
            key_package_id: new_prefixed_id("kp"),
            owner_npub: owner_npub.to_string(),
            ciphersuite: clean_optional_field(request.ciphersuite),
            payload,
            created_at: now,
            lease_token: None,
            leased_at: None,
            lease_until: None,
            leased_by_npub: None,
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
            None,
            now,
        )?;
        self.welcomes_by_recipient
            .entry(welcome.recipient_npub.clone())
            .or_default()
            .push(welcome.clone());
        Ok(welcome)
    }

    pub fn claim_welcomes(&mut self, recipient_npub: &str, now: u64) -> Vec<WelcomeRecord> {
        let lease_until = now.saturating_add(LEASE_TTL_SECS);
        let Some(welcomes) = self.welcomes_by_recipient.get_mut(recipient_npub) else {
            return Vec::new();
        };

        let mut leased = Vec::new();
        for welcome in welcomes.iter_mut() {
            if welcome.acked_at.is_some() || !lease_is_claimable(welcome.lease_until, now) {
                continue;
            }
            welcome.lease_token = Some(new_prefixed_id("wlease"));
            welcome.leased_at = Some(now);
            welcome.lease_until = Some(lease_until);
            welcome.leased_by_npub = Some(recipient_npub.to_string());
            leased.push(welcome.clone());
        }
        leased
    }

    pub fn ack_welcome(
        &mut self,
        recipient_npub: &str,
        request: AckWelcomeRequest,
        now: u64,
    ) -> Result<(), StoreError> {
        let welcomes = self
            .welcomes_by_recipient
            .get_mut(recipient_npub)
            .ok_or(StoreError::WelcomeNotFound)?;
        let index = welcomes
            .iter()
            .position(|welcome| welcome.welcome_id == request.welcome_id)
            .ok_or(StoreError::WelcomeNotFound)?;
        ensure_active_welcome_lease(&welcomes[index], recipient_npub, &request.lease_token, now)?;
        welcomes.remove(index);
        if welcomes.is_empty() {
            self.welcomes_by_recipient.remove(recipient_npub);
        }
        Ok(())
    }

    pub fn release_welcome(
        &mut self,
        recipient_npub: &str,
        request: ReleaseWelcomeRequest,
    ) -> Result<(), StoreError> {
        let welcomes = self
            .welcomes_by_recipient
            .get_mut(recipient_npub)
            .ok_or(StoreError::WelcomeNotFound)?;
        let welcome = welcomes
            .iter_mut()
            .find(|welcome| welcome.welcome_id == request.welcome_id)
            .ok_or(StoreError::WelcomeNotFound)?;
        ensure_owned_lease(
            welcome.leased_by_npub.as_deref(),
            recipient_npub,
            welcome.lease_token.as_deref(),
            &request.lease_token,
        )?;
        clear_welcome_lease(welcome);
        Ok(())
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
        let Some(key_package) = key_packages.iter_mut().find(|record| {
            record.claimed_at.is_none() && lease_is_claimable(record.lease_until, now)
        }) else {
            return Err(StoreError::KeyPackageNotFound);
        };

        key_package.lease_token = Some(new_prefixed_id("kplease"));
        key_package.leased_at = Some(now);
        key_package.lease_until = Some(now.saturating_add(LEASE_TTL_SECS));
        key_package.leased_by_npub = Some(claimer_npub.to_string());
        key_package.claimed_by_room_id = request.room_id;
        Ok(key_package.clone())
    }

    pub fn finalize_key_package(
        &mut self,
        claimer_npub: &str,
        request: FinalizeKeyPackageRequest,
        now: u64,
    ) -> Result<KeyPackageRecord, StoreError> {
        let key_package = self.find_key_package_mut(&request.key_package_id)?;
        ensure_active_key_package_lease(key_package, claimer_npub, &request.lease_token, now)?;
        key_package.claimed_at = Some(now);
        key_package.claimed_by_npub = Some(claimer_npub.to_string());
        clear_key_package_lease(key_package);
        Ok(key_package.clone())
    }

    pub fn release_key_package(
        &mut self,
        claimer_npub: &str,
        request: ReleaseKeyPackageRequest,
    ) -> Result<(), StoreError> {
        let key_package = self.find_key_package_mut(&request.key_package_id)?;
        ensure_owned_lease(
            key_package.leased_by_npub.as_deref(),
            claimer_npub,
            key_package.lease_token.as_deref(),
            &request.lease_token,
        )?;
        clear_key_package_lease(key_package);
        key_package.claimed_by_room_id = None;
        Ok(())
    }

    pub fn append_room_event(
        &mut self,
        sender_npub: &str,
        room_id: &str,
        request: AppendRoomEventRequest,
        now: u64,
    ) -> Result<RoomEvent, StoreError> {
        if request.event_type == RoomEventType::Commit {
            return Err(StoreError::CommitRequiresCommitEndpoint);
        }

        let content = request.content.trim().to_string();
        if content.is_empty() {
            return Err(StoreError::EmptyEventContent);
        }
        let room = self
            .rooms
            .get_mut(room_id)
            .ok_or(StoreError::RoomNotFound)?;
        let validated = match validate_room_event_wrapper(&content, &request.event_type) {
            Ok(validated) => validated,
            Err(err) => {
                ensure_room_member(room, sender_npub)?;
                return Err(err);
            }
        };
        if let Some(existing) =
            existing_event_for_wrapper_id(room, &validated.wrapper_event_id, sender_npub)?
        {
            return Ok(existing);
        }
        ensure_room_member(room, sender_npub)?;
        let envelope = &validated.envelope;
        let expected_epoch = request.expected_epoch.unwrap_or(envelope.epoch);
        if expected_epoch != envelope.epoch {
            return Err(StoreError::InvalidRoomEventEnvelope(
                "request expected_epoch does not match MLS envelope epoch".to_string(),
            ));
        }
        if expected_epoch != room.summary.epoch {
            return Err(StoreError::RoomEpochMismatch {
                expected: expected_epoch,
                actual: room.summary.epoch,
            });
        }
        bind_or_validate_room_mls_group(room, envelope)?;

        let next_seq = room.summary.last_seq.saturating_add(1);
        let event = RoomEvent {
            event_id: new_prefixed_id("evt"),
            wrapper_event_id: Some(validated.wrapper_event_id),
            room_id: room.summary.room_id.clone(),
            seq: next_seq,
            event_type: request.event_type,
            epoch: room.summary.epoch,
            sender_npub: sender_npub.to_string(),
            content,
            created_at: now,
        };
        room.summary.last_seq = next_seq;
        room.events.push(StoredRoomEvent {
            event: event.clone(),
            visible_to_npubs: room.summary.members.clone(),
        });
        Ok(event)
    }

    pub fn submit_membership_commit(
        &mut self,
        sender_npub: &str,
        room_id: &str,
        request: SubmitMembershipCommitRequest,
        now: u64,
    ) -> Result<(RoomSummary, RoomEvent), StoreError> {
        self.submit_room_commit(
            sender_npub,
            room_id,
            SubmitRoomCommitRequest {
                expected_epoch: request.expected_epoch,
                member_npubs: Some(request.member_npubs),
                wrapper_event_json: request.wrapper_event_json,
                welcomes: request.welcomes,
            },
            now,
        )
    }

    pub fn submit_room_commit(
        &mut self,
        sender_npub: &str,
        room_id: &str,
        request: SubmitRoomCommitRequest,
        now: u64,
    ) -> Result<(RoomSummary, RoomEvent), StoreError> {
        let SubmitRoomCommitRequest {
            expected_epoch,
            member_npubs,
            wrapper_event_json,
            welcomes,
        } = request;
        let content = wrapper_event_json.trim().to_string();
        if content.is_empty() {
            return Err(StoreError::EmptyEventContent);
        }
        let validated = validate_room_event_wrapper(&content, &RoomEventType::Commit)?;
        let envelope = &validated.envelope;
        if envelope.epoch != expected_epoch {
            return Err(StoreError::InvalidRoomEventEnvelope(
                "commit expected_epoch does not match MLS envelope epoch".to_string(),
            ));
        }

        let (summary, event, welcomes) = {
            let room = self
                .rooms
                .get_mut(room_id)
                .ok_or(StoreError::RoomNotFound)?;
            if let Some(existing) =
                existing_event_for_wrapper_id(room, &validated.wrapper_event_id, sender_npub)?
            {
                return Ok((room.summary.clone(), existing));
            }
            ensure_room_member(room, sender_npub)?;

            if expected_epoch != room.summary.epoch {
                return Err(StoreError::RoomEpochMismatch {
                    expected: expected_epoch,
                    actual: room.summary.epoch,
                });
            }
            bind_or_validate_room_mls_group(room, envelope)?;

            let next_seq = room.summary.last_seq.saturating_add(1);
            let next_epoch = room.summary.epoch.saturating_add(1);
            let event_visible_to_npubs = room.summary.members.clone();
            let welcomes = welcomes
                .into_iter()
                .map(|welcome| {
                    prepare_welcome_record(
                        sender_npub,
                        welcome.recipient_npub,
                        welcome.wrapper_event_json,
                        welcome.server_url,
                        welcome.room_id,
                        Some(next_seq),
                        now,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let event = RoomEvent {
                event_id: new_prefixed_id("evt"),
                wrapper_event_id: Some(validated.wrapper_event_id),
                room_id: room.summary.room_id.clone(),
                seq: next_seq,
                event_type: RoomEventType::Commit,
                epoch: next_epoch,
                sender_npub: sender_npub.to_string(),
                content,
                created_at: now,
            };

            room.summary.last_seq = next_seq;
            room.summary.epoch = next_epoch;
            if let Some(member_npubs) = member_npubs {
                room.summary.members = normalized_member_npubs(member_npubs).into_iter().collect();
            }
            room.events.push(StoredRoomEvent {
                event: event.clone(),
                visible_to_npubs: event_visible_to_npubs,
            });
            (room.summary.clone(), event, welcomes)
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
        self.sync_room(member_npub, room_id, after_seq, limit)
            .map(|(_, events)| events)
    }

    pub fn sync_room(
        &self,
        member_npub: &str,
        room_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<(RoomSummary, Vec<RoomEvent>), StoreError> {
        let room = self.rooms.get(room_id).ok_or(StoreError::RoomNotFound)?;
        let is_current_member = room
            .summary
            .members
            .iter()
            .any(|member| member == member_npub);
        let limit = limit.clamp(1, MAX_SYNC_LIMIT);
        let events = room
            .events
            .iter()
            .filter(|event| event.event.seq > after_seq)
            .filter(|event| room_event_visible_to(event, member_npub))
            .take(limit)
            .map(|event| event.event.clone())
            .collect::<Vec<_>>();
        if !is_current_member && events.is_empty() {
            return Err(StoreError::NotRoomMember);
        }
        Ok((room.summary.clone(), events))
    }

    fn find_key_package_mut(
        &mut self,
        key_package_id: &str,
    ) -> Result<&mut KeyPackageRecord, StoreError> {
        for records in self.key_packages_by_owner.values_mut() {
            if let Some(record) = records
                .iter_mut()
                .find(|record| record.key_package_id == key_package_id)
            {
                return Ok(record);
            }
        }
        Err(StoreError::KeyPackageNotFound)
    }
}

fn clean_optional_field(value: Option<String>) -> Option<String> {
    value
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

fn lease_is_claimable(lease_until: Option<u64>, now: u64) -> bool {
    match lease_until {
        Some(expires_at) => expires_at <= now,
        None => true,
    }
}

fn ensure_owned_lease(
    leased_by_npub: Option<&str>,
    expected_npub: &str,
    lease_token: Option<&str>,
    expected_token: &str,
) -> Result<(), StoreError> {
    if leased_by_npub != Some(expected_npub) {
        return Err(StoreError::LeaseNotActive);
    }
    if lease_token != Some(expected_token) {
        return Err(StoreError::LeaseTokenMismatch);
    }
    Ok(())
}

fn ensure_active_key_package_lease(
    key_package: &KeyPackageRecord,
    claimer_npub: &str,
    lease_token: &str,
    now: u64,
) -> Result<(), StoreError> {
    ensure_owned_lease(
        key_package.leased_by_npub.as_deref(),
        claimer_npub,
        key_package.lease_token.as_deref(),
        lease_token,
    )?;
    if key_package.claimed_at.is_some() {
        return Err(StoreError::KeyPackageNotFound);
    }
    if key_package
        .lease_until
        .is_none_or(|expires_at| expires_at <= now)
    {
        return Err(StoreError::LeaseNotActive);
    }
    Ok(())
}

fn ensure_active_welcome_lease(
    welcome: &WelcomeRecord,
    recipient_npub: &str,
    lease_token: &str,
    now: u64,
) -> Result<(), StoreError> {
    ensure_owned_lease(
        welcome.leased_by_npub.as_deref(),
        recipient_npub,
        welcome.lease_token.as_deref(),
        lease_token,
    )?;
    if welcome.acked_at.is_some() {
        return Err(StoreError::WelcomeNotFound);
    }
    if welcome
        .lease_until
        .is_none_or(|expires_at| expires_at <= now)
    {
        return Err(StoreError::LeaseNotActive);
    }
    Ok(())
}

fn clear_key_package_lease(key_package: &mut KeyPackageRecord) {
    key_package.lease_token = None;
    key_package.leased_at = None;
    key_package.lease_until = None;
    key_package.leased_by_npub = None;
}

fn clear_welcome_lease(welcome: &mut WelcomeRecord) {
    welcome.lease_token = None;
    welcome.leased_at = None;
    welcome.lease_until = None;
    welcome.leased_by_npub = None;
}

fn bind_or_validate_room_mls_group(
    room: &mut StoredRoom,
    envelope: &MlsMessageEnvelope,
) -> Result<(), StoreError> {
    let mls_group_id = envelope.mls_group_id.trim().to_ascii_lowercase();
    if mls_group_id.is_empty() {
        return Err(StoreError::InvalidRoomEventEnvelope(
            "MLS group id must not be empty".to_string(),
        ));
    }
    match room.summary.mls_group_id.as_deref() {
        Some(existing) if existing != mls_group_id.as_str() => {
            Err(StoreError::InvalidRoomEventEnvelope(format!(
                "MLS group id {mls_group_id} does not match room group id {existing}"
            )))
        }
        Some(_) => Ok(()),
        None => {
            room.summary.mls_group_id = Some(mls_group_id);
            Ok(())
        }
    }
}

fn prepare_welcome_record(
    sender_npub: &str,
    recipient_npub: String,
    wrapper_event_json: String,
    server_url: Option<String>,
    room_id: Option<String>,
    commit_seq: Option<u64>,
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
        commit_seq,
        lease_token: None,
        leased_at: None,
        lease_until: None,
        leased_by_npub: None,
        acked_at: None,
        created_at: now,
    })
}

fn validate_room_event_wrapper(
    wrapper_event_json: &str,
    event_type: &RoomEventType,
) -> Result<ValidatedRoomEventWrapper, StoreError> {
    let wrapper = serde_json::from_str::<Event>(wrapper_event_json)
        .map_err(|err| StoreError::InvalidRoomEventEnvelope(err.to_string()))?;
    wrapper
        .verify()
        .map_err(|err| StoreError::InvalidRoomEventEnvelope(err.to_string()))?;
    if wrapper.kind != Kind::MlsGroupMessage {
        return Err(StoreError::InvalidRoomEventEnvelope(format!(
            "expected kind {}, got {}",
            Kind::MlsGroupMessage.as_u16(),
            wrapper.kind.as_u16()
        )));
    }
    let envelope = serde_json::from_str::<MlsMessageEnvelope>(&wrapper.content)
        .map_err(|err| StoreError::InvalidRoomEventEnvelope(err.to_string()))?;
    if envelope.version != 1 {
        return Err(StoreError::InvalidRoomEventEnvelope(format!(
            "unsupported MLS envelope version {}",
            envelope.version
        )));
    }
    let expected_kind = expected_mls_message_kind(event_type)?;
    if envelope.message_kind != expected_kind {
        return Err(StoreError::RoomEventKindMismatch {
            event_type: event_type.clone(),
            message_kind: envelope.message_kind.as_str().to_string(),
        });
    }
    Ok(ValidatedRoomEventWrapper {
        wrapper_event_id: wrapper.id.to_hex(),
        envelope,
    })
}

fn expected_mls_message_kind(event_type: &RoomEventType) -> Result<MlsMessageKind, StoreError> {
    match event_type {
        RoomEventType::ApplicationMessage => Ok(MlsMessageKind::Application),
        RoomEventType::Commit => Ok(MlsMessageKind::Commit),
        RoomEventType::Proposal => Ok(MlsMessageKind::Proposal),
        RoomEventType::Welcome => Err(StoreError::InvalidRoomEventEnvelope(
            "welcome room events are not accepted; use /v1/welcomes".to_string(),
        )),
    }
}

impl MlsMessageKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Commit => "commit",
            Self::Proposal => "proposal",
        }
    }
}

fn room_event_visible_to(event: &StoredRoomEvent, member_npub: &str) -> bool {
    if event.visible_to_npubs.is_empty() {
        return true;
    }
    event
        .visible_to_npubs
        .iter()
        .any(|member| member == member_npub)
}

fn existing_event_for_wrapper_id(
    room: &StoredRoom,
    wrapper_event_id: &str,
    sender_npub: &str,
) -> Result<Option<RoomEvent>, StoreError> {
    let Some(stored) = room
        .events
        .iter()
        .find(|stored| stored.event.wrapper_event_id.as_deref() == Some(wrapper_event_id))
    else {
        return Ok(None);
    };

    if stored.event.sender_npub == sender_npub {
        return Ok(Some(stored.event.clone()));
    }

    ensure_room_member(room, sender_npub)?;
    Err(StoreError::InvalidRoomEventEnvelope(format!(
        "wrapper event id {wrapper_event_id} already belongs to another room event"
    )))
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
        AppendRoomEventRequest, ClaimKeyPackageRequest, CreateRoomRequest, RoomEventType,
        SubmitMembershipCommitRequest, UploadKeyPackageRequest, WelcomeEnvelope,
    };
    use nostr_sdk::prelude::{EventBuilder, Keys, Kind, ToBech32};

    const TEST_MLS_GROUP_ID: &str =
        "0000000000000000000000000000000000000000000000000000000000000000";
    const OTHER_TEST_MLS_GROUP_ID: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";

    fn npub(keys: &Keys) -> String {
        keys.public_key().to_bech32().unwrap().to_lowercase()
    }

    fn mls_wrapper_json(keys: &Keys, epoch: u64, message_kind: &str) -> String {
        mls_wrapper_json_with_group(keys, TEST_MLS_GROUP_ID, epoch, message_kind)
    }

    fn mls_wrapper_json_with_group(
        keys: &Keys,
        mls_group_id: &str,
        epoch: u64,
        message_kind: &str,
    ) -> String {
        let content = serde_json::json!({
            "version": 1,
            "mls_group_id": mls_group_id,
            "epoch": epoch,
            "message_kind": message_kind,
            "mls_message": format!("opaque-{}", Uuid::new_v4()),
        })
        .to_string();
        let event = EventBuilder::new(Kind::MlsGroupMessage, content)
            .sign_with_keys(keys)
            .expect("sign MLS wrapper");
        serde_json::to_string(&event).expect("serialize MLS wrapper")
    }

    fn tampered_mls_wrapper_json(keys: &Keys, epoch: u64, message_kind: &str) -> String {
        let wrapper_json = mls_wrapper_json(keys, epoch, message_kind);
        let mut wrapper: serde_json::Value =
            serde_json::from_str(&wrapper_json).expect("decode MLS wrapper JSON");
        wrapper["content"] = serde_json::json!(serde_json::json!({
                "version": 1,
                "mls_group_id": TEST_MLS_GROUP_ID,
            "epoch": epoch,
            "message_kind": message_kind,
            "mls_message": "tampered",
        })
        .to_string());
        serde_json::to_string(&wrapper).expect("serialize tampered MLS wrapper")
    }

    #[test]
    fn create_room_deduplicates_members() {
        let mut store = ChatStore::default();
        let room = store.create_room(
            "npub1alice",
            CreateRoomRequest {
                member_npubs: vec!["npub1bob".to_string(), "npub1alice".to_string()],
                mls_group_id: None,
                initial_epoch: None,
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
                mls_group_id: None,
                initial_epoch: None,
            },
            100,
        );
        let err = store
            .append_room_event(
                "npub1mallory",
                &room.room_id,
                AppendRoomEventRequest {
                    event_type: RoomEventType::ApplicationMessage,
                    expected_epoch: None,
                    epoch: 1,
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
        let key_package = store
            .upload_key_package(
                "npub1alice",
                UploadKeyPackageRequest {
                    ciphersuite: Some("mls128".to_string()),
                    payload: "server-test-key-package-payload".to_string(),
                },
                100,
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
        assert_eq!(claimed.leased_by_npub.as_deref(), Some("npub1bob"));
        assert!(claimed.lease_token.is_some());
        assert!(claimed.claimed_at.is_none());
        assert_eq!(claimed.payload, "server-test-key-package-payload");

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
    fn key_package_lease_can_be_released_and_finalized() {
        let mut store = ChatStore::default();
        let uploaded = store
            .upload_key_package(
                "npub1alice",
                UploadKeyPackageRequest {
                    ciphersuite: None,
                    payload: "server-test-key-package-payload".to_string(),
                },
                100,
            )
            .expect("upload key package");

        let claimed = store
            .claim_key_package(
                "npub1bob",
                ClaimKeyPackageRequest {
                    owner_npub: "npub1alice".to_string(),
                    room_id: None,
                },
                101,
            )
            .expect("claim key package");
        let lease_token = claimed.lease_token.clone().expect("lease token");

        store
            .release_key_package(
                "npub1bob",
                ReleaseKeyPackageRequest {
                    key_package_id: uploaded.key_package_id.clone(),
                    lease_token,
                },
            )
            .expect("release key package lease");

        let reclaimed = store
            .claim_key_package(
                "npub1carol",
                ClaimKeyPackageRequest {
                    owner_npub: "npub1alice".to_string(),
                    room_id: None,
                },
                102,
            )
            .expect("reclaim key package");
        let final_lease_token = reclaimed.lease_token.clone().expect("lease token");

        let finalized = store
            .finalize_key_package(
                "npub1carol",
                FinalizeKeyPackageRequest {
                    key_package_id: uploaded.key_package_id,
                    lease_token: final_lease_token,
                },
                103,
            )
            .expect("finalize key package");
        assert_eq!(finalized.claimed_by_npub.as_deref(), Some("npub1carol"));
        assert_eq!(finalized.claimed_by_room_id, None);
        assert!(finalized.claimed_at.is_some());
        assert!(finalized.lease_token.is_none());
    }

    #[test]
    fn welcome_lease_can_be_released_and_acked() {
        let mut store = ChatStore::default();
        let welcome = store
            .enqueue_welcome(
                "npub1alice",
                UploadWelcomeRequest {
                    recipient_npub: "npub1bob".to_string(),
                    wrapper_event_json: "{\"kind\":1059}".to_string(),
                    server_url: Some("https://chat.example".to_string()),
                    room_id: Some("room_123".to_string()),
                },
                100,
            )
            .expect("enqueue welcome");

        let claimed = store.claim_welcomes("npub1bob", 101);
        assert_eq!(claimed.len(), 1);
        let lease_token = claimed[0].lease_token.clone().expect("lease token");
        assert!(store.claim_welcomes("npub1bob", 102).is_empty());

        store
            .release_welcome(
                "npub1bob",
                ReleaseWelcomeRequest {
                    welcome_id: welcome.welcome_id.clone(),
                    lease_token,
                },
            )
            .expect("release welcome lease");

        let reclaimed = store.claim_welcomes("npub1bob", 103);
        assert_eq!(reclaimed.len(), 1);
        let ack_token = reclaimed[0].lease_token.clone().expect("lease token");

        store
            .ack_welcome(
                "npub1bob",
                AckWelcomeRequest {
                    welcome_id: welcome.welcome_id,
                    lease_token: ack_token,
                },
                104,
            )
            .expect("ack welcome");
        assert!(store.claim_welcomes("npub1bob", 105).is_empty());
    }

    #[test]
    fn membership_commit_updates_room_epoch_members_and_welcomes() {
        let mut store = ChatStore::default();
        let alice = Keys::generate();
        let alice_npub = npub(&alice);
        let bob_npub = npub(&Keys::generate());
        let carol_npub = npub(&Keys::generate());
        let room = store.create_room(
            &alice_npub,
            CreateRoomRequest {
                member_npubs: vec![bob_npub.clone()],
                mls_group_id: Some(TEST_MLS_GROUP_ID.to_string()),
                initial_epoch: Some(1),
            },
            101,
        );

        let (summary, event) = store
            .submit_membership_commit(
                &alice_npub,
                &room.room_id,
                SubmitMembershipCommitRequest {
                    expected_epoch: 1,
                    member_npubs: vec![alice_npub.clone(), bob_npub, carol_npub.clone()],
                    wrapper_event_json: mls_wrapper_json(&alice, 1, "commit"),
                    welcomes: vec![WelcomeEnvelope {
                        recipient_npub: carol_npub.clone(),
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
        assert_eq!(event.epoch, 2);
        assert_eq!(summary.epoch, 2);
        assert_eq!(summary.last_seq, 1);
        assert!(summary.members.contains(&carol_npub));

        let claimed = store.claim_welcomes(&carol_npub, 103);
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].sender_npub, alice_npub);
        assert_eq!(
            claimed[0].server_url.as_deref(),
            Some("https://chat.example")
        );
        assert_eq!(claimed[0].room_id.as_deref(), Some(room.room_id.as_str()));

        let synced_summary = store
            .room_summary_for_member(&carol_npub, &room.room_id)
            .expect("new member should see room");
        assert_eq!(synced_summary.epoch, 2);
        assert!(
            store
                .sync_room_events(&carol_npub, &room.room_id, 0, 10)
                .expect("new member can sync the room")
                .is_empty(),
            "new members should not replay the commit already applied by their welcome"
        );
    }

    #[test]
    fn removed_member_can_sync_removal_commit_only() {
        let mut store = ChatStore::default();
        let alice = Keys::generate();
        let alice_npub = npub(&alice);
        let bob_npub = npub(&Keys::generate());
        let room = store.create_room(
            &alice_npub,
            CreateRoomRequest {
                member_npubs: vec![bob_npub.clone()],
                mls_group_id: Some(TEST_MLS_GROUP_ID.to_string()),
                initial_epoch: Some(1),
            },
            101,
        );

        let (summary, event) = store
            .submit_membership_commit(
                &alice_npub,
                &room.room_id,
                SubmitMembershipCommitRequest {
                    expected_epoch: 1,
                    member_npubs: vec![alice_npub.clone()],
                    wrapper_event_json: mls_wrapper_json(&alice, 1, "commit"),
                    welcomes: Vec::new(),
                },
                102,
            )
            .expect("submit removal commit");

        assert_eq!(summary.members, vec![alice_npub]);
        assert_eq!(event.seq, 1);

        let events = store
            .sync_room_events(&bob_npub, &room.room_id, 0, 10)
            .expect("removed member should see removal commit");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, RoomEventType::Commit);

        let err = store
            .sync_room_events(&bob_npub, &room.room_id, 1, 10)
            .expect_err("removed member should not see future room state");
        assert!(matches!(err, StoreError::NotRoomMember));
    }

    #[test]
    fn append_rejects_stale_mls_epoch() {
        let mut store = ChatStore::default();
        let alice = Keys::generate();
        let alice_npub = npub(&alice);
        let room = store.create_room(
            &alice_npub,
            CreateRoomRequest {
                member_npubs: vec![],
                mls_group_id: Some(TEST_MLS_GROUP_ID.to_string()),
                initial_epoch: Some(1),
            },
            100,
        );

        let err = store
            .append_room_event(
                &alice_npub,
                &room.room_id,
                AppendRoomEventRequest {
                    event_type: RoomEventType::ApplicationMessage,
                    expected_epoch: Some(0),
                    epoch: 0,
                    content: mls_wrapper_json(&alice, 0, "application"),
                },
                101,
            )
            .expect_err("stale application event should fail");
        assert!(matches!(
            err,
            StoreError::RoomEpochMismatch {
                expected: 0,
                actual: 1
            }
        ));
    }

    #[test]
    fn append_rejects_invalid_wrapper_signature() {
        let mut store = ChatStore::default();
        let alice = Keys::generate();
        let alice_npub = npub(&alice);
        let room = store.create_room(
            &alice_npub,
            CreateRoomRequest {
                member_npubs: vec![],
                mls_group_id: Some(TEST_MLS_GROUP_ID.to_string()),
                initial_epoch: None,
            },
            100,
        );

        let err = store
            .append_room_event(
                &alice_npub,
                &room.room_id,
                AppendRoomEventRequest {
                    event_type: RoomEventType::ApplicationMessage,
                    expected_epoch: Some(0),
                    epoch: 0,
                    content: tampered_mls_wrapper_json(&alice, 0, "application"),
                },
                101,
            )
            .expect_err("tampered wrapper should fail signature verification");
        assert!(matches!(err, StoreError::InvalidRoomEventEnvelope(_)));
    }

    #[test]
    fn append_binds_legacy_room_to_first_mls_group_id() {
        let mut store = ChatStore::default();
        let alice = Keys::generate();
        let alice_npub = npub(&alice);
        let room = store.create_room(
            &alice_npub,
            CreateRoomRequest {
                member_npubs: vec![],
                mls_group_id: None,
                initial_epoch: None,
            },
            100,
        );

        store
            .append_room_event(
                &alice_npub,
                &room.room_id,
                AppendRoomEventRequest {
                    event_type: RoomEventType::ApplicationMessage,
                    expected_epoch: Some(0),
                    epoch: 0,
                    content: mls_wrapper_json(&alice, 0, "application"),
                },
                101,
            )
            .expect("first room event should bind MLS group id");
        let summary = store
            .room_summary_for_member(&alice_npub, &room.room_id)
            .expect("room summary");
        assert_eq!(summary.mls_group_id.as_deref(), Some(TEST_MLS_GROUP_ID));
    }

    #[test]
    fn append_duplicate_wrapper_returns_original_event() {
        let mut store = ChatStore::default();
        let alice = Keys::generate();
        let alice_npub = npub(&alice);
        let room = store.create_room(
            &alice_npub,
            CreateRoomRequest {
                member_npubs: vec![],
                mls_group_id: Some(TEST_MLS_GROUP_ID.to_string()),
                initial_epoch: None,
            },
            100,
        );
        let wrapper = mls_wrapper_json(&alice, 0, "application");

        let first = store
            .append_room_event(
                &alice_npub,
                &room.room_id,
                AppendRoomEventRequest {
                    event_type: RoomEventType::ApplicationMessage,
                    expected_epoch: Some(0),
                    epoch: 0,
                    content: wrapper.clone(),
                },
                101,
            )
            .expect("append event");
        let duplicate = store
            .append_room_event(
                &alice_npub,
                &room.room_id,
                AppendRoomEventRequest {
                    event_type: RoomEventType::ApplicationMessage,
                    expected_epoch: Some(0),
                    epoch: 0,
                    content: wrapper,
                },
                102,
            )
            .expect("duplicate append should return existing event");

        assert_eq!(duplicate.event_id, first.event_id);
        assert_eq!(duplicate.seq, first.seq);
        assert_eq!(duplicate.wrapper_event_id, first.wrapper_event_id);
        let summary = store
            .room_summary_for_member(&alice_npub, &room.room_id)
            .expect("room summary");
        assert_eq!(summary.last_seq, 1);
    }

    #[test]
    fn append_rejects_wrong_mls_group_id() {
        let mut store = ChatStore::default();
        let alice = Keys::generate();
        let alice_npub = npub(&alice);
        let room = store.create_room(
            &alice_npub,
            CreateRoomRequest {
                member_npubs: vec![],
                mls_group_id: Some(TEST_MLS_GROUP_ID.to_string()),
                initial_epoch: None,
            },
            100,
        );

        let err = store
            .append_room_event(
                &alice_npub,
                &room.room_id,
                AppendRoomEventRequest {
                    event_type: RoomEventType::ApplicationMessage,
                    expected_epoch: Some(0),
                    epoch: 0,
                    content: mls_wrapper_json_with_group(
                        &alice,
                        OTHER_TEST_MLS_GROUP_ID,
                        0,
                        "application",
                    ),
                },
                101,
            )
            .expect_err("wrong MLS group id should fail");
        assert!(matches!(err, StoreError::InvalidRoomEventEnvelope(_)));
    }

    #[test]
    fn membership_commit_duplicate_wrapper_returns_original_event_without_duplicate_welcomes() {
        let mut store = ChatStore::default();
        let alice = Keys::generate();
        let alice_npub = npub(&alice);
        let bob_npub = npub(&Keys::generate());
        let carol_npub = npub(&Keys::generate());
        let room = store.create_room(
            &alice_npub,
            CreateRoomRequest {
                member_npubs: vec![bob_npub.clone()],
                mls_group_id: Some(TEST_MLS_GROUP_ID.to_string()),
                initial_epoch: Some(1),
            },
            101,
        );
        let wrapper = mls_wrapper_json(&alice, 1, "commit");

        let (_, first) = store
            .submit_membership_commit(
                &alice_npub,
                &room.room_id,
                SubmitMembershipCommitRequest {
                    expected_epoch: 1,
                    member_npubs: vec![alice_npub.clone(), bob_npub.clone(), carol_npub.clone()],
                    wrapper_event_json: wrapper.clone(),
                    welcomes: vec![WelcomeEnvelope {
                        recipient_npub: carol_npub.clone(),
                        wrapper_event_json: "{\"kind\":1059,\"content\":\"welcome\"}".to_string(),
                        server_url: Some("https://chat.example".to_string()),
                        room_id: Some(room.room_id.clone()),
                    }],
                },
                102,
            )
            .expect("submit commit");
        let (summary, duplicate) = store
            .submit_membership_commit(
                &alice_npub,
                &room.room_id,
                SubmitMembershipCommitRequest {
                    expected_epoch: 1,
                    member_npubs: vec![alice_npub.clone(), bob_npub, carol_npub.clone()],
                    wrapper_event_json: wrapper,
                    welcomes: vec![WelcomeEnvelope {
                        recipient_npub: carol_npub.clone(),
                        wrapper_event_json: "{\"kind\":1059,\"content\":\"welcome\"}".to_string(),
                        server_url: Some("https://chat.example".to_string()),
                        room_id: Some(room.room_id.clone()),
                    }],
                },
                103,
            )
            .expect("duplicate commit should return existing event");

        assert_eq!(duplicate.event_id, first.event_id);
        assert_eq!(duplicate.seq, first.seq);
        assert_eq!(summary.epoch, 2);
        assert_eq!(summary.last_seq, 1);
        assert_eq!(store.claim_welcomes(&carol_npub, 203).len(), 1);
    }

    #[test]
    fn membership_commit_epoch_mismatch_is_atomic() {
        let mut store = ChatStore::default();
        let alice = Keys::generate();
        let alice_npub = npub(&alice);
        let bob_npub = npub(&Keys::generate());
        let carol_npub = npub(&Keys::generate());
        let room = store.create_room(
            &alice_npub,
            CreateRoomRequest {
                member_npubs: vec![bob_npub.clone()],
                mls_group_id: Some(TEST_MLS_GROUP_ID.to_string()),
                initial_epoch: Some(1),
            },
            101,
        );

        let err = store
            .submit_membership_commit(
                &alice_npub,
                &room.room_id,
                SubmitMembershipCommitRequest {
                    expected_epoch: 0,
                    member_npubs: vec![alice_npub.clone(), carol_npub.clone()],
                    wrapper_event_json: mls_wrapper_json(&alice, 0, "commit"),
                    welcomes: vec![WelcomeEnvelope {
                        recipient_npub: carol_npub.clone(),
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
                expected: 0,
                actual: 1
            }
        ));

        let summary = store
            .room_summary_for_member(&alice_npub, &room.room_id)
            .expect("room should still exist");
        assert_eq!(summary.epoch, 1);
        assert_eq!(summary.last_seq, 0);
        assert_eq!(
            summary.members.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([alice_npub.clone(), bob_npub])
        );
        assert!(store.claim_welcomes(&carol_npub, 203).is_empty());
        assert!(store
            .sync_room_events(&alice_npub, &room.room_id, 0, 10)
            .expect("sync events")
            .is_empty());
    }

    #[test]
    fn append_rejects_commit_events() {
        let mut store = ChatStore::default();
        let room = store.create_room(
            "npub1alice",
            CreateRoomRequest {
                member_npubs: vec![],
                mls_group_id: None,
                initial_epoch: None,
            },
            100,
        );
        let err = store
            .append_room_event(
                "npub1alice",
                &room.room_id,
                AppendRoomEventRequest {
                    event_type: RoomEventType::Commit,
                    expected_epoch: None,
                    epoch: 0,
                    content: "commit-wrapper".to_string(),
                },
                101,
            )
            .expect_err("commit appends should use the commit endpoint");
        assert!(matches!(err, StoreError::CommitRequiresCommitEndpoint));
    }

    #[test]
    fn persistent_store_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("chat-store.json");
        let handle = StoreHandle::load_or_create(path.clone()).expect("create persistent store");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let alice = Keys::generate();
        let alice_npub = npub(&alice);
        let bob_npub = npub(&Keys::generate());
        let room_id = rt.block_on(async {
            let room = handle
                .create_room(
                    &alice_npub,
                    CreateRoomRequest {
                        member_npubs: vec![bob_npub],
                        mls_group_id: Some(TEST_MLS_GROUP_ID.to_string()),
                        initial_epoch: None,
                    },
                    100,
                )
                .await
                .expect("create room");
            let _ = handle
                .append_room_event(
                    &alice_npub,
                    &room.room_id,
                    AppendRoomEventRequest {
                        event_type: RoomEventType::ApplicationMessage,
                        expected_epoch: Some(0),
                        epoch: 0,
                        content: mls_wrapper_json(&alice, 0, "application"),
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
                .room_summary_for_member(&alice_npub, &room_id)
                .await
                .expect("room should reload");
            assert_eq!(room.last_seq, 1);

            let events = reloaded
                .sync_room_events(&alice_npub, &room_id, 0, 10)
                .await
                .expect("events should reload");
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event_type, RoomEventType::ApplicationMessage);
        });
    }

    #[test]
    fn persistent_store_write_failure_does_not_mutate_live_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let not_a_dir = dir.path().join("not-a-dir");
        std::fs::write(&not_a_dir, b"file").expect("write parent file");
        let handle = StoreHandle {
            inner: Arc::new(RwLock::new(ChatStore::default())),
            state_path: Some(not_a_dir.join("chat-store.json")),
        };
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let err = rt
            .block_on(async {
                handle
                    .create_room(
                        "npub1alice",
                        CreateRoomRequest {
                            member_npubs: Vec::new(),
                            mls_group_id: None,
                            initial_epoch: None,
                        },
                        100,
                    )
                    .await
            })
            .expect_err("persist failure should fail create_room");
        assert!(matches!(err, StoreHandleError::Persist(_)));
        rt.block_on(async {
            assert!(handle.inner.read().await.rooms.is_empty());
        });
    }
}
