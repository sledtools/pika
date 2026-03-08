use std::future::Future;

use anyhow::{Context, Result};
use mdk_storage_traits::GroupId;
use nostr_sdk::prelude::{Event, PublicKey, UnsignedEvent};

use crate::PikaMdk;
use crate::relay::PublishOutcome;

#[derive(Debug, Clone)]
pub struct PreparedMembershipEvolution {
    pub mls_group_id: GroupId,
    pub nostr_group_id_hex: String,
    pub evolution_event: Event,
    pub added_pubkeys: Vec<PublicKey>,
    pub welcome_rumors: Vec<UnsignedEvent>,
}

#[derive(Debug, Clone)]
pub struct WelcomeDeliveryPlan {
    pub recipients: Vec<PublicKey>,
    pub welcome_rumors: Vec<UnsignedEvent>,
}

#[derive(Debug, Clone)]
pub struct MembershipUpdateResult {
    pub mls_group_id: GroupId,
    pub nostr_group_id_hex: String,
    pub added_pubkeys: Vec<PublicKey>,
    pub merge_error: Option<String>,
    pub welcome_delivery: Option<WelcomeDeliveryPlan>,
}

#[derive(Debug, Clone)]
pub enum EvolutionPublishStatus {
    Published,
    PublishFailed(String),
}

pub struct MembershipRuntime<'a> {
    mdk: &'a PikaMdk,
}

impl<'a> MembershipRuntime<'a> {
    pub fn new(mdk: &'a PikaMdk) -> Self {
        Self { mdk }
    }

    pub fn prepare_add_members(
        &self,
        mls_group_id: &GroupId,
        key_package_events: &[Event],
    ) -> Result<PreparedMembershipEvolution> {
        for event in key_package_events {
            self.mdk
                .parse_key_package(event)
                .context("parse key package")?;
        }

        let result = self
            .mdk
            .add_members(mls_group_id, key_package_events)
            .context("add members")?;
        let added_pubkeys = key_package_events
            .iter()
            .map(|event| event.pubkey)
            .collect();

        self.prepare_evolution(
            mls_group_id.clone(),
            result.evolution_event,
            result.welcome_rumors,
            added_pubkeys,
        )
    }

    pub fn prepare_evolution(
        &self,
        mls_group_id: GroupId,
        evolution_event: Event,
        welcome_rumors: Option<Vec<UnsignedEvent>>,
        added_pubkeys: Vec<PublicKey>,
    ) -> Result<PreparedMembershipEvolution> {
        let nostr_group_id_hex = self
            .mdk
            .get_group(&mls_group_id)
            .context("get group for evolution")?
            .map(|group| hex::encode(group.nostr_group_id))
            .unwrap_or_default();

        Ok(PreparedMembershipEvolution {
            mls_group_id,
            nostr_group_id_hex,
            evolution_event,
            added_pubkeys,
            welcome_rumors: welcome_rumors.unwrap_or_default(),
        })
    }

    pub fn finalize_published_evolution(
        &self,
        prepared: PreparedMembershipEvolution,
    ) -> MembershipUpdateResult {
        let PreparedMembershipEvolution {
            mls_group_id,
            nostr_group_id_hex,
            added_pubkeys,
            welcome_rumors,
            ..
        } = prepared;

        let merge_error = self
            .mdk
            .merge_pending_commit(&mls_group_id)
            .err()
            .map(|err| err.to_string());

        let welcome_delivery = if merge_error.is_none() && !welcome_rumors.is_empty() {
            Some(WelcomeDeliveryPlan {
                recipients: added_pubkeys.clone(),
                welcome_rumors,
            })
        } else {
            None
        };

        MembershipUpdateResult {
            mls_group_id,
            nostr_group_id_hex,
            added_pubkeys,
            merge_error,
            welcome_delivery,
        }
    }
}

impl PreparedMembershipEvolution {
    pub async fn publish_with<F, Fut>(&self, mut publish: F) -> EvolutionPublishStatus
    where
        F: FnMut(Event) -> Fut,
        Fut: Future<Output = PublishOutcome>,
    {
        match publish(self.evolution_event.clone()).await {
            PublishOutcome::Ok => EvolutionPublishStatus::Published,
            PublishOutcome::Err(err) => EvolutionPublishStatus::PublishFailed(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::open_mdk;
    use mdk_core::prelude::NostrGroupConfigData;
    use nostr_sdk::prelude::{EventBuilder, Keys, Kind, RelayUrl};

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

    fn create_base_group() -> (tempfile::TempDir, tempfile::TempDir, PikaMdk, GroupId, Keys) {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Membership runtime".to_string(),
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
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        (
            inviter_dir,
            invitee_dir,
            inviter_mdk,
            created.group.mls_group_id,
            inviter_keys,
        )
    }

    #[test]
    fn prepare_add_members_validates_and_returns_welcome_plan() {
        let (_inviter_dir, _invitee_dir, inviter_mdk, group_id, _keys) = create_base_group();
        let peer_dir = tempfile::tempdir().expect("peer tempdir");
        let peer_keys = Keys::generate();
        let peer_mdk = open_mdk(peer_dir.path()).expect("open peer mdk");
        let peer_kp = make_key_package_event(&peer_mdk, &peer_keys);

        let prepared = MembershipRuntime::new(&inviter_mdk)
            .prepare_add_members(&group_id, &[peer_kp])
            .expect("prepare add members");

        assert_eq!(prepared.added_pubkeys, vec![peer_keys.public_key()]);
        assert_eq!(prepared.welcome_rumors.len(), 1);
        assert_eq!(prepared.evolution_event.kind, Kind::MlsGroupMessage);
    }

    #[test]
    fn finalize_published_evolution_merges_and_returns_welcome_delivery() {
        let (_inviter_dir, _invitee_dir, inviter_mdk, group_id, _keys) = create_base_group();
        let peer_dir = tempfile::tempdir().expect("peer tempdir");
        let peer_keys = Keys::generate();
        let peer_mdk = open_mdk(peer_dir.path()).expect("open peer mdk");
        let peer_kp = make_key_package_event(&peer_mdk, &peer_keys);
        let runtime = MembershipRuntime::new(&inviter_mdk);

        let prepared = runtime
            .prepare_add_members(&group_id, &[peer_kp])
            .expect("prepare add members");

        let before_merge = inviter_mdk
            .get_members(&group_id)
            .expect("members before merge")
            .len();

        let finalized = runtime.finalize_published_evolution(prepared);

        let after_merge = inviter_mdk
            .get_members(&group_id)
            .expect("members after merge")
            .len();
        assert_eq!(before_merge + 1, after_merge);
        assert!(finalized.merge_error.is_none());
        assert_eq!(
            finalized
                .welcome_delivery
                .as_ref()
                .expect("welcome delivery")
                .recipients,
            vec![peer_keys.public_key()]
        );
    }

    #[tokio::test]
    async fn prepared_evolution_publish_status_tracks_shared_publish_outcome() {
        let (_inviter_dir, _invitee_dir, inviter_mdk, group_id, _keys) = create_base_group();
        let peer_dir = tempfile::tempdir().expect("peer tempdir");
        let peer_keys = Keys::generate();
        let peer_mdk = open_mdk(peer_dir.path()).expect("open peer mdk");
        let peer_kp = make_key_package_event(&peer_mdk, &peer_keys);

        let prepared = MembershipRuntime::new(&inviter_mdk)
            .prepare_add_members(&group_id, &[peer_kp])
            .expect("prepare add members");

        let ok = prepared
            .publish_with(|_| async { PublishOutcome::Ok })
            .await;
        assert!(matches!(ok, EvolutionPublishStatus::Published));

        let failed = prepared
            .publish_with(|_| async { PublishOutcome::Err("relay down".to_string()) })
            .await;
        assert!(matches!(
            failed,
            EvolutionPublishStatus::PublishFailed(ref err) if err == "relay down"
        ));
    }
}
