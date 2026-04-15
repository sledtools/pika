use std::future::Future;

use anyhow::{anyhow, Context, Result};
use mdk_storage_traits::GroupId;
use nostr_sdk::prelude::{Event, PublicKey, UnsignedEvent};

use crate::mdk_support::PikaMdk;

use super::relay_publish::PublishOutcome;

#[derive(Debug, Clone)]
pub(crate) struct PreparedMembershipEvolution {
    pub mls_group_id: GroupId,
    pub nostr_group_id_hex: String,
    pub evolution_event: Event,
    pub expected_epoch: u64,
    pub added_pubkeys: Vec<PublicKey>,
    pub removed_pubkeys: Vec<PublicKey>,
    pub self_removed: bool,
    pub welcome_rumors: Vec<UnsignedEvent>,
    pub transport_applied_membership: bool,
    pub transport_delivered_welcomes: bool,
    pub stale_epoch_conflict: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct WelcomeDeliveryPlan {
    pub recipients: Vec<PublicKey>,
    pub welcome_rumors: Vec<UnsignedEvent>,
}

#[derive(Debug, Clone)]
pub(crate) struct MembershipUpdateResult {
    pub mls_group_id: GroupId,
    pub nostr_group_id_hex: String,
    pub added_pubkeys: Vec<PublicKey>,
    pub merge_error: Option<String>,
    pub welcome_delivery: Option<WelcomeDeliveryPlan>,
}

#[derive(Debug, Clone)]
pub(crate) enum EvolutionPublishStatus {
    Published,
    PublishFailed(String),
}

pub(crate) fn prepare_add_members(
    mdk: &PikaMdk,
    mls_group_id: &GroupId,
    key_package_events: &[Event],
) -> Result<PreparedMembershipEvolution> {
    for event in key_package_events {
        mdk.parse_key_package(event).context("parse key package")?;
    }

    let result = mdk
        .add_members(mls_group_id, key_package_events)
        .context("add members")?;
    let added_pubkeys = key_package_events
        .iter()
        .map(|event| event.pubkey)
        .collect();

    prepare_evolution(
        mdk,
        mls_group_id.clone(),
        result.evolution_event,
        result.welcome_rumors,
        added_pubkeys,
    )
}

pub(crate) fn prepare_evolution(
    mdk: &PikaMdk,
    mls_group_id: GroupId,
    evolution_event: Event,
    welcome_rumors: Option<Vec<UnsignedEvent>>,
    added_pubkeys: Vec<PublicKey>,
) -> Result<PreparedMembershipEvolution> {
    let group = mdk
        .get_group(&mls_group_id)
        .context("get group for evolution")?
        .ok_or_else(|| anyhow!("group not found"))?;

    Ok(PreparedMembershipEvolution {
        mls_group_id,
        nostr_group_id_hex: hex::encode(group.nostr_group_id),
        evolution_event,
        expected_epoch: group.epoch,
        added_pubkeys,
        removed_pubkeys: Vec::new(),
        self_removed: false,
        welcome_rumors: welcome_rumors.unwrap_or_default(),
        transport_applied_membership: false,
        transport_delivered_welcomes: false,
        stale_epoch_conflict: false,
    })
}

pub(crate) fn finalize_published_evolution(
    mdk: &PikaMdk,
    prepared: PreparedMembershipEvolution,
) -> MembershipUpdateResult {
    let PreparedMembershipEvolution {
        mls_group_id,
        nostr_group_id_hex,
        added_pubkeys,
        welcome_rumors,
        transport_delivered_welcomes,
        ..
    } = prepared;

    let merge_error = mdk
        .merge_pending_commit(&mls_group_id)
        .err()
        .map(|err| err.to_string());

    let welcome_delivery =
        if merge_error.is_none() && !transport_delivered_welcomes && !welcome_rumors.is_empty() {
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

impl PreparedMembershipEvolution {
    pub fn is_membership_change(&self) -> bool {
        !self.added_pubkeys.is_empty() || !self.removed_pubkeys.is_empty() || self.self_removed
    }

    pub fn mark_stale_epoch_conflict(&mut self) {
        self.stale_epoch_conflict = true;
    }

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
