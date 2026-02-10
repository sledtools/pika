# Marmot App v2 Spec: MDK Integration (Agreed)

This document specifies how to implement the Marmot v1 app architecture using MDK (~/code/mdk) for MLS and Nostr group messaging.

## References

- v1 architecture spec: `spec-v1.md`
- Negotiation sources used: `negotiation-v2/sources/spec-v2-codex.md`, `negotiation-v2/sources/spec-v2-pi.md`

## Hard Requirements From v1 (Non-Negotiable)

- Rust owns all app state and business logic.
- SwiftUI / Jetpack Compose are pure renders of Rust-provided state.
- `dispatch(action)` is enqueue-only and must not block the UI thread.
- Native applies `AppUpdate` slices; each slice update carries a monotonic `rev`.
- Native enforces `rev` continuity. On gap, it calls `state()` to resync.
- Updates are full slices (e.g., `ChatListChanged(ChatListState)`), not incremental diffs.
- Ephemeral UI state (scroll offset, gestures, focus) stays native.

Everything below must be implemented within these constraints.

## Goal / Non-Goals

Goal (MVP): 1:1 text chats over Nostr with MLS encryption via MDK.

Non-goals: group chats (>2), media, push notifications, profile system, and any UI-side business logic.

## What MDK Does vs What We Build

MDK provides:
- MLS group lifecycle: `create_group`, `process_welcome`, `accept_welcome`
- MLS message lifecycle: `create_message`, `process_message`
- Key package creation/validation: `create_key_package_for_event`, `parse_key_package`
- Persistent MLS state backed by encrypted SQLite via `mdk-sqlite-storage`

We build (inside Rust actor):
- Nostr relay connection management (nostr-sdk)
- Relay selection policy (inbox relays, key package relays, group relays)
- Subscription filter management
- GiftWrap decryption and event routing to MDK
- Transforming MDK storage into v1 `AppState` slices and emitting updates
- Paging strategy that maps v1 anchor-based actions onto MDK offset pagination

## Core Event Kinds and Routing

- Key package event: kind 443 ("MlsKeyPackage")
- Welcome rumor: kind 444 ("MlsWelcome")
- Group message wrapper: kind 445 ("MlsGroupMessage")
- Welcome delivery wrapper: NIP-59 GiftWrap (outer kind is GiftWrap; inner rumor is kind 444)

Routing:
- Group message wrapper (445) is routed to a chat via a single `h` tag whose value is `hex(nostr_group_id)`.
- Subscriptions for 445 must be filtered by `kind=445` and `#h` in the set of joined group ids.

## Rust Internal Architecture

### AppCore (Actor) Owns Everything Mutable

`AppCore` owns:
- v1 `AppState` and `rev`
- the logged-in Nostr keys (in-memory)
- an `MDK<MdkSqliteStorage>` instance (while logged in)
- a `nostr-sdk` client / relay pool
- minimal internal indices (not exposed over FFI), e.g. per-chat `loaded_count`

Async tasks (relay notifications, publish outcomes) must never mutate `AppState` directly. They enqueue internal events back into `AppCore`.

### Internal Events (Not Exposed Over UniFFI)

Examples:
- `Internal::NostrEventReceived(Event)`
- `Internal::GiftWrapReceived { wrapper: Event, rumor: UnsignedEvent }`
- `Internal::PublishResult { event_id, outcome }`
- Lifecycle is modeled as an `AppAction` (e.g., `AppAction::Foregrounded`) which may enqueue an internal resync event if needed.

## Storage: Encrypted SQLite + Keyring

### Per-Identity Database

Each identity must have its own MDK SQLite DB.

- DB file path: `${data_dir}/mls/<pubkey_hex>/mdk.sqlite3`
- Create the `${data_dir}/mls/<pubkey_hex>/` directory if needed.

Rationale: OpenMLS/MDK storage is not partitioned per identity; a shared DB risks cross-identity state corruption and blocks future multi-account.

### Key IDs

Use stable identifiers:
- `service_id = "com.marmot.app"`
- `db_key_id = "mdk.db.key.<pubkey_hex>"`

Do not derive key IDs from absolute filesystem paths.

### Platform Keyring Initialization

Before constructing `MdkSqliteStorage`, initialize `keyring-core` default store once per process (guard with `Once`).

- iOS: use `apple_native_keyring_store::AppleStore` and call `keyring_core::set_default_store(...)`.
- Android: prefer `android_native_keyring_store::AndroidStore::from_ndk_context()` if available.
  - Otherwise, require a Kotlin/JNI early-init hook (per MDK docs) to supply the Android credential builder and/or context.

Important: the MDK DB encryption key is distinct from the Nostr secret key (`nsec`). Per v1, `nsec` is stored only in native Keychain/Keystore and never persisted by Rust.

## MDK Construction

Construct MDK using the builder API (even if using defaults):

- `MDK::builder(storage).with_config(MdkConfig::default()).build()`

This makes it easy to tune options such as `out_of_order_tolerance` later without rewriting construction.

## Account Lifecycle

### Create Account

- Rust generates the keypair.
- Rust must hand the secret to native via an update (see "nsec Handoff" below).
- Rust initializes per-identity MDK storage and MDK instance.
- Rust publishes an initial key package (kind 443).
- Rust starts subscriptions (GiftWrap inbox + joined groups).

### Restore Session

- Native provides `nsec` to Rust action (per v1).
- Rust derives keys, opens per-identity MDK DB.
- Rust rebuilds view state slices from MDK storage.
- Rust ensures a key package exists (publish if missing/stale).
- Rust starts subscriptions.

### Logout

- Drop MDK instance and keys from memory.
- Unsubscribe and/or stop relay processing.
- Reset `AppState`.

## nsec Handoff (Required Amendment)

v1 requires "actions return nothing" and "native stores secrets". Therefore, on account creation Rust must send an update that carries the generated `nsec` (and identifiers) to native.

Decision: add an update variant like:
- `AppUpdate::AccountCreated { rev, nsec, pubkey, npub }`

Native stores `nsec` in Keychain/Keystore immediately.

## Key Packages (Kind 443)

### Publish Our Key Package

After login/restore, ensure a current key package is published.

Flow:
1. Choose key package relays (MVP: defaults; future: explicit KP relay list).
2. Call `mdk.create_key_package_for_event(my_pubkey, relays)` to get content + tags.
3. Build/sign kind 443 event and publish to the chosen relays.

### Fetch Peer Key Package for DM Creation

Flow:
1. Determine peer key package relays (preferred chain):
   - peer KP relays if discoverable (future: kind 10051)
   - else defaults
2. Fetch a recent kind 443 event for the peer.
3. Validate via `mdk.parse_key_package(&event)` before use.

### Rotate After Welcome Acceptance

After successfully accepting a welcome that referenced a particular key package event id:
- Best-effort delete that key package event.
- Publish a fresh key package.

## DM Group Creation (1:1)

### Admin Policy

Decision: both participants are admins in a 1:1 DM.

- `admins = [my_pubkey, peer_pubkey]`

### Create Group

- Create `NostrGroupConfigData` with:
  - `relays`: MVP defaults (kept conceptually separate from inbox/KP relays)
  - `admins`: both participants
  - other metadata: minimal
- Call `mdk.create_group(&my_pubkey, vec![peer_key_package_event], config)`.

MDK returns group identifiers and one or more welcome rumors (unsigned).

### Deliver Welcome (GiftWrap)

- Wrap each welcome rumor using NIP-59 GiftWrap addressed to the peer.
- Add a NIP-40 expiration tag to the GiftWrap (~30 days).
- Publish to relays chosen by the welcome delivery relay policy (below).

### Welcome Delivery Relay Selection

Decision: publish gift-wrapped welcomes via this preference chain:
1. peer inbox relays (kind 10050) if known
2. else peer key package relays (from KP metadata/tags) if known
3. else our default relay set

MVP likely uses (2) then (3).

## Receiving Welcomes (GiftWrap -> MDK)

### Subscribe

- Subscribe to GiftWrap events relevant to our inbox.

### Process

When a GiftWrap arrives:
1. Decrypt/unwrap to obtain the inner rumor (`UnsignedEvent`).
2. If rumor kind is 444 (MlsWelcome):
   - `welcome = mdk.process_welcome(wrapper_event_id, &rumor)`
   - For MVP, auto-accept: `mdk.accept_welcome(&welcome)`
   - Update group subscriptions (add this group id to 445 filters)
   - Rotate the referenced key package (best effort)
   - Emit v1 slice updates (chat list, toasts, etc.)

All of the above state changes happen in the actor.

## Sending Messages

### Inner Rumor (Plaintext)

Decision: inner rumor kind is `Kind::Custom(9)`.

- Build an `UnsignedEvent` with:
  - `kind = Custom(9)`
  - `pubkey = my_pubkey`
  - `content = plaintext`
  - tags: empty for MVP

Important: call `ensure_id()` before passing the rumor to MDK.

Rationale: we need a stable message id for optimistic UI (Pending -> Sent), and we should not rely on MDK to ensure the id after the fact.

### Create MLS Wrapper via MDK

- Call `mdk.create_message(mls_group_id, rumor)` to get a kind 445 wrapper event.
- Publish the wrapper event to group relays.

### Optimistic UI and Delivery State

- On send intent, insert a Pending message into `current_chat.messages` using the rumor id.
- When publish succeeds, transition delivery state to Sent.
- On failure, transition to Failed (and allow Retry).

Emitted updates must be full-slice updates per v1.

## Receiving Group Messages (Kind 445)

- Subscribe to kind 445 filtered by `#h` values for all joined groups.
- On receipt of a 445 event, route to `mdk.process_message(&event)`.
- Use MDK-returned results to update slices (chat list last message, unread counts, current chat messages, etc.).

All state changes happen in the actor.

## Paging Strategy (v1 Action vs MDK Pagination)

v1 action is anchor-based:
- `LoadOlderMessages { chat_id, before_message_id, limit }`

MDK pagination is offset-based:
- `get_messages(group_id, Pagination(limit, offset))`

Decision: implement paging using actor-internal bookkeeping.

- Track `loaded_count: usize` per chat internally (not in `AppState`).
- On `LoadOlderMessages`:
  - `offset = loaded_count`
  - fetch `limit` messages from MDK with that offset
  - prepend to the chat’s message list
  - `loaded_count += fetched.len()`
  - `can_load_older = fetched.len() == limit`
- Treat `before_message_id` as a sanity check only.
  - If it does not match the oldest currently loaded message, trigger a slice refresh/resync rather than scanning full history.

## Rollback / Commit Races

Decision (MVP): do not implement `MdkCallback`.

- Rely on MDK’s internal epoch snapshot/rollback.
- When processing results implies a commit/epoch change, refresh the relevant slice(s) from MDK and emit full-slice updates.
- Revisit callback plumbing post-MVP for multi-member groups.

## Subscription Recompute Rules

- After accepting a welcome or creating a group, recompute the joined-group set.
- Update the kind 445 subscription filter to include all current group `h` tags.
- Keep GiftWrap subscription active while logged in.

## Compliance Checklist

The implementation is compliant with v1 if:
- `dispatch()` only enqueues.
- All MDK calls and state mutation happen on the actor worker.
- Every emitted update includes `rev` and is a full-slice.
- Native enforces `rev` continuity and resyncs on gaps.
- No per-frame or gesture-driven FFI calls are required.
