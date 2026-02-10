use crate::state::{AppState, AuthState, ChatSummary, ChatViewState, Router};
use crate::AppAction;

#[derive(uniffi::Enum, Clone, Debug)]
pub enum AppUpdate {
    FullState(AppState),
    AccountCreated {
        rev: u64,
        nsec: String,
        pubkey: String,
        npub: String,
    },
    RouterChanged {
        rev: u64,
        router: Router,
    },
    AuthChanged {
        rev: u64,
        auth: AuthState,
    },
    ChatListChanged {
        rev: u64,
        chat_list: Vec<ChatSummary>,
    },
    CurrentChatChanged {
        rev: u64,
        current_chat: Option<ChatViewState>,
    },
    ToastChanged {
        rev: u64,
        toast: Option<String>,
    },
}

impl AppUpdate {
    pub fn rev(&self) -> u64 {
        match self {
            AppUpdate::FullState(s) => s.rev,
            AppUpdate::AccountCreated { rev, .. } => *rev,
            AppUpdate::RouterChanged { rev, .. } => *rev,
            AppUpdate::AuthChanged { rev, .. } => *rev,
            AppUpdate::ChatListChanged { rev, .. } => *rev,
            AppUpdate::CurrentChatChanged { rev, .. } => *rev,
            AppUpdate::ToastChanged { rev, .. } => *rev,
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
        // Relays we used (or discovered via kind 10051) when fetching the peer's key package.
        // These are valuable as an interop baseline: if the peer published their key package
        // there, they almost certainly have connectivity to them, so using them for the new
        // group's relay set increases the chance of immediate bidirectional message delivery.
        candidate_kp_relays: Vec<nostr_sdk::prelude::RelayUrl>,
        key_package_event: Option<nostr_sdk::prelude::Event>,
        error: Option<String>,
    },

    // Subscription recompute result. Kept internal because it carries nostr-sdk types.
    SubscriptionsRecomputed {
        token: u64,
        giftwrap_sub: Option<nostr_sdk::prelude::SubscriptionId>,
        group_sub: Option<nostr_sdk::prelude::SubscriptionId>,
    },
}
