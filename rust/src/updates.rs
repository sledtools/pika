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

    // Async CreateChat fetch result (1:1)
    PeerKeyPackageFetched {
        peer_pubkey: nostr_sdk::prelude::PublicKey,
        candidate_kp_relays: Vec<nostr_sdk::prelude::RelayUrl>,
        key_package_event: Option<nostr_sdk::prelude::Event>,
        error: Option<String>,
    },

    // Async CreateGroupChat: all key packages collected
    GroupKeyPackagesFetched {
        peer_pubkeys: Vec<nostr_sdk::prelude::PublicKey>,
        group_name: String,
        key_package_events: Vec<nostr_sdk::prelude::Event>,
        failed_peers: Vec<(nostr_sdk::prelude::PublicKey, String)>,
        candidate_kp_relays: Vec<nostr_sdk::prelude::RelayUrl>,
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
    SubscriptionsRecomputed {
        token: u64,
        giftwrap_sub: Option<nostr_sdk::prelude::SubscriptionId>,
        group_sub: Option<nostr_sdk::prelude::SubscriptionId>,
    },

    // Nostr kind:0 profile metadata fetched for peers.
    ProfilesFetched {
        profiles: Vec<(String, Option<String>, Option<String>)>, // (hex_pubkey, name, picture_url)
    },
}
