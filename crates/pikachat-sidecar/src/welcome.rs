use std::collections::HashSet;
use std::future::Future;

use anyhow::{Context, Result};
use nostr_sdk::prelude::{Event, EventId, Keys, PublicKey, Tag, UnsignedEvent};
use pika_mls::prelude::NostrGroupConfigData;
pub use pika_mls::welcome::{
    CreatedGroup, GroupWelcomeDeliveryPlan, IngestedWelcome, PendingWelcomeSnapshot,
    PlannedGroupCreation, PublishedWelcome, accept_pending_welcome, find_pending_welcome,
    find_pending_welcome_index, stage_pending_welcome, take_pending_welcome,
};
use pika_mls::welcome::{
    WelcomeQueries, accept_pending_welcome as shared_accept_pending_welcome,
    create_group_and_plan_welcome_delivery as shared_create_group_and_plan_welcome_delivery,
    create_group_and_publish_welcomes as shared_create_group_and_publish_welcomes,
    ingest_unwrapped_welcome as shared_ingest_unwrapped_welcome,
    ingest_welcome_from_giftwrap as shared_ingest_welcome_from_giftwrap,
    publish_welcome_rumors as shared_publish_welcome_rumors,
};

use crate::{PikaMls, ingest_group_backlog};

#[derive(Debug, Clone)]
pub struct AcceptedWelcome {
    pub wrapper_event_id: EventId,
    pub welcome_event_id: EventId,
    pub nostr_group_id_hex: String,
    pub mls_group_id: pika_mls::storage_traits::GroupId,
    pub group_name: String,
    pub ingested_messages: Vec<pika_mls::storage_traits::messages::types::Message>,
}

pub fn list_pending_welcome_snapshots(mls: &PikaMls) -> Result<Vec<PendingWelcomeSnapshot>> {
    WelcomeQueries::new(mls).list_pending_welcome_snapshots()
}

pub fn lookup_pending_welcome(
    mls: &PikaMls,
    target: &EventId,
) -> Result<Option<pika_mls::storage_traits::welcomes::types::Welcome>> {
    WelcomeQueries::new(mls).lookup_pending_welcome(target)
}

pub fn ingest_unwrapped_welcome<F>(
    mls: &PikaMls,
    wrapper_event_id: &EventId,
    sender: PublicKey,
    rumor: &UnsignedEvent,
    sender_allowed: F,
) -> Result<Option<IngestedWelcome>>
where
    F: Fn(&str) -> bool,
{
    shared_ingest_unwrapped_welcome(mls, wrapper_event_id, sender, rumor, sender_allowed)
}

/// Unwrap and process a gift-wrapped MLS welcome into MLS pending-welcome
/// storage. This intentionally does not accept the welcome; hosts decide
/// whether to stage, auto-accept, subscribe, or backfill after ingest. MLS may
/// already expose a pending group row before accept.
pub async fn ingest_welcome_from_giftwrap<F>(
    mls: &PikaMls,
    keys: &Keys,
    event: &Event,
    sender_allowed: F,
) -> Result<Option<IngestedWelcome>>
where
    F: Fn(&str) -> bool,
{
    shared_ingest_welcome_from_giftwrap(mls, keys, event, sender_allowed).await
}

/// Accept a known pending welcome, optionally let the host run a narrow
/// post-accept hook, then backfill recent group messages through the shared
/// backlog ingest path.
///
/// Hosts still own policy. They choose when to call this, which relays to use
/// for catch-up, and what to do in the `after_accept` hook (for example daemon
/// subscription bookkeeping before backlog fetch).
pub async fn accept_welcome_and_catch_up<F, Fut>(
    mls: &PikaMls,
    client: &nostr_sdk::Client,
    relay_urls: &[nostr_sdk::RelayUrl],
    welcome: &pika_mls::storage_traits::welcomes::types::Welcome,
    seen: &mut HashSet<EventId>,
    limit: usize,
    after_accept: F,
) -> Result<AcceptedWelcome>
where
    F: FnOnce(&AcceptedWelcome) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let mut accepted = AcceptedWelcome {
        wrapper_event_id: welcome.wrapper_event_id,
        welcome_event_id: welcome.id,
        nostr_group_id_hex: hex::encode(welcome.nostr_group_id),
        mls_group_id: welcome.mls_group_id.clone(),
        group_name: welcome.group_name.clone(),
        ingested_messages: Vec::new(),
    };

    shared_accept_pending_welcome(mls, welcome)?;
    after_accept(&accepted).await?;

    if !relay_urls.is_empty() {
        accepted.ingested_messages = ingest_group_backlog(
            mls,
            client,
            relay_urls,
            &accepted.nostr_group_id_hex,
            seen,
            limit,
        )
        .await
        .context("ingest accepted welcome backlog")?;
    }

    Ok(accepted)
}

pub async fn publish_welcome_rumors<F, Fut>(
    signer: &Keys,
    welcome_rumors: &[nostr_sdk::prelude::UnsignedEvent],
    recipients: &[PublicKey],
    welcome_tags: Vec<nostr_sdk::prelude::Tag>,
    publish_giftwrap: F,
) -> Result<Vec<PublishedWelcome>>
where
    F: FnMut(PublicKey, Event) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    shared_publish_welcome_rumors(
        signer,
        welcome_rumors,
        recipients,
        welcome_tags,
        publish_giftwrap,
    )
    .await
}

pub fn create_group_and_plan_welcome_delivery(
    creator_pubkey: &PublicKey,
    mls: &PikaMls,
    peer_key_packages: Vec<Event>,
    config: NostrGroupConfigData,
    recipients: &[PublicKey],
) -> Result<PlannedGroupCreation> {
    shared_create_group_and_plan_welcome_delivery(
        creator_pubkey,
        mls,
        peer_key_packages,
        config,
        recipients,
    )
}

pub async fn create_group_and_publish_welcomes<F, Fut>(
    keys: &Keys,
    mls: &PikaMls,
    peer_key_packages: Vec<Event>,
    config: NostrGroupConfigData,
    recipients: &[PublicKey],
    welcome_tags: Vec<Tag>,
    publish_giftwrap: F,
) -> Result<CreatedGroup>
where
    F: FnMut(PublicKey, Event) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    shared_create_group_and_publish_welcomes(
        keys,
        mls,
        peer_key_packages,
        config,
        recipients,
        welcome_tags,
        publish_giftwrap,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::open_mls;
    use nostr_sdk::prelude::{EventBuilder, Kind, RelayUrl};

    fn make_key_package_event(mls: &PikaMls, keys: &Keys) -> Event {
        let relay = RelayUrl::parse("wss://test.relay").expect("relay url");
        let (content, tags, _hash_ref) = pika_mls::key_package::create_key_package_for_event(
            mls,
            &keys.public_key(),
            vec![relay],
        )
        .expect("create key package");
        EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign key package")
    }

    #[test]
    fn find_pending_welcome_matches_wrapper_or_welcome_id_only() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mls = open_mls(inviter_dir.path()).expect("open inviter mls");
        let invitee_mls = open_mls(invitee_dir.path()).expect("open invitee mls");

        let invitee_kp = make_key_package_event(&invitee_mls, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime pending welcome test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let group_result = inviter_mls
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let mut welcome_rumor = group_result
            .welcome_rumors
            .into_iter()
            .next()
            .expect("welcome rumor");
        let welcome_event_id = welcome_rumor.id();

        let wrapper = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                EventBuilder::gift_wrap(
                    &inviter_keys,
                    &invitee_keys.public_key(),
                    welcome_rumor,
                    [],
                )
                .await
                .expect("build giftwrap")
            });

        tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                ingest_welcome_from_giftwrap(&invitee_mls, &invitee_keys, &wrapper, |_| true)
                    .await
                    .expect("ingest welcome")
                    .expect("welcome should ingest");
            });

        let mut pending = WelcomeQueries::new(&invitee_mls)
            .list_pending_welcomes()
            .expect("get pending welcomes");

        let by_wrapper = find_pending_welcome(&pending, &wrapper.id).expect("match wrapper id");
        assert_eq!(by_wrapper.wrapper_event_id, wrapper.id);

        let by_welcome =
            find_pending_welcome(&pending, &welcome_event_id).expect("match welcome id");
        assert_eq!(by_welcome.id, welcome_event_id);
        let looked_up = lookup_pending_welcome(&invitee_mls, &welcome_event_id)
            .expect("lookup pending welcome")
            .expect("pending welcome should exist");
        assert_eq!(looked_up.id, welcome_event_id);

        let taken = take_pending_welcome(&mut pending, &welcome_event_id).expect("take welcome");
        assert_eq!(taken.id, welcome_event_id);
        assert!(pending.is_empty(), "take should remove the matched welcome");

        let missing = EventId::from_hex(&"e".repeat(64)).expect("missing event id");
        assert!(find_pending_welcome(&pending, &missing).is_none());
        assert!(find_pending_welcome_index(&pending, &missing).is_none());
    }

    #[test]
    fn list_pending_welcome_snapshots_surface_shared_metadata() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mls = open_mls(inviter_dir.path()).expect("open inviter mls");
        let invitee_mls = open_mls(invitee_dir.path()).expect("open invitee mls");

        let relay = RelayUrl::parse("wss://test.relay").expect("relay url");
        let invitee_kp = make_key_package_event(&invitee_mls, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime pending welcome snapshot".to_string(),
            "Shared pending welcome query".to_string(),
            None,
            None,
            None,
            vec![relay.clone()],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let group_result = inviter_mls
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let mut welcome_rumor = group_result
            .welcome_rumors
            .into_iter()
            .next()
            .expect("welcome rumor");
        let welcome_event_id = welcome_rumor.id();
        let welcome_created_at = welcome_rumor.created_at;
        let wrapper = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                EventBuilder::gift_wrap(
                    &inviter_keys,
                    &invitee_keys.public_key(),
                    welcome_rumor,
                    [],
                )
                .await
                .expect("build giftwrap")
            });
        tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                ingest_welcome_from_giftwrap(&invitee_mls, &invitee_keys, &wrapper, |_| true)
                    .await
                    .expect("ingest welcome")
                    .expect("welcome should ingest");
            });

        let snapshots =
            list_pending_welcome_snapshots(&invitee_mls).expect("list pending welcome snapshots");
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(snapshot.wrapper_event_id, wrapper.id);
        assert_eq!(snapshot.welcome_event_id, welcome_event_id);
        assert_eq!(snapshot.welcomer, inviter_keys.public_key());
        assert_eq!(
            snapshot.nostr_group_id_hex,
            hex::encode(group_result.group.nostr_group_id)
        );
        assert_eq!(snapshot.group_name, "Runtime pending welcome snapshot");
        assert_eq!(snapshot.group_description, "Shared pending welcome query");
        assert_eq!(snapshot.member_count, 2);
        assert_eq!(snapshot.group_relays, vec![relay]);
        assert_eq!(snapshot.created_at, welcome_created_at);
        assert_eq!(snapshot.mls_group_id, group_result.group.mls_group_id);
    }

    #[test]
    fn accept_welcome_and_catch_up_accepts_and_returns_group_ids_without_relays() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mls = open_mls(inviter_dir.path()).expect("open inviter mls");
        let invitee_mls = open_mls(invitee_dir.path()).expect("open invitee mls");
        let invitee_client = nostr_sdk::Client::builder()
            .signer(invitee_keys.clone())
            .build();

        let invitee_kp = make_key_package_event(&invitee_mls, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime accept test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let group_result = inviter_mls
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let welcome_rumor = group_result
            .welcome_rumors
            .into_iter()
            .next()
            .expect("welcome rumor");
        let wrapper = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                EventBuilder::gift_wrap(
                    &inviter_keys,
                    &invitee_keys.public_key(),
                    welcome_rumor,
                    [],
                )
                .await
                .expect("build giftwrap")
            });

        tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                ingest_welcome_from_giftwrap(&invitee_mls, &invitee_keys, &wrapper, |_| true)
                    .await
                    .expect("ingest welcome")
                    .expect("welcome should ingest");
            });

        let pending = WelcomeQueries::new(&invitee_mls)
            .list_pending_welcomes()
            .expect("get pending welcomes");
        let welcome = pending.first().expect("pending welcome");
        let mut seen = HashSet::new();
        let accepted = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                accept_welcome_and_catch_up(
                    &invitee_mls,
                    &invitee_client,
                    &[],
                    welcome,
                    &mut seen,
                    200,
                    |_| async { Ok(()) },
                )
                .await
                .expect("accept welcome and catch up")
            });

        assert_eq!(accepted.wrapper_event_id, wrapper.id);
        assert_eq!(
            accepted.nostr_group_id_hex,
            hex::encode(group_result.group.nostr_group_id)
        );
        assert_eq!(accepted.group_name, "Runtime accept test");
        assert!(
            accepted.ingested_messages.is_empty(),
            "empty relay list should preserve manual/narrow host behavior"
        );
        assert!(
            WelcomeQueries::new(&invitee_mls)
                .list_pending_welcomes()
                .expect("get pending welcomes")
                .is_empty(),
            "accept should clear the pending welcome"
        );
    }

    #[test]
    fn ingest_welcome_from_giftwrap_stages_pending_welcome_without_joining() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mls = open_mls(inviter_dir.path()).expect("open inviter mls");
        let invitee_mls = open_mls(invitee_dir.path()).expect("open invitee mls");

        let invitee_kp = make_key_package_event(&invitee_mls, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime ingest test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let group_result = inviter_mls
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let mut welcome_rumor = group_result
            .welcome_rumors
            .into_iter()
            .next()
            .expect("welcome rumor");
        let welcome_event_id = welcome_rumor.id();

        let wrapper = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                EventBuilder::gift_wrap(
                    &inviter_keys,
                    &invitee_keys.public_key(),
                    welcome_rumor,
                    [],
                )
                .await
                .expect("build giftwrap")
            });

        let ingested = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                ingest_welcome_from_giftwrap(&invitee_mls, &invitee_keys, &wrapper, |_| true)
                    .await
                    .expect("ingest welcome")
                    .expect("welcome should be accepted for ingest")
            });

        assert_eq!(ingested.wrapper_event_id, wrapper.id);
        assert_eq!(ingested.welcome_event_id, welcome_event_id);
        assert_eq!(
            ingested.nostr_group_id_hex,
            hex::encode(group_result.group.nostr_group_id)
        );
        assert_eq!(ingested.group_name, "Runtime ingest test");

        let pending = WelcomeQueries::new(&invitee_mls)
            .list_pending_welcomes()
            .expect("get pending welcomes");
        assert_eq!(pending.len(), 1, "ingest should stage exactly one welcome");
        assert_eq!(
            pending[0].wrapper_event_id, wrapper.id,
            "staged welcome should keep the wrapper id for explicit accept flows"
        );
        let groups = pika_mls::conversation::ConversationQueries::new(&invitee_mls)
            .list_joined_group_snapshots()
            .expect("get groups");
        assert_eq!(
            groups.len(),
            1,
            "shared ingest already surfaces a pending group before accept"
        );
        assert_eq!(
            groups[0].nostr_group_id_hex, ingested.nostr_group_id_hex,
            "pending group should line up with the staged welcome metadata"
        );
    }

    #[tokio::test]
    async fn publish_welcome_rumors_pairs_each_recipient_with_one_welcome() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let bob_dir = tempfile::tempdir().expect("bob tempdir");
        let charlie_dir = tempfile::tempdir().expect("charlie tempdir");
        let inviter_keys = Keys::generate();
        let bob_keys = Keys::generate();
        let charlie_keys = Keys::generate();
        let inviter_mls = open_mls(inviter_dir.path()).expect("open inviter mls");
        let bob_mls = open_mls(bob_dir.path()).expect("open bob mls");
        let charlie_mls = open_mls(charlie_dir.path()).expect("open charlie mls");

        let bob_kp = make_key_package_event(&bob_mls, &bob_keys);
        let charlie_kp = make_key_package_event(&charlie_mls, &charlie_keys);
        let config = NostrGroupConfigData::new(
            "Runtime multi invite test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![
                inviter_keys.public_key(),
                bob_keys.public_key(),
                charlie_keys.public_key(),
            ],
        );

        let published =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::<(PublicKey, Event)>::new()));
        let published_capture = std::sync::Arc::clone(&published);
        let group_result = inviter_mls
            .create_group(&inviter_keys.public_key(), vec![bob_kp, charlie_kp], config)
            .expect("create group");
        let published_welcomes = publish_welcome_rumors(
            &inviter_keys,
            &group_result.welcome_rumors,
            &[bob_keys.public_key(), charlie_keys.public_key()],
            vec![],
            move |receiver, giftwrap| {
                let published_capture = std::sync::Arc::clone(&published_capture);
                async move {
                    published_capture
                        .lock()
                        .expect("published lock")
                        .push((receiver, giftwrap));
                    Ok(())
                }
            },
        )
        .await
        .expect("publish welcome rumors");

        assert_eq!(published_welcomes.len(), 2);
        let published = published.lock().expect("published lock").clone();
        assert_eq!(published.len(), 2);

        for (receiver, wrapper) in published {
            match receiver {
                receiver if receiver == bob_keys.public_key() => {
                    let ingested =
                        ingest_welcome_from_giftwrap(&bob_mls, &bob_keys, &wrapper, |_| true)
                            .await
                            .expect("ingest bob welcome");
                    assert!(ingested.is_some(), "bob should ingest exactly one welcome");
                }
                receiver if receiver == charlie_keys.public_key() => {
                    let ingested =
                        ingest_welcome_from_giftwrap(&charlie_mls, &charlie_keys, &wrapper, |_| {
                            true
                        })
                        .await
                        .expect("ingest charlie welcome");
                    assert!(
                        ingested.is_some(),
                        "charlie should ingest exactly one welcome"
                    );
                }
                other => panic!("unexpected receiver {}", other.to_hex()),
            }
        }
    }

    #[tokio::test]
    async fn publish_welcome_rumors_rejects_recipient_count_mismatch() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mls = open_mls(inviter_dir.path()).expect("open inviter mls");
        let invitee_mls = open_mls(invitee_dir.path()).expect("open invitee mls");

        let invitee_kp = make_key_package_event(&invitee_mls, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime mismatch test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );

        let group_result = inviter_mls
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let err = publish_welcome_rumors(
            &inviter_keys,
            &group_result.welcome_rumors,
            &[],
            vec![],
            |_receiver, _giftwrap| async move { Ok(()) },
        )
        .await
        .expect_err("recipient mismatch should fail");

        assert!(err.to_string().contains("recipient/welcome mismatch"));
    }

    #[test]
    fn create_group_and_plan_welcome_delivery_returns_group_and_welcome_plan() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mls = open_mls(inviter_dir.path()).expect("open inviter mls");
        let invitee_mls = open_mls(invitee_dir.path()).expect("open invitee mls");

        let invitee_kp = make_key_package_event(&invitee_mls, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime create plan test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );

        let planned = create_group_and_plan_welcome_delivery(
            &inviter_keys.public_key(),
            &inviter_mls,
            vec![invitee_kp],
            config,
            &[invitee_keys.public_key()],
        )
        .expect("create group and plan welcomes");

        assert_eq!(planned.group.name, "Runtime create plan test");
        let welcome_delivery = planned.welcome_delivery.expect("welcome delivery plan");
        assert_eq!(welcome_delivery.recipients, vec![invitee_keys.public_key()]);
        assert_eq!(welcome_delivery.welcome_rumors.len(), 1);
    }

    #[test]
    fn create_group_and_plan_welcome_delivery_returns_no_plan_without_recipients() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let inviter_keys = Keys::generate();
        let inviter_mls = open_mls(inviter_dir.path()).expect("open inviter mls");

        let config = NostrGroupConfigData::new(
            "Runtime local create plan test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key()],
        );

        let planned = create_group_and_plan_welcome_delivery(
            &inviter_keys.public_key(),
            &inviter_mls,
            vec![],
            config,
            &[],
        )
        .expect("create local-only group");

        assert_eq!(planned.group.name, "Runtime local create plan test");
        assert!(
            planned.welcome_delivery.is_none(),
            "local-only create should not enqueue welcome delivery work"
        );
    }

    #[tokio::test]
    async fn create_group_and_publish_welcomes_returns_group_and_published_metadata() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mls = open_mls(inviter_dir.path()).expect("open inviter mls");
        let invitee_mls = open_mls(invitee_dir.path()).expect("open invitee mls");

        let invitee_kp = make_key_package_event(&invitee_mls, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime create test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );

        let published = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Event>::new()));
        let published_capture = std::sync::Arc::clone(&published);
        let created = create_group_and_publish_welcomes(
            &inviter_keys,
            &inviter_mls,
            vec![invitee_kp],
            config,
            &[invitee_keys.public_key()],
            vec![],
            move |_receiver, giftwrap| {
                let published_capture = std::sync::Arc::clone(&published_capture);
                async move {
                    published_capture
                        .lock()
                        .expect("published lock")
                        .push(giftwrap);
                    Ok(())
                }
            },
        )
        .await
        .expect("create group and publish welcomes");

        assert_eq!(created.group.name, "Runtime create test");
        assert_eq!(created.published_welcomes.len(), 1);
        assert_eq!(published.lock().expect("published lock").len(), 1);
    }

    #[test]
    fn create_group_and_plan_welcome_delivery_rejects_mismatch_before_create() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mls = open_mls(inviter_dir.path()).expect("open inviter mls");
        let invitee_mls = open_mls(invitee_dir.path()).expect("open invitee mls");

        let invitee_kp = make_key_package_event(&invitee_mls, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime mismatch plan test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );

        let err = create_group_and_plan_welcome_delivery(
            &inviter_keys.public_key(),
            &inviter_mls,
            vec![invitee_kp],
            config,
            &[],
        )
        .expect_err("recipient mismatch should fail");

        assert!(err.to_string().contains("recipient/keypackage mismatch"));
    }
}
