#[derive(uniffi::Record, Clone, Debug)]
pub struct AppState {
    pub rev: u64,
    pub router: Router,
    pub auth: AuthState,
    pub busy: BusyState,
    pub chat_list: Vec<ChatSummary>,
    pub current_chat: Option<ChatViewState>,
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
            busy: BusyState::idle(),
            chat_list: vec![],
            current_chat: None,
            toast: None,
        }
    }
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct BusyState {
    pub creating_account: bool,
    pub logging_in: bool,
    pub creating_chat: bool,
}

impl BusyState {
    pub fn idle() -> Self {
        Self {
            creating_account: false,
            logging_in: false,
            creating_chat: false,
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

#[derive(uniffi::Enum, Clone, Debug)]
pub enum AuthState {
    LoggedOut,
    LoggedIn { npub: String, pubkey: String },
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MemberInfo {
    pub pubkey: String,
    pub npub: String,
    pub name: Option<String>,
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
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ChatMessage {
    pub id: String,
    pub sender_pubkey: String,
    pub sender_name: Option<String>,
    pub content: String,
    pub display_content: String,
    pub mentions: Vec<Mention>,
    pub timestamp: i64,
    pub is_mine: bool,
    pub delivery: MessageDeliveryState,
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
                .unwrap_or_else(|| hex[..8].to_string())
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
