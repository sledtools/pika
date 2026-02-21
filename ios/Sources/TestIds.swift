import Foundation

enum TestIds {
    // Login
    static let loginCreateAccount = "login_create_account"
    static let loginNsecInput = "login_nsec_input"
    static let loginSubmit = "login_submit"

    // Chat list
    static let chatListLogout = "chatlist_logout"
    static let chatListLogoutConfirm = "chatlist_logout_confirm"
    static let chatListNewChat = "chatlist_new_chat"
    static let chatListMyNpub = "chatlist_my_npub"
    static let chatListMyNpubValue = "chatlist_my_npub_value"
    static let chatListMyNpubQr = "chatlist_my_npub_qr"
    static let chatListMyNpubCopy = "chatlist_my_npub_copy"
    static let chatListMyNpubClose = "chatlist_my_npub_close"

    // My Profile (nsec export)
    static let myNpubNsecValue = "my_npub_nsec_value"
    static let myNpubNsecToggle = "my_npub_nsec_toggle"
    static let myNpubNsecCopy = "my_npub_nsec_copy"
    static let myProfileAppVersionValue = "my_profile_app_version_value"
    static let myProfileAppVersionCopy = "my_profile_app_version_copy"

    // New chat
    static let newChatPeerNpub = "newchat_peer_npub"
    static let newChatStart = "newchat_start"
    static let newChatScanQr = "newchat_scan_qr"
    static let newChatPaste = "newchat_paste"

    // New group chat
    static let newGroupName = "newgroup_name"
    static let newGroupPeerNpub = "newgroup_peer_npub"
    static let newGroupAddMember = "newgroup_add_member"
    static let newGroupCreate = "newgroup_create"

    // Chat
    static let chatMessageInput = "chat_message_input"
    static let chatSend = "chat_send"
    static let chatGroupInfo = "chat_group_info"
    static let chatReactionBar = "chat_reaction_bar"
    static let chatActionCard = "chat_action_card"
    static let chatActionCopy = "chat_action_copy"

    // Group info
    static let groupInfoAddNpub = "groupinfo_add_npub"
    static let groupInfoAddButton = "groupinfo_add_button"
    static let groupInfoLeave = "groupinfo_leave"
    static let chatCallStart = "chat_call_start"
    static let chatCallOpen = "chat_call_open"
    static let chatCallAccept = "chat_call_accept"
    static let chatCallReject = "chat_call_reject"
    static let chatCallEnd = "chat_call_end"
    static let chatCallMute = "chat_call_mute"
    static let chatCallSpeaker = "chat_call_speaker"
    static let callScreenDismiss = "call_screen_dismiss"
    static let callReturnToCall = "call_return_to_call"
    static let callTimelineEvent = "call_timeline_event"
}
