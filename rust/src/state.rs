#[derive(uniffi::Record, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct CallTimelineEvent {
    pub id: String,
    pub chat_id: String,
    pub text: String,
    pub timestamp: i64,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct AppState {
    pub rev: u64,
    pub router: Router,
    pub auth: AuthState,
    pub my_profile: MyProfileState,
    pub busy: BusyState,
    pub chat_list: Vec<ChatSummary>,
    pub current_chat: Option<ChatViewState>,
    pub follow_list: Vec<FollowListEntry>,
    pub peer_profile: Option<PeerProfileState>,
    pub active_call: Option<CallState>,
    pub call_timeline: Vec<CallTimelineEvent>,
    pub toast: Option<String>,
}

impl AppState {
    pub fn empty() -> Self {
        Self {
            rev: 0,
            router: Router {
                default_screen: Screen::Login,
                screen_stack: vec![],
            },
            auth: AuthState::LoggedOut,
            my_profile: MyProfileState::empty(),
            busy: BusyState::idle(),
            chat_list: vec![],
            current_chat: None,
            follow_list: vec![],
            peer_profile: None,
            active_call: None,
            call_timeline: vec![],
            toast: None,
        }
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct CallState {
    pub call_id: String,
    pub chat_id: String,
    pub peer_npub: String,
    pub status: CallStatus,
    pub is_live: bool,
    pub should_auto_present_call_screen: bool,
    pub should_enable_proximity_lock: bool,
    pub started_at: Option<i64>,
    pub is_muted: bool,
    pub is_video_call: bool,
    pub is_camera_enabled: bool,
    pub debug: Option<CallDebugStats>,
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum CallStatus {
    Offering,
    Ringing,
    Connecting,
    Active,
    Ended { reason: String },
}

impl CallStatus {
    pub fn is_live(&self) -> bool {
        matches!(
            self,
            Self::Offering | Self::Ringing | Self::Connecting | Self::Active
        )
    }

    pub fn should_auto_present_call_screen(&self) -> bool {
        self.is_live()
    }

    pub fn should_enable_proximity_lock(&self) -> bool {
        matches!(self, Self::Offering | Self::Connecting | Self::Active)
    }
}

impl CallState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        call_id: String,
        chat_id: String,
        peer_npub: String,
        status: CallStatus,
        started_at: Option<i64>,
        is_muted: bool,
        is_video_call: bool,
        debug: Option<CallDebugStats>,
    ) -> Self {
        let should_enable_proximity_lock = if is_video_call {
            false
        } else {
            status.should_enable_proximity_lock()
        };
        Self {
            call_id,
            chat_id,
            peer_npub,
            is_live: status.is_live(),
            should_auto_present_call_screen: status.should_auto_present_call_screen(),
            should_enable_proximity_lock,
            status,
            started_at,
            is_muted,
            is_video_call,
            is_camera_enabled: is_video_call,
            debug,
        }
    }

    pub fn set_status(&mut self, status: CallStatus) {
        self.is_live = status.is_live();
        self.should_auto_present_call_screen = status.should_auto_present_call_screen();
        self.should_enable_proximity_lock = if self.is_video_call {
            false
        } else {
            status.should_enable_proximity_lock()
        };
        self.status = status;
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct CallDebugStats {
    pub tx_frames: u64,
    pub rx_frames: u64,
    pub rx_dropped: u64,
    pub jitter_buffer_ms: u32,
    pub last_rtt_ms: Option<u32>,
    pub video_tx: u64,
    pub video_rx: u64,
    pub video_rx_decrypt_fail: u64,
}

/// "In flight" flags for long-ish operations that the UI should reflect.
///
/// Spec-v1 allows ephemeral UI state to remain native (scroll position, focus, etc),
/// but UX-relevant async operation state should live in Rust to avoid native-side
/// heuristics (e.g., resetting spinners on toast).
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct BusyState {
    pub creating_account: bool,
    pub logging_in: bool,
    pub creating_chat: bool,
    pub fetching_follow_list: bool,
}

impl BusyState {
    pub fn idle() -> Self {
        Self {
            creating_account: false,
            logging_in: false,
            creating_chat: false,
            fetching_follow_list: false,
        }
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct Router {
    pub default_screen: Screen,
    pub screen_stack: Vec<Screen>,
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum Screen {
    Login,
    ChatList,
    Chat { chat_id: String },
    NewChat,
    NewGroupChat,
    GroupInfo { chat_id: String },
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum AuthMode {
    LocalNsec,
    ExternalSigner {
        pubkey: String,
        signer_package: String,
        current_user: String,
    },
    BunkerSigner {
        bunker_uri: String,
    },
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum AuthState {
    LoggedOut,
    LoggedIn {
        npub: String,
        pubkey: String,
        mode: AuthMode,
    },
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct MyProfileState {
    pub name: String,
    pub about: String,
    pub picture_url: Option<String>,
}

impl MyProfileState {
    pub fn empty() -> Self {
        Self {
            name: String::new(),
            about: String::new(),
            picture_url: None,
        }
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MemberInfo {
    pub pubkey: String,
    pub npub: String,
    pub name: Option<String>,
    pub picture_url: Option<String>,
    pub is_admin: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct PeerProfileState {
    pub pubkey: String,
    pub npub: String,
    pub name: Option<String>,
    pub about: Option<String>,
    pub picture_url: Option<String>,
    pub is_followed: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct FollowListEntry {
    pub pubkey: String,
    pub npub: String,
    pub name: Option<String>,
    pub username: Option<String>,
    pub picture_url: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ChatSummary {
    pub chat_id: String,
    pub is_group: bool,
    pub group_name: Option<String>,
    pub members: Vec<MemberInfo>,
    pub last_message: Option<String>,
    pub last_message_at: Option<i64>,
    pub unread_count: u32,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ChatViewState {
    pub chat_id: String,
    pub is_group: bool,
    pub group_name: Option<String>,
    pub members: Vec<MemberInfo>,
    pub is_admin: bool,
    pub messages: Vec<ChatMessage>,
    pub can_load_older: bool,
    pub typing_members: Vec<TypingMember>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct TypingMember {
    pub pubkey: String,
    pub name: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct PollTally {
    pub option: String,
    pub count: u32,
    pub voter_names: Vec<String>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ChatMessage {
    pub id: String,
    pub sender_pubkey: String,
    pub sender_name: Option<String>,
    pub content: String,
    pub display_content: String,
    pub reply_to_message_id: Option<String>,
    pub mentions: Vec<Mention>,
    pub timestamp: i64,
    pub is_mine: bool,
    pub delivery: MessageDeliveryState,
    pub reactions: Vec<ReactionSummary>,
    pub media: Vec<ChatMediaAttachment>,
    pub poll_tally: Vec<PollTally>,
    pub my_poll_vote: Option<String>,
    pub html_state: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ChatMediaAttachment {
    pub original_hash_hex: String,
    pub encrypted_hash_hex: Option<String>,
    pub url: String,
    pub mime_type: String,
    pub filename: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub nonce_hex: String,
    pub scheme_version: String,
    pub local_path: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ReactionSummary {
    pub emoji: String,
    pub count: u32,
    pub reacted_by_me: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct Mention {
    pub npub: String,
    pub display_name: String,
    pub start: u32,
    pub end: u32,
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum MessageDeliveryState {
    Pending,
    Sent,
    Failed { reason: String },
}

pub fn now_seconds() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Scan `content` for `nostr:npub1...` tokens, resolve display names via `lookup`,
/// and return `(display_content, mentions)`.
pub fn resolve_mentions(
    content: &str,
    lookup: &std::collections::HashMap<String, String>,
) -> (String, Vec<Mention>) {
    use nostr_sdk::prelude::PublicKey;

    let mut mentions = Vec::new();
    let mut display = String::with_capacity(content.len());
    let mut rest = content;

    while let Some(pos) = rest.find("nostr:npub1") {
        display.push_str(&rest[..pos]);
        let token_start = pos + "nostr:".len();
        let npub_str = &rest[token_start..];
        let end = npub_str
            .find(|c: char| c.is_whitespace() || c == ',' || c == '.' || c == '!' || c == '?')
            .unwrap_or(npub_str.len());
        let npub = &npub_str[..end];

        let display_name = if let Ok(pk) = PublicKey::parse(npub) {
            let hex = pk.to_hex();
            lookup
                .get(&hex)
                .cloned()
                .unwrap_or_else(|| npub[..npub.len().min(13)].to_string())
        } else {
            npub[..npub.len().min(12)].to_string()
        };

        let mention_label = format!("@{display_name}");
        let start = display.len() as u32;
        let end_pos = start + mention_label.len() as u32;
        display.push_str(&mention_label);

        mentions.push(Mention {
            npub: npub.to_string(),
            display_name: display_name.clone(),
            start,
            end: end_pos,
        });

        rest = &rest[pos + "nostr:".len() + end..];
    }
    display.push_str(rest);

    (display, mentions)
}

#[cfg(test)]
mod tests {
    use super::{CallState, CallStatus};

    #[test]
    fn call_status_proximity_lock_mapping() {
        assert!(CallStatus::Offering.should_enable_proximity_lock());
        assert!(!CallStatus::Ringing.should_enable_proximity_lock());
        assert!(CallStatus::Connecting.should_enable_proximity_lock());
        assert!(CallStatus::Active.should_enable_proximity_lock());
        assert!(!CallStatus::Ended {
            reason: "user_hangup".to_string(),
        }
        .should_enable_proximity_lock());
    }

    #[test]
    fn call_status_live_and_auto_present_mapping() {
        assert!(CallStatus::Offering.is_live());
        assert!(CallStatus::Offering.should_auto_present_call_screen());

        assert!(CallStatus::Ringing.is_live());
        assert!(CallStatus::Ringing.should_auto_present_call_screen());

        assert!(CallStatus::Connecting.is_live());
        assert!(CallStatus::Connecting.should_auto_present_call_screen());

        assert!(CallStatus::Active.is_live());
        assert!(CallStatus::Active.should_auto_present_call_screen());

        let ended = CallStatus::Ended {
            reason: "user_hangup".to_string(),
        };
        assert!(!ended.is_live());
        assert!(!ended.should_auto_present_call_screen());
    }

    #[test]
    fn call_state_set_status_keeps_lifecycle_policy_synced() {
        let mut call = CallState::new(
            "call-1".to_string(),
            "chat-1".to_string(),
            "npub1test".to_string(),
            CallStatus::Offering,
            None,
            false,
            false,
            None,
        );
        assert!(call.is_live);
        assert!(call.should_auto_present_call_screen);
        assert!(call.should_enable_proximity_lock);

        call.set_status(CallStatus::Ringing);
        assert!(call.is_live);
        assert!(call.should_auto_present_call_screen);
        assert!(!call.should_enable_proximity_lock);

        call.set_status(CallStatus::Active);
        assert!(call.is_live);
        assert!(call.should_auto_present_call_screen);
        assert!(call.should_enable_proximity_lock);

        call.set_status(CallStatus::Ended {
            reason: "user_hangup".to_string(),
        });
        assert!(!call.is_live);
        assert!(!call.should_auto_present_call_screen);
        assert!(!call.should_enable_proximity_lock);
    }

    #[test]
    fn video_call_disables_proximity_lock() {
        let call = CallState::new(
            "call-1".to_string(),
            "chat-1".to_string(),
            "npub1test".to_string(),
            CallStatus::Active,
            None,
            false,
            true,
            None,
        );
        assert!(!call.should_enable_proximity_lock);
        assert!(call.is_video_call);
        assert!(call.is_camera_enabled);
    }
}
