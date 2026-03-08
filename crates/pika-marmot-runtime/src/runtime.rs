use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use nostr_sdk::prelude::*;

use crate::PikaMdk;
use crate::relay::subscribe_group_msgs;

pub struct RuntimeSession {
    pub pubkey: PublicKey,
    pub client: Client,
    pub mdk: PikaMdk,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuntimeStartupState {
    pub existing_group_ids: Vec<String>,
    pub seen_welcomes: HashSet<EventId>,
    pub seen_group_events: HashSet<EventId>,
}

pub struct BootstrappedRuntimeSession {
    pub session: RuntimeSession,
    pub startup: RuntimeStartupState,
}

impl RuntimeSession {
    pub async fn connect_relays(
        &self,
        relays: &[RelayUrl],
        reconnect: bool,
        wait_timeout: Option<Duration>,
    ) {
        connect_runtime_relays(&self.client, relays, reconnect, wait_timeout).await;
    }

    pub async fn subscribe_welcome_inbox(
        &self,
        lookback: Option<Duration>,
        limit: Option<usize>,
    ) -> Result<SubscriptionId> {
        subscribe_welcome_inbox(&self.client, self.pubkey, lookback, limit).await
    }

    pub async fn subscribe_group_messages_combined(
        &self,
        nostr_group_ids: &[String],
    ) -> Result<Option<SubscriptionId>> {
        subscribe_group_messages_combined(&self.client, nostr_group_ids).await
    }

    pub async fn subscribe_existing_groups_individual(
        &self,
    ) -> Result<HashMap<SubscriptionId, String>> {
        subscribe_group_messages_individual(&self.client, &existing_group_ids_from_mdk(&self.mdk)?)
            .await
    }

    pub fn existing_group_ids(&self) -> Result<Vec<String>> {
        existing_group_ids_from_mdk(&self.mdk)
    }
}

pub async fn connect_runtime_relays(
    client: &Client,
    relays: &[RelayUrl],
    reconnect: bool,
    wait_timeout: Option<Duration>,
) {
    for relay in relays {
        let _ = client.add_relay(relay.clone()).await;
    }
    if reconnect {
        client.disconnect().await;
    }
    client.connect().await;
    if let Some(timeout) = wait_timeout {
        client.wait_for_connection(timeout).await;
    }
}

pub async fn subscribe_welcome_inbox(
    client: &Client,
    pubkey: PublicKey,
    lookback: Option<Duration>,
    limit: Option<usize>,
) -> Result<SubscriptionId> {
    let mut filter = Filter::new().kind(Kind::GiftWrap).custom_tag(
        SingleLetterTag::lowercase(Alphabet::P),
        pubkey.to_hex().to_lowercase(),
    );
    if let Some(limit) = limit {
        filter = filter.limit(limit);
    }
    if let Some(lookback) = lookback {
        filter = filter.since(Timestamp::now() - lookback);
    }
    let out = client.subscribe(filter, None).await?;
    Ok(out.val)
}

pub async fn subscribe_group_messages_combined(
    client: &Client,
    nostr_group_ids: &[String],
) -> Result<Option<SubscriptionId>> {
    if nostr_group_ids.is_empty() {
        return Ok(None);
    }
    let filter = Filter::new().kind(Kind::MlsGroupMessage).custom_tags(
        SingleLetterTag::lowercase(Alphabet::H),
        nostr_group_ids.to_vec(),
    );
    let out = client.subscribe(filter, None).await?;
    Ok(Some(out.val))
}

pub async fn subscribe_existing_groups_individual(
    client: &Client,
    mdk: &PikaMdk,
) -> Result<HashMap<SubscriptionId, String>> {
    subscribe_group_messages_individual(client, &existing_group_ids_from_mdk(mdk)?).await
}

pub async fn subscribe_group_messages_individual(
    client: &Client,
    nostr_group_ids: &[String],
) -> Result<HashMap<SubscriptionId, String>> {
    let mut out = HashMap::new();
    for nostr_group_id_hex in nostr_group_ids {
        match subscribe_group_msgs(client, nostr_group_id_hex).await {
            Ok(subscription_id) => {
                out.insert(subscription_id, nostr_group_id_hex.clone());
            }
            Err(err) => {
                tracing::warn!(
                    nostr_group_id = nostr_group_id_hex,
                    error = %err,
                    "subscribe existing group failed"
                );
            }
        }
    }
    Ok(out)
}

pub fn existing_group_ids_from_mdk(mdk: &PikaMdk) -> Result<Vec<String>> {
    Ok(mdk
        .get_groups()?
        .into_iter()
        .map(|group| hex::encode(group.nostr_group_id))
        .collect())
}

pub fn bootstrap_runtime_session<F>(
    pubkey: PublicKey,
    signer: Arc<dyn NostrSigner>,
    open_mdk: F,
) -> Result<BootstrappedRuntimeSession>
where
    F: FnOnce() -> Result<PikaMdk>,
{
    let mdk = open_mdk()?;
    let client = Client::new(signer);
    let session = RuntimeSession {
        pubkey,
        client,
        mdk,
    };
    let startup = RuntimeStartupState {
        existing_group_ids: existing_group_ids_from_mdk(&session.mdk)?,
        seen_welcomes: HashSet::new(),
        seen_group_events: HashSet::new(),
    };
    Ok(BootstrappedRuntimeSession { session, startup })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdk_core::prelude::NostrGroupConfigData;

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

    #[test]
    fn bootstrap_runtime_session_surfaces_existing_group_ids() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir);
        let invitee_mdk = open_test_mdk(&invitee_dir);
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "runtime bootstrap test".to_string(),
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
        let expected_group_id = hex::encode(created.group.nostr_group_id);

        let bootstrapped = bootstrap_runtime_session(
            inviter_keys.public_key(),
            Arc::new(inviter_keys.clone()),
            || Ok(inviter_mdk),
        )
        .expect("bootstrap runtime session");

        assert_eq!(bootstrapped.session.pubkey, inviter_keys.public_key());
        assert_eq!(
            bootstrapped.startup.existing_group_ids,
            vec![expected_group_id]
        );
        assert!(bootstrapped.startup.seen_welcomes.is_empty());
        assert!(bootstrapped.startup.seen_group_events.is_empty());
    }
}
