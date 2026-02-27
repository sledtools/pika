# pikachat

Encrypted messaging over Nostr + MLS from the command line.

## Quickstart

```sh
# Build
cargo build -p pikachat

# Create your identity (generates keypair + publishes key package)
pikachat init

# Set your display name
pikachat update-profile --name "Alice"

# Send a message to someone by npub (creates a DM automatically)
pikachat send --to npub1... --content "hello!"

# Listen for incoming messages
pikachat listen
```

That's it. No relay flags needed — sensible defaults are built in.

## Commands

| Command | Description |
|---------|-------------|
| `init` | Create or import identity, publish key package |
| `identity` | Show current identity (pubkey + npub) |
| `profile` | View your Nostr profile |
| `update-profile` | Update your Nostr profile (name, picture) |
| `send` | Send a message or media to a group or peer |
| `send-hypernote` | Send a hypernote (MDX + interactive components) via `--content` or `--file` |
| `download-media` | Download and decrypt a media attachment |
| `listen` | Stream incoming messages and invitations |
| `groups` | List groups you belong to |
| `messages` | Fetch recent messages from a group |
| `invite` | Create a group with a peer |
| `welcomes` | List pending invitations |
| `accept-welcome` | Accept an invitation |
| `publish-kp` | Refresh your key package |
| `daemon` | Long-running JSONL sidecar daemon (OpenClaw integration) |
| `scenario` | Interop lab scenarios (Phase 1–4) |
| `bot` | Deterministic Rust bot fixture |

Run `pikachat <command> --help` for details and examples.

## Relay defaults

pikachat uses the same default relays as the Pika app:

- **Message relays**: `relay.primal.net`, `nos.lol`, `relay.damus.io`, `us-east.nostr.pikachat.org`, `eu.nostr.pikachat.org`
- **Key-package relays**: `nostr-pub.wellorder.net`, `nostr-01.yakihonne.com`, `nostr-02.yakihonne.com`

Override with `--relay` and `--kp-relay` for testing or custom setups.

## State directory

Identity and MLS state are stored under `${XDG_STATE_HOME:-$HOME/.local/state}/pikachat` by default (override with `--state-dir`). The state directory contains:

- `identity.json` — your keypair (plaintext, not for production use)
- `mdk.sqlite` — MLS group state

## Smart send

`send --to <npub>` searches your groups for an existing 1:1 DM with that peer. If one exists, it sends there. If not, it automatically creates a new conversation (fetches their key package, creates the group, sends the invitation, and delivers your message).

```sh
# First message to someone — group is created automatically
pikachat send --to npub1xyz... --content "hey!"

# Subsequent messages find the existing DM
pikachat send --to npub1xyz... --content "how's it going?"

# You can also send directly to a group ID
pikachat send --group <hex-id> --content "hello group"
```

## Encrypted media (Blossom)

Send and receive files encrypted with MLS group keys, stored on a [Blossom](https://github.com/hzrd149/blossom) server. The default server is `blossom.yakihonne.com`.

```sh
# Send an encrypted file to a peer (works with --to or --group)
pika-cli send --to npub1xyz... --media photo.jpg

# Add a caption and override MIME type
pika-cli send --group <hex-id> --media doc.pdf --mime-type application/pdf --content "the doc"

# After receiving, find the message_id in `messages` output
pika-cli messages --group <hex-id>

# Download and decrypt (saves as the original filename by default)
pika-cli download-media <message-id>
pika-cli download-media <message-id> --output photo.jpg
```

## Smoke tests

```sh
# Enter nix shell
nix develop

# Text-only (starts its own relay automatically)
just cli-smoke

# With encrypted media upload/download (requires internet for Blossom)
just cli-smoke-media
```
