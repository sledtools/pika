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
    BunkerSessionDescriptor {
        rev: u64,
        bunker_uri: String,
        client_nsec: String,
    },
}

impl AppUpdate {
    pub fn rev(&self) -> u64 {
        match self {
            AppUpdate::FullState(s) => s.rev,
            AppUpdate::AccountCreated { rev, .. } => *rev,
            AppUpdate::BunkerSessionDescriptor { rev, .. } => *rev,
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
    ChatMediaUploadCompleted {
        request_id: String,
        uploaded_url: Option<String>,
        descriptor_sha256_hex: Option<String>,
        error: Option<String>,
    },
    ChatMediaDownloadFetched {
        request_id: String,
        encrypted_data: Option<Vec<u8>>,
        error: Option<String>,
    },
    KeyPackagePublished {
        ok: bool,
        error: Option<String>,
        event_id: Option<String>,
    },
    PushSubscriptionsSynced {
        groups: Vec<String>,
    },
    PushUnsubscriptionsSynced {
        groups: Vec<String>,
    },
    Toast(String),

    // Async CreateChat fetch result (1:1)
    PeerKeyPackageFetched {
        peer_pubkey: nostr_sdk::prelude::PublicKey,
        key_package_events: Vec<nostr_sdk::prelude::Event>,
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

    // Result of publishing a group evolution event (add/remove/leave/rename commit).
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
        profiles: Vec<(String, Option<String>, i64)>, // (hex_pubkey, metadata_json, event_created_at)
    },

    // Nostr kind:0 profile metadata for the logged-in user.
    MyProfileFetched {
        metadata: Option<nostr_sdk::prelude::Metadata>,
    },
    MyProfileSaved {
        metadata: nostr_sdk::prelude::Metadata,
    },
    MyProfileError {
        message: String,
        toast: bool,
    },

    // Async NIP-46 client-initiated login response became available.
    NostrConnectConnectResponseReady,
    NostrConnectTimeout {
        attempt_id: u64,
    },
    // Rust integration-test hook: inject a validated connect response without relays.
    NostrConnectInjectConnectResponseForTests {
        remote_signer_pubkey: String,
    },

    // Follow list (NIP-02 kind 3)
    FollowListFetched {
        followed_pubkeys: Vec<String>,
        fetched_profiles: Vec<(String, Option<String>, i64)>, // (hex_pubkey, metadata_json, event_created_at)
        checked_pubkeys: std::collections::HashSet<String>,
    },

    // Profile picture downloaded and cached to disk.
    ProfilePicCached {
        pubkey: String,
        url: String,
    },

    // Peer profile fetch result
    PeerProfileFetched {
        pubkey: String,
        metadata_json: Option<String>,
        event_created_at: i64,
    },

    // Contact list modification result
    ContactListModifyFailed {
        pubkey: String,
        revert_to: bool, // revert is_followed to this value
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
        video_tx: u64,
        video_rx: u64,
        video_rx_decrypt_fail: u64,
    },

    // Video frame sent from platform (camera capture → H.264 NALUs).
    VideoFrameFromPlatform {
        payload: Vec<u8>,
    },

    // Multi-device: new key packages for our own pubkey discovered on relays.
    OtherDeviceKeyPackagesFetched {
        key_package_events: Vec<nostr_sdk::prelude::Event>,
    },

    // Multi-device: own key packages fetched from relays for device list display.
    MyDevicesFetched {
        devices: Vec<crate::state::DeviceInfo>,
    },
}
