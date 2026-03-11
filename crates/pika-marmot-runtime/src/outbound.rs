use anyhow::Result;
use hypernote_protocol as hn;
use mdk_storage_traits::GroupId;
use mdk_storage_traits::groups::types::Group;
use nostr_sdk::prelude::{
    Client, Event, EventId, Kind, PublicKey, RelayUrl, Tag, TagKind, Timestamp, UnsignedEvent,
};

use crate::PikaMdk;
use crate::conversation::{ConversationRuntime, RuntimeJoinedGroupSnapshot};
use crate::relay::publish_and_confirm;

#[derive(Debug, Clone)]
pub struct ResolvedConversationTarget {
    pub mls_group_id: GroupId,
    pub nostr_group_id_hex: String,
}

impl ResolvedConversationTarget {
    pub fn from_group(group: Group) -> Self {
        Self {
            mls_group_id: group.mls_group_id,
            nostr_group_id_hex: hex::encode(group.nostr_group_id),
        }
    }

    pub fn from_joined_group_snapshot(snapshot: RuntimeJoinedGroupSnapshot) -> Self {
        Self {
            mls_group_id: snapshot.mls_group_id,
            nostr_group_id_hex: snapshot.nostr_group_id_hex,
        }
    }
}

#[derive(Debug, Clone)]
pub enum OutboundConversationAction {
    Message {
        kind: Kind,
        content: String,
        tags: Vec<Tag>,
        created_at: Timestamp,
    },
    Hypernote {
        content: String,
        title: Option<String>,
        state: Option<String>,
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
pub struct PreparedConversationAction {
    pub target: ResolvedConversationTarget,
    pub kind: Kind,
    pub rumor_id: EventId,
    pub wrapper: Event,
}

#[derive(Debug, Clone)]
pub struct PublishedConversationAction {
    pub target: ResolvedConversationTarget,
    pub kind: Kind,
    pub rumor_id: EventId,
    pub wrapper_event_id: EventId,
}

#[derive(Debug, Clone)]
pub enum OutboundConversationPublishStatus {
    Published { wrapper_event_id: EventId },
    PublishFailed(String),
}

pub struct OutboundConversationRuntime<'a> {
    mdk: &'a PikaMdk,
}

impl<'a> OutboundConversationRuntime<'a> {
    pub fn new(mdk: &'a PikaMdk) -> Self {
        Self { mdk }
    }

    pub fn resolve_target(&self, nostr_group_id_hex: &str) -> Result<ResolvedConversationTarget> {
        Ok(ResolvedConversationTarget::from_joined_group_snapshot(
            ConversationRuntime::new(self.mdk).lookup_joined_group_snapshot(nostr_group_id_hex)?,
        ))
    }

    pub fn prepare_action(
        &self,
        sender: PublicKey,
        nostr_group_id_hex: &str,
        action: OutboundConversationAction,
    ) -> Result<PreparedConversationAction> {
        let target = self.resolve_target(nostr_group_id_hex)?;
        self.prepare_action_for_target(sender, target, action)
    }

    pub fn prepare_action_for_group(
        &self,
        sender: PublicKey,
        group: Group,
        action: OutboundConversationAction,
    ) -> Result<PreparedConversationAction> {
        self.prepare_action_for_target(
            sender,
            ResolvedConversationTarget::from_group(group),
            action,
        )
    }

    pub fn prepare_action_for_group_ids(
        &self,
        sender: PublicKey,
        mls_group_id: GroupId,
        nostr_group_id_hex: String,
        action: OutboundConversationAction,
    ) -> Result<PreparedConversationAction> {
        self.prepare_action_for_target(
            sender,
            ResolvedConversationTarget {
                mls_group_id,
                nostr_group_id_hex,
            },
            action,
        )
    }

    pub fn prepare_action_for_target(
        &self,
        sender: PublicKey,
        target: ResolvedConversationTarget,
        action: OutboundConversationAction,
    ) -> Result<PreparedConversationAction> {
        let (kind, mut rumor) = build_unsigned_action(sender, action);
        rumor.ensure_id();
        let rumor_id = rumor.id();
        let wrapper = self.mdk.create_message(&target.mls_group_id, rumor)?;

        Ok(PreparedConversationAction {
            target,
            kind,
            rumor_id,
            wrapper,
        })
    }

    pub async fn publish_prepared_with_confirm(
        &self,
        client: &Client,
        relay_urls: &[RelayUrl],
        prepared: &PreparedConversationAction,
        label: &str,
    ) -> Result<PublishedConversationAction> {
        publish_and_confirm(client, relay_urls, &prepared.wrapper, label).await?;
        Ok(PublishedConversationAction {
            target: prepared.target.clone(),
            kind: prepared.kind,
            rumor_id: prepared.rumor_id,
            wrapper_event_id: prepared.wrapper.id,
        })
    }
}

fn build_unsigned_action(
    sender: PublicKey,
    action: OutboundConversationAction,
) -> (Kind, UnsignedEvent) {
    match action {
        OutboundConversationAction::Message {
            kind,
            content,
            tags,
            created_at,
        } => (
            kind,
            UnsignedEvent::new(sender, created_at, kind, tags, content),
        ),
        OutboundConversationAction::Hypernote {
            content,
            title,
            state,
            created_at,
        } => {
            let mut tags = Vec::new();
            if let Some(title) = title {
                tags.push(Tag::custom(TagKind::custom("title"), vec![title]));
            }
            if let Some(state) = state {
                tags.push(Tag::custom(TagKind::custom("state"), vec![state]));
            }
            (
                Kind::Custom(hn::HYPERNOTE_KIND),
                UnsignedEvent::new(
                    sender,
                    created_at,
                    Kind::Custom(hn::HYPERNOTE_KIND),
                    tags,
                    content,
                ),
            )
        }
        OutboundConversationAction::Reaction {
            target_event_id,
            emoji,
            created_at,
        } => (
            Kind::Reaction,
            UnsignedEvent::new(
                sender,
                created_at,
                Kind::Reaction,
                [Tag::event(target_event_id)],
                emoji,
            ),
        ),
        OutboundConversationAction::Typing {
            created_at,
            expires_at,
        } => (
            crate::message::TYPING_INDICATOR_KIND,
            UnsignedEvent::new(
                sender,
                created_at,
                crate::message::TYPING_INDICATOR_KIND,
                [
                    Tag::custom(TagKind::d(), ["pika"]),
                    Tag::expiration(expires_at),
                ],
                "typing",
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use mdk_core::prelude::NostrGroupConfigData;
    use nostr_sdk::prelude::{EventBuilder, Keys, RelayUrl};

    fn open_test_mdk(dir: &tempfile::TempDir) -> PikaMdk {
        crate::open_mdk(dir.path()).expect("open test mdk")
    }

    fn make_key_package_event(mdk: &PikaMdk, keys: &Keys) -> Event {
        let relay = RelayUrl::parse("wss://test.relay").expect("relay url");
        let (content, tags, _hash_ref) = mdk
            .create_key_package_for_event(&keys.public_key(), vec![relay])
            .expect("create key package");
        EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign key package")
    }

    fn create_test_group() -> (tempfile::TempDir, tempfile::TempDir, PikaMdk, Keys, Group) {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir);
        let invitee_mdk = open_test_mdk(&invitee_dir);

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Outbound runtime test".to_string(),
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

        (
            inviter_dir,
            invitee_dir,
            inviter_mdk,
            inviter_keys,
            created.group,
        )
    }

    #[test]
    fn prepare_message_action_resolves_target_and_wraps_message() {
        let (_inviter_dir, _invitee_dir, mdk, keys, group) = create_test_group();
        let runtime = OutboundConversationRuntime::new(&mdk);

        let prepared = runtime
            .prepare_action(
                keys.public_key(),
                &hex::encode(group.nostr_group_id),
                OutboundConversationAction::Message {
                    kind: Kind::ChatMessage,
                    content: "hello shared outbound".to_string(),
                    tags: vec![],
                    created_at: Timestamp::from(123_u64),
                },
            )
            .expect("prepare action");

        assert_eq!(prepared.kind, Kind::ChatMessage);
        assert_eq!(
            prepared.target.nostr_group_id_hex,
            hex::encode(group.nostr_group_id)
        );
        assert_eq!(prepared.wrapper.kind, Kind::MlsGroupMessage);
        assert_ne!(prepared.rumor_id, EventId::all_zeros());
    }

    #[test]
    fn prepare_hypernote_reaction_and_typing_actions_use_shared_kinds() {
        let (_inviter_dir, _invitee_dir, mdk, keys, group) = create_test_group();
        let runtime = OutboundConversationRuntime::new(&mdk);
        let target = ResolvedConversationTarget::from_group(group);

        let hypernote = runtime
            .prepare_action_for_target(
                keys.public_key(),
                target.clone(),
                OutboundConversationAction::Hypernote {
                    content: "# Shared".to_string(),
                    title: Some("Title".to_string()),
                    state: Some("{\"ready\":true}".to_string()),
                    created_at: Timestamp::from(123_u64),
                },
            )
            .expect("prepare hypernote");
        assert_eq!(hypernote.kind, Kind::Custom(hn::HYPERNOTE_KIND));

        let reaction = runtime
            .prepare_action_for_target(
                keys.public_key(),
                target.clone(),
                OutboundConversationAction::Reaction {
                    target_event_id: EventId::all_zeros(),
                    emoji: "👍".to_string(),
                    created_at: Timestamp::from(124_u64),
                },
            )
            .expect("prepare reaction");
        assert_eq!(reaction.kind, Kind::Reaction);

        let typing = runtime
            .prepare_action_for_target(
                keys.public_key(),
                target,
                OutboundConversationAction::Typing {
                    created_at: Timestamp::from(125_u64),
                    expires_at: Timestamp::from(135_u64),
                },
            )
            .expect("prepare typing");
        assert_eq!(typing.kind, crate::message::TYPING_INDICATOR_KIND);
    }
}
