use anyhow::Result;
use nostr_sdk::prelude::{Event, PublicKey};
use pika_mls::membership::{
    clear_pending_commit as shared_clear_pending_commit,
    validate_key_package_events as shared_validate_key_package_events, IntoEvolutionPublishStatus,
    MembershipRuntime,
};
use pika_mls::prelude::NostrGroupDataUpdate;
use pika_mls::storage_traits::GroupId;

use crate::mdk_support::PikaMdk;

pub(crate) use pika_mls::membership::{
    EvolutionPublishStatus, MembershipUpdateResult, PreparedMembershipEvolution,
};

use super::relay_publish::PublishOutcome;

impl IntoEvolutionPublishStatus for PublishOutcome {
    fn into_evolution_publish_status(self) -> EvolutionPublishStatus {
        match self {
            PublishOutcome::Ok => EvolutionPublishStatus::Published,
            PublishOutcome::Err(err) => EvolutionPublishStatus::PublishFailed(err),
        }
    }
}

pub(crate) fn prepare_add_members(
    mdk: &PikaMdk,
    mls_group_id: &GroupId,
    key_package_events: &[Event],
) -> Result<PreparedMembershipEvolution> {
    MembershipRuntime::new(mdk).prepare_add_members(mls_group_id, key_package_events)
}

pub(crate) fn prepare_remove_members(
    mdk: &PikaMdk,
    mls_group_id: &GroupId,
    removed_pubkeys: &[PublicKey],
) -> Result<PreparedMembershipEvolution> {
    MembershipRuntime::new(mdk).prepare_remove_members(mls_group_id, removed_pubkeys)
}

pub(crate) fn prepare_leave_group(
    mdk: &PikaMdk,
    mls_group_id: &GroupId,
) -> Result<PreparedMembershipEvolution> {
    MembershipRuntime::new(mdk).prepare_leave_group(mls_group_id)
}

pub(crate) fn prepare_group_data_update(
    mdk: &PikaMdk,
    mls_group_id: &GroupId,
    update: NostrGroupDataUpdate,
) -> Result<PreparedMembershipEvolution> {
    MembershipRuntime::new(mdk).prepare_group_data_update(mls_group_id, update)
}

pub(crate) fn validate_key_package_events(
    mdk: &PikaMdk,
    key_package_events: &[Event],
) -> Result<()> {
    shared_validate_key_package_events(mdk, key_package_events)
}

pub(crate) fn clear_pending_commit(mdk: &PikaMdk, mls_group_id: &GroupId) -> Result<()> {
    shared_clear_pending_commit(mdk, mls_group_id)
}

pub(crate) fn finalize_published_evolution(
    mdk: &PikaMdk,
    prepared: PreparedMembershipEvolution,
) -> MembershipUpdateResult {
    MembershipRuntime::new(mdk).finalize_published_evolution(prepared)
}
