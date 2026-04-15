use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use nostr_sdk::prelude::{
    Alphabet, Client, Event, EventId, Filter, Kind, RelayUrl, SingleLetterTag,
};
use pika_mls::conversation::{process_group_message_event, ConversationQueries};
pub(crate) use pika_mls::conversation::{JoinedGroupSnapshot, MessagePage, MessagePageQuery};
use pika_mls::prelude::{GroupId, MessageProcessingResult};
use pika_mls::storage_traits::messages::types::Message;

use super::AppMessageKind;
use crate::mls_support::PikaMls;

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

pub(crate) fn lookup_joined_group_snapshot(
    mls: &PikaMls,
    nostr_group_id_hex: &str,
) -> Result<JoinedGroupSnapshot> {
    ConversationQueries::new(mls).lookup_joined_group_snapshot(nostr_group_id_hex)
}

pub(crate) fn list_joined_group_snapshots(mls: &PikaMls) -> Result<Vec<JoinedGroupSnapshot>> {
    ConversationQueries::new(mls).list_joined_group_snapshots()
}

pub(crate) fn load_message_page(
    mls: &PikaMls,
    nostr_group_id_hex: &str,
    query: MessagePageQuery,
) -> Result<MessagePage> {
    ConversationQueries::new(mls).load_message_page(nostr_group_id_hex, query)
}

pub(crate) fn process_event(mls: &PikaMls, event: &Event) -> Result<Option<ConversationEvent>> {
    let Some(result) = process_group_message_event(mls, event)? else {
        return Ok(None);
    };
    Ok(interpret_processing_result(mls, result))
}

pub(crate) fn interpret_processing_result(
    mls: &PikaMls,
    result: MessageProcessingResult,
) -> Option<ConversationEvent> {
    match result {
        MessageProcessingResult::ApplicationMessage(message) => {
            let classification = super::message_support::classify_message(
                message.kind,
                &message.content,
                message.tags.iter(),
            )?;
            let nostr_group_id_hex = ConversationQueries::new(mls)
                .nostr_group_id_hex(&message.mls_group_id)
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
            mls,
            update.mls_group_id.clone(),
            RuntimeGroupUpdateKind::Proposal,
        ),
        MessageProcessingResult::PendingProposal { mls_group_id } => {
            group_update(mls, mls_group_id, RuntimeGroupUpdateKind::PendingProposal)
        }
        MessageProcessingResult::IgnoredProposal { mls_group_id, .. } => {
            group_update(mls, mls_group_id, RuntimeGroupUpdateKind::IgnoredProposal)
        }
        MessageProcessingResult::ExternalJoinProposal { mls_group_id } => group_update(
            mls,
            mls_group_id,
            RuntimeGroupUpdateKind::ExternalJoinProposal,
        ),
        MessageProcessingResult::Commit { mls_group_id } => {
            group_update(mls, mls_group_id, RuntimeGroupUpdateKind::Commit)
        }
        MessageProcessingResult::Unprocessable { mls_group_id } => {
            group_update(mls, mls_group_id, RuntimeGroupUpdateKind::Unprocessable)
        }
        MessageProcessingResult::PreviouslyFailed => Some(ConversationEvent::PreviouslyFailed),
    }
}

pub(crate) async fn ingest_backlog_messages(
    mls: &PikaMls,
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
        if let Some(ConversationEvent::Application(message)) = process_event(mls, event)? {
            messages.push(message.message);
        }
    }
    Ok(messages)
}

fn nostr_group_id_hex_for_mls_group_id(
    mls: &PikaMls,
    mls_group_id: &GroupId,
) -> Result<Option<String>> {
    ConversationQueries::new(mls).nostr_group_id_hex(mls_group_id)
}

fn group_update(
    mls: &PikaMls,
    mls_group_id: GroupId,
    kind: RuntimeGroupUpdateKind,
) -> Option<ConversationEvent> {
    let Some(nostr_group_id_hex) = nostr_group_id_hex_for_mls_group_id(mls, &mls_group_id)
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
