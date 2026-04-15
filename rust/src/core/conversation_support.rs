use std::collections::HashSet;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use mdk_core::prelude::{GroupId, MessageProcessingResult};
use mdk_storage_traits::{
    groups::{types::Group, Pagination},
    messages::types::Message,
};
use nostr_sdk::prelude::{
    Alphabet, Client, Event, EventId, Filter, Kind, PublicKey, RelayUrl, SingleLetterTag, Timestamp,
};

use super::AppMessageKind;
use crate::mdk_support::PikaMdk;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JoinedGroupMemberSnapshot {
    pub pubkey: PublicKey,
    pub is_admin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JoinedGroupSnapshot {
    pub nostr_group_id_hex: String,
    pub mls_group_id: GroupId,
    pub mls_group_id_hex: String,
    pub name: String,
    pub description: String,
    pub relay_urls: Vec<RelayUrl>,
    pub member_snapshots: Vec<JoinedGroupMemberSnapshot>,
    pub last_message_at: Option<Timestamp>,
}

impl JoinedGroupSnapshot {
    pub fn member_count(&self) -> u32 {
        self.member_snapshots.len() as u32
    }

    pub fn other_member_snapshots(
        &self,
        local_pubkey: &PublicKey,
    ) -> Vec<JoinedGroupMemberSnapshot> {
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

#[derive(Debug, Clone)]
pub(crate) struct RuntimeApplicationMessage {
    pub nostr_group_id_hex: String,
    pub classification: AppMessageKind,
    pub message: Message,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeGroupUpdateKind {
    Proposal,
    PendingProposal,
    IgnoredProposal,
    ExternalJoinProposal,
    Commit,
    Unprocessable,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeGroupUpdate {
    pub mls_group_id: GroupId,
    pub nostr_group_id_hex: String,
    pub kind: RuntimeGroupUpdateKind,
}

#[derive(Debug, Clone)]
pub(crate) enum ConversationEvent {
    Application(Box<RuntimeApplicationMessage>),
    GroupUpdate(RuntimeGroupUpdate),
    UnresolvedGroup { mls_group_id: GroupId },
    PreviouslyFailed,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) struct MessagePageQuery {
    pub limit: usize,
    pub offset: usize,
}

impl MessagePageQuery {
    pub const fn new(limit: usize, offset: usize) -> Self {
        Self { limit, offset }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MessagePage {
    pub messages: Vec<Message>,
    pub fetched_count: usize,
    pub next_offset: usize,
    pub storage_exhausted: bool,
}

pub(crate) fn lookup_joined_group_snapshot(
    mdk: &PikaMdk,
    nostr_group_id_hex: &str,
) -> Result<JoinedGroupSnapshot> {
    joined_group_snapshot(mdk, find_group(mdk, nostr_group_id_hex)?)
}

pub(crate) fn list_joined_group_snapshots(mdk: &PikaMdk) -> Result<Vec<JoinedGroupSnapshot>> {
    let groups = mdk.get_groups().context("get_groups")?;
    groups
        .into_iter()
        .map(|group| joined_group_snapshot(mdk, group))
        .collect()
}

pub(crate) fn load_message_page(
    mdk: &PikaMdk,
    nostr_group_id_hex: &str,
    query: MessagePageQuery,
) -> Result<MessagePage> {
    let mls_group_id = mls_group_id_for_nostr_group_id(mdk, nostr_group_id_hex)?;
    let messages = mdk
        .get_messages(
            &mls_group_id,
            Some(Pagination::new(Some(query.limit), Some(query.offset))),
        )
        .context("get message page")?;
    let fetched_count = messages.len();
    Ok(MessagePage {
        messages,
        fetched_count,
        next_offset: query.offset + fetched_count,
        storage_exhausted: fetched_count < query.limit,
    })
}

pub(crate) fn process_event(mdk: &PikaMdk, event: &Event) -> Result<Option<ConversationEvent>> {
    if event.kind != Kind::MlsGroupMessage {
        return Ok(None);
    }
    let result = mdk
        .process_message(event)
        .context("process group message")?;
    Ok(interpret_processing_result(mdk, result))
}

pub(crate) fn interpret_processing_result(
    mdk: &PikaMdk,
    result: MessageProcessingResult,
) -> Option<ConversationEvent> {
    match result {
        MessageProcessingResult::ApplicationMessage(message) => {
            let classification = super::message_support::classify_message(
                message.kind,
                &message.content,
                message.tags.iter(),
            )?;
            let nostr_group_id_hex =
                nostr_group_id_hex_for_mls_group_id(mdk, &message.mls_group_id)
                    .ok()
                    .flatten()?;
            Some(ConversationEvent::Application(Box::new(
                RuntimeApplicationMessage {
                    nostr_group_id_hex,
                    classification,
                    message,
                },
            )))
        }
        MessageProcessingResult::Proposal(update) => group_update(
            mdk,
            update.mls_group_id.clone(),
            RuntimeGroupUpdateKind::Proposal,
        ),
        MessageProcessingResult::PendingProposal { mls_group_id } => {
            group_update(mdk, mls_group_id, RuntimeGroupUpdateKind::PendingProposal)
        }
        MessageProcessingResult::IgnoredProposal { mls_group_id, .. } => {
            group_update(mdk, mls_group_id, RuntimeGroupUpdateKind::IgnoredProposal)
        }
        MessageProcessingResult::ExternalJoinProposal { mls_group_id } => group_update(
            mdk,
            mls_group_id,
            RuntimeGroupUpdateKind::ExternalJoinProposal,
        ),
        MessageProcessingResult::Commit { mls_group_id } => {
            group_update(mdk, mls_group_id, RuntimeGroupUpdateKind::Commit)
        }
        MessageProcessingResult::Unprocessable { mls_group_id } => {
            group_update(mdk, mls_group_id, RuntimeGroupUpdateKind::Unprocessable)
        }
        MessageProcessingResult::PreviouslyFailed => Some(ConversationEvent::PreviouslyFailed),
    }
}

pub(crate) async fn ingest_backlog_messages(
    mdk: &PikaMdk,
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
        if let Some(ConversationEvent::Application(message)) = process_event(mdk, event)? {
            messages.push(message.message);
        }
    }
    Ok(messages)
}

fn find_group(mdk: &PikaMdk, nostr_group_id_hex: &str) -> Result<Group> {
    let group_id_bytes =
        hex::decode(nostr_group_id_hex).map_err(|_| anyhow!("nostr_group_id must be hex"))?;
    if group_id_bytes.len() != 32 {
        anyhow::bail!("nostr_group_id must be 32 bytes hex");
    }
    mdk.get_groups()
        .context("get_groups")?
        .into_iter()
        .find(|group| group.nostr_group_id.as_slice() == group_id_bytes.as_slice())
        .ok_or_else(|| anyhow!("group not found"))
}

fn mls_group_id_for_nostr_group_id(mdk: &PikaMdk, nostr_group_id_hex: &str) -> Result<GroupId> {
    Ok(find_group(mdk, nostr_group_id_hex)?.mls_group_id)
}

fn nostr_group_id_hex_for_mls_group_id(
    mdk: &PikaMdk,
    mls_group_id: &GroupId,
) -> Result<Option<String>> {
    Ok(mdk
        .get_group(mls_group_id)?
        .map(|group| hex::encode(group.nostr_group_id)))
}

fn group_update(
    mdk: &PikaMdk,
    mls_group_id: GroupId,
    kind: RuntimeGroupUpdateKind,
) -> Option<ConversationEvent> {
    let Some(nostr_group_id_hex) = nostr_group_id_hex_for_mls_group_id(mdk, &mls_group_id)
        .ok()
        .flatten()
    else {
        return Some(ConversationEvent::UnresolvedGroup { mls_group_id });
    };
    Some(ConversationEvent::GroupUpdate(RuntimeGroupUpdate {
        mls_group_id,
        nostr_group_id_hex,
        kind,
    }))
}

fn joined_group_snapshot(mdk: &PikaMdk, group: Group) -> Result<JoinedGroupSnapshot> {
    let admin_pubkeys = group.admin_pubkeys.clone();
    let mls_group_id = group.mls_group_id.clone();
    let member_snapshots = mdk.get_members(&mls_group_id).unwrap_or_default();
    let relay_urls = mdk
        .get_relays(&mls_group_id)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let member_snapshots = member_snapshots
        .into_iter()
        .map(|pubkey| JoinedGroupMemberSnapshot {
            is_admin: admin_pubkeys.contains(&pubkey),
            pubkey,
        })
        .collect();
    Ok(JoinedGroupSnapshot {
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
