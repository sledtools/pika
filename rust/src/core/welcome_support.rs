use std::collections::HashSet;
use std::future::Future;

use anyhow::{Context, Result};
use nostr_sdk::prelude::{
    Event, EventBuilder, EventId, NostrSigner, PublicKey, Tag, UnsignedEvent,
};
#[cfg(test)]
use nostr_sdk::prelude::{Keys, Kind};
use pika_mls::prelude::NostrGroupConfigData;
pub(crate) use pika_mls::welcome::PendingWelcomeSnapshot;
use pika_mls::welcome::WelcomeQueries;

use crate::mdk_support::PikaMdk;

type StoredGroup = pika_mls::storage_traits::groups::types::Group;
type StoredMessage = pika_mls::storage_traits::messages::types::Message;
type StoredWelcome = pika_mls::storage_traits::welcomes::types::Welcome;

#[cfg(test)]
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct IngestedWelcome {
    pub wrapper_event_id: EventId,
    pub welcome_event_id: EventId,
    pub sender: PublicKey,
    pub sender_hex: String,
    pub nostr_group_id_hex: String,
    pub group_name: String,
}

#[derive(Debug, Clone)]
pub(crate) struct GroupWelcomeDeliveryPlan {
    pub recipients: Vec<PublicKey>,
    pub welcome_rumors: Vec<UnsignedEvent>,
}

#[derive(Debug, Clone)]
pub(crate) struct PlannedGroupCreation {
    pub group: StoredGroup,
    pub welcome_delivery: Option<GroupWelcomeDeliveryPlan>,
}

pub(crate) fn list_pending_welcome_snapshots(mdk: &PikaMdk) -> Result<Vec<PendingWelcomeSnapshot>> {
    WelcomeQueries::new(mdk).list_pending_welcome_snapshots()
}

pub(crate) fn lookup_pending_welcome(
    mdk: &PikaMdk,
    target: &EventId,
) -> Result<Option<StoredWelcome>> {
    WelcomeQueries::new(mdk).lookup_pending_welcome(target)
}

#[cfg(test)]
pub(crate) fn ingest_unwrapped_welcome<F>(
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

    let welcome_event_id = rumor.clone().id();

    Ok(Some(IngestedWelcome {
        wrapper_event_id: *wrapper_event_id,
        welcome_event_id,
        sender,
        sender_hex,
        nostr_group_id_hex,
        group_name,
    }))
}

#[cfg(test)]
pub(crate) async fn ingest_welcome_from_giftwrap<F>(
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

    let unwrapped = nostr_sdk::nostr::nips::nip59::extract_rumor(keys, event)
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

pub(crate) async fn accept_welcome_and_catch_up<F, Fut>(
    mdk: &PikaMdk,
    client: &nostr_sdk::Client,
    relay_urls: &[nostr_sdk::RelayUrl],
    welcome: &StoredWelcome,
    seen: &mut HashSet<EventId>,
    limit: usize,
    after_accept: F,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    mdk.accept_welcome(welcome).context("accept welcome")?;
    after_accept().await?;

    if !relay_urls.is_empty() {
        let nostr_group_id_hex = hex::encode(welcome.nostr_group_id);
        let _ingested_messages: Vec<StoredMessage> =
            super::conversation_support::ingest_backlog_messages(
                mdk,
                client,
                relay_urls,
                &nostr_group_id_hex,
                seen,
                limit,
            )
            .await
            .context("ingest accepted welcome backlog")?;
    }

    Ok(())
}

pub(crate) async fn publish_welcome_rumors<T, F, Fut>(
    signer: &T,
    welcome_rumors: &[UnsignedEvent],
    recipients: &[PublicKey],
    welcome_tags: Vec<Tag>,
    mut publish_giftwrap: F,
) -> Result<()>
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

    for (receiver, rumor) in recipients
        .iter()
        .copied()
        .zip(welcome_rumors.iter().cloned())
    {
        let giftwrap =
            EventBuilder::gift_wrap(signer, &receiver, rumor.clone(), welcome_tags.clone())
                .await
                .context("build welcome giftwrap")?;
        publish_giftwrap(receiver, giftwrap.clone())
            .await
            .with_context(|| format!("publish welcome to {}", receiver.to_hex()))?;
    }

    Ok(())
}

pub(crate) fn create_group_and_plan_welcome_delivery(
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
