use crate::state::Screen;

#[derive(uniffi::Enum, Debug, Clone)]
pub enum AppAction {
    // Auth
    CreateAccount,
    Login {
        nsec: String,
    },
    RestoreSession {
        nsec: String,
    },
    Logout,

    // Navigation
    PushScreen {
        screen: Screen,
    },
    UpdateScreenStack {
        stack: Vec<Screen>,
    },

    // Chat
    CreateChat {
        peer_npub: String,
    },
    SendMessage {
        chat_id: String,
        content: String,
    },
    RetryMessage {
        chat_id: String,
        message_id: String,
    },
    OpenChat {
        chat_id: String,
    },
    LoadOlderMessages {
        chat_id: String,
        before_message_id: String,
        limit: u32,
    },
    StartCall {
        chat_id: String,
    },
    AcceptCall {
        chat_id: String,
    },
    RejectCall {
        chat_id: String,
    },
    EndCall,
    ToggleMute,

    // UI
    ClearToast,

    // Lifecycle
    Foregrounded,
}

impl AppAction {
    /// Log-safe action tag (never includes secrets like `nsec`).
    pub fn tag(&self) -> &'static str {
        match self {
            // Auth
            AppAction::CreateAccount => "CreateAccount",
            AppAction::Login { .. } => "Login",
            AppAction::RestoreSession { .. } => "RestoreSession",
            AppAction::Logout => "Logout",

            // Navigation
            AppAction::PushScreen { .. } => "PushScreen",
            AppAction::UpdateScreenStack { .. } => "UpdateScreenStack",

            // Chat
            AppAction::CreateChat { .. } => "CreateChat",
            AppAction::SendMessage { .. } => "SendMessage",
            AppAction::RetryMessage { .. } => "RetryMessage",
            AppAction::OpenChat { .. } => "OpenChat",
            AppAction::LoadOlderMessages { .. } => "LoadOlderMessages",
            AppAction::StartCall { .. } => "StartCall",
            AppAction::AcceptCall { .. } => "AcceptCall",
            AppAction::RejectCall { .. } => "RejectCall",
            AppAction::EndCall => "EndCall",
            AppAction::ToggleMute => "ToggleMute",

            // UI
            AppAction::ClearToast => "ClearToast",

            // Lifecycle
            AppAction::Foregrounded => "Foregrounded",
        }
    }
}
