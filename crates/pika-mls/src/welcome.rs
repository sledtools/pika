use std::future::Future;

use anyhow::{Context, Result};
use nostr::{
    Event, EventBuilder, EventId, Keys, Kind, NostrSigner, PublicKey, RelayUrl, Tag, Timestamp,
    UnsignedEvent,
};

use crate::PikaMdk;
use crate::prelude::NostrGroupConfigData;
use crate::storage_traits::groups::types::Group;
use crate::storage_traits::welcomes::types::Welcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWelcomeSnapshot {
    pub wrapper_event_id: EventId,
    pub welcome_event_id: EventId,
    pub welcomer: PublicKey,
    pub created_at: Timestamp,
    pub nostr_group_id_hex: String,
    pub mls_group_id: crate::storage_traits::GroupId,
    pub group_name: String,
    pub group_description: String,
    pub member_count: u32,
    pub group_relays: Vec<RelayUrl>,
}

impl PendingWelcomeSnapshot {
    fn from_welcome(welcome: &Welcome) -> Self {
        Self {
            wrapper_event_id: welcome.wrapper_event_id,
            welcome_event_id: welcome.id,
            welcomer: welcome.welcomer,
            created_at: welcome.event.created_at,
            nostr_group_id_hex: hex::encode(welcome.nostr_group_id),
            mls_group_id: welcome.mls_group_id.clone(),
            group_name: welcome.group_name.clone(),
            group_description: welcome.group_description.clone(),
            member_count: welcome.member_count,
            group_relays: welcome.group_relays.iter().cloned().collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IngestedWelcome {
    pub wrapper_event_id: EventId,
    pub welcome_event_id: EventId,
    pub sender: PublicKey,
    pub sender_hex: String,
    pub nostr_group_id_hex: String,
    pub group_name: String,
}

#[derive(Debug, Clone)]
pub struct PublishedWelcome {
    pub receiver: PublicKey,
    pub wrapper_event_id: EventId,
    pub welcome_event_id: EventId,
    pub rumor: UnsignedEvent,
}

#[derive(Debug, Clone)]
pub struct GroupWelcomeDeliveryPlan {
    pub recipients: Vec<PublicKey>,
    pub welcome_rumors: Vec<UnsignedEvent>,
}

#[derive(Debug, Clone)]
pub struct PlannedGroupCreation {
    pub group: Group,
    pub welcome_delivery: Option<GroupWelcomeDeliveryPlan>,
}

#[derive(Debug, Clone)]
pub struct CreatedGroup {
    pub group: Group,
    pub published_welcomes: Vec<PublishedWelcome>,
}

fn pending_welcome_matches_event_id(welcome: &Welcome, target: &EventId) -> bool {
    welcome.wrapper_event_id == *target || welcome.id == *target
}

pub fn find_pending_welcome<'a>(welcomes: &'a [Welcome], target: &EventId) -> Option<&'a Welcome> {
    welcomes
        .iter()
        .find(|welcome| pending_welcome_matches_event_id(welcome, target))
}

pub fn find_pending_welcome_index(welcomes: &[Welcome], target: &EventId) -> Option<usize> {
    welcomes
        .iter()
        .position(|welcome| pending_welcome_matches_event_id(welcome, target))
}

pub fn take_pending_welcome(welcomes: &mut Vec<Welcome>, target: &EventId) -> Option<Welcome> {
    find_pending_welcome_index(welcomes, target).map(|idx| welcomes.swap_remove(idx))
}

pub fn ingest_unwrapped_welcome<F>(
    mdk: &PikaMdk,
    wrapper_event_id: &EventId,
    sender: PublicKey,
    rumor: &UnsignedEvent,
    sender_allowed: F,
) -> Result<Option<IngestedWelcome>>
where
    F: Fn(&str) -> bool,
{
    if rumor.kind != Kind::MlsWelcome {
        return Ok(None);
    }

    let sender_hex = sender.to_hex().to_lowercase();
    if !sender_allowed(&sender_hex) {
        return Ok(None);
    }

    mdk.process_welcome(wrapper_event_id, rumor)
        .context("process welcome rumor")?;

    let pending = mdk
        .get_pending_welcomes(None)
        .context("get pending welcomes")?;
    let stored = pending
        .into_iter()
        .find(|welcome| welcome.wrapper_event_id == *wrapper_event_id);
    let (nostr_group_id_hex, group_name) = match stored {
        Some(welcome) => (hex::encode(welcome.nostr_group_id), welcome.group_name),
        None => (String::new(), String::new()),
    };

    Ok(Some(IngestedWelcome {
        wrapper_event_id: *wrapper_event_id,
        welcome_event_id: rumor.clone().id(),
        sender,
        sender_hex,
        nostr_group_id_hex,
        group_name,
    }))
}

pub async fn ingest_welcome_from_giftwrap<F>(
    mdk: &PikaMdk,
    keys: &Keys,
    event: &Event,
    sender_allowed: F,
) -> Result<Option<IngestedWelcome>>
where
    F: Fn(&str) -> bool,
{
    if event.kind != Kind::GiftWrap {
        return Ok(None);
    }

    let unwrapped = nostr::nips::nip59::extract_rumor(keys, event)
        .await
        .context("unwrap giftwrap rumor")?;
    ingest_unwrapped_welcome(
        mdk,
        &event.id,
        unwrapped.sender,
        &unwrapped.rumor,
        sender_allowed,
    )
}

pub async fn publish_welcome_rumors<T, F, Fut>(
    signer: &T,
    welcome_rumors: &[UnsignedEvent],
    recipients: &[PublicKey],
    welcome_tags: Vec<Tag>,
    mut publish_giftwrap: F,
) -> Result<Vec<PublishedWelcome>>
where
    T: NostrSigner,
    F: FnMut(PublicKey, Event) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    if recipients.len() != welcome_rumors.len() {
        anyhow::bail!(
            "recipient/welcome mismatch: {} recipients for {} welcome rumors",
            recipients.len(),
            welcome_rumors.len()
        );
    }

    let mut published_welcomes = Vec::new();
    for (receiver, mut rumor) in recipients
        .iter()
        .copied()
        .zip(welcome_rumors.iter().cloned())
    {
        let welcome_event_id = rumor.id();
        let giftwrap =
            EventBuilder::gift_wrap(signer, &receiver, rumor.clone(), welcome_tags.clone())
                .await
                .context("build welcome giftwrap")?;
        publish_giftwrap(receiver, giftwrap.clone())
            .await
            .with_context(|| format!("publish welcome to {}", receiver.to_hex()))?;
        published_welcomes.push(PublishedWelcome {
            receiver,
            wrapper_event_id: giftwrap.id,
            welcome_event_id,
            rumor,
        });
    }

    Ok(published_welcomes)
}

pub fn create_group_and_plan_welcome_delivery(
    creator_pubkey: &PublicKey,
    mdk: &PikaMdk,
    peer_key_packages: Vec<Event>,
    config: NostrGroupConfigData,
    recipients: &[PublicKey],
) -> Result<PlannedGroupCreation> {
    if recipients.len() != peer_key_packages.len() {
        anyhow::bail!(
            "recipient/keypackage mismatch: {} recipients for {} key packages",
            recipients.len(),
            peer_key_packages.len()
        );
    }

    let result = mdk
        .create_group(creator_pubkey, peer_key_packages, config)
        .context("create group")?;

    let welcome_delivery = if recipients.is_empty() || result.welcome_rumors.is_empty() {
        None
    } else {
        Some(GroupWelcomeDeliveryPlan {
            recipients: recipients.to_vec(),
            welcome_rumors: result.welcome_rumors,
        })
    };

    Ok(PlannedGroupCreation {
        group: result.group,
        welcome_delivery,
    })
}

pub async fn create_group_and_publish_welcomes<F, Fut>(
    keys: &Keys,
    mdk: &PikaMdk,
    peer_key_packages: Vec<Event>,
    config: NostrGroupConfigData,
    recipients: &[PublicKey],
    welcome_tags: Vec<Tag>,
    publish_giftwrap: F,
) -> Result<CreatedGroup>
where
    F: FnMut(PublicKey, Event) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let creator_pubkey = keys.public_key();
    let planned = create_group_and_plan_welcome_delivery(
        &creator_pubkey,
        mdk,
        peer_key_packages,
        config,
        recipients,
    )?;
    let published_welcomes = match planned.welcome_delivery.as_ref() {
        Some(plan) => {
            publish_welcome_rumors(
                keys,
                &plan.welcome_rumors,
                &plan.recipients,
                welcome_tags,
                publish_giftwrap,
            )
            .await?
        }
        None => Vec::new(),
    };

    Ok(CreatedGroup {
        group: planned.group,
        published_welcomes,
    })
}

pub struct WelcomeQueries<'a> {
    mdk: &'a PikaMdk,
}

impl<'a> WelcomeQueries<'a> {
    pub fn new(mdk: &'a PikaMdk) -> Self {
        Self { mdk }
    }

    pub fn list_pending_welcome_snapshots(&self) -> Result<Vec<PendingWelcomeSnapshot>> {
        Ok(self
            .mdk
            .get_pending_welcomes(None)
            .context("get pending welcomes")?
            .iter()
            .map(PendingWelcomeSnapshot::from_welcome)
            .collect())
    }

    pub fn lookup_pending_welcome(&self, target: &EventId) -> Result<Option<Welcome>> {
        let pending = self
            .mdk
            .get_pending_welcomes(None)
            .context("get pending welcomes")?;
        Ok(find_pending_welcome(&pending, target).cloned())
    }
}
