# Android Audio Calls E2E — Problem Space & Lessons from iOS

## Goal

Get audio calls working end-to-end on Android, including automated testing. The deployed OpenClaw bot on the Hetzner server accepts calls via MOQ relay (`moq.justinmoon.com`).

## What We Know Works

- **Rust core on macOS**: Full call pipeline works — QUIC connect to MOQ relay, publish audio, subscribe to bot's audio, STT/TTS with OpenAI. All Rust e2e tests pass.
- **iOS simulator**: XCUITest (`CallE2ETests.swift`) passes — login, create chat, start call, "Call active", tx=300+ rx=1, end call. 26s runtime.
- **FfiApp-level Rust test** (`rust/tests/e2e_deployed_bot_call.rs`): Exercises the exact same Rust code path as iOS/Android apps (Login, CreateChat, StartCall, media flow, EndCall). Passes on macOS.

## What's Broken on iOS Device (You'll Hit Similar Issues)

### 1. TLS/QUIC connection fails on real devices

The Pika app crashes ~5s after starting a call on a physical iPhone. Root cause: `rustls_native_certs::load_native_certs()` tries to read certificate files from the filesystem. On iOS devices, system root CAs are in the Keychain, not on disk, so it returns zero certs and the TLS handshake fails.

**Fix applied (may need Android equivalent):**
- Added `NetworkRelay::with_options(moq_url, tls_disable_verify)` in `crates/pika-media/src/network.rs`
- In `rust/src/core/call_runtime.rs`, set `tls_disable_verify = cfg!(target_os = "ios")` 
- **Android will need the same**: `cfg!(target_os = "android")` — `rustls_native_certs` also doesn't work on Android. The native cert store is in Java-land (KeyStore), not accessible from Rust.
- Long-term fix: use `webpki-roots` crate (embedded Mozilla CA bundle) instead of `tls_disable_verify`. This is better than disabling verification.

### 2. OpenSSL dependency from SQLCipher

`mdk-sqlite-storage` uses `rusqlite` with `bundled-sqlcipher`. The `pika_core` Cargo.toml previously had `libsqlite3-sys` with `bundled-sqlcipher-vendored-openssl`, pulling in OpenSSL (which fails to cross-compile for mobile targets).

**Fix applied:** Changed to `bundled-sqlcipher` (no vendored OpenSSL). On Apple platforms, this uses CommonCrypto. On Android, `libsqlite3-sys` will use OpenSSL if available or may need a different crypto backend. Check if the Android NDK provides a compatible crypto library, or if you need to explicitly configure this.

### 3. Config not written on device

`ios/Sources/AppManager.swift` wasn't writing `pika_config.json` with `call_moq_url` when no relay env vars were set. Fixed to always write config with default `https://moq.justinmoon.com/anon`.

**Android equivalent:** Check `android/` app code to ensure `call_moq_url` and `call_broadcast_prefix` are written to the config file the Rust core reads.

### 4. `reqwest::blocking::Client` panics inside tokio runtime

Both STT and TTS code used `reqwest::blocking::Client` which panics when called from within a tokio runtime context. Fixed by wrapping in `spawn_blocking` (STT) and `thread::spawn` (TTS).

**This fix is in the Rust core, so Android benefits automatically.**

## Testing Strategy Options

### Option A: cargo-dinghy (Rust tests on device)

We installed `cargo-dinghy` for iOS. It cross-compiles Rust test binaries, wraps them in an app bundle, signs, deploys, runs on device, and streams stdout back. Same tool works for Android (with Android NDK).

```
cargo dinghy -d <android_device> test -p pika-media --features network
cargo dinghy -d <android_device> run --example quic_connect_test -p pika-media --features network
```

Requires: Android NDK, device connected via ADB, `cargo-dinghy` installed.

**Pros:** Fast iteration, pure Rust, see exact panic/crash output.
**Cons:** Doesn't test the full app (Kotlin UI, JNI bridge, Android lifecycle).

### Option B: Android Instrumented Tests (like XCUITest)

The existing `PikaE2eUiTest` in `android/` already tests ping/pong against the deployed bot. Extend it to test calls.

See `tools/ui-e2e-public` for how the existing test is invoked:
```
cd android && ./gradlew :app:connectedDebugAndroidTest \
  -Pandroid.testInstrumentationRunnerArguments.class=com.pika.app.PikaE2eUiTest \
  -Pandroid.testInstrumentationRunnerArguments.pika_e2e=1 \
  ...
```

**Pros:** Tests the full app stack (Kotlin + JNI + Rust).
**Cons:** Slower iteration, harder to debug Rust-level crashes.

### Option C: FfiApp Rust test with Android target

`rust/tests/e2e_deployed_bot_call.rs` uses `FfiApp` which is the same Rust entry point both iOS and Android use. It already passes on macOS. If you can get it to compile and run for the Android target (via dinghy or an emulator), it tests the exact code path without needing the Kotlin UI.

### Option D: Emulator-only

Android emulators run on x86_64 (or arm64 on Apple Silicon). You might be able to run tests on an emulator without a physical device. The emulator has a full Android network stack, so you'd hit the same TLS/cert issues as a real device (good for testing fixes).

## Key Files

| File | What |
|------|------|
| `crates/pika-media/src/network.rs` | NetworkRelay — QUIC/MOQ transport, `with_options()` for TLS bypass |
| `rust/src/core/call_runtime.rs` | Where `NetworkRelay` is created, iOS TLS bypass logic |
| `rust/tests/e2e_deployed_bot_call.rs` | FfiApp-level e2e test (Login → Chat → Call → Media → EndCall) |
| `crates/pika-media/examples/quic_connect_test.rs` | Minimal QUIC connect test (two modes: native certs vs disable-verify) |
| `ios/UITests/CallE2ETests.swift` | Swift XCUITest for reference (Android equivalent needed) |
| `tools/ui-e2e-public` | Script that runs both iOS and Android UI e2e tests |
| `rust/Cargo.toml` | `libsqlite3-sys` feature flags (OpenSSL removal) |
| `.env` | Test nsec, bot npub, relay URLs |
| `AGENTS.md` | Bot npub, whitelisted keys, relay list |

## What To Do

1. **Make QUIC connect work on Android** — Add `cfg!(target_os = "android")` to the `tls_disable_verify` check in `call_runtime.rs`, or better, switch to `webpki-roots` for all mobile targets.

2. **Verify SQLCipher builds without OpenSSL** — The `bundled-sqlcipher` feature may need different crypto backend config for Android NDK. Test `cargo check --target aarch64-linux-android -p pika_core`.

3. **Test QUIC connectivity** — Use `cargo-dinghy` or the Android emulator to run `quic_connect_test` example and confirm the connection succeeds.

4. **Run FfiApp e2e test on Android** — Get `e2e_deployed_bot_call.rs` running on Android (emulator or device).

5. **Create Android instrumented test for calls** — Extend `PikaE2eUiTest` to start a call, wait for "Call active", verify media frames, end call. Mirror what `CallE2ETests.swift` does.

6. **Ensure Android app writes call config** — Check that `call_moq_url` reaches the Rust core when running on device/emulator.

## Environment

- Bot npub: `npub1z6ujr8rad5zp9sr9w22rkxm0truulf2jntrks6rlwskhdmqsawpqmnjlcp`
- MOQ relay: `https://moq.justinmoon.com/anon`
- Test nsec: in `.env` as `PIKA_TEST_NSEC`
- Relays: `wss://relay.primal.net`, `wss://nos.lol`, `wss://relay.damus.io`
- cargo-dinghy is installed globally
- `pymobiledevice3` installed at `/Users/justin/Library/Python/3.9/bin`
