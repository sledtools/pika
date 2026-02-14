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
    RefreshMyProfile,
    SaveMyProfile {
        name: String,
        about: String,
    },
    UploadMyProfileImage {
        image_base64: String,
        mime_type: String,
    },

    // Navigation
    PushScreen {
        screen: Screen,
    },
    UpdateScreenStack {
        stack: Vec<Screen>,
    },

    // Chat (1:1)
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

    // Group chat
    CreateGroupChat {
        peer_npubs: Vec<String>,
        group_name: String,
    },
    AddGroupMembers {
        chat_id: String,
        peer_npubs: Vec<String>,
    },
    RemoveGroupMembers {
        chat_id: String,
        member_pubkeys: Vec<String>,
    },
    LeaveGroup {
        chat_id: String,
    },
    RenameGroup {
        chat_id: String,
        name: String,
    },

    // UI
    ClearToast,

    // Lifecycle
    Foregrounded,

    // Follow list
    RefreshFollowList,
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
            AppAction::RefreshMyProfile => "RefreshMyProfile",
            AppAction::SaveMyProfile { .. } => "SaveMyProfile",
            AppAction::UploadMyProfileImage { .. } => "UploadMyProfileImage",

            // Navigation
            AppAction::PushScreen { .. } => "PushScreen",
            AppAction::UpdateScreenStack { .. } => "UpdateScreenStack",

            // Chat
            AppAction::CreateChat { .. } => "CreateChat",
            AppAction::SendMessage { .. } => "SendMessage",
            AppAction::RetryMessage { .. } => "RetryMessage",
            AppAction::OpenChat { .. } => "OpenChat",
            AppAction::LoadOlderMessages { .. } => "LoadOlderMessages",

            // Group chat
            AppAction::CreateGroupChat { .. } => "CreateGroupChat",
            AppAction::AddGroupMembers { .. } => "AddGroupMembers",
            AppAction::RemoveGroupMembers { .. } => "RemoveGroupMembers",
            AppAction::LeaveGroup { .. } => "LeaveGroup",
            AppAction::RenameGroup { .. } => "RenameGroup",

            // UI
            AppAction::ClearToast => "ClearToast",

            // Lifecycle
            AppAction::Foregrounded => "Foregrounded",

            // Follow list
            AppAction::RefreshFollowList => "RefreshFollowList",
        }
    }
}
