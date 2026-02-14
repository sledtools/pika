# Pika Audio Calls via MOQ — Specification v0

## 1. Overview

Audio calling for Pika using Media over QUIC (MOQ) as the transport layer. MLS-encrypted signaling over Nostr relays. MLS-derived frame encryption for audio payloads. Designed for 1:1 calls with architecture that generalizes to multi-party.

### Design Principles
- Rust-first: audio pipeline entirely in Rust, minimal native shims
- Simple v0: ship working 1:1 calls, defer complexity
- Security gate: no non-lab deployment without frame-level encryption
- Test-first: each phase has verifiable test criteria

## 2. Architecture

### 2.1 Crate Structure

```
pika/
├── rust/                          # pika_core (existing)
│   └── src/
│       ├── core/
│       │   ├── call_control.rs    # call state machine + signaling parsing
│       │   ├── call_runtime.rs    # media worker lifecycle orchestration
│       │   └── ...
│       ├── state.rs               # CallState + CallDebugStats in AppState
│       └── actions.rs             # call actions (StartCall, AcceptCall, etc)
│
├── crates/
│   └── pika-media/                # NEW crate (no MDK/Nostr/UI deps)
│       └── src/
│           ├── lib.rs
│           ├── session.rs         # MOQ session: connect, disconnect, publish, subscribe
│           ├── tracks.rs          # hang catalog + track creation/subscription
│           ├── codec_opus.rs      # Opus encode/decode adapters
│           ├── jitter.rs          # receive jitter/playout buffer
│           ├── crypto.rs          # MLS exporter + frame AEAD (late v0)
│           └── directory.rs       # placeholder for future DirectoryMessage
```

**Rules:**
- `pika-media` has zero dependency on MDK, Nostr, or UI
- `pika_core` owns all call state, signaling parsing, and PTT gating
- `openclaw-marmot/marmotd` depends on `pika-media` for transport/codec/crypto
- Bot-specific orchestration (STT, TTS, call handling) stays in marmotd

### 2.2 Dependencies (pika-media)

- `moq-lite` — core pub/sub protocol
- `moq-native` — QUIC client (Quinn backend)
- `hang` — media catalog + container format
- `opus` crate — Opus codec (links libopus)
- `aes-gcm`, `hkdf`, `sha2` — frame encryption (late v0)

### 2.3 Data Planes

| Plane | Transport | Purpose |
|-------|-----------|---------|
| Control/signaling | MLS app messages via Nostr relays (kind 445) | Call lifecycle (invite/accept/reject/end) |
| Media | MOQ (moq-lite + hang over QUIC) | Opus audio frames |
| UI/state | AppState → AppUpdate via UniFFI | Call status, debug stats |

## 3. Call Signaling Protocol

### 3.1 Envelope Format

All call signaling messages are JSON payloads sent as MLS application messages:

```json
{
  "v": 1,
  "ns": "pika.call",
  "type": "<message_type>",
  "call_id": "<uuid-v4>",
  "ts_ms": 1730000000000,
  "body": { ... }
}
```

- `v`: protocol version (integer, currently `1`)
- `ns`: namespace (`"pika.call"`)
- `type`: message type (see below)
- `call_id`: UUID identifying the call session
- `ts_ms`: sender timestamp (milliseconds since epoch)
- `from`: **optional**, non-authoritative convenience field. MLS sender identity is always authoritative.
- `body`: type-specific payload

### 3.2 Message Types

#### `call.invite`
```json
{
  "v": 1, "ns": "pika.call", "type": "call.invite",
  "call_id": "550e8400-e29b-41d4-a716-446655440000",
  "ts_ms": 1730000000000,
  "body": {
    "moq_url": "https://moq.justinmoon.com/anon",
    "broadcast_base": "pika/calls/550e8400-e29b-41d4-a716-446655440000",
    "tracks": [
      { "name": "audio0", "codec": "opus", "sample_rate": 48000, "channels": 1, "frame_ms": 20 }
    ]
  }
}
```

#### `call.accept`
```json
{
  "v": 1, "ns": "pika.call", "type": "call.accept",
  "call_id": "550e8400-e29b-41d4-a716-446655440000",
  "ts_ms": 1730000000001,
  "body": {
    "moq_url": "https://moq.justinmoon.com/anon",
    "broadcast_base": "pika/calls/550e8400-e29b-41d4-a716-446655440000",
    "tracks": [
      { "name": "audio0", "codec": "opus", "sample_rate": 48000, "channels": 1, "frame_ms": 20 }
    ]
  }
}
```

#### `call.reject`
```json
{
  "v": 1, "ns": "pika.call", "type": "call.reject",
  "call_id": "...",
  "ts_ms": 1730000000002,
  "body": { "reason": "busy" }
}
```

#### `call.end`
```json
{
  "v": 1, "ns": "pika.call", "type": "call.end",
  "call_id": "...",
  "ts_ms": 1730000000003,
  "body": { "reason": "user_hangup" }
}
```

### 3.3 Call Flow (1:1)

```
Alice                          Nostr Relays                    Bob
  │                                │                             │
  ├─ call.invite (MLS) ──────────>│────────────────────────────>│
  │  [creates MOQ session,         │                             │
  │   starts publishing audio]     │                             │
  │                                │                             │
  │                                │<── call.accept (MLS) ──────┤
  │<───────────────────────────────│   [creates MOQ session,     │
  │                                │    publishes + subscribes]  │
  │  [subscribes to Bob's          │                             │
  │   broadcast]                   │                             │
  │                                │                             │
  │ ═══════════ Audio via MOQ relay (bidirectional) ═══════════ │
  │                                │                             │
  │                                │<── call.end (MLS) ─────────┤
  │<───────────────────────────────│                             │
  │  [teardown]                    │                    [teardown]│
```

### 3.4 Busy Handling

If a `call.invite` arrives while a call is already active, auto-reject:
```json
{ "v": 1, "ns": "pika.call", "type": "call.reject", "call_id": "...", "body": { "reason": "busy" } }
```

### 3.5 Future: MOQ In-Call Control Track

The spec reserves a `control` track on each participant's MOQ broadcast for future in-call signaling (mute notifications, end, etc). v0 does NOT implement this — all signaling uses Nostr. This track may be added when Nostr latency proves problematic for in-call events.

## 4. MOQ Audio Transport

### 4.1 Broadcast Naming

Each participant publishes one MOQ broadcast:

```
Path: <broadcast_base>/<full_pubkey_hex>
```

Example:
```
pika/calls/550e8400-e29b-41d4-a716-446655440000/11b9a894813efe60d39f8621ae9dc4c6d26de4732411c1cdf4bb15e88898a19c
```

**Path rules:**
- `broadcast_base` MUST NOT start or end with `/`
- `moq_url` is the connection endpoint (may include auth root like `/anon`)
- Full 64-character hex pubkey in path (no truncation)

**Privacy note:** v0 paths are readable and leak call/participant metadata to the relay. Production hardening replaces with MLS-exporter-derived opaque labels (bundled with encryption milestone).

### 4.2 Track Layout

Per participant broadcast:
- `catalog.json` — hang catalog describing available tracks
- `audio0` — Opus audio frames

Future: `video0`, `control` tracks (same broadcast, new catalog entries).

### 4.3 Codec

**Opus**, 48kHz, mono, ~32kbps (voice-optimized profile).

### 4.4 Frame Grouping

**Recommended default:** One Opus frame (20ms) per MOQ group.

This is an implementation recommendation, not a protocol requirement. Tradeoffs:
- One-per-group: maximum skip-ability, ~50 groups/sec, simple
- Batched: lower overhead, but frame loss impacts more audio

Implementations may tune grouping without protocol changes.

### 4.5 Frame Format (hang container)

Each frame in the hang container:
- Timestamp: microseconds, monotonic
- Keyframe: `true` (every Opus frame is independently decodable)
- Payload: Opus packet (plaintext in v0, encrypted post-hardening)

## 5. Encryption

### 5.1 v0: Plaintext (dev/test only)

Audio frames are sent as plaintext Opus. QUIC provides TLS encryption in transit. **Plaintext is only acceptable for dev phases using test identities on test relay.**

### 5.2 Production: MLS-Derived Frame AEAD

Hard gate before any non-lab deployment. Ported from av-demo `MediaCrypto`:

- **Key derivation:** `base_key = MLS-Exporter("moq-media-base-v1", sender_leaf || track_label || epoch, 32)`
- **Per-generation keys:** `K_gen, N_salt = HKDF-SHA256(base, "k"/"n" || generation)`
- **Algorithm:** AES-128-GCM
- **Nonce:** 32-bit frame counter, MSB = generation
- **AAD binding:** version, group root, track label, epoch, group_seq, frame_idx, keyframe flag
- **Opus payload:** fully encrypted (no plaintext header exceptions)
- **Generation rollover:** on MLS epoch change, derive new generation keys

### 5.3 Relay Authentication

v0: anonymous relay access (auth root in `moq_url`, e.g., `/anon`).

**Risk:** Anonymous access allows any client to publish/subscribe. Acceptable for dev with isolated test relay.

**Hardening:** Self-issued capabilities (preferred, aligned with Nostr ethos) or JWT auth. Required before broader deployment, bundled with encryption milestone.

## 6. State Management

### 6.1 AppState Additions

```rust
#[derive(uniffi::Record, Clone, Debug)]
pub struct CallState {
    pub call_id: String,
    pub chat_id: String,
    pub peer_npub: String,
    pub status: CallStatus,
    pub started_at: Option<i64>,
    pub is_muted: bool,
    pub debug: Option<CallDebugStats>,
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum CallStatus {
    Offering,         // we sent invite, waiting
    Ringing,          // we received invite, showing UI
    Connecting,       // accepted, establishing MOQ session
    Active,           // audio flowing
    Ended { reason: String },
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct CallDebugStats {
    pub tx_frames: u64,
    pub rx_frames: u64,
    pub rx_dropped: u64,
    pub jitter_buffer_ms: u32,
    pub last_rtt_ms: Option<u32>,
}
```

In `AppState`:
```rust
pub active_call: Option<CallState>,
```

### 6.2 AppAction Additions

```rust
pub enum AppAction {
    // existing...
    StartCall { chat_id: String },
    AcceptCall { chat_id: String },
    RejectCall { chat_id: String },
    EndCall,
    ToggleMute,
}
```

### 6.3 Audio Data Path

Audio frames bypass AppCore entirely — they flow through a separate high-frequency path:
- **Capture:** cpal callback → Opus encode → MOQ publish (all in Rust)
- **Playback:** MOQ subscribe → jitter buffer → Opus decode → cpal playback (all in Rust)

Only call state changes (status, mute, debug stats) flow through AppState/AppUpdate.

## 7. Platform Audio

### 7.1 Primary: Rust via cpal

`cpal` crate for audio capture and playback on all platforms:
- **iOS:** CoreAudio backend
- **Android:** AAudio backend
- **Linux (bot):** ALSA/PulseAudio backend

### 7.2 Native Shims (pre-approved, documented)

Minimal native code allowed only for:
1. **AVAudioSession setup** (iOS) — set category to `.playAndRecord`, activate session
2. **Microphone permission prompt** (iOS/Android) — platform UI requirement
3. **Audio focus/interruption handling** (iOS/Android) — respond to phone calls, other apps

Each native shim must include a comment documenting the specific blocker that required it. Timebox platform integration spikes to prevent delays.

### 7.3 Fallback: UniFFI Audio Bridge

If cpal has blocking issues on a platform:
- Use UniFFI push model: `push_audio_frame(Vec<i16>, sample_rate, channels)`
- Instrument frame-drop and latency counters from day one
- Lower-level FFI bridge only if counters show measurable problems

## 8. Bot Integration (openclaw-marmot)

### 8.1 Architecture

marmotd depends on `pika-media` for MOQ transport, Opus codec, and crypto. Bot-specific logic (STT, TTS, call handling) stays in marmotd.

### 8.2 Sidecar Protocol (JSONL)

**Inbound commands (TS plugin → marmotd):**
```json
{ "cmd": "accept_call", "request_id": "...", "call_id": "..." }
{ "cmd": "reject_call", "request_id": "...", "call_id": "...", "reason": "..." }
{ "cmd": "end_call", "request_id": "...", "call_id": "..." }
{ "cmd": "send_audio_response", "request_id": "...", "call_id": "...", "tts_text": "..." }
```

**Outbound events (marmotd → TS plugin):**
```json
{ "type": "call_invite_received", "call_id": "...", "from_pubkey": "...", "group_id": "..." }
{ "type": "call_session_started", "call_id": "..." }
{ "type": "call_session_ended", "call_id": "...", "reason": "..." }
{ "type": "call_transcript_partial", "call_id": "...", "text": "..." }
{ "type": "call_transcript_final", "call_id": "...", "text": "..." }
{ "type": "call_debug", "call_id": "...", "tx_frames": 0, "rx_frames": 0, "rx_dropped": 0 }
```

**Boundary principle:** Plugin sees semantic call events only. No Opus frames, MOQ paths, or codec parameters cross the sidecar boundary.

### 8.3 Bot Phases

| Phase | Description | Test Criteria |
|-------|-------------|---------------|
| Echo | Subscribe → decode → re-encode → publish back | Frame count in = frame count out |
| STT → Text | Opus → PCM → Whisper → text MLS message | Fixture transcript matches expected output |
| Full duplex | STT → LLM → TTS → Opus, continuous | Round-trip speech in → speech out verified |

**STT/TTS defaults:** OpenAI Whisper (or gpt-4o-mini-transcribe) for STT. OpenAI TTS (or gpt-4o-mini-tts) for speech synthesis. Reuse existing OpenClaw provider code paths where possible.

## 9. Multi-Party Readiness

The architecture is inherently multi-party:
- 1 broadcast per participant (not shared)
- Each participant subscribes to N-1 others
- `tracks` array in signaling extends to multiple participants
- MOQ relay handles fan-out natively
- Client-side audio mixing in Rust for N>2
- `DirectoryMessage` (placeholder in v0) activates for group calls

For v0 1:1: 2 broadcasts, 2 subscriptions. The same code handles N participants with a loop.

## 10. Implementation Phases

### Phase 0: Control Scaffold
- Call signaling envelope + state machine (no live media)
- Wire up `call.invite`/`accept`/`reject`/`end` as MLS app messages
- Test: deterministic unit/integration flow through all state transitions

### Phase 1: MOQ Media Plumbing (Synthetic)
- `pika-media` crate: MOQ session connect, Opus encode/decode
- Publish synthetic Opus frames, subscribe and verify receipt
- Test: frame count + order verification

### Phase 2: Push-to-Talk Audio (iOS first)
- cpal mic capture → Opus → MOQ publish on button hold
- MOQ subscribe → Opus decode → cpal playback
- iOS AVAudioSession shim (as needed)
- iOS is primary dev platform through Phases 2–6; Android ported in Phase 7
- Test: manual smoke test + debug stats counters

### Phase 3: Bot Echo
- marmotd audio session: subscribe → decode → re-encode → publish
- Test: echo round-trip, frame count match

### Phase 4: Bot STT → Text
- Opus → PCM → buffer → Whisper API → text response via MLS
- Test: fixture transcript matches expected

### Phase 5: Full Duplex Audio
- Continuous simultaneous send/receive
- Jitter buffer tuning
- Test: two-way overlap speech verification

### Phase 6: Encryption Hardening (HARD GATE)
- Port MediaCrypto from av-demo
- Frame-level MLS-derived AEAD encryption
- Privacy-preserving broadcast naming (MLS exporter-derived)
- Relay auth (self-issued capabilities)
- **Must complete before any non-lab / non-test-key deployment**

### Phase 7: Android Port
- cpal AAudio integration (Phases 2–6 are iOS-first)
- Android-specific audio focus/permissions shim
- Verify Opus cross-compilation on Android target

### Phase 8: Bot Full Duplex Voice
- STT → LLM → TTS → Opus pipeline
- OpenAI Realtime API or ElevenLabs streaming TTS
- OpenClaw plugin integration

### Phase 9: Video
- Add `video0` track to catalog
- Same architecture: encode → encrypt → MOQ publish → subscribe → decrypt → decode
- Activate DirectoryMessage for multi-track discovery

## 11. Deployment

### MOQ Relay
- **Config authority:** `~/configs/hosts/hetzner/moq.nix`
- **Endpoint:** `moq.justinmoon.com:443` (QUIC/WebTransport)
- **Action:** Pin flake input to latest stable upstream commit. Redeploy. Verify Quinn client connectivity.

### OpenClaw / marmotd
- **Config authority:** `~/code/infra`
- **Deployment:** NixOS systemd service

### Protocol
- `moq_url` in signaling messages — never hardcode relay in code
- v0 auth: anonymous (`anon/` path in relay config)
- Production auth: self-issued capabilities or JWT (hardening milestone)

## 12. Risk Notes

| Risk | Mitigation |
|------|-----------|
| cpal limited on iOS (AVAudioSession) | Pre-approved native shims with documented blockers |
| QUIC on mobile (network transitions) | QUIC connection migration; test WiFi↔cellular |
| Plaintext audio in dev | Hard gate: encryption before non-lab deployment |
| Anonymous relay access | Call IDs are random UUIDs; auth hardening before production |
| Opus cross-compilation (iOS) | Verify libopus links on iOS target early in Phase 1 |
| Nostr signaling latency | Reserved MOQ control track for future in-call migration |

## 13. Execution Status (2026-02-13)

Clarification: Phase 3 is complete. The only phase marked partial in 0-8 is Phase 2.

| Phase | Status | Notes |
|------|--------|-------|
| 0. Control scaffold | Complete | Call signaling/state machine + tests are in place. |
| 1. MOQ media plumbing (synthetic) | Complete | In-memory media session + Opus path + frame/order tests. |
| 2. Push-to-talk audio | Partial | Rust audio backend implemented; full device/manual QA remains. |
| 3. Bot echo | Complete | Implemented in `openclaw-marmot`; `scripts/phase3_audio.sh` passes (`sent_frames=50`, `echoed_frames=50`). |
| 4. Bot STT -> text | Complete | Implemented in `openclaw-marmot` daemon/sidecar flow (`call_stt.rs`, transcript events/emit path). |
| 5. Full duplex audio | Complete | Simultaneous send/receive, jitter prefill/bounds, overlap verification in local relay E2E. |
| 6. Encryption hardening | Complete | MLS-derived frame keys are required (fallback removed), opaque participant labels are enforced, relay auth capability token format/match is enforced in signaling/session, frame nonce now uses full counter width, replay-window drops duplicate/stale decrypted frames, and tx counter exhaustion is handled (unit + local relay E2E coverage). |
| 7. Android port | Complete | Android `cpal` path compiles; mic permission + audio focus shims are wired; `just android-rust`, `just android-assemble`, `just android-ui-test`, and `just android-ui-e2e-local` pass. |
| 8. Bot full duplex voice | Complete (in-memory lane) | `openclaw-marmot` now supports `call_transcript_final -> agent reply -> send_audio_response -> TTS -> Opus publish`; sidecar/plugin command path is wired and verified via `just pre-merge` + `phase8-voice` lane. |
| 9. Video | Pending | Not started. |

## 14. Pinned Follow-Ups / Blockers

1. Call media key derivation still uses an interim MDK path.
Current implementation derives call media keys from MLS group exporter material via MDK encrypted-media helper APIs. Replace with a dedicated first-class call media exporter API to reduce coupling.

2. Real cross-process MOQ media transport is still pending.
Current call media tests use the in-memory relay/session transport. To validate full real-device bot voice calls over process boundaries, switch to real MOQ relay transport in both `pika` and `openclaw-marmot` lanes.

3. Real speech E2E is not yet scripted as one command against the deployed bot.
The duplex voice pipeline is wired (STT -> LLM -> TTS -> Opus), but there is no single committed scenario that drives a live deployed call end-to-end with OpenAI keys and asserts audible round-trip.

4. Keep OpenClaw phase-4 script/plugin-id wiring in sync across checkouts.
The working config uses plugin id `openclaw-marmot` in `scripts/phase4_openclaw_marmot.sh`. Older copies that still reference `marmot` under `plugins.entries` will fail startup.

5. Android build pitfall: raw `cargo check --target aarch64-linux-android` is not a valid signal in this nix shell.
Use `cargo ndk ... -P 26` (now wired in `just android-rust` and `rmp-cli` bindings build) to ensure the correct NDK linker/sysroot and AAudio availability.
