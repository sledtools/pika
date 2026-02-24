---
summary: Plan to bring pika-cli to parity with marmotd and add smart send, profile editing, and self-service onboarding UX
read_when:
  - working on pika-cli features or commands
  - improving CLI onboarding or help text
  - adding profile or send functionality to pika-cli
---

# pika-cli Design: Parity + Onboarding UX

## Goals

- Bring pika-cli to parity with marmotd's `init` command for identity management
- Make `send` usable by new users who only know an npub (no need to understand group IDs upfront)
- Add `profile` command for setting name and profile picture
- Make the CLI fully self-service: `--help` is enough, errors guide you to recovery
- Use the same default relays as the app so commands work without `--relay`

## Non-Goals

- Daemon / long-running mode (that's marmotd's job)
- Audio/calling features
- Multi-member group creation from CLI (use the app for that)
- Encrypted key storage (CLI remains dev/test tool with plaintext identity.json)

## Backward compatibility

All changes must preserve existing CLI usage. Specifically:

- `--relay` stays accepted everywhere it was before — it just becomes optional (has defaults now).
- The existing `identity`, `publish-kp`, `invite`, `welcomes`, `accept-welcome`, `groups`, `send --group`, `messages`, and `listen` commands keep their current behavior and argument shapes.
- The `tools/cli-smoke` script (used in CI via `just cli-smoke`) exercises: `identity`, `publish-kp`, `invite --peer`, `welcomes`, `accept-welcome`, `send --group`, `messages`. These must all keep working unchanged. The smoke test passes `--relay` explicitly so relay defaults don't affect it.
- `just cli-identity` and `just cli-smoke` recipes pass `--relay` explicitly — they continue to work as-is.

---

## Phase 0: Default relays

### Problem

Every pika-cli command currently requires `--relay <URL>`, which is verbose and confusing for new users. The app itself has sensible defaults in `rust/src/core/config.rs`.

### Change

Make `--relay` optional with defaults matching the app. Also add `--kp-relay` for key package relays (kind 443), which need different relay infrastructure because many popular relays reject NIP-70 protected events.

**Defaults from `config.rs`:**

| Purpose | Relays |
|---------|--------|
| Message relays (`DEFAULT_RELAY_URLS`) | `wss://us-east.nostr.pikachat.org`, `wss://eu.nostr.pikachat.org` |
| Key package relays (`DEFAULT_KEY_PACKAGE_RELAY_URLS`) | `wss://nostr-pub.wellorder.net`, `wss://nostr-01.yakihonne.com`, `wss://nostr-02.yakihonne.com` |

**New global options:**

```rust
/// Relay websocket URLs (default: us-east.nostr.pikachat.org, eu.nostr.pikachat.org)
#[arg(long)]
relay: Vec<String>,

/// Key-package relay URLs (default: wellorder.net, yakihonne x2)
#[arg(long)]
kp_relay: Vec<String>,
```

**Resolution logic:**
- If `--relay` is provided, use those for message traffic. Otherwise use `DEFAULT_RELAY_URLS`.
- If `--kp-relay` is provided, use those for key package publish/fetch. Otherwise use `DEFAULT_KEY_PACKAGE_RELAY_URLS`.
- Commands that need both (e.g., `invite` fetches a key package then publishes a welcome) connect to the union of both sets, same as the app's `all_session_relays()`.

**Implementation note:** Extract the default constants into a shared location (e.g., a small `pika-common` crate or just duplicate the constants in the CLI crate — they're small and rarely change). Duplicating is simpler and avoids coupling the CLI to the app's `AppCore` type.

**Backward compatibility:** `--relay` was `required = true` before. Changing it to optional with a default is backward-compatible: existing scripts that pass `--relay` explicitly still work. The smoke test and justfile recipes all pass `--relay` and continue to function identically.

---

## Phase 1: `init` command

Replace the implicit key-creation side effect inside `identity` with an explicit `init` command, matching marmotd's design.

### Current behavior

`identity` silently creates a new keypair if none exists. There is no way to import an existing nsec.

### New behavior

Add an `Init` subcommand:

```
pika-cli init [--nsec <NSEC>]
```

- **No `--nsec`**: Generate a fresh keypair (same as today's implicit behavior, but explicit).
- **With `--nsec`**: Import an existing key. Accepts `nsec1...` bech32 or raw hex.
- **Idempotent**: If identity.json already exists with the same pubkey, print info and exit.
- **Conflict warnings** (matching marmotd):
  - If identity.json exists with a *different* pubkey, warn.
  - If mdk.sqlite exists, warn about stale MLS state.
  - On any warning, prompt `Continue anyway? (yes/abort):` and bail if not confirmed.
- **Output**: Print the pubkey (hex) and npub (bech32) on success.
- **Also publishes a key package** (kind 443) to the key-package relays so the user is immediately invitable. This eliminates the separate `publish-kp` step from onboarding.

The existing `identity` command stays as-is (read-only: shows current identity, still auto-creates if missing for backward compat). `init` is the recommended write path for new users.

### Help text

```
Initialize your identity and publish a key package so peers can invite you.

Without --nsec, generates a fresh keypair.
With --nsec, imports an existing Nostr secret key (nsec1... or hex).

Examples:
  pika-cli init
  pika-cli init --nsec nsec1abc...
```

---

## Phase 2: Smart `send`

### Current behavior

```
pika-cli send --group <HEX> --content <TEXT>
```

Requires the user to know a nostr group ID hex string. Unusable for new users.

### New behavior

Change `--group` from required to optional and add `--to`:

```
pika-cli send --content <TEXT> (--group <HEX> | --to <NPUB_OR_HEX>)
```

- **`--group <HEX>`**: Existing behavior, unchanged. Send directly to a known group.
- **`--to <NPUB_OR_HEX>`**: New. Accepts an npub (bech32) or hex pubkey. Resolution logic:
  1. Parse the pubkey.
  2. Call `mdk.get_groups()` and for each group, call `mdk.get_members(&mls_group_id)`.
  3. Find a 1:1 DM group where the *only other member* is the target pubkey. (A group is 1:1 when: members minus self == 1 member, and the group has no explicit name or uses the default "DM" name.)
  4. **If found**: Send to that group.
  5. **If not found**: Auto-create the conversation:
     - Fetch the peer's key package (kind 443) from key-package relays.
     - Create a new group via `mdk.create_group()` with default name "DM".
     - Send the welcome giftwrap to the peer.
     - Send the message to the new group.
     - Print the new group ID so the user can use `--group` in future sends.

- **Exactly one of `--group` or `--to` must be provided.** If neither or both, print a clear error.

### Output

```json
{
  "event_id": "abc123...",
  "nostr_group_id": "def456...",
  "auto_created_group": true
}
```

`auto_created_group` is only present (and `true`) when a new group was created. `nostr_group_id` is always included so the user can see which group was used.

### Help text

```
Send a message to a group or a peer.

Use --group to send to a known group ID, or --to to send to a peer by npub/hex.
When using --to, pika-cli searches your groups for an existing 1:1 conversation.
If none exists, it automatically creates one and sends your message.

Examples:
  pika-cli send --group abc123 --content "hello"
  pika-cli send --to npub1xyz... --content "hey!"
```

### Error guidance

- Peer has no key package: `"Error: peer has no key package published. Ask them to run: pika-cli init"`
- Group not found: `"Error: no group with ID <hex>. Run 'pika-cli groups' to list your groups."`

---

## Phase 3: `profile` and `update-profile` commands

### `profile` (read-only)

```
pika-cli profile
```

Fetch and display current profile from relays (kind 0 metadata). Prints name, about, picture URL, pubkey, npub.

### `update-profile` (write)

```
pika-cli update-profile (--name <NAME> | --picture <FILE_PATH> | both)
```

- **`--name <NAME>`**: Update display name. Publishes a kind-0 metadata event preserving all other fields.
- **`--picture <FILE_PATH>`**: Upload image to Blossom, then update metadata with the returned URL. Max 8 MB, accepts JPEG/PNG.
- **Both together**: Update name and picture in a single metadata publish.
- **At least one flag required.** If neither provided, prints a clear error pointing to `profile` for viewing.

### Implementation notes

Reuse patterns from `rust/src/core/profile.rs`:

- **Metadata construction**: Load existing metadata from relay (via `client.fetch_metadata()`), overlay edits, preserve unmapped fields. Use the same `nostr_sdk::Metadata` struct.
- **Blossom upload**: Use `nostr_blossom::BlossomClient`. Upload to `https://us-east.nostr.pikachat.org`. The upload returns a `BlobDescriptor` with the URL.
- **Publish**: `client.set_metadata(&metadata).await`.

### Output

```json
{
  "name": "Alice",
  "picture_url": "https://us-east.nostr.pikachat.org/abc123.jpg",
  "pubkey": "abc...",
  "npub": "npub1..."
}
```

### Help text

```
pika-cli profile                                    # view current profile
pika-cli update-profile --name "Alice"
pika-cli update-profile --picture ./avatar.jpg
pika-cli update-profile --name "Alice" --picture ./avatar.jpg
```

---

## Phase 4: Help text and onboarding UX polish

### Top-level help

Update the CLI `about` text and add an `after_help` with a quickstart guide:

```
Pika CLI — encrypted messaging over Nostr + MLS

Quickstart:
  1. pika-cli init
  2. pika-cli update-profile --name "Alice"
  3. pika-cli send --to npub1... --content "hello!"
  4. pika-cli listen
```

### Per-command improvements

- Every subcommand gets an `after_help` with at least one concrete example.
- Error messages always suggest the next step (e.g., "run `pika-cli init` first").

### Relay visibility

Add a `--verbose` or just always print the relays being used to stderr on connect, so the user can see which defaults are active:

```
[pika-cli] relays: us-east.nostr.pikachat.org, eu.nostr.pikachat.org
[pika-cli] kp-relays: nostr-pub.wellorder.net, nostr-01.yakihonne.com, ...
```

This is especially helpful for debugging and makes the "magic" of defaults transparent.

---

## Implementation order

| Step | What | Depends on |
|------|------|------------|
| 0 | Default relays + `--kp-relay` | — |
| 1 | `init` command | 0 (needs kp-relay defaults to publish key package) |
| 2 | Smart `send` (`--to` flag) | 0 (needs kp-relay defaults to fetch peer key packages) |
| 3 | `profile` command | 0 (needs relay defaults) |
| 4 | Help text + UX polish | 0, 1, 2, 3 |

Phase 0 should land first since every other phase benefits from it. After that, 1-3 are independent and can be done in any order. Phase 4 is a polish pass over all commands once the functionality is in place.

## Smoke test update

After all phases land, update `tools/cli-smoke` to also exercise:
- `init` (both generate and import paths)
- `send --to` (smart send with auto-invite)
- `update-profile --name`

The existing smoke test commands (`identity`, `publish-kp`, `invite`, `welcomes`, `accept-welcome`, `send --group`, `messages`) must continue to pass unchanged.

## Risk areas

- **Key package relay divergence**: The key package relays are different from message relays because many popular relays reject NIP-70 protected events. If this landscape changes, the defaults need updating. Keeping the constants duplicated (not shared) means updating in two places (app + CLI), but it's simple and rarely changes.
- **Key package availability**: The auto-invite in smart send depends on the peer having published a key package. If they haven't, we need a clear error. The `init` command publishing a key package automatically mitigates this for pika-cli users.
- **1:1 DM detection heuristic**: Matching "DM" name or no-name groups with exactly 1 other member. Could false-positive on renamed DMs. The `get_members()` count is the authoritative signal; name is secondary.
- **Blossom server availability**: Single server (`blossom.yakihonne.com`). If it's down, picture upload fails. Acceptable for a CLI tool — error message should be clear.
- **Relay requirement on init**: `init` publishing a key package means it needs a relay connection. If the user just wants to generate keys offline, they can use `identity` which remains local-only. Document this distinction.
