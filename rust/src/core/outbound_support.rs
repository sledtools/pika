use anyhow::Result;
use nostr_sdk::prelude::{Event, EventId, Kind, PublicKey, Tag, TagKind, Timestamp, UnsignedEvent};
use pika_mls::conversation::wrap_rumor;
use pika_mls::storage_traits::GroupId;

use crate::mls_support::PikaMls;

use super::message_support::TYPING_INDICATOR_KIND;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedConversationTarget {
    pub mls_group_id: GroupId,
    pub nostr_group_id_hex: String,
}

impl ResolvedConversationTarget {
    fn from_joined_group_snapshot(
        snapshot: super::conversation_support::JoinedGroupSnapshot,
    ) -> Self {
        Self {
            mls_group_id: snapshot.mls_group_id,
            nostr_group_id_hex: snapshot.nostr_group_id_hex,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum OutboundConversationAction {
    Message {
        kind: Kind,
        content: String,
        tags: Vec<Tag>,
        created_at: Timestamp,
    },
    Reaction {
        target_event_id: EventId,
        emoji: String,
        created_at: Timestamp,
    },
    Typing {
        created_at: Timestamp,
        expires_at: Timestamp,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedConversationAction {
    pub target: ResolvedConversationTarget,
    pub rumor_id: EventId,
    pub wrapper: Event,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) enum OutboundConversationPublishStatus {
    Published,
    PublishFailed(String),
}

pub(crate) fn prepare_action(
    mls: &PikaMls,
    sender: PublicKey,
    nostr_group_id_hex: &str,
    action: OutboundConversationAction,
) -> Result<PreparedConversationAction> {
    let target = resolve_target(mls, nostr_group_id_hex)?;
    prepare_action_for_target(mls, sender, target, action)
}

pub(crate) fn prepare_action_for_group_ids(
    mls: &PikaMls,
    sender: PublicKey,
    mls_group_id: GroupId,
    nostr_group_id_hex: String,
    action: OutboundConversationAction,
) -> Result<PreparedConversationAction> {
    prepare_action_for_target(
        mls,
        sender,
        ResolvedConversationTarget {
            mls_group_id,
            nostr_group_id_hex,
        },
        action,
    )
}

fn resolve_target(mls: &PikaMls, nostr_group_id_hex: &str) -> Result<ResolvedConversationTarget> {
    Ok(ResolvedConversationTarget::from_joined_group_snapshot(
        super::conversation_support::lookup_joined_group_snapshot(mls, nostr_group_id_hex)?,
    ))
}

fn prepare_action_for_target(
    mls: &PikaMls,
    sender: PublicKey,
    target: ResolvedConversationTarget,
    action: OutboundConversationAction,
) -> Result<PreparedConversationAction> {
    let wrapped = wrap_rumor(
        mls,
        &target.mls_group_id,
        build_unsigned_action(sender, action),
    )?;

    Ok(PreparedConversationAction {
        target,
        rumor_id: wrapped.rumor_id,
        wrapper: wrapped.wrapper,
    })
}

fn build_unsigned_action(sender: PublicKey, action: OutboundConversationAction) -> UnsignedEvent {
    match action {
        OutboundConversationAction::Message {
            kind,
            content,
            tags,
            created_at,
        } => UnsignedEvent::new(sender, created_at, kind, tags, content),
        OutboundConversationAction::Reaction {
            target_event_id,
            emoji,
            created_at,
        } => UnsignedEvent::new(
            sender,
            created_at,
            Kind::Reaction,
            [Tag::event(target_event_id)],
            emoji,
        ),
        OutboundConversationAction::Typing {
            created_at,
            expires_at,
        } => UnsignedEvent::new(
            sender,
            created_at,
            TYPING_INDICATOR_KIND,
            [
                Tag::custom(TagKind::d(), ["pika"]),
                Tag::expiration(expires_at),
            ],
            "typing",
        ),
    }
}
