use std::collections::HashSet;
use std::future::Future;

use anyhow::{Context, Result};
use nostr_sdk::prelude::{Event, EventId, Keys, Kind, PublicKey};

use crate::{PikaMdk, ingest_group_backlog};

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
pub struct AcceptedWelcome {
    pub wrapper_event_id: EventId,
    pub welcome_event_id: EventId,
    pub nostr_group_id_hex: String,
    pub mls_group_id: mdk_storage_traits::GroupId,
    pub group_name: String,
    pub ingested_messages: Vec<mdk_storage_traits::messages::types::Message>,
}

pub fn find_pending_welcome<'a>(
    welcomes: &'a [mdk_storage_traits::welcomes::types::Welcome],
    target: &EventId,
) -> Option<&'a mdk_storage_traits::welcomes::types::Welcome> {
    welcomes
        .iter()
        .find(|welcome| welcome.wrapper_event_id == *target)
        .or_else(|| welcomes.iter().find(|welcome| welcome.id == *target))
}

pub fn find_pending_welcome_index(
    welcomes: &[mdk_storage_traits::welcomes::types::Welcome],
    target: &EventId,
) -> Option<usize> {
    welcomes
        .iter()
        .position(|welcome| welcome.wrapper_event_id == *target)
        .or_else(|| welcomes.iter().position(|welcome| welcome.id == *target))
}

pub fn take_pending_welcome(
    welcomes: &mut Vec<mdk_storage_traits::welcomes::types::Welcome>,
    target: &EventId,
) -> Option<mdk_storage_traits::welcomes::types::Welcome> {
    find_pending_welcome_index(welcomes, target).map(|idx| welcomes.swap_remove(idx))
}

/// Unwrap and process a gift-wrapped MLS welcome into MDK pending-welcome
/// storage. This intentionally does not accept the welcome; hosts decide
/// whether to stage, auto-accept, subscribe, or backfill after ingest. MDK may
/// already expose a pending group row before accept.
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

    let unwrapped = nostr_sdk::nostr::nips::nip59::extract_rumor(keys, event)
        .await
        .context("unwrap giftwrap rumor")?;
    if unwrapped.rumor.kind != Kind::MlsWelcome {
        return Ok(None);
    }

    let sender_hex = unwrapped.sender.to_hex().to_lowercase();
    if !sender_allowed(&sender_hex) {
        return Ok(None);
    }

    let mut rumor = unwrapped.rumor;
    mdk.process_welcome(&event.id, &rumor)
        .context("process welcome rumor")?;

    let pending = mdk
        .get_pending_welcomes(None)
        .context("get pending welcomes")?;
    let stored = pending.into_iter().find(|w| w.wrapper_event_id == event.id);
    let (nostr_group_id_hex, group_name) = match stored {
        Some(w) => (hex::encode(w.nostr_group_id), w.group_name),
        None => (String::new(), String::new()),
    };

    Ok(Some(IngestedWelcome {
        wrapper_event_id: event.id,
        welcome_event_id: rumor.id(),
        sender: unwrapped.sender,
        sender_hex,
        nostr_group_id_hex,
        group_name,
    }))
}

/// Accept a known pending welcome, optionally let the host run a narrow
/// post-accept hook, then backfill recent group messages through the shared
/// backlog ingest path.
///
/// Hosts still own policy. They choose when to call this, which relays to use
/// for catch-up, and what to do in the `after_accept` hook (for example daemon
/// subscription bookkeeping before backlog fetch).
pub async fn accept_welcome_and_catch_up<F, Fut>(
    mdk: &PikaMdk,
    client: &nostr_sdk::Client,
    relay_urls: &[nostr_sdk::RelayUrl],
    welcome: &mdk_storage_traits::welcomes::types::Welcome,
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

    mdk.accept_welcome(welcome).context("accept welcome")?;
    after_accept(&accepted).await?;

    if !relay_urls.is_empty() {
        accepted.ingested_messages = ingest_group_backlog(
            mdk,
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::open_mdk;
    use mdk_core::prelude::NostrGroupConfigData;
    use nostr_sdk::prelude::{EventBuilder, RelayUrl};

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

    #[test]
    fn find_pending_welcome_matches_wrapper_or_welcome_id_only() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime pending welcome test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let group_result = inviter_mdk
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
                ingest_welcome_from_giftwrap(&invitee_mdk, &invitee_keys, &wrapper, |_| true)
                    .await
                    .expect("ingest welcome")
                    .expect("welcome should ingest");
            });

        let mut pending = invitee_mdk
            .get_pending_welcomes(None)
            .expect("get pending welcomes");

        let by_wrapper = find_pending_welcome(&pending, &wrapper.id).expect("match wrapper id");
        assert_eq!(by_wrapper.wrapper_event_id, wrapper.id);

        let by_welcome =
            find_pending_welcome(&pending, &welcome_event_id).expect("match welcome id");
        assert_eq!(by_welcome.id, welcome_event_id);

        let taken = take_pending_welcome(&mut pending, &welcome_event_id).expect("take welcome");
        assert_eq!(taken.id, welcome_event_id);
        assert!(pending.is_empty(), "take should remove the matched welcome");

        let missing = EventId::from_hex(&"e".repeat(64)).expect("missing event id");
        assert!(find_pending_welcome(&pending, &missing).is_none());
        assert!(find_pending_welcome_index(&pending, &missing).is_none());
    }

    #[test]
    fn accept_welcome_and_catch_up_accepts_and_returns_group_ids_without_relays() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let invitee_client = nostr_sdk::Client::builder()
            .signer(invitee_keys.clone())
            .build();

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime accept test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let group_result = inviter_mdk
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
                ingest_welcome_from_giftwrap(&invitee_mdk, &invitee_keys, &wrapper, |_| true)
                    .await
                    .expect("ingest welcome")
                    .expect("welcome should ingest");
            });

        let pending = invitee_mdk
            .get_pending_welcomes(None)
            .expect("get pending welcomes");
        let welcome = pending.first().expect("pending welcome");
        let mut seen = HashSet::new();
        let accepted = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                accept_welcome_and_catch_up(
                    &invitee_mdk,
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
            invitee_mdk
                .get_pending_welcomes(None)
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
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime ingest test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let group_result = inviter_mdk
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
                ingest_welcome_from_giftwrap(&invitee_mdk, &invitee_keys, &wrapper, |_| true)
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

        let pending = invitee_mdk
            .get_pending_welcomes(None)
            .expect("get pending welcomes");
        assert_eq!(pending.len(), 1, "ingest should stage exactly one welcome");
        assert_eq!(
            pending[0].wrapper_event_id, wrapper.id,
            "staged welcome should keep the wrapper id for explicit accept flows"
        );
        let groups = invitee_mdk.get_groups().expect("get groups");
        assert_eq!(
            groups.len(),
            1,
            "shared ingest already surfaces a pending group before accept"
        );
        assert_eq!(
            hex::encode(groups[0].nostr_group_id),
            ingested.nostr_group_id_hex,
            "pending group should line up with the staged welcome metadata"
        );
    }
}
