use std::future::Future;

use anyhow::{Context, Result, anyhow};
use nostr::{Event, PublicKey, UnsignedEvent};

use crate::PikaMdk;
use crate::prelude::NostrGroupDataUpdate;
use crate::storage_traits::GroupId;

#[derive(Debug, Clone)]
pub struct PreparedMembershipEvolution {
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
    pub transport_applied_membership: bool,
}

#[derive(Debug, Clone)]
pub enum EvolutionPublishStatus {
    Published,
    PublishFailed(String),
}

pub trait IntoEvolutionPublishStatus {
    fn into_evolution_publish_status(self) -> EvolutionPublishStatus;
}

impl IntoEvolutionPublishStatus for EvolutionPublishStatus {
    fn into_evolution_publish_status(self) -> EvolutionPublishStatus {
        self
    }
}

impl<T, E> IntoEvolutionPublishStatus for std::result::Result<T, E>
where
    E: std::fmt::Display,
{
    fn into_evolution_publish_status(self) -> EvolutionPublishStatus {
        match self {
            Ok(_) => EvolutionPublishStatus::Published,
            Err(err) => EvolutionPublishStatus::PublishFailed(err.to_string()),
        }
    }
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
        validate_key_package_events(self.mdk, key_package_events)?;

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

    pub fn prepare_remove_members(
        &self,
        mls_group_id: &GroupId,
        removed_pubkeys: &[PublicKey],
    ) -> Result<PreparedMembershipEvolution> {
        let result = self
            .mdk
            .remove_members(mls_group_id, removed_pubkeys)
            .context("remove members")?;

        self.prepare_evolution(
            mls_group_id.clone(),
            result.evolution_event,
            None,
            Vec::new(),
        )
        .map(|mut prepared| {
            prepared.removed_pubkeys = removed_pubkeys.to_vec();
            prepared
        })
    }

    pub fn prepare_leave_group(
        &self,
        mls_group_id: &GroupId,
    ) -> Result<PreparedMembershipEvolution> {
        let result = self.mdk.leave_group(mls_group_id).context("leave group")?;

        self.prepare_evolution(
            mls_group_id.clone(),
            result.evolution_event,
            None,
            Vec::new(),
        )
        .map(|mut prepared| {
            prepared.self_removed = true;
            prepared
        })
    }

    pub fn prepare_evolution(
        &self,
        mls_group_id: GroupId,
        evolution_event: Event,
        welcome_rumors: Option<Vec<UnsignedEvent>>,
        added_pubkeys: Vec<PublicKey>,
    ) -> Result<PreparedMembershipEvolution> {
        let group = self
            .mdk
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

    pub fn prepare_group_data_update(
        &self,
        mls_group_id: &GroupId,
        update: NostrGroupDataUpdate,
    ) -> Result<PreparedMembershipEvolution> {
        let result = self
            .mdk
            .update_group_data(mls_group_id, update)
            .context("update group data")?;

        self.prepare_evolution(
            mls_group_id.clone(),
            result.evolution_event,
            None,
            Vec::new(),
        )
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
            transport_applied_membership,
            transport_delivered_welcomes,
            ..
        } = prepared;

        let merge_error = self
            .mdk
            .merge_pending_commit(&mls_group_id)
            .err()
            .map(|err| err.to_string());

        let welcome_delivery =
            if merge_error.is_none() && !transport_delivered_welcomes && !welcome_rumors.is_empty()
            {
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
            transport_applied_membership,
        }
    }
}

pub fn validate_key_package_events(mdk: &PikaMdk, key_package_events: &[Event]) -> Result<()> {
    for event in key_package_events {
        mdk.parse_key_package(event).context("parse key package")?;
    }
    Ok(())
}

pub fn clear_pending_commit(mdk: &PikaMdk, mls_group_id: &GroupId) -> Result<()> {
    mdk.clear_pending_commit(mls_group_id)
        .context("clear pending commit")
}

impl PreparedMembershipEvolution {
    pub fn is_membership_change(&self) -> bool {
        !self.added_pubkeys.is_empty() || !self.removed_pubkeys.is_empty() || self.self_removed
    }

    pub fn mark_stale_epoch_conflict(&mut self) {
        self.stale_epoch_conflict = true;
    }

    pub async fn publish_with<F, Fut, Status>(&self, mut publish: F) -> EvolutionPublishStatus
    where
        F: FnMut(Event) -> Fut,
        Fut: Future<Output = Status>,
        Status: IntoEvolutionPublishStatus,
    {
        publish(self.evolution_event.clone())
            .await
            .into_evolution_publish_status()
    }
}
