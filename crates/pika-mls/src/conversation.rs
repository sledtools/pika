use std::collections::BTreeSet;

use anyhow::{Context, Result, anyhow};
use nostr::{Event, EventId, Kind, PublicKey, RelayUrl, Timestamp, UnsignedEvent};

use crate::PikaMls;
use crate::prelude::MessageProcessingResult;
use crate::storage_traits::{
    GroupId,
    groups::{Pagination, types::Group},
    messages::types::Message,
};

#[derive(Debug, Clone)]
pub struct WrappedRumor {
    pub rumor_id: EventId,
    pub wrapper: Event,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinedGroupMemberSnapshot {
    pub pubkey: PublicKey,
    pub is_admin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinedGroupSnapshot {
    pub nostr_group_id_hex: String,
    pub mls_group_id: GroupId,
    pub mls_group_id_hex: String,
    pub name: String,
    pub description: String,
    pub relay_urls: Vec<RelayUrl>,
    pub member_snapshots: Vec<JoinedGroupMemberSnapshot>,
    pub last_message_at: Option<Timestamp>,
}

impl JoinedGroupSnapshot {
    pub fn member_count(&self) -> u32 {
        self.member_snapshots.len() as u32
    }

    pub fn other_member_snapshots(
        &self,
        local_pubkey: &PublicKey,
    ) -> Vec<JoinedGroupMemberSnapshot> {
        self.member_snapshots
            .iter()
            .filter(|member| member.pubkey != *local_pubkey)
            .cloned()
            .collect()
    }

    pub fn is_admin(&self, pubkey: &PublicKey) -> bool {
        self.member_snapshots
            .iter()
            .any(|member| member.pubkey == *pubkey && member.is_admin)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct MessagePageQuery {
    pub limit: usize,
    pub offset: usize,
}

impl MessagePageQuery {
    pub const fn new(limit: usize, offset: usize) -> Self {
        Self { limit, offset }
    }
}

#[derive(Debug, Clone)]
pub struct MessagePage {
    pub nostr_group_id_hex: String,
    pub mls_group_id: GroupId,
    pub messages: Vec<Message>,
    pub fetched_count: usize,
    pub next_offset: usize,
    pub storage_exhausted: bool,
}

pub struct ConversationQueries<'a> {
    mls: &'a PikaMls,
}

impl<'a> ConversationQueries<'a> {
    pub fn new(mls: &'a PikaMls) -> Self {
        Self { mls }
    }

    pub fn find_group(&self, nostr_group_id_hex: &str) -> Result<Group> {
        let group_id_bytes =
            hex::decode(nostr_group_id_hex).map_err(|_| anyhow!("nostr_group_id must be hex"))?;
        if group_id_bytes.len() != 32 {
            anyhow::bail!("nostr_group_id must be 32 bytes hex");
        }
        self.mls
            .inner
            .get_groups()
            .context("get_groups")?
            .into_iter()
            .find(|group| group.nostr_group_id.as_slice() == group_id_bytes.as_slice())
            .ok_or_else(|| anyhow!("group not found"))
    }

    pub fn mls_group_id_for_nostr_group_id(&self, nostr_group_id_hex: &str) -> Result<GroupId> {
        Ok(self.find_group(nostr_group_id_hex)?.mls_group_id)
    }

    pub fn nostr_group_id_hex(&self, mls_group_id: &GroupId) -> Result<Option<String>> {
        Ok(self
            .mls
            .inner
            .get_group(mls_group_id)?
            .map(|group| hex::encode(group.nostr_group_id)))
    }

    pub fn get_group(&self, mls_group_id: &GroupId) -> Result<Option<Group>> {
        self.mls.inner.get_group(mls_group_id).context("get group")
    }

    pub fn get_members(&self, mls_group_id: &GroupId) -> Result<BTreeSet<PublicKey>> {
        self.mls
            .inner
            .get_members(mls_group_id)
            .context("get members")
    }

    pub fn get_relays(&self, mls_group_id: &GroupId) -> Result<BTreeSet<RelayUrl>> {
        self.mls
            .inner
            .get_relays(mls_group_id)
            .context("get relays")
    }

    pub fn get_message(
        &self,
        mls_group_id: &GroupId,
        message_id: &EventId,
    ) -> Result<Option<Message>> {
        self.mls
            .inner
            .get_message(mls_group_id, message_id)
            .context("get message")
    }

    pub fn lookup_joined_group_snapshot(
        &self,
        nostr_group_id_hex: &str,
    ) -> Result<JoinedGroupSnapshot> {
        self.joined_group_snapshot(self.find_group(nostr_group_id_hex)?)
    }

    pub fn list_joined_group_snapshots(&self) -> Result<Vec<JoinedGroupSnapshot>> {
        let groups = self.mls.inner.get_groups().context("get_groups")?;
        groups
            .into_iter()
            .map(|group| self.joined_group_snapshot(group))
            .collect()
    }

    pub fn get_messages(
        &self,
        nostr_group_id_hex: &str,
        pagination: Option<Pagination>,
    ) -> Result<Vec<Message>> {
        let mls_group_id = self.mls_group_id_for_nostr_group_id(nostr_group_id_hex)?;
        self.mls
            .inner
            .get_messages(&mls_group_id, pagination)
            .context("get messages")
    }

    pub fn load_message_page(
        &self,
        nostr_group_id_hex: &str,
        query: MessagePageQuery,
    ) -> Result<MessagePage> {
        let mls_group_id = self.mls_group_id_for_nostr_group_id(nostr_group_id_hex)?;
        let messages = self
            .mls
            .inner
            .get_messages(
                &mls_group_id,
                Some(Pagination::new(Some(query.limit), Some(query.offset))),
            )
            .context("get message page")?;
        let fetched_count = messages.len();
        Ok(MessagePage {
            nostr_group_id_hex: nostr_group_id_hex.to_string(),
            mls_group_id,
            messages,
            fetched_count,
            next_offset: query.offset + fetched_count,
            storage_exhausted: fetched_count < query.limit,
        })
    }

    pub fn find_direct_message_group(
        &self,
        local_pubkey: &PublicKey,
        peer_pubkey: &PublicKey,
    ) -> Result<Option<Group>> {
        let groups = self.mls.inner.get_groups().context("get_groups")?;
        Ok(groups.into_iter().find(|group| {
            let members = self.get_members(&group.mls_group_id).unwrap_or_default();
            let others: Vec<_> = members
                .iter()
                .filter(|pubkey| *pubkey != local_pubkey)
                .collect();
            others.len() == 1 && *others[0] == *peer_pubkey
        }))
    }

    pub fn find_message_across_groups(
        &self,
        message_id: &EventId,
    ) -> Result<Option<(GroupId, Message)>> {
        let groups = self.mls.inner.get_groups().context("get_groups")?;
        for group in groups {
            if let Some(message) = self.get_message(&group.mls_group_id, message_id)? {
                return Ok(Some((group.mls_group_id, message)));
            }
        }
        Ok(None)
    }

    fn joined_group_snapshot(&self, group: Group) -> Result<JoinedGroupSnapshot> {
        let admin_pubkeys = group.admin_pubkeys.clone();
        let mls_group_id = group.mls_group_id.clone();
        let member_snapshots = self
            .mls
            .inner
            .get_members(&mls_group_id)
            .unwrap_or_default();
        let relay_urls = self
            .mls
            .inner
            .get_relays(&mls_group_id)
            .unwrap_or_default()
            .into_iter()
            .collect();
        let member_snapshots = member_snapshots
            .into_iter()
            .map(|pubkey| JoinedGroupMemberSnapshot {
                is_admin: admin_pubkeys.contains(&pubkey),
                pubkey,
            })
            .collect();
        Ok(JoinedGroupSnapshot {
            nostr_group_id_hex: hex::encode(group.nostr_group_id),
            mls_group_id_hex: hex::encode(group.mls_group_id.as_slice()),
            name: group.name,
            description: group.description,
            relay_urls,
            member_snapshots,
            last_message_at: group.last_message_at,
            mls_group_id,
        })
    }
}

pub fn wrap_rumor(
    mls: &PikaMls,
    mls_group_id: &GroupId,
    mut rumor: UnsignedEvent,
) -> Result<WrappedRumor> {
    rumor.ensure_id();
    let rumor_id = rumor.id();
    let wrapper = mls
        .inner
        .create_message(mls_group_id, rumor)
        .context("create group wrapper")?;
    Ok(WrappedRumor { rumor_id, wrapper })
}

pub fn process_group_message_event(
    mls: &PikaMls,
    event: &Event,
) -> Result<Option<MessageProcessingResult>> {
    if event.kind != Kind::MlsGroupMessage {
        return Ok(None);
    }
    mls.inner
        .process_message(event)
        .context("process group message")
        .map(Some)
}
