use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use nostr::{EventId, Kind, PublicKey, RelayUrl, Tags, Timestamp, UnsignedEvent};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GroupId(Vec<u8>);

impl GroupId {
    pub fn from_slice(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.0.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Secret<T>(pub T);

pub mod groups {
    use super::*;

    pub const DEFAULT_MESSAGE_LIMIT: usize = 1000;
    pub const MAX_MESSAGE_LIMIT: usize = 10000;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
    pub enum MessageSortOrder {
        #[default]
        CreatedAtFirst,
        ProcessedAtFirst,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    pub struct Pagination {
        pub limit: Option<usize>,
        pub offset: Option<usize>,
        pub sort_order: Option<MessageSortOrder>,
    }

    impl Pagination {
        pub fn new(limit: Option<usize>, offset: Option<usize>) -> Self {
            Self {
                limit,
                offset,
                sort_order: None,
            }
        }

        pub fn with_sort_order(
            limit: Option<usize>,
            offset: Option<usize>,
            sort_order: MessageSortOrder,
        ) -> Self {
            Self {
                limit,
                offset,
                sort_order: Some(sort_order),
            }
        }

        pub fn limit(&self) -> usize {
            self.limit
                .unwrap_or(DEFAULT_MESSAGE_LIMIT)
                .min(MAX_MESSAGE_LIMIT)
        }

        pub fn offset(&self) -> usize {
            self.offset.unwrap_or(0)
        }

        pub fn sort_order(&self) -> MessageSortOrder {
            self.sort_order.unwrap_or_default()
        }
    }

    impl Default for Pagination {
        fn default() -> Self {
            Self {
                limit: Some(DEFAULT_MESSAGE_LIMIT),
                offset: Some(0),
                sort_order: None,
            }
        }
    }

    pub mod types {
        use super::*;
        use crate::storage_traits::messages::types::Message;

        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub enum SelfUpdateState {
            Required,
            CompletedAt(Timestamp),
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum GroupState {
            Active,
            Inactive,
            Pending,
        }

        impl GroupState {
            pub fn as_str(&self) -> &str {
                match self {
                    Self::Active => "active",
                    Self::Inactive => "inactive",
                    Self::Pending => "pending",
                }
            }
        }

        impl fmt::Display for GroupState {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for GroupState {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    "active" => Ok(Self::Active),
                    "inactive" => Ok(Self::Inactive),
                    "pending" => Ok(Self::Pending),
                    _ => Err(format!("invalid group state: {s}")),
                }
            }
        }

        impl Serialize for GroupState {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for GroupState {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                Self::from_str(&s).map_err(serde::de::Error::custom)
            }
        }

        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct Group {
            pub mls_group_id: GroupId,
            pub nostr_group_id: [u8; 32],
            pub name: String,
            pub description: String,
            pub image_hash: Option<[u8; 32]>,
            pub image_key: Option<Secret<[u8; 32]>>,
            pub image_nonce: Option<Secret<[u8; 12]>>,
            pub admin_pubkeys: BTreeSet<PublicKey>,
            pub last_message_id: Option<EventId>,
            pub last_message_at: Option<Timestamp>,
            pub last_message_processed_at: Option<Timestamp>,
            pub epoch: u64,
            pub state: GroupState,
            pub self_update_state: SelfUpdateState,
        }

        impl Group {
            pub fn update_last_message_if_newer(&mut self, message: &Message) -> bool {
                let should_update = match (
                    self.last_message_at,
                    self.last_message_processed_at,
                    self.last_message_id,
                ) {
                    (None, _, _) => true,
                    (Some(existing_at), Some(existing_processed_at), Some(existing_id)) => {
                        Message::compare_display_keys(
                            message.created_at,
                            message.processed_at,
                            message.id,
                            existing_at,
                            existing_processed_at,
                            existing_id,
                        )
                        .is_gt()
                    }
                    (Some(existing_at), None, _) => message.created_at >= existing_at,
                    (Some(existing_at), Some(_), None) => message.created_at > existing_at,
                };

                if should_update {
                    self.last_message_at = Some(message.created_at);
                    self.last_message_processed_at = Some(message.processed_at);
                    self.last_message_id = Some(message.id);
                }
                should_update
            }
        }

        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct GroupRelay {
            pub relay_url: RelayUrl,
            pub mls_group_id: GroupId,
        }

        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct GroupExporterSecret {
            pub mls_group_id: GroupId,
            pub epoch: u64,
            pub secret: Secret<[u8; 32]>,
        }
    }
}

pub mod messages {
    use super::*;

    pub mod types {
        use super::*;

        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct ProcessedMessage {
            pub wrapper_event_id: EventId,
            pub message_event_id: Option<EventId>,
            pub processed_at: Timestamp,
            pub epoch: Option<u64>,
            pub mls_group_id: Option<GroupId>,
            pub state: ProcessedMessageState,
            pub failure_reason: Option<String>,
        }

        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct Message {
            pub id: EventId,
            pub pubkey: PublicKey,
            pub kind: Kind,
            pub mls_group_id: GroupId,
            pub created_at: Timestamp,
            pub processed_at: Timestamp,
            pub content: String,
            pub tags: Tags,
            pub event: UnsignedEvent,
            pub wrapper_event_id: EventId,
            pub epoch: Option<u64>,
            pub state: MessageState,
        }

        impl Message {
            pub fn display_order_cmp(&self, other: &Self) -> Ordering {
                Self::compare_display_keys(
                    self.created_at,
                    self.processed_at,
                    self.id,
                    other.created_at,
                    other.processed_at,
                    other.id,
                )
            }

            pub fn compare_display_keys(
                a_created_at: Timestamp,
                a_processed_at: Timestamp,
                a_id: EventId,
                b_created_at: Timestamp,
                b_processed_at: Timestamp,
                b_id: EventId,
            ) -> Ordering {
                a_created_at
                    .cmp(&b_created_at)
                    .then_with(|| a_processed_at.cmp(&b_processed_at))
                    .then_with(|| a_id.cmp(&b_id))
            }

            pub fn processed_at_order_cmp(&self, other: &Self) -> Ordering {
                Self::compare_processed_at_keys(
                    self.processed_at,
                    self.created_at,
                    self.id,
                    other.processed_at,
                    other.created_at,
                    other.id,
                )
            }

            pub fn compare_processed_at_keys(
                a_processed_at: Timestamp,
                a_created_at: Timestamp,
                a_id: EventId,
                b_processed_at: Timestamp,
                b_created_at: Timestamp,
                b_id: EventId,
            ) -> Ordering {
                a_processed_at
                    .cmp(&b_processed_at)
                    .then_with(|| a_created_at.cmp(&b_created_at))
                    .then_with(|| a_id.cmp(&b_id))
            }
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum MessageState {
            Created,
            Processed,
            Deleted,
            EpochInvalidated,
        }

        impl MessageState {
            pub fn as_str(&self) -> &str {
                match self {
                    Self::Created => "created",
                    Self::Processed => "processed",
                    Self::Deleted => "deleted",
                    Self::EpochInvalidated => "epoch_invalidated",
                }
            }
        }

        impl fmt::Display for MessageState {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for MessageState {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    "created" => Ok(Self::Created),
                    "processed" => Ok(Self::Processed),
                    "deleted" => Ok(Self::Deleted),
                    "epoch_invalidated" => Ok(Self::EpochInvalidated),
                    _ => Err(format!("invalid message state: {s}")),
                }
            }
        }

        impl Serialize for MessageState {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for MessageState {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                Self::from_str(&s).map_err(serde::de::Error::custom)
            }
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum ProcessedMessageState {
            Created,
            Processed,
            Failed,
            EpochInvalidated,
        }

        impl Serialize for ProcessedMessageState {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                let value = match self {
                    Self::Created => "created",
                    Self::Processed => "processed",
                    Self::Failed => "failed",
                    Self::EpochInvalidated => "epoch_invalidated",
                };
                serializer.serialize_str(value)
            }
        }

        impl<'de> Deserialize<'de> for ProcessedMessageState {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                match String::deserialize(deserializer)?.as_str() {
                    "created" => Ok(Self::Created),
                    "processed" => Ok(Self::Processed),
                    "failed" => Ok(Self::Failed),
                    "epoch_invalidated" => Ok(Self::EpochInvalidated),
                    other => Err(serde::de::Error::custom(format!(
                        "invalid processed message state: {other}"
                    ))),
                }
            }
        }
    }
}

pub mod welcomes {
    use super::*;

    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    pub struct Pagination {
        pub limit: Option<usize>,
        pub offset: Option<usize>,
    }

    impl Pagination {
        pub fn new(limit: Option<usize>, offset: Option<usize>) -> Self {
            Self { limit, offset }
        }
    }

    pub mod types {
        use super::*;

        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct ProcessedWelcome {
            pub wrapper_event_id: EventId,
            pub welcome_event_id: Option<EventId>,
            pub processed_at: Timestamp,
            pub state: ProcessedWelcomeState,
            pub failure_reason: Option<String>,
        }

        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct Welcome {
            pub id: EventId,
            pub event: UnsignedEvent,
            pub mls_group_id: GroupId,
            pub nostr_group_id: [u8; 32],
            pub group_name: String,
            pub group_description: String,
            pub group_image_hash: Option<[u8; 32]>,
            pub group_image_key: Option<Secret<[u8; 32]>>,
            pub group_image_nonce: Option<Secret<[u8; 12]>>,
            pub group_admin_pubkeys: BTreeSet<PublicKey>,
            pub group_relays: BTreeSet<RelayUrl>,
            pub welcomer: PublicKey,
            pub member_count: u32,
            pub state: WelcomeState,
            pub wrapper_event_id: EventId,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum ProcessedWelcomeState {
            Processed,
            Failed,
        }

        impl Serialize for ProcessedWelcomeState {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(match self {
                    Self::Processed => "processed",
                    Self::Failed => "failed",
                })
            }
        }

        impl<'de> Deserialize<'de> for ProcessedWelcomeState {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                match String::deserialize(deserializer)?.as_str() {
                    "processed" => Ok(Self::Processed),
                    "failed" => Ok(Self::Failed),
                    other => Err(serde::de::Error::custom(format!(
                        "invalid processed welcome state: {other}"
                    ))),
                }
            }
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum WelcomeState {
            Pending,
            Accepted,
            Declined,
            Ignored,
        }

        impl WelcomeState {
            pub fn as_str(&self) -> &str {
                match self {
                    Self::Pending => "pending",
                    Self::Accepted => "accepted",
                    Self::Declined => "declined",
                    Self::Ignored => "ignored",
                }
            }
        }

        impl fmt::Display for WelcomeState {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for WelcomeState {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    "pending" => Ok(Self::Pending),
                    "accepted" => Ok(Self::Accepted),
                    "declined" => Ok(Self::Declined),
                    "ignored" => Ok(Self::Ignored),
                    _ => Err(format!("invalid welcome state: {s}")),
                }
            }
        }

        impl Serialize for WelcomeState {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for WelcomeState {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                Self::from_str(&s).map_err(serde::de::Error::custom)
            }
        }
    }
}
