use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use mdk_core::prelude::{GroupId, MessageProcessingResult};
use mdk_storage_traits::{
    groups::{Pagination, types::Group},
    messages::types::Message,
};
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
pub struct RuntimeJoinedGroupSnapshot {
    pub nostr_group_id_hex: String,
    pub mls_group_id: GroupId,
    pub mls_group_id_hex: String,
    pub name: String,
    pub description: String,
    pub admin_pubkeys: BTreeSet<PublicKey>,
    pub member_pubkeys: BTreeSet<PublicKey>,
    pub last_message_at: Option<Timestamp>,
}

impl RuntimeJoinedGroupSnapshot {
    pub fn member_count(&self) -> u32 {
        self.member_pubkeys.len() as u32
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

    pub fn list_joined_group_snapshots(&self) -> Result<Vec<RuntimeJoinedGroupSnapshot>> {
        let groups = self.mdk.get_groups().context("get_groups")?;
        Ok(groups
            .into_iter()
            .map(|group| {
                let member_pubkeys = self
                    .mdk
                    .get_members(&group.mls_group_id)
                    .unwrap_or_default();
                let mls_group_id = group.mls_group_id.clone();
                RuntimeJoinedGroupSnapshot {
                    nostr_group_id_hex: hex::encode(group.nostr_group_id),
                    mls_group_id_hex: hex::encode(group.mls_group_id.as_slice()),
                    name: group.name,
                    description: group.description,
                    admin_pubkeys: group.admin_pubkeys,
                    member_pubkeys,
                    last_message_at: group.last_message_at,
                    mls_group_id,
                }
            })
            .collect())
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
                .admin_pubkeys
                .contains(&inviter_keys.public_key())
        );
        assert!(
            snapshots[0]
                .member_pubkeys
                .contains(&inviter_keys.public_key())
        );
        assert!(
            snapshots[0]
                .member_pubkeys
                .contains(&invitee_keys.public_key())
        );
        assert_eq!(snapshots[0].member_count(), 2);
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
}
