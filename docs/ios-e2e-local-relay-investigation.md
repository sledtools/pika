---
summary: Investigation into why iOS simulator E2E tests cannot communicate with the local Rust bot
read_when:
  - debugging ios-ui-e2e-local test failures
  - investigating relay connectivity between iOS simulator and local pikahub infrastructure
---

# iOS Simulator E2E Local Relay Investigation

**Date:** 2026-03-02
**Status:** Open — bot receives welcome but never receives messages from the iOS app
**Affects:** `just ios-ui-e2e-local` (both existing ping/pong test AND new hypernote tests)

## Problem

When running `just ios-ui-e2e-local`, the iOS simulator app successfully creates a chat with the local Rust bot, but subsequent messages never reach the bot. Both `testE2E_deployedRustBot_pingPong` and `testE2E_hypernoteDetailsAndCodeBlock` time out after 180s waiting for a reply that never comes.

## What works

- **CLI-to-bot roundtrip is fully functional.** Using `pikachat` CLI on the host:
  1. `pikachat invite --peer <bot_npub>` — bot receives welcome, joins group
  2. `pikachat send --group <group_id> --content "ping:test123"` — bot receives message
  3. Bot replies `pong:test123` — CLI receives reply via `pikachat listen`

  This was verified with `--relay ws://localhost:<port> --kp-relay ws://localhost:<port>` against the same local relay+bot infrastructure that `ios-ui-e2e-local` uses.

- **iOS app UI flow works.** The app on the simulator:
  1. Launches, logs in with nsec
  2. Creates a chat with the bot's npub (chat creation succeeds — composer appears within ~5s)
  3. Sends `ping:<nonce>` or `hn:details` messages (send button works, message appears in chat)

- **Bot processes welcomes correctly.** Bot log shows:
  ```
  [openclaw_bot] welcome_unwrapped wrapper_id=... rumor_kind=444 ...
  [openclaw_bot] joined_group nostr_group_id=...
  ```

## What doesn't work

After the bot joins the group, it subscribes to kind=445 (MLS group message) events for that `nostr_group_id`. The app sends messages. But the bot's subscription **never delivers any kind=445 events**. The bot sits idle until timeout.

## What we tried

### 1. Added `ping:<nonce>` support to Rust bot
The `pikachat bot` (used by pikahub) only handled bare `ping` and `openclaw: reply exactly "..."` protocols. The iOS test sends `ping:<nonce>`. Added full support in `cli/src/harness.rs` — but this didn't help because the bot never receives any messages at all.

### 2. Verified relay connectivity
- Relay starts fine on random port (e.g. `ws://localhost:50448`)
- Bot connects and publishes key package to the same relay
- CLI client successfully invites, sends, and receives on the same relay
- The relay itself is not the problem

### 3. Checked environment variable propagation
The relay URL flows through this chain:
1. `tools/ui-e2e-local` sets `PIKA_UI_E2E_RELAYS="${RELAY_URL}"` and `PIKA_UI_E2E_KP_RELAYS="${RELAY_URL}"`
2. `tools/ios-sim-ensure` propagates via `simctl spawn <udid> launchctl setenv`
3. XCUITest reads from `ProcessInfo.processInfo.environment`
4. Test sets `app.launchEnvironment["PIKA_RELAY_URLS"] = relays` and `app.launchEnvironment["PIKA_KEY_PACKAGE_RELAY_URLS"] = kpRelays`
5. App reads in `AppManager.swift:178-179`

The env var early-validation checks in the test pass (the test doesn't fail with "Missing env var"), so the values DO reach the XCUITest process. But we haven't confirmed the values actually reach the app process.

### 4. Checked bot message processing
The bot uses `subscribe_group_msgs` to subscribe to kind=445 events filtered by `nostr_group_id`. If the app sends to a different relay or a different group ID, the subscription would miss them.

## Hypotheses (in order of likelihood)

### H1: App ignores local relay for message sending
The app may successfully use the local relay for key package fetch (explaining why chat creation works) but fall back to default public relays for sending messages. The `pika_config.json` file that the Rust core reads might not get the relay URL written correctly, or the Rust core might have hardcoded fallback relays for message publishing.

**How to test:** Add logging in `AppManager.swift` to print the relay URLs being written to `pika_config.json`, and in `rust/src/core/config.rs` to print what the Rust core actually loads.

### H2: App sends to correct relay but with wrong group metadata
The app might send kind=445 events without the `nostr_group_id` tag that the bot's subscription filter expects. Or it might use a different tagging convention.

**How to test:** Query the local relay directly for all kind=445 events after the test runs, to see if the app's messages are actually there.

### H3: Simulator networking issue
The iOS simulator should route `localhost` to the host, but there may be edge cases with certain URL schemes or WebSocket connections that don't work reliably.

**How to test:** Try with `ws://127.0.0.1:<port>` instead of `ws://localhost:<port>`.

### H4: Timing / subscription race
The bot's subscription might start too late (after the app already sent the message), and the relay might not deliver historical events to new subscriptions.

**How to test:** Send the message from the app, wait 30s, then check if the bot's next listen cycle picks it up. The bot restarts its subscription every 30s chunk, but `pikachat bot` uses a persistent subscription, so this is less likely.

## Key files

| File | Role |
|------|------|
| `cli/src/harness.rs` | Rust bot implementation (`cmd_bot`, `parse_openclaw_prompt`, `parse_hypernote_probe`) |
| `ios/Sources/AppManager.swift:169-179` | App reads relay URLs from env |
| `rust/src/core/config.rs:31-36` | Rust core loads `pika_config.json` |
| `tools/ios-sim-ensure` | Propagates env vars into simulator via `launchctl setenv` |
| `crates/pikahut/src/testing/scenarios/deterministic.rs` | New Rust-based E2E test runner |

## Suggested next steps

1. **Add relay debug logging** to the iOS app — print what URLs end up in `pika_config.json` and what the Rust core actually uses for relay connections
2. **Query the relay directly** after a failed test run to see if the app's messages arrived at the local relay
3. **Try the Android E2E** — if Android works, the issue is iOS-specific (simulator networking or env propagation)
4. **Try with a physical device** instead of simulator to rule out simulator networking
