use anyhow::{Context, Result};
use nostr::{EventId, PublicKey, RelayUrl, Timestamp};

use crate::PikaMdk;
use crate::storage_traits::welcomes::types::Welcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWelcomeSnapshot {
    pub wrapper_event_id: EventId,
    pub welcome_event_id: EventId,
    pub welcomer: PublicKey,
    pub created_at: Timestamp,
    pub nostr_group_id_hex: String,
    pub mls_group_id: crate::storage_traits::GroupId,
    pub group_name: String,
    pub group_description: String,
    pub member_count: u32,
    pub group_relays: Vec<RelayUrl>,
}

impl PendingWelcomeSnapshot {
    fn from_welcome(welcome: &Welcome) -> Self {
        Self {
            wrapper_event_id: welcome.wrapper_event_id,
            welcome_event_id: welcome.id,
            welcomer: welcome.welcomer,
            created_at: welcome.event.created_at,
            nostr_group_id_hex: hex::encode(welcome.nostr_group_id),
            mls_group_id: welcome.mls_group_id.clone(),
            group_name: welcome.group_name.clone(),
            group_description: welcome.group_description.clone(),
            member_count: welcome.member_count,
            group_relays: welcome.group_relays.iter().cloned().collect(),
        }
    }
}

fn pending_welcome_matches_event_id(welcome: &Welcome, target: &EventId) -> bool {
    welcome.wrapper_event_id == *target || welcome.id == *target
}

pub fn find_pending_welcome<'a>(welcomes: &'a [Welcome], target: &EventId) -> Option<&'a Welcome> {
    welcomes
        .iter()
        .find(|welcome| pending_welcome_matches_event_id(welcome, target))
}

pub fn find_pending_welcome_index(welcomes: &[Welcome], target: &EventId) -> Option<usize> {
    welcomes
        .iter()
        .position(|welcome| pending_welcome_matches_event_id(welcome, target))
}

pub fn take_pending_welcome(welcomes: &mut Vec<Welcome>, target: &EventId) -> Option<Welcome> {
    find_pending_welcome_index(welcomes, target).map(|idx| welcomes.swap_remove(idx))
}

pub struct WelcomeQueries<'a> {
    mdk: &'a PikaMdk,
}

impl<'a> WelcomeQueries<'a> {
    pub fn new(mdk: &'a PikaMdk) -> Self {
        Self { mdk }
    }

    pub fn list_pending_welcome_snapshots(&self) -> Result<Vec<PendingWelcomeSnapshot>> {
        Ok(self
            .mdk
            .get_pending_welcomes(None)
            .context("get pending welcomes")?
            .iter()
            .map(PendingWelcomeSnapshot::from_welcome)
            .collect())
    }

    pub fn lookup_pending_welcome(&self, target: &EventId) -> Result<Option<Welcome>> {
        let pending = self
            .mdk
            .get_pending_welcomes(None)
            .context("get pending welcomes")?;
        Ok(find_pending_welcome(&pending, target).cloned())
    }
}
