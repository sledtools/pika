use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use mdk_core::prelude::{GroupId, MessageProcessingResult};
use mdk_storage_traits::{
    groups::{Pagination, types::Group},
    messages::types::Message,
};
use nostr_sdk::Metadata;
use nostr_sdk::prelude::{
    Alphabet, Client, Event, EventId, Filter, Kind, PublicKey, RelayUrl, SingleLetterTag, Timestamp,
};

use crate::PikaMdk;
use crate::message::{MessageClassification, classify_message};

#[derive(Debug, Clone)]
pub struct RuntimeApplicationMessage {
    pub mls_group_id: GroupId,
    pub nostr_group_id_hex: String,
    pub classification: MessageClassification,
    pub message: Message,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RuntimeGroupUpdateKind {
    Proposal,
    PendingProposal,
    IgnoredProposal,
    ExternalJoinProposal,
    Commit,
    Unprocessable,
}

#[derive(Debug, Clone)]
pub struct RuntimeGroupUpdate {
    pub mls_group_id: GroupId,
    pub nostr_group_id_hex: String,
    pub kind: RuntimeGroupUpdateKind,
}

#[derive(Debug, Clone)]
pub enum ConversationEvent {
    Application(Box<RuntimeApplicationMessage>),
    GroupUpdate(RuntimeGroupUpdate),
    UnresolvedGroup { mls_group_id: GroupId },
    PreviouslyFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeJoinedGroupMemberSnapshot {
    pub pubkey: PublicKey,
    pub is_admin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeJoinedGroupSnapshot {
    pub nostr_group_id_hex: String,
    pub mls_group_id: GroupId,
    pub mls_group_id_hex: String,
    pub name: String,
    pub description: String,
    pub relay_urls: Vec<RelayUrl>,
    pub member_snapshots: Vec<RuntimeJoinedGroupMemberSnapshot>,
    pub last_message_at: Option<Timestamp>,
}

impl RuntimeJoinedGroupSnapshot {
    pub fn member_count(&self) -> u32 {
        self.member_snapshots.len() as u32
    }

    pub fn other_member_snapshots(
        &self,
        local_pubkey: &PublicKey,
    ) -> Vec<RuntimeJoinedGroupMemberSnapshot> {
        self.member_snapshots
            .iter()
            .filter(|member| member.pubkey != *local_pubkey)
            .cloned()
            .collect()
    }

    pub fn is_admin(&self, pubkey: &PublicKey) -> bool {
        self.member_snapshots
            .iter()
            .any(|member| member.pubkey == *pubkey && member.is_admin)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeGroupSummary {
    pub nostr_group_id_hex: String,
    pub mls_group_id_hex: String,
    pub name: String,
    pub description: String,
    pub member_count: u32,
}

#[derive(Debug, Clone)]
pub struct RuntimeGroupProfileSnapshot {
    pub nostr_group_id_hex: String,
    pub owner_pubkey: PublicKey,
    pub metadata_json: String,
    pub metadata: Metadata,
    pub created_at: Timestamp,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct RuntimeMessagePageQuery {
    pub limit: usize,
    pub offset: usize,
}

impl RuntimeMessagePageQuery {
    pub const fn new(limit: usize, offset: usize) -> Self {
        Self { limit, offset }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeMessagePage {
    pub nostr_group_id_hex: String,
    pub mls_group_id: GroupId,
    pub messages: Vec<Message>,
    pub fetched_count: usize,
    pub next_offset: usize,
    pub storage_exhausted: bool,
}

pub struct ConversationRuntime<'a> {
    mdk: &'a PikaMdk,
}

impl<'a> ConversationRuntime<'a> {
    pub fn new(mdk: &'a PikaMdk) -> Self {
        Self { mdk }
    }

    pub fn process_event(&self, event: &Event) -> Result<Option<ConversationEvent>> {
        if event.kind != Kind::MlsGroupMessage {
            return Ok(None);
        }
        let result = self
            .mdk
            .process_message(event)
            .context("process group message")?;
        Ok(self.interpret_processing_result(result))
    }

    pub fn interpret_processing_result(
        &self,
        result: MessageProcessingResult,
    ) -> Option<ConversationEvent> {
        match result {
            MessageProcessingResult::ApplicationMessage(message) => {
                let classification =
                    classify_message(message.kind, &message.content, message.tags.iter())?;
                let nostr_group_id_hex = self
                    .nostr_group_id_hex(&message.mls_group_id)
                    .ok()
                    .flatten()?;
                Some(ConversationEvent::Application(Box::new(
                    RuntimeApplicationMessage {
                        mls_group_id: message.mls_group_id.clone(),
                        nostr_group_id_hex,
                        classification,
                        message,
                    },
                )))
            }
            MessageProcessingResult::Proposal(update) => self.group_update(
                update.mls_group_id.clone(),
                RuntimeGroupUpdateKind::Proposal,
            ),
            MessageProcessingResult::PendingProposal { mls_group_id } => {
                self.group_update(mls_group_id, RuntimeGroupUpdateKind::PendingProposal)
            }
            MessageProcessingResult::IgnoredProposal { mls_group_id, .. } => {
                self.group_update(mls_group_id, RuntimeGroupUpdateKind::IgnoredProposal)
            }
            MessageProcessingResult::ExternalJoinProposal { mls_group_id } => {
                self.group_update(mls_group_id, RuntimeGroupUpdateKind::ExternalJoinProposal)
            }
            MessageProcessingResult::Commit { mls_group_id } => {
                self.group_update(mls_group_id, RuntimeGroupUpdateKind::Commit)
            }
            MessageProcessingResult::Unprocessable { mls_group_id } => {
                self.group_update(mls_group_id, RuntimeGroupUpdateKind::Unprocessable)
            }
            MessageProcessingResult::PreviouslyFailed => Some(ConversationEvent::PreviouslyFailed),
        }
    }

    pub fn find_group(&self, nostr_group_id_hex: &str) -> Result<Group> {
        let group_id_bytes =
            hex::decode(nostr_group_id_hex).map_err(|_| anyhow!("nostr_group_id must be hex"))?;
        if group_id_bytes.len() != 32 {
            anyhow::bail!("nostr_group_id must be 32 bytes hex");
        }
        self.mdk
            .get_groups()
            .context("get_groups")?
            .into_iter()
            .find(|group| group.nostr_group_id.as_slice() == group_id_bytes.as_slice())
            .ok_or_else(|| anyhow!("group not found"))
    }

    pub fn mls_group_id_for_nostr_group_id(&self, nostr_group_id_hex: &str) -> Result<GroupId> {
        Ok(self.find_group(nostr_group_id_hex)?.mls_group_id)
    }

    pub fn lookup_joined_group_snapshot(
        &self,
        nostr_group_id_hex: &str,
    ) -> Result<RuntimeJoinedGroupSnapshot> {
        self.joined_group_snapshot(self.find_group(nostr_group_id_hex)?)
    }

    pub fn list_joined_group_snapshots(&self) -> Result<Vec<RuntimeJoinedGroupSnapshot>> {
        let groups = self.mdk.get_groups().context("get_groups")?;
        groups
            .into_iter()
            .map(|group| self.joined_group_snapshot(group))
            .collect()
    }

    pub fn list_groups(&self) -> Result<Vec<RuntimeGroupSummary>> {
        Ok(self
            .list_joined_group_snapshots()?
            .into_iter()
            .map(|group| {
                let member_count = group.member_count();
                RuntimeGroupSummary {
                    nostr_group_id_hex: group.nostr_group_id_hex,
                    mls_group_id_hex: group.mls_group_id_hex,
                    name: group.name,
                    description: group.description,
                    member_count,
                }
            })
            .collect())
    }

    pub fn get_messages(
        &self,
        nostr_group_id_hex: &str,
        pagination: Option<Pagination>,
    ) -> Result<Vec<Message>> {
        let mls_group_id = self.mls_group_id_for_nostr_group_id(nostr_group_id_hex)?;
        self.mdk
            .get_messages(&mls_group_id, pagination)
            .context("get messages")
    }

    pub fn load_message_page(
        &self,
        nostr_group_id_hex: &str,
        query: RuntimeMessagePageQuery,
    ) -> Result<RuntimeMessagePage> {
        let mls_group_id = self.mls_group_id_for_nostr_group_id(nostr_group_id_hex)?;
        let messages = self
            .mdk
            .get_messages(
                &mls_group_id,
                Some(Pagination::new(Some(query.limit), Some(query.offset))),
            )
            .context("get message page")?;
        let fetched_count = messages.len();
        Ok(RuntimeMessagePage {
            nostr_group_id_hex: nostr_group_id_hex.to_string(),
            mls_group_id,
            messages,
            fetched_count,
            next_offset: query.offset + fetched_count,
            storage_exhausted: fetched_count < query.limit,
        })
    }

    pub fn lookup_group_profile_snapshot(
        &self,
        nostr_group_id_hex: &str,
        owner_pubkey: &PublicKey,
    ) -> Result<Option<RuntimeGroupProfileSnapshot>> {
        let messages = self.get_messages(nostr_group_id_hex, None)?;
        let mut latest: Option<RuntimeGroupProfileSnapshot> = None;

        for message in messages {
            if message.kind != Kind::Metadata {
                continue;
            }

            let profile_owner = message
                .tags
                .iter()
                .find(|tag| tag.kind() == nostr_sdk::TagKind::p())
                .and_then(|tag| tag.content())
                .and_then(|content| PublicKey::parse(content).ok())
                .unwrap_or(message.pubkey);
            if profile_owner != *owner_pubkey {
                continue;
            }

            let Ok(metadata) = serde_json::from_str::<Metadata>(&message.content) else {
                continue;
            };
            let candidate = RuntimeGroupProfileSnapshot {
                nostr_group_id_hex: nostr_group_id_hex.to_string(),
                owner_pubkey: profile_owner,
                metadata_json: message.content.clone(),
                metadata,
                created_at: message.created_at,
            };

            latest = Some(candidate);
        }

        Ok(latest)
    }

    pub async fn ingest_backlog_messages(
        &self,
        client: &Client,
        relay_urls: &[RelayUrl],
        nostr_group_id_hex: &str,
        seen: &mut HashSet<EventId>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let filter = Filter::new()
            .kind(Kind::MlsGroupMessage)
            .custom_tag(SingleLetterTag::lowercase(Alphabet::H), nostr_group_id_hex)
            .limit(limit);

        let events = client
            .fetch_events_from(relay_urls.to_vec(), filter, Duration::from_secs(10))
            .await
            .context("fetch group backlog")?;

        let mut messages = Vec::new();
        for event in events.iter() {
            if !seen.insert(event.id) {
                continue;
            }
            if let Some(ConversationEvent::Application(message)) = self.process_event(event)? {
                messages.push(message.message);
            }
        }
        Ok(messages)
    }

    fn group_update(
        &self,
        mls_group_id: GroupId,
        kind: RuntimeGroupUpdateKind,
    ) -> Option<ConversationEvent> {
        let Some(nostr_group_id_hex) = self.nostr_group_id_hex(&mls_group_id).ok().flatten() else {
            return Some(ConversationEvent::UnresolvedGroup { mls_group_id });
        };
        Some(ConversationEvent::GroupUpdate(RuntimeGroupUpdate {
            mls_group_id,
            nostr_group_id_hex,
            kind,
        }))
    }

    fn nostr_group_id_hex(&self, mls_group_id: &GroupId) -> Result<Option<String>> {
        Ok(self
            .mdk
            .get_group(mls_group_id)?
            .map(|group| hex::encode(group.nostr_group_id)))
    }

    fn joined_group_snapshot(&self, group: Group) -> Result<RuntimeJoinedGroupSnapshot> {
        let admin_pubkeys = group.admin_pubkeys.clone();
        let mls_group_id = group.mls_group_id.clone();
        let member_snapshots = self.mdk.get_members(&mls_group_id).unwrap_or_default();
        let relay_urls = self
            .mdk
            .get_relays(&mls_group_id)
            .unwrap_or_default()
            .into_iter()
            .collect();
        let member_snapshots = member_snapshots
            .into_iter()
            .map(|pubkey| RuntimeJoinedGroupMemberSnapshot {
                is_admin: admin_pubkeys.contains(&pubkey),
                pubkey,
            })
            .collect();
        Ok(RuntimeJoinedGroupSnapshot {
            nostr_group_id_hex: hex::encode(group.nostr_group_id),
            mls_group_id_hex: hex::encode(group.mls_group_id.as_slice()),
            name: group.name,
            description: group.description,
            relay_urls,
            member_snapshots,
            last_message_at: group.last_message_at,
            mls_group_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use mdk_core::prelude::NostrGroupConfigData;
    use nostr_sdk::prelude::{Keys, RelayUrl, TagKind, Tags, Timestamp};

    fn open_test_mdk(dir: &tempfile::TempDir) -> PikaMdk {
        crate::open_mdk(dir.path()).expect("open test mdk")
    }

    fn make_key_package_event(mdk: &PikaMdk, keys: &Keys) -> Event {
        let relay = RelayUrl::parse("wss://test.relay").expect("relay url");
        let (content, tags, _hash_ref) = mdk
            .create_key_package_for_event(&keys.public_key(), vec![relay])
            .expect("create key package");
        nostr_sdk::prelude::EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign key package")
    }

    fn make_test_message(
        pubkey: &nostr_sdk::PublicKey,
        kind: Kind,
        content: &str,
        group_id: &GroupId,
        tags: Tags,
    ) -> Message {
        let created_at = Timestamp::from(123_u64);
        Message {
            id: EventId::all_zeros(),
            mls_group_id: group_id.clone(),
            pubkey: *pubkey,
            kind,
            created_at,
            processed_at: created_at,
            content: content.to_string(),
            tags: tags.clone(),
            event: nostr_sdk::prelude::UnsignedEvent::new(
                *pubkey,
                created_at,
                kind,
                tags,
                content.to_string(),
            ),
            wrapper_event_id: EventId::all_zeros(),
            epoch: None,
            state: mdk_storage_traits::messages::types::MessageState::Processed,
        }
    }

    fn store_group_message(
        mdk: &PikaMdk,
        keys: &Keys,
        mls_group_id: &GroupId,
        kind: Kind,
        content: &str,
    ) -> Event {
        let rumor = nostr_sdk::prelude::EventBuilder::new(kind, content).build(keys.public_key());
        let event = mdk
            .create_message(mls_group_id, rumor)
            .expect("create group message");
        mdk.process_message(&event).expect("process group message");
        event
    }

    #[test]
    fn interpret_processing_result_classifies_and_resolves_group() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir);
        let invitee_mdk = open_test_mdk(&invitee_dir);
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "conversation runtime test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let runtime = ConversationRuntime::new(&inviter_mdk);
        let tags: Tags = vec![nostr_sdk::prelude::Tag::custom(TagKind::d(), ["pika"])]
            .into_iter()
            .collect();
        let message = make_test_message(
            &invitee_keys.public_key(),
            Kind::ChatMessage,
            "hello",
            &created.group.mls_group_id,
            tags,
        );

        let interpreted = runtime
            .interpret_processing_result(MessageProcessingResult::ApplicationMessage(message))
            .expect("application message");

        match interpreted {
            ConversationEvent::Application(message) => {
                assert_eq!(message.classification, MessageClassification::Chat);
                assert_eq!(
                    message.nostr_group_id_hex,
                    hex::encode(created.group.nostr_group_id)
                );
            }
            other => panic!("unexpected conversation event: {other:?}"),
        }
    }

    #[test]
    fn list_joined_group_snapshots_surface_current_group_membership_state() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir);
        let invitee_mdk = open_test_mdk(&invitee_dir);
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "list groups test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let runtime = ConversationRuntime::new(&inviter_mdk);

        let snapshots = runtime
            .list_joined_group_snapshots()
            .expect("list group snapshots");
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].nostr_group_id_hex,
            hex::encode(created.group.nostr_group_id)
        );
        assert_eq!(snapshots[0].mls_group_id, created.group.mls_group_id);
        assert_eq!(snapshots[0].name, "list groups test");
        assert!(
            snapshots[0]
                .member_snapshots
                .iter()
                .any(|member| member.pubkey == inviter_keys.public_key() && member.is_admin)
        );
        assert!(
            snapshots[0]
                .member_snapshots
                .iter()
                .any(|member| member.pubkey == inviter_keys.public_key())
        );
        let expected_inviter_admin = created
            .group
            .admin_pubkeys
            .contains(&inviter_keys.public_key());
        let expected_invitee_admin = created
            .group
            .admin_pubkeys
            .contains(&invitee_keys.public_key());
        assert!(
            snapshots[0]
                .member_snapshots
                .iter()
                .any(|member| member.pubkey == invitee_keys.public_key())
        );
        assert_eq!(
            snapshots[0].is_admin(&inviter_keys.public_key()),
            expected_inviter_admin
        );
        assert_eq!(
            snapshots[0].is_admin(&invitee_keys.public_key()),
            expected_invitee_admin
        );
        assert_eq!(
            snapshots[0].other_member_snapshots(&inviter_keys.public_key())[0].pubkey,
            invitee_keys.public_key()
        );
        assert_eq!(snapshots[0].member_count(), 2);
        assert_eq!(
            snapshots[0].relay_urls,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")]
        );
        assert_eq!(snapshots[0].last_message_at, None);
    }

    #[test]
    fn lookup_joined_group_snapshot_surfaces_current_workflow_context() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir);
        let invitee_mdk = open_test_mdk(&invitee_dir);
        let relay_a = RelayUrl::parse("wss://relay-a.test").expect("relay a");
        let relay_b = RelayUrl::parse("wss://relay-b.test").expect("relay b");
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "lookup group context".to_string(),
            "shared workflow context".to_string(),
            None,
            None,
            None,
            vec![relay_a.clone(), relay_b.clone()],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let runtime = ConversationRuntime::new(&inviter_mdk);
        let nostr_group_id_hex = hex::encode(created.group.nostr_group_id);

        let snapshot = runtime
            .lookup_joined_group_snapshot(&nostr_group_id_hex)
            .expect("lookup joined group snapshot");

        assert_eq!(snapshot.nostr_group_id_hex, nostr_group_id_hex);
        assert_eq!(snapshot.mls_group_id, created.group.mls_group_id);
        assert_eq!(snapshot.relay_urls, vec![relay_a, relay_b]);
        assert!(snapshot.is_admin(&inviter_keys.public_key()));
        assert_eq!(
            snapshot
                .other_member_snapshots(&inviter_keys.public_key())
                .len(),
            1
        );
    }

    #[test]
    fn load_message_page_surfaces_shared_pagination_metadata() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir);
        let invitee_mdk = open_test_mdk(&invitee_dir);
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "message page test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge pending commit");
        let nostr_group_id_hex = hex::encode(created.group.nostr_group_id);
        store_group_message(
            &inviter_mdk,
            &inviter_keys,
            &created.group.mls_group_id,
            Kind::ChatMessage,
            "first",
        );
        store_group_message(
            &inviter_mdk,
            &inviter_keys,
            &created.group.mls_group_id,
            Kind::ChatMessage,
            "second",
        );
        let runtime = ConversationRuntime::new(&inviter_mdk);

        let first_page = runtime
            .load_message_page(&nostr_group_id_hex, RuntimeMessagePageQuery::new(1, 0))
            .expect("load first page");
        let second_page = runtime
            .load_message_page(&nostr_group_id_hex, RuntimeMessagePageQuery::new(1, 1))
            .expect("load second page");
        let empty_page = runtime
            .load_message_page(&nostr_group_id_hex, RuntimeMessagePageQuery::new(1, 2))
            .expect("load empty page");

        assert_eq!(first_page.nostr_group_id_hex, nostr_group_id_hex);
        assert_eq!(first_page.mls_group_id, created.group.mls_group_id);
        assert_eq!(first_page.fetched_count, 1);
        assert_eq!(first_page.next_offset, 1);
        assert!(!first_page.storage_exhausted);
        assert_eq!(first_page.messages.len(), 1);
        assert_eq!(second_page.fetched_count, 1);
        assert_eq!(second_page.next_offset, 2);
        assert!(!second_page.storage_exhausted);
        assert_eq!(empty_page.fetched_count, 0);
        assert_eq!(empty_page.next_offset, 2);
        assert!(empty_page.storage_exhausted);
    }

    #[test]
    fn list_groups_and_get_messages_use_shared_lookup_rules() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir);
        let invitee_mdk = open_test_mdk(&invitee_dir);
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "list groups test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let runtime = ConversationRuntime::new(&inviter_mdk);

        let groups = runtime.list_groups().expect("list groups");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "list groups test");
        assert_eq!(groups[0].member_count, 2);
        assert_eq!(
            runtime
                .find_group(&hex::encode(created.group.nostr_group_id))
                .expect("find group")
                .mls_group_id,
            created.group.mls_group_id
        );
        assert!(
            runtime
                .get_messages(
                    &hex::encode(created.group.nostr_group_id),
                    Some(Pagination::new(Some(20), None))
                )
                .expect("get messages")
                .is_empty()
        );
    }

    #[test]
    fn lookup_group_profile_snapshot_returns_latest_owner_profile() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir);
        let invitee_mdk = open_test_mdk(&invitee_dir);
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "group profile snapshot".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge pending commit");
        let nostr_group_id_hex = hex::encode(created.group.nostr_group_id);

        store_group_message(
            &inviter_mdk,
            &inviter_keys,
            &created.group.mls_group_id,
            Kind::Metadata,
            r#"{"display_name":"First","picture":"https://example.com/first.jpg"}"#,
        );
        store_group_message(
            &inviter_mdk,
            &inviter_keys,
            &created.group.mls_group_id,
            Kind::Metadata,
            r#"{"display_name":"Second","about":"Latest","picture":"https://example.com/second.jpg"}"#,
        );

        let snapshot = ConversationRuntime::new(&inviter_mdk)
            .lookup_group_profile_snapshot(&nostr_group_id_hex, &inviter_keys.public_key())
            .expect("lookup group profile snapshot")
            .expect("group profile snapshot");

        assert_eq!(snapshot.nostr_group_id_hex, nostr_group_id_hex);
        assert_eq!(snapshot.owner_pubkey, inviter_keys.public_key());
        assert_eq!(snapshot.metadata.display_name.as_deref(), Some("Second"));
        assert_eq!(snapshot.metadata.about.as_deref(), Some("Latest"));
        assert_eq!(
            snapshot.metadata.picture.as_deref(),
            Some("https://example.com/second.jpg")
        );
    }
}
