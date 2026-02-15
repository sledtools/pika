# marmot-interop-lab-rust

> [!WARNING]
> Alpha software. This project was largely vibe-coded and likely contains privacy and security flaws. Do not use it for sensitive or production workloads.

Phased plan for a Rust-based Marmot interop harness.

## OpenClaw Setup Guide

Use Marmot as an [OpenClaw](https://openclaw.dev) channel plugin so your AI agent can send and receive messages over Nostr MLS groups.

**No Rust toolchain required.** The plugin automatically downloads a prebuilt `marmotd` binary for your platform from GitHub releases.

### Prerequisites

- **OpenClaw** installed and running (`openclaw onboard`)
- **A Nostr keypair** in hex format (optional — a random identity is generated if you skip this)

### 1. Install the plugin

```bash
openclaw plugins install @justinmoon/marmot
```

This installs the plugin via npm. The `marmotd` sidecar binary is auto-downloaded on first launch (Linux and macOS, x64 and arm64).

### 2. (Optional) Set up an identity

If you want a specific Nostr identity, create a state directory and identity file:

```bash
mkdir -p ~/.openclaw/.marmot-state
```

Create `~/.openclaw/.marmot-state/identity.json`:

```json
{
  "secret_key_hex": "<your-hex-secret-key>",
  "public_key_hex": "<your-hex-public-key>"
}
```

```bash
chmod 600 ~/.openclaw/.marmot-state/identity.json
```

> **⚠️ Important:** You must include **both** `secret_key_hex` and `public_key_hex`. Omitting the public key causes a silent sidecar crash.

If you skip this step entirely, `marmotd` will generate a random identity on first run.

### 3. Configure the channel

Add the channel config to `~/.openclaw/openclaw.json`:

```json
{
  "channels": {
    "marmot": {
      "relays": ["wss://relay.damus.io", "wss://nos.lol", "wss://relay.primal.net"],
      "sidecarCmd": "marmotd",
      "stateDir": "~/.openclaw/.marmot-state",
      "autoAcceptWelcomes": true,
      "groupPolicy": "open",
      "groupAllowFrom": ["<hex-pubkey-of-allowed-sender>"]
    }
  }
}
```

Replace `<hex-pubkey-of-allowed-sender>` with the Nostr public key(s) you want to accept messages from.

### Group Chat Support

The plugin supports multi-participant MLS group chats with mention gating, sender identity resolution, and owner/friend permission tiers. See **[docs/group-chat.md](docs/group-chat.md)** for the full guide.

Quick setup for group chats — add these fields to your `channels.marmot` config:

```json
{
  "channels": {
    "marmot": {
      "groupPolicy": "open",
      "groupAllowFrom": ["<owner-pubkey>", "<friend-pubkey>"],
      "owner": "<owner-pubkey>",
      "memberNames": {
        "<owner-pubkey>": "Alice",
        "<friend-pubkey>": "Bob"
      }
    }
  }
}
```

**Key features:**
- **Mention gating** — bot only responds when @mentioned, buffers other messages as context
- **Sender identity** — resolves display names from Nostr profiles (kind:0), with in-memory caching
- **Owner/friend tiers** — owner gets `CommandAuthorized`, friends can chat but not run commands
- **Per-group sessions** — each group gets isolated conversation history
- **Sender metadata** — exposes npub and owner/friend tag for verifiable identity

> **Note:** Setting `sidecarCmd` to just `"marmotd"` (no path) tells the plugin to auto-download the correct prebuilt binary. Binaries are cached at `~/.openclaw/tools/marmot/<version>/marmotd`.

### 4. Restart OpenClaw gateway

```bash
openclaw gateway restart
```

### 5. Verify

```bash
openclaw status
```

You should see: `Marmot | ON | OK | configured`

### 6. Connect from a client

Use [Pika](https://pika.team) or another Marmot-compatible client to create a group and invite the bot's pubkey. With `autoAcceptWelcomes: true`, the bot joins automatically and starts responding.

### Gotchas

- **`identity.json` needs both fields** — omitting `public_key_hex` causes a silent sidecar crash with no useful error.
- **Relay loading** — the sidecar starts with only the first relay; the rest are added via `setRelays` after startup.
- **`groupPolicy: "allowlist"`** requires explicit group IDs in the `groups` config. Use `"open"` with `groupAllowFrom` if you just want sender-level filtering.
- **Duplicate sidecars** — multiple rapid gateway restarts can spawn duplicate sidecar processes fighting over the SQLite state. Kill extras manually if this happens.

### Building from source

If you prefer to compile `marmotd` yourself (requires the Rust toolchain):

```bash
git clone https://github.com/justinmoon/openclaw-marmot
cd openclaw-marmot/marmotd
cargo build --release
# binary at target/release/marmotd
```

Then set `sidecarCmd` in your channel config to the absolute path of the binary:

```json
"sidecarCmd": "/path/to/openclaw-marmot/marmotd/target/release/marmotd"
```

---

## Phase Tests

- Phase 1: `PLAN.md` (Rust <-> Rust over local Docker relay)
- Phase 2: `OPENCLAW-INTEGRATION-PLAN.md` (Rust harness <-> deterministic Rust bot process)
- Phase 3: `OPENCLAW-CHANNEL-DESIGN.md` + `rust_harness daemon` (JSONL sidecar integration surface)
- Phase 4: Local OpenClaw gateway E2E: Rust harness <-> OpenClaw `marmot` channel (Rust sidecar spawned by OpenClaw)

### Run Phase 1

```sh
./scripts/phase1.sh
```

Defaults:
- Relay URL: random free localhost port (discovered via `docker compose port`)
- State dir: `.state/` (reset each run by the script)

### Run Phase 2

```sh
./scripts/phase2.sh
```

### Run Phase 3 (Daemon JSONL Smoke)

```sh
./scripts/phase3.sh
```

### Run Phase 4 (OpenClaw Marmot Plugin E2E)

This uses the pinned OpenClaw checkout under `./openclaw/`, runs a local relay on a random port,
starts OpenClaw gateway with the `marmot` plugin enabled, then runs a strict Rust harness invite+reply
scenario against the plugin's pubkey.

```sh
./scripts/phase4_openclaw_marmot.sh
```
