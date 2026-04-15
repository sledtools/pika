use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use nostr_sdk::prelude::{
    Alphabet, Client, Event, EventId, Filter, Kind, RelayUrl, SingleLetterTag,
};
use pika_mls::conversation::ConversationQueries;
pub(crate) use pika_mls::conversation::{JoinedGroupSnapshot, MessagePage, MessagePageQuery};
use pika_mls::prelude::{GroupId, MessageProcessingResult};
use pika_mls::storage_traits::messages::types::Message;

use super::AppMessageKind;
use crate::mdk_support::PikaMdk;

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
    mdk: &PikaMdk,
    nostr_group_id_hex: &str,
) -> Result<JoinedGroupSnapshot> {
    ConversationQueries::new(mdk).lookup_joined_group_snapshot(nostr_group_id_hex)
}

pub(crate) fn list_joined_group_snapshots(mdk: &PikaMdk) -> Result<Vec<JoinedGroupSnapshot>> {
    ConversationQueries::new(mdk).list_joined_group_snapshots()
}

pub(crate) fn load_message_page(
    mdk: &PikaMdk,
    nostr_group_id_hex: &str,
    query: MessagePageQuery,
) -> Result<MessagePage> {
    ConversationQueries::new(mdk).load_message_page(nostr_group_id_hex, query)
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
            let nostr_group_id_hex = ConversationQueries::new(mdk)
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

fn nostr_group_id_hex_for_mls_group_id(
    mdk: &PikaMdk,
    mls_group_id: &GroupId,
) -> Result<Option<String>> {
    ConversationQueries::new(mdk).nostr_group_id_hex(mls_group_id)
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
