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
    WipeLocalData,
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
        kind: Option<u16>,
        reply_to_message_id: Option<String>,
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

    // Chat management
    ArchiveChat {
        chat_id: String,
    },
    ReactToMessage {
        chat_id: String,
        message_id: String,
        emoji: String,
    },
    TypingStarted {
        chat_id: String,
    },

    // UI
    ClearToast,

    // Lifecycle
    Foregrounded,
    ReloadConfig,

    // Peer profile
    OpenPeerProfile {
        pubkey: String,
    },
    ClosePeerProfile,

    // Push notifications
    SetPushToken {
        token: String,
    },
    ReregisterPush,

    // Follow list
    RefreshFollowList,
    FollowUser {
        pubkey: String,
    },
    UnfollowUser {
        pubkey: String,
    },
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
            AppAction::WipeLocalData => "WipeLocalData",
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
            AppAction::StartCall { .. } => "StartCall",
            AppAction::AcceptCall { .. } => "AcceptCall",
            AppAction::RejectCall { .. } => "RejectCall",
            AppAction::EndCall => "EndCall",
            AppAction::ToggleMute => "ToggleMute",

            // Group chat
            AppAction::CreateGroupChat { .. } => "CreateGroupChat",
            AppAction::AddGroupMembers { .. } => "AddGroupMembers",
            AppAction::RemoveGroupMembers { .. } => "RemoveGroupMembers",
            AppAction::LeaveGroup { .. } => "LeaveGroup",
            AppAction::RenameGroup { .. } => "RenameGroup",

            // Chat management
            AppAction::ArchiveChat { .. } => "ArchiveChat",
            AppAction::ReactToMessage { .. } => "ReactToMessage",
            AppAction::TypingStarted { .. } => "TypingStarted",

            // UI
            AppAction::ClearToast => "ClearToast",

            // Lifecycle
            AppAction::Foregrounded => "Foregrounded",
            AppAction::ReloadConfig => "ReloadConfig",

            // Peer profile
            AppAction::OpenPeerProfile { .. } => "OpenPeerProfile",
            AppAction::ClosePeerProfile => "ClosePeerProfile",

            // Push notifications
            AppAction::SetPushToken { .. } => "SetPushToken",
            AppAction::ReregisterPush => "ReregisterPush",

            // Follow list
            AppAction::RefreshFollowList => "RefreshFollowList",
            AppAction::FollowUser { .. } => "FollowUser",
            AppAction::UnfollowUser { .. } => "UnfollowUser",
        }
    }
}
