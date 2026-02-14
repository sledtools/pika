---
summary: Group chat implementation plan covering state model, actions, group creation/management, and iOS UI
read_when:
  - working on group chat features
  - modifying group creation or management flows
  - updating group-related UI views
---

# Group Chat Implementation Plan

## Status: Phase 1-6 Complete (Initial Implementation)

All Rust core and iOS UI changes are implemented and compiling. The implementation:
- Generalizes the state model from 1:1 to N-member groups
- Adds actions for group creation, add/remove members, leave, rename
- Parallel key package fetching for multi-peer group creation
- Evolution event publishing with MIP-02 timing (relay confirm before Welcome)
- iOS UI: ChatListView, ChatView (sender names), NewGroupChatView, GroupInfoView
- 1:1 chats continue to work as before (they're just 2-member groups)

Remaining: device testing, E2E testing, Android parity.

## Overview

Pika now supports both 1:1 DM-style chats and multi-member group chats. The underlying protocol (Marmot/MLS via MDK) provides all group operations. This document covers what MDK provides, the implementation approach, and the detailed task list.

## What MDK Already Provides

MDK's API surface covers everything needed for group management. No changes to MDK are required.

### Group Lifecycle

| API | Description |
|-----|-------------|
| `create_group(creator_pubkey, key_package_events, config)` | Creates a group with N members. Already accepts a `Vec<Event>` of key packages -- works for 0, 1, or N members. |
| `add_members(group_id, key_package_events)` | Adds members to an existing group. Returns `UpdateGroupResult` with evolution_event (kind 445 Commit) + welcome_rumors (kind 444). |
| `remove_members(group_id, pubkeys)` | Removes members. Admin-only. Returns evolution_event. |
| `leave_group(group_id)` | Current user leaves. Returns evolution_event. |
| `self_update(group_id)` | Rotates own signing key. Any member can do this. |
| `update_group_data(group_id, update)` | Updates group name, description, relays, admins, image. Admin-only. |

### Group Queries

| API | Description |
|-----|-------------|
| `get_groups()` | Returns all groups. Each `Group` has `mls_group_id`, `nostr_group_id`, `name`, `description`, `admin_pubkeys`. |
| `get_group(group_id)` | Single group lookup. |
| `get_members(group_id)` | Returns `BTreeSet<PublicKey>` of all current members. |
| `get_relays(group_id)` | Returns relay URLs for the group. |
| `get_messages(group_id, pagination)` | Returns messages. Each message has `pubkey` (sender), `content`, `created_at`, `id`. |

### Message Flow

| API | Description |
|-----|-------------|
| `create_message(mls_group_id, rumor)` | Encrypts a message for the group. Works identically for 1:1 and N-member groups. |
| `process_message(event)` | Decrypts incoming kind 445 events. Returns `MessageProcessingResult` with variants for app messages, proposals, commits. |
| `process_welcome(event_id, rumor)` | Processes kind 444 Welcome for group invitations. |
| `accept_welcome(welcome)` | Accepts a pending welcome, joining the group. |

### Key Takeaway

MDK treats 1:1 chats and group chats identically -- a 1:1 is just a 2-member group. The entire limitation is in Pika's Rust core and UI layers.

## Current Architecture (1:1 Only)

### Rust Core (`rust/src/`)

**State model** (`state.rs`):
```rust
// Every chat is modeled as having exactly one peer
struct ChatSummary {
    chat_id: String,
    peer_npub: String,        // singular
    peer_name: Option<String>,
    peer_picture_url: Option<String>,
    last_message: Option<String>,
    last_message_at: Option<i64>,
    unread_count: u32,
}

struct ChatViewState {
    chat_id: String,
    peer_npub: String,        // singular
    peer_name: Option<String>,
    peer_picture_url: Option<String>,
    messages: Vec<ChatMessage>,
    can_load_older: bool,
}

struct ChatMessage {
    id: String,
    sender_pubkey: String,
    content: String,
    timestamp: i64,
    is_mine: bool,            // binary: mine or theirs
    delivery: MessageDeliveryState,
}
```

**Actions** (`actions.rs`):
```rust
enum AppAction {
    CreateChat { peer_npub: String },  // single peer only
    // no AddMember, RemoveMember, LeaveGroup, RenameGroup, etc.
}
```

**Group index** (`core/mod.rs`):
```rust
struct GroupIndexEntry {
    mls_group_id: GroupId,
    peer_npub: String,        // singular
    peer_name: Option<String>,
    peer_picture_url: Option<String>,
}
```

**Storage refresh** (`core/storage.rs`):
- `refresh_chat_list_from_storage()` calls `get_members()` but only picks the first non-self member
- `refresh_current_chat()` uses singular peer fields
- Profile fetching only fetches one peer per group

### iOS UI (`ios/Sources/`)

- `ChatListView`: Shows one peer name/avatar per row
- `ChatView`: Messages are just "mine" (blue, right) or "theirs" (gray, left) -- no sender identification
- `NewChatView`: Single npub text field
- `ViewState.swift`: `ChatListViewState`, `ChatScreenState` mirror the singular-peer Rust state

### What Works Without Changes

- **Subscriptions** (`core/session.rs`): Already subscribes to `#h` tags per group -- multi-member groups will just work.
- **Notifications loop**: Handles kind 445 (group messages) and kind 1059 (gift wrap / welcomes) generically.
- **Message send**: `create_message()` encrypts for the MLS group regardless of member count.
- **Message receive**: `process_message()` handles any group size.

## Implementation Plan

### Phase 1: Generalize Rust State Model

Replace singular peer fields with member-aware structures throughout the state layer.

#### 1.1 Update `state.rs`

```rust
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
    pub members: Vec<MemberInfo>,     // all members except self
    pub last_message: Option<String>,
    pub last_message_at: Option<i64>,
    pub unread_count: u32,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ChatViewState {
    pub chat_id: String,
    pub is_group: bool,
    pub group_name: Option<String>,
    pub members: Vec<MemberInfo>,     // all members except self
    pub is_admin: bool,               // can current user manage members?
    pub messages: Vec<ChatMessage>,
    pub can_load_older: bool,
}

// ChatMessage gains sender info for groups
#[derive(uniffi::Record, Clone, Debug)]
pub struct ChatMessage {
    pub id: String,
    pub sender_pubkey: String,
    pub sender_name: Option<String>,  // NEW: resolved display name
    pub content: String,
    pub timestamp: i64,
    pub is_mine: bool,
    pub delivery: MessageDeliveryState,
}
```

#### 1.2 Update `GroupIndexEntry` in `core/mod.rs`

```rust
struct GroupIndexEntry {
    mls_group_id: GroupId,
    is_group: bool,
    group_name: Option<String>,
    members: Vec<(PublicKey, Option<String>, Option<String>)>, // (pubkey, name, picture_url)
    admin_pubkeys: Vec<String>,
}
```

#### 1.3 Update `core/storage.rs`

- `refresh_chat_list_from_storage()`: Build `MemberInfo` vec from `get_members()`, resolve all member profiles from cache, request missing profiles for all members (not just one peer).
- `refresh_current_chat()`: Populate `sender_name` on each `ChatMessage` by looking up `sender_pubkey` in the profile cache.
- Determine `is_group` by checking member count (>2 members or group name != "DM").

### Phase 2: Add Group Chat Actions

#### 2.1 New actions in `actions.rs`

```rust
enum AppAction {
    // Existing
    CreateChat { peer_npub: String },
    
    // New
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
    
    // ... existing actions unchanged
}
```

#### 2.2 New internal events in `updates.rs`

```rust
enum InternalEvent {
    // Existing events unchanged...
    
    // New: collecting key packages for multiple peers
    GroupKeyPackagesFetched {
        peer_pubkeys: Vec<PublicKey>,
        group_name: String,
        key_package_events: Vec<Event>,       // successfully fetched
        failed_peers: Vec<(PublicKey, String)>, // (pubkey, error)
        candidate_kp_relays: Vec<RelayUrl>,
    },
    
    // New: result of add/remove/leave operations
    GroupMembershipChanged {
        chat_id: String,
        evolution_event: Event,
        welcome_rumors: Option<Vec<UnsignedEvent>>,
        added_pubkeys: Vec<PublicKey>,
    },
    
    GroupUpdatePublished {
        chat_id: String,
        ok: bool,
        error: Option<String>,
    },
}
```

### Phase 3: Group Creation Flow

#### 3.1 Multi-peer key package fetch (`core/mod.rs`)

The current flow fetches one peer's key package, then creates the group in `PeerKeyPackageFetched`. For groups, we need to:

1. On `CreateGroupChat` action: spawn async tasks to fetch key packages for all peers in parallel.
2. Collect results. If some peers fail, show which ones failed and let the user decide.
3. Once all (or accepted subset) are collected, call `mdk.create_group()` with all key package events.

```
CreateGroupChat { peer_npubs, group_name }
  -> spawn N parallel key package fetches
  -> GroupKeyPackagesFetched { all results }
  -> mdk.create_group(creator, all_kp_events, config)
  -> publish welcomes to each peer
  -> refresh UI, navigate to chat
```

#### 3.2 Config for groups

```rust
let config = NostrGroupConfigData {
    name: group_name,  // user-provided, not "DM"
    description: String::new(),
    image_hash: None,
    image_key: None,
    image_nonce: None,
    relays: group_relays,
    admins: vec![my_pubkey], // creator is admin; can add others later
};
```

### Phase 4: Group Management Actions

#### 4.1 Add members to existing group

```
AddGroupMembers { chat_id, peer_npubs }
  -> fetch key packages for new peers
  -> mdk.add_members(mls_group_id, kp_events) -> UpdateGroupResult
  -> publish evolution_event to group relays (kind 445 Commit)
  -> wait for relay confirmation
  -> mdk.merge_pending_commit(mls_group_id)
  -> publish welcome_rumors to new peers (gift-wrapped kind 444)
  -> refresh UI
```

**Important**: Per MIP-02/MIP-03, the Commit MUST be confirmed by relays BEFORE sending Welcomes to prevent state forks.

#### 4.2 Remove members

```
RemoveGroupMembers { chat_id, member_pubkeys }
  -> mdk.remove_members(mls_group_id, pubkeys) -> UpdateGroupResult
  -> publish evolution_event to group relays
  -> wait for relay confirmation
  -> mdk.merge_pending_commit(mls_group_id)
  -> refresh UI
```

#### 4.3 Leave group

```
LeaveGroup { chat_id }
  -> mdk.leave_group(mls_group_id) -> UpdateGroupResult
  -> publish evolution_event to group relays
  -> navigate back to chat list
  -> refresh UI
```

#### 4.4 Rename group

```
RenameGroup { chat_id, name }
  -> mdk.update_group_data(mls_group_id, NostrGroupDataUpdate { name: Some(name), .. })
  -> publish evolution_event
  -> wait for relay confirmation
  -> mdk.merge_pending_commit(mls_group_id)
  -> refresh UI
```

### Phase 5: Handle Incoming Group Changes

Currently `InternalEvent::GroupMessageReceived` handles `MessageProcessingResult` variants. Most already work for groups, but we need to handle membership changes:

- `MessageProcessingResult::Commit { mls_group_id }`: A commit was processed -- member list or group metadata may have changed. Re-fetch members and group data from MDK, update `GroupIndexEntry`, refresh UI.
- `MessageProcessingResult::Proposal { .. }`: A proposal was queued. Could display a UI indicator for pending changes (optional, can defer).

Key change: after processing any Commit, call `refresh_all_from_storage()` so member lists and group names update.

### Phase 6: iOS UI Changes

#### 6.1 Update `ViewState.swift`

The Rust `ChatSummary`, `ChatViewState`, `ChatMessage`, and `MemberInfo` are UniFFI-generated -- Swift types update automatically when the Rust types change. Update `ViewState.swift` mappers if needed.

#### 6.2 Update `ChatListView.swift`

- For groups (`chat.isGroup`): show group name, member count, group avatar (initials or member composite)
- For 1:1: show peer name/avatar as before (use `members[0]`)
- Sort order unchanged (by `last_message_at`)

#### 6.3 Update `ChatView.swift`

- For groups: show sender name above each message bubble (for messages that aren't mine)
- Different colors or subtle label per sender
- Navigation title: group name (tappable to show group info)
- Add group info sheet/screen accessible from nav bar

#### 6.4 Create `NewGroupChatView.swift`

New view for creating group chats:
- Text field for group name
- Add multiple npubs (text field + scan QR, add to list, remove from list)
- "Create Group" button
- Loading state while fetching key packages

#### 6.5 Create `GroupInfoView.swift`

Sheet/screen showing group details:
- Group name (editable if admin)
- Member list with names/npubs
- "Add Member" button (if admin)
- "Remove Member" swipe action (if admin)
- "Leave Group" button
- Member count

#### 6.6 Update `NewChatView.swift`

Add a "New Group" button/option alongside the existing 1:1 flow.

#### 6.7 Update `ContentView.swift`

- Add `Screen` cases for new views (e.g. `Screen::GroupInfo { chat_id }`)
- Wire up new views in `screenView()` function
- Alternatively, group info can be a `.sheet()` from ChatView without needing a Screen case

### Phase 7: Android Parity (Deferred)

Same changes mirrored in Kotlin/Compose. The Rust state changes are shared, so Android just needs UI updates to match. Not blocking iOS-first development.

## Detailed Task Checklist

### Phase 1: Rust State Model
- [ ] Add `MemberInfo` struct to `state.rs`
- [ ] Update `ChatSummary` with `is_group`, `group_name`, `members: Vec<MemberInfo>`
- [ ] Remove `peer_npub`, `peer_name`, `peer_picture_url` from `ChatSummary`
- [ ] Update `ChatViewState` with `is_group`, `group_name`, `members`, `is_admin`
- [ ] Remove `peer_npub`, `peer_name`, `peer_picture_url` from `ChatViewState`
- [ ] Add `sender_name` to `ChatMessage`
- [ ] Update `GroupIndexEntry` in `core/mod.rs` to store member list + admin list
- [ ] Update `refresh_chat_list_from_storage()` to build `MemberInfo` vec from all members
- [ ] Update `refresh_current_chat()` to populate `sender_name` from profile cache
- [ ] Update profile fetching to request profiles for ALL group members
- [ ] Ensure `is_group` logic works (member count > 2 OR explicit group name)
- [ ] Update all existing code that reads old `peer_*` fields

### Phase 2: Rust Actions
- [ ] Add `CreateGroupChat` action
- [ ] Add `AddGroupMembers` action
- [ ] Add `RemoveGroupMembers` action
- [ ] Add `LeaveGroup` action
- [ ] Add `RenameGroup` action
- [ ] Add `GroupKeyPackagesFetched` internal event
- [ ] Add `GroupMembershipChanged` internal event
- [ ] Add `GroupUpdatePublished` internal event
- [ ] Add BusyState fields for group operations (optional)

### Phase 3: Group Creation
- [ ] Implement parallel key package fetching for N peers
- [ ] Handle partial failures (some peers' KP not found)
- [ ] Call `create_group()` with all key packages + group config
- [ ] Publish welcomes to all new members
- [ ] Navigate to new group chat
- [ ] Add `Screen::NewGroupChat` if needed

### Phase 4: Group Management
- [ ] Implement `AddGroupMembers` handler (fetch KPs, add_members, publish commit, wait, merge, publish welcomes)
- [ ] Implement `RemoveGroupMembers` handler (remove_members, publish commit, wait, merge)
- [ ] Implement `LeaveGroup` handler (leave_group, publish evolution, navigate away)
- [ ] Implement `RenameGroup` handler (update_group_data, publish, wait, merge)
- [ ] Handle relay confirmation before sending welcomes (MIP-02 timing requirement)

### Phase 5: Incoming Group Changes
- [ ] On `Commit` processing: re-fetch member list, update group index, refresh UI
- [ ] Handle group name/metadata changes from commits
- [ ] Handle being removed from a group (group no longer in `get_groups()`)

### Phase 6: iOS UI
- [ ] Update `ChatListView` for group display (name, member count, composite avatar)
- [ ] Update `ChatView` for sender names on group messages
- [ ] Update `ChatView` nav bar with group info access
- [ ] Create `NewGroupChatView` (multi-npub input, group name, create button)
- [ ] Create `GroupInfoView` (member list, add/remove, leave, rename)
- [ ] Update `NewChatView` with "New Group" option
- [ ] Update `ContentView` routing for new screens
- [ ] Update `ViewState.swift` if needed
- [ ] Update `TestIds.swift` with new identifiers
- [ ] Update `PreviewData.swift` with group chat preview data

### Phase 7: Testing
- [ ] Unit test: group creation with multiple members (Rust)
- [ ] Unit test: add/remove members (Rust)
- [ ] Unit test: state model outputs correct `is_group`, member lists
- [ ] E2E test: create group, send messages, verify all members receive
- [ ] Build and install on device, manual QA

## Risk Areas

1. **Key package fetch latency**: Fetching N key packages serially could be slow. Parallel fetch mitigates this, but partial failures need UX.
2. **Commit ordering (MIP-03)**: Must wait for relay confirmation before merging commits and sending welcomes. Current 1:1 flow skips this for initial creation (correct per spec) but add_members must not.
3. **Profile resolution**: Groups with many members means more kind:0 profile fetches. The existing profile cache handles this but may need larger batch fetches.
4. **Message ordering in groups**: With multiple senders, timestamp collisions are more likely. Current monotonic timestamp logic only handles own messages; display ordering should rely on `created_at` from MDK.
5. **State model migration**: Removing `peer_npub` / `peer_name` / `peer_picture_url` from `ChatSummary` and `ChatViewState` will break all existing iOS and Android UI code in one shot. This is intentional (no backwards compat) but requires updating all consumers in the same PR.

## Recommended Implementation Order

**Start with Phase 1** (state model) because everything else depends on it. It will temporarily break iOS/Android compilation, but that forces updating the UI to match.

Then **Phase 6.1-6.3** (update existing iOS views to compile with new state model). At this point, 1:1 chats work again with the generalized model.

Then **Phase 2 + 3** (group creation actions + flow). This enables creating groups.

Then **Phase 6.4-6.6** (new iOS views for group creation and info).

Then **Phase 4 + 5** (group management + incoming changes).

Finally **Phase 7** (testing).

This ordering minimizes time in a broken state and delivers incremental value.
