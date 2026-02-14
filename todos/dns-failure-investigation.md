# DNS Failure in marmotd STT Worker Subscriber

## Problem (Historical)

When marmotd accepts a call and starts the STT (speech-to-text) worker, the worker's
subscriber fails to connect to the MOQ relay at `moq.justinmoon.com` with `failed DNS lookup`.
This prevents the bot from receiving the caller's audio frames for transcription.

The **echo worker** path uses the exact same `CallMediaTransport::for_session()` code and
works perfectly -- bidirectional audio flows (caller tx=85, rx=5). The DNS failure is 100%
reproducible in STT mode and 0% in echo mode across repeated runs.

## Architecture (Old)

```
marmotd process (child of test process)
  |
  +-- tokio main runtime (#[tokio::main])
  |     |
  |     +-- daemon event loop (select! on commands, nostr events, call events)
  |           |
  |           +-- on accept_call:
  |                 if MARMOT_ECHO_MODE=1 -> start_echo_worker()   [WORKS]
  |                 else                  -> start_stt_worker()     [DNS FAILS]
  |
  +-- Both workers call CallMediaTransport::for_session() which:
        1. NetworkRelay::new("https://moq.justinmoon.com/anon")
           - spawns a dedicated std::thread ("pika-network-relay")
           - thread creates its own tokio Runtime::new() (multi-threaded)
        2. relay.connect()
           - sends Command::Connect to worker thread
           - worker does rt.block_on(client.with_publish(origin).connect(url))
           - THIS SUCCEEDS (DNS resolves, QUIC handshake works, publish session established)
        3. transport.subscribe(peer_track)
           - sends Command::Subscribe to worker thread
           - worker does rt.spawn(async { ... }) to launch subscriber task
           - subscriber task creates a NEW moq_native::ClientConfig + QuinnClient
           - QuinnClient binds a new UDP socket
           - subscriber calls client.with_consume(origin).connect(url)
           - inside connect(): tokio::net::lookup_host(("moq.justinmoon.com", 443))
           - THIS FAILS with "failed DNS lookup" in STT mode
           - THIS WORKS in echo mode
```

## What's identical between the two paths

- Same `CallMediaTransport::for_session()` call
- Same `NetworkRelay::new()` + `connect()` + `subscribe()` sequence
- Same URL: `https://moq.justinmoon.com/anon`
- Same bind address: `[::]:0` (also tried `0.0.0.0:0`, same failure)
- Same caller code site in daemon.rs accept_call handler
- Both called from the same async select! loop context
- `connect()` (publish session) always succeeds in both cases
- `subscribe()` spawns the async task identically in both cases
- The `ClientConfig::init()` (QuinnClient::new) succeeds in both cases
- The child process has full internet access (DNS works from shell, curl works)

## What's different

The ONLY code difference between the two paths:

**Echo path** (`start_echo_worker`):
```rust
let transport = CallMediaTransport::for_session(session)?;
// subscribes, then spawns a spawn_blocking task that polls rx.try_recv()
```

**STT path** (`start_stt_worker`):
```rust
let transport = CallMediaTransport::for_session(session)?;
// subscribes, then calls transcriber_from_env() (returns a FixtureTranscriber),
// then spawns a spawn_blocking task that polls rx.try_recv()
```

The `transcriber_from_env()` call just checks an env var and returns a struct. It doesn't
open any network connections, files, or sockets. It's called AFTER the subscribe is already
spawned (the subscribe fires an async task and returns immediately).

## DNS resolution details

The failing call is in `moq-native/src/quinn.rs`:
```rust
let ip = tokio::net::lookup_host((host.clone(), port))
    .await
    .context("failed DNS lookup")?
```

This runs inside a tokio::spawn task on the NetworkRelay worker's own runtime (a multi-threaded
tokio runtime on a dedicated std::thread). The same DNS lookup succeeds moments earlier when
`connect()` establishes the publish session.

## Things ruled out

- **IPv6 vs IPv4**: Tried forcing subscriber bind to `0.0.0.0:0`, same failure
- **DNS resolution from process**: `dig moq.justinmoon.com` resolves fine (135.181.179.143)
- **HTTP3/QUIC connectivity**: `curl --http3 https://moq.justinmoon.com/` works
- **File descriptor limits**: `ulimit -n` is unlimited
- **Environment clearing**: child process inherits full parent environment
- **pika-media version**: Same failure with github version and local patch
- **Flakiness**: 0% pass rate for STT, 100% for echo across 6+ runs each

## MOQ server info

The MOQ relay at `moq.justinmoon.com` is hosted on a baremetal Hetzner server (135.181.179.143).
Hetzner sometimes has quirky networking behavior. The server is configured via `~/configs`.

## Hypothesis

Unknown. The publish session's `connect()` resolves DNS fine. The subscribe session's
`connect()` uses identical code path and fails. The only timing difference is that the subscribe
task runs asynchronously after the `subscribe()` method returns. Something about having two
concurrent QUIC endpoints (one from publish, one from subscribe) within the same
`NetworkRelay` worker runtime may trigger a condition where `tokio::net::lookup_host` fails.

## Root Cause (Found)

This is not a real DNS problem.

In the STT worker path, `CallMediaTransport` (and thus `NetworkRelay`) is dropped immediately
after `transport.subscribe(...)` returns, because the STT worker never uses `transport` again
and does not move it into the long-lived worker task/struct.

Dropping `NetworkRelay` sends `Command::Shutdown`, joins the relay thread, and drops the relay
thread's Tokio runtime. The subscriber task (spawned via `rt.spawn`) races with teardown and
fails during `tokio::net::lookup_host`, which surfaces as `"failed DNS lookup"`.

Echo mode works because the echo worker *does* keep `transport` alive (it publishes on it), so
the relay runtime stays up while the subscriber finishes connecting.

## Fix (Implemented)

### 1. API Hardening: Subscription Keepalive Guard

- `pika-media` now returns a guarded subscription type (`MediaFrameSubscription`) rather than a
  bare `std::sync::mpsc::Receiver`.
- The subscription holds a keepalive reference to the `NetworkRelay` worker thread/runtime, so
  dropping the transport handle no longer tears down the runtime out from under an in-flight
  subscriber task.

### 2. Remove Second QUIC Connection (Publish+Consume on One Session)

- `NetworkRelay::connect()` now configures the moq client with both `with_publish(...)` and
  `with_consume(...)` on a single connection.
- `subscribe()` no longer creates a second moq-native client/session, so the second DNS lookup
  path is gone entirely.

### 3. marmotd: Transcript Event Delivery After Call End

- `CallWorkerEvent::TranscriptFinal` is now always emitted to stdout, even if the call was
  already torn down (previously it was dropped if `active_call` was cleared before the worker
  flushed).

## Test Coverage (Implemented)

- `e2e_local_marmotd_call` now:
  - in echo mode: requires caller `rx_frames > 0` (existing)
  - in STT mode: waits for daemon `call_debug.rx_frames > 0`, then requires a
    `call_transcript_final` containing the fixture text

## Relevant files

- `crates/pika-media/src/network.rs` -- NetworkRelay implementation (subscribe at line ~284)
- `~/.cargo/git/checkouts/moq-60c58c1d36212c6d/5ad5c06/rs/moq-native/src/quinn.rs` -- DNS lookup
- `~/code/openclaw-marmot-audio-2/marmotd/src/daemon.rs` -- start_stt_worker vs start_echo_worker
- `~/code/openclaw-marmot-audio-2/marmotd/src/call_stt.rs` -- transcriber_from_env()

## Reproduction

```bash
# STT mode:
cd ~/code/pika/worktrees/audio-2-dns-codex
MARMOTD_BIN=~/code/openclaw-marmot-audio-2/target/debug/marmotd \
  RUST_LOG=info cargo test -p pika_core call_with_local_marmotd \
  --test e2e_local_marmotd_call -- --nocapture --exact

# Echo mode (PASSES):
MARMOTD_BIN=~/code/openclaw-marmot-audio-2/target/debug/marmotd \
  MARMOT_ECHO_MODE=1 RUST_LOG=info cargo test -p pika_core call_with_local_marmotd \
  --test e2e_local_marmotd_call -- --nocapture --exact
```

Note: openclaw-marmot-audio-2 must have `~/.cargo/config.toml` patch pointing pika-media at
the local worktree for latest subscribe retry changes. See
`~/code/openclaw-marmot-audio-2/.cargo/config.toml`.
