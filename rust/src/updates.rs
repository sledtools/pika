use crate::state::AppState;
use crate::AppAction;

#[derive(uniffi::Enum, Clone, Debug)]
#[allow(clippy::large_enum_variant)] // uniffi enums cannot use Box<T> indirection
pub enum AppUpdate {
    /// Primary update stream: always send a full state snapshot.
    ///
    /// MVP tradeoff: simplest reconciliation story on iOS/Android; can be made more granular later.
    FullState(AppState),
    AccountCreated {
        rev: u64,
        nsec: String,
        pubkey: String,
        npub: String,
    },
}

impl AppUpdate {
    pub fn rev(&self) -> u64 {
        match self {
            AppUpdate::FullState(s) => s.rev,
            AppUpdate::AccountCreated { rev, .. } => *rev,
        }
    }
}

#[derive(Debug)]
pub enum CoreMsg {
    Action(AppAction),
    Internal(Box<InternalEvent>),
}

#[derive(Debug)]
pub enum InternalEvent {
    // Nostr receive path
    GiftWrapReceived {
        wrapper: nostr_sdk::prelude::Event,
        rumor: nostr_sdk::prelude::UnsignedEvent,
    },
    GroupMessageReceived {
        event: nostr_sdk::prelude::Event,
    },

    // Async results
    PublishMessageResult {
        chat_id: String,
        rumor_id: String,
        ok: bool,
        error: Option<String>,
    },
    KeyPackagePublished {
        ok: bool,
        error: Option<String>,
    },
    Toast(String),

    // Async CreateChat fetch result
    PeerKeyPackageFetched {
        peer_pubkey: nostr_sdk::prelude::PublicKey,
<<<<<<< HEAD
        // Relays we used (or discovered via kind 10051) when fetching the peer's key package.
        // These are valuable as an interop baseline: if the peer published their key package
        // there, they almost certainly have connectivity to them, so using them for the new
        // group's relay set increases the chance of immediate bidirectional message delivery.
        candidate_kp_relays: Vec<nostr_sdk::prelude::RelayUrl>,
=======
>>>>>>> b36ed4f (Remove NIP-42/NIP-70 and separate key-package relay infrastructure)
        key_package_event: Option<nostr_sdk::prelude::Event>,
        error: Option<String>,
    },

<<<<<<< HEAD
    // Subscription recompute result. Kept internal because it carries nostr-sdk types.
=======
    // Async CreateGroupChat: all key packages collected
    GroupKeyPackagesFetched {
        peer_pubkeys: Vec<nostr_sdk::prelude::PublicKey>,
        group_name: String,
        key_package_events: Vec<nostr_sdk::prelude::Event>,
        failed_peers: Vec<(nostr_sdk::prelude::PublicKey, String)>,
    },

    // Result of publishing a group evolution event (add/remove/leave/rename commit)
    GroupEvolutionPublished {
        chat_id: String,
        mls_group_id: mdk_core::prelude::GroupId,
        welcome_rumors: Option<Vec<nostr_sdk::prelude::UnsignedEvent>>,
        added_pubkeys: Vec<nostr_sdk::prelude::PublicKey>,
        ok: bool,
        error: Option<String>,
    },

    // Subscription recompute result.
>>>>>>> b36ed4f (Remove NIP-42/NIP-70 and separate key-package relay infrastructure)
    SubscriptionsRecomputed {
        token: u64,
        giftwrap_sub: Option<nostr_sdk::prelude::SubscriptionId>,
        group_sub: Option<nostr_sdk::prelude::SubscriptionId>,
    },

    // Nostr kind:0 profile metadata fetched for peers.
    ProfilesFetched {
        profiles: Vec<(String, Option<String>, Option<String>)>, // (hex_pubkey, name, picture_url)
    },

    // Synthetic media runtime updates (Phase-1 plumbing).
    CallRuntimeConnected {
        call_id: String,
    },
    CallRuntimeStats {
        call_id: String,
        tx_frames: u64,
        rx_frames: u64,
        rx_dropped: u64,
        jitter_buffer_ms: u32,
        last_rtt_ms: Option<u32>,
    },
}
