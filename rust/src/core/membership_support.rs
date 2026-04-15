use anyhow::Result;
use nostr_sdk::prelude::{Event, PublicKey, UnsignedEvent};
use pika_mls::membership::{IntoEvolutionPublishStatus, MembershipRuntime};
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

pub(crate) fn prepare_evolution(
    mdk: &PikaMdk,
    mls_group_id: GroupId,
    evolution_event: Event,
    welcome_rumors: Option<Vec<UnsignedEvent>>,
    added_pubkeys: Vec<PublicKey>,
) -> Result<PreparedMembershipEvolution> {
    MembershipRuntime::new(mdk).prepare_evolution(
        mls_group_id,
        evolution_event,
        welcome_rumors,
        added_pubkeys,
    )
}

pub(crate) fn finalize_published_evolution(
    mdk: &PikaMdk,
    prepared: PreparedMembershipEvolution,
) -> MembershipUpdateResult {
    MembershipRuntime::new(mdk).finalize_published_evolution(prepared)
}
