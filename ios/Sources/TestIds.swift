import Foundation

enum TestIds {
    // Login
    static let loginCreateAccount = "login_create_account"
    static let loginNsecInput = "login_nsec_input"
    static let loginSubmit = "login_submit"

    // Chat list
    static let chatListLogout = "chatlist_logout"
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

    // Group info
    static let groupInfoAddNpub = "groupinfo_add_npub"
    static let groupInfoAddButton = "groupinfo_add_button"
    static let groupInfoLeave = "groupinfo_leave"
}
