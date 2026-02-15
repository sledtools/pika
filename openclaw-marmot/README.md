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
This code lives in the Pika monorepo under `openclaw-marmot/`.
cd crates/marmotd
cargo build --release
# binary at target/release/marmotd
```

Then set `sidecarCmd` in your channel config to the absolute path of the binary:

```json
"sidecarCmd": "/path/to/pika/target/release/marmotd"
```

---

## Phase Tests

- Phase 1: `PLAN.md` (Rust <-> Rust over local Docker relay)
- Phase 2: `OPENCLAW-INTEGRATION-PLAN.md` (Rust harness <-> deterministic Rust bot process)
- Phase 3: `OPENCLAW-CHANNEL-DESIGN.md` + `rust_harness daemon` (JSONL sidecar integration surface)
- Phase 3 Audio: in-memory call echo smoke (`marmotd scenario audio-echo`)
- Phase 4: Local OpenClaw gateway E2E: Rust harness <-> OpenClaw `marmot` channel (Rust sidecar spawned by OpenClaw)

### Run Phase 1

```sh
./scripts/phase1.sh
```

Defaults:
- Relay URL: random free localhost port (started via `nostr-rs-relay` from the Nix devShell)
- State dir: a fresh temp dir (override via `STATE_DIR=...` if you want to keep artifacts)

### Run Phase 2

```sh
./scripts/phase2.sh
```

### Run Phase 3 (Daemon JSONL Smoke)

```sh
./scripts/phase3.sh
```

### Run Phase 3 Audio Echo Smoke

```sh
./scripts/phase3_audio.sh
```

### Run Phase 4 (OpenClaw Marmot Plugin E2E)

This uses the pinned OpenClaw checkout under `./openclaw/`, runs a local relay on a random port,
starts OpenClaw gateway with the `marmot` plugin enabled, then runs a strict Rust harness invite+reply
scenario against the plugin's pubkey.

```sh
./scripts/phase4_openclaw_marmot.sh
```

### Phase 4 Call STT -> Text (marmotd daemon)

During active calls, `marmotd` now runs:
- `Opus -> PCM -> buffer -> transcription`
- publishes transcript text back into the same MLS group as a normal app message
- emits sidecar event `call_transcript_final`

Runtime configuration:
- `OPENAI_API_KEY` (required for real STT)
- `OPENAI_STT_MODEL` (optional, default `gpt-4o-mini-transcribe`)
- `OPENAI_BASE_URL` (optional, default `https://api.openai.com/v1`)
- `MARMOT_STT_FIXTURE_TEXT` (optional deterministic fixture mode for tests/dev; bypasses OpenAI)

### Phase 8 Bot Full Duplex Voice (STT -> LLM -> TTS -> Opus)

The sidecar/plugin path now supports:
- daemon command `send_audio_response { call_id, tts_text }`
- OpenClaw plugin wiring: on `call_transcript_final`, dispatch transcript to the agent and stream
  the agent reply back into the active call as synthesized Opus audio
- TTS synthesis defaults to OpenAI audio speech endpoint

Runtime configuration for TTS:
- `OPENAI_API_KEY` (required for real TTS, unless fixture mode enabled)
- `OPENAI_TTS_MODEL` (optional, default `gpt-4o-mini-tts`)
- `OPENAI_TTS_VOICE` (optional, default `alloy`)
- `OPENAI_BASE_URL` (optional, default `https://api.openai.com/v1`)
- `MARMOT_TTS_FIXTURE=1` (optional deterministic fixture tone mode for tests/dev)

Phase-8 local verification lane:

```sh
just phase8-voice
```

### Run Pre-Merge Suite

```sh
just pre-merge
```

This is the canonical local/CI verification lane for the marmot sidecar + harness.

## Cleanup

Deferred cleanup notes (intentionally postponed to keep momentum):

- Replace local path dependency on `pika-media` (`marmotd/Cargo.toml`) with a proper published/git dependency once the API stabilizes.
- Replace in-memory media relay scaffolding with real MOQ relay transport for call media E2E.
- When call transport tests move into `pika`, prefer a real local MOQ relay path where feasible (not only in-memory relay tests).
- Keep MOQ versions aligned between code dependencies and dev environment tooling (same pinned revision for Cargo deps and `flake.nix` input).
