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
    pub timestamp: i64,
    pub is_mine: bool,
    pub delivery: MessageDeliveryState,
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
