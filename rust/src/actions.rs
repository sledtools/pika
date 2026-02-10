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

    // UI
    ClearToast,

    // Lifecycle
    Foregrounded,
}
