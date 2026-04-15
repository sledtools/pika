use std::collections::HashSet;
use std::future::Future;

use anyhow::{Context, Result};
#[cfg(test)]
use nostr_sdk::prelude::Keys;
use nostr_sdk::prelude::{Event, EventId, NostrSigner, PublicKey, Tag, UnsignedEvent};
use pika_mls::prelude::NostrGroupConfigData;
#[cfg(test)]
use pika_mls::welcome::ingest_welcome_from_giftwrap as shared_ingest_welcome_from_giftwrap;
#[cfg(test)]
pub(crate) use pika_mls::welcome::IngestedWelcome;
use pika_mls::welcome::{
    create_group_and_plan_welcome_delivery as shared_create_group_and_plan_welcome_delivery,
    publish_welcome_rumors as shared_publish_welcome_rumors, WelcomeQueries,
};
pub(crate) use pika_mls::welcome::{
    GroupWelcomeDeliveryPlan, PendingWelcomeSnapshot, PlannedGroupCreation,
};

use crate::mdk_support::PikaMdk;

type StoredMessage = pika_mls::storage_traits::messages::types::Message;
type StoredWelcome = pika_mls::storage_traits::welcomes::types::Welcome;

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
pub(crate) async fn ingest_welcome_from_giftwrap<F>(
    mdk: &PikaMdk,
    keys: &Keys,
    event: &Event,
    sender_allowed: F,
) -> Result<Option<IngestedWelcome>>
where
    F: Fn(&str) -> bool,
{
    shared_ingest_welcome_from_giftwrap(mdk, keys, event, sender_allowed).await
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
    publish_giftwrap: F,
) -> Result<()>
where
    T: NostrSigner,
    F: FnMut(PublicKey, Event) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let _ = shared_publish_welcome_rumors(
        signer,
        welcome_rumors,
        recipients,
        welcome_tags,
        publish_giftwrap,
    )
    .await?;
    Ok(())
}

pub(crate) fn create_group_and_plan_welcome_delivery(
    creator_pubkey: &PublicKey,
    mdk: &PikaMdk,
    peer_key_packages: Vec<Event>,
    config: NostrGroupConfigData,
    recipients: &[PublicKey],
) -> Result<PlannedGroupCreation> {
    shared_create_group_and_plan_welcome_delivery(
        creator_pubkey,
        mdk,
        peer_key_packages,
        config,
        recipients,
    )
}
