---
summary: Precise definitions for app channels vs backend environments, with a minimal model and staging plan
read_when:
  - deciding dev/test/prod naming
  - debugging deep-link routing across installed app variants
  - introducing a staging backend
---

# Channel vs Environment

This doc uses two separate terms on purpose:

- **Channel** = app/build identity on a device (`bundle id`, `application id`, URL scheme).
- **Environment** = backend/network target that app instance talks to (relays, MoQ, notification server, etc.).

If you only remember one rule: **deep-link routing is a channel problem, not an environment problem**.

## Why This Distinction Exists

Without this split, one word ("environment") ends up meaning both:

- "which app opens this URL?"
- "which backend does this app talk to?"

Those are different concerns and change at different times.

## Channel Definitions

Canonical channel IDs should be: `prod`, `dev`, `test`.

| Channel | Exact Meaning | iOS Identity | Android Identity | URL Scheme | Typical Distribution |
|---|---|---|---|---|---|
| `prod` | Public app identity. Customer-facing. | `org.pikachat.pika` | `org.pikachat.pika` | `pika://` | App Store / Play / signed release artifacts |
| `dev` | Developer local build identity. Safe to install beside prod. | `org.pikachat.pika.dev` (or per-dev override) | `org.pikachat.pika.dev` | `pikadev://` | Local debug installs |
| `test` | Optional internal QA/dogfood identity (separate install). | `org.pikachat.pikatest` (team-dependent) | `org.pikachat.pika.test` | `pikatest://` | Internal prerelease lane |

Important clarification:

- A TestFlight build using `org.pikachat.pika` is still **channel=`prod`**, even if the build is prerelease.
- `test` channel only exists if you intentionally use a distinct app identity/scheme.

## Environment Definitions

Canonical environment IDs should be: `prod`, `staging`, `local`, `ephemeral`.

| Environment | Exact Meaning | Endpoint Source | Lifecycle | Primary Use |
|---|---|---|---|---|
| `prod` | Stable public backend and relays used by real users. | Pinned production endpoints | Long-lived | Normal user traffic |
| `staging` | Production-like backend for integration validation before release. | Auto-deployed from `master` | Long-lived | Dogfooding, release checks |
| `local` | Developer machine stack (e.g. `pikahub`, local relay/server/MoQ). | Local process URLs | Short/medium-lived | Fast iteration, deterministic local tests |
| `ephemeral` | Temporary isolated backend spun up for CI or short-lived test runs. | Job-specific dynamic URLs | Very short-lived | CI integration tests, throwaway test envs |

## Recommended Defaults Matrix

| Channel | Default Environment | Allowed Overrides |
|---|---|---|
| `prod` | `prod` | `staging` only for explicit internal smoke checks |
| `dev` | `local` | `staging`, `prod` |
| `test` | `staging` | `local`, `prod` (rare) |

This avoids combinatorial chaos while keeping flexibility.

## Minimal Abstraction (Not Over-Engineered)

You do not need a giant framework. Keep only:

1. One small **channel manifest** (source of truth for IDs/schemes).
2. One small **environment profile map** (source of truth for endpoints).
3. Runtime/tooling flags that select one channel and one environment.

Do **not** add channel/environment into Rust `AppState`; this is startup/build/runtime config, not user-visible core state.

## Concrete Values To Standardize

| Key | `prod` | `dev` | `test` |
|---|---|---|---|
| iOS bundle ID | `org.pikachat.pika` | `org.pikachat.pika.dev` | `org.pikachat.pikatest` |
| Android app ID | `org.pikachat.pika` | `org.pikachat.pika.dev` | `org.pikachat.pika.test` |
| Deep-link scheme | `pika` | `pikadev` | `pikatest` |
| QR prefix | `pika://chat/...` | `pikadev://chat/...` | `pikatest://chat/...` |

## Manual QA Commands

Use these to verify channel-specific deep-link routing:

1. CLI QR scheme:
   - `cargo run -q -p pikachat -- qr --channel prod`
   - `cargo run -q -p pikachat -- qr --channel test`
2. iOS open-url routing:
   - `PIKA_CHANNEL=test ./tools/run-ios --sim`
   - `xcrun simctl openurl booted "pikatest://chat/<npub-or-hex>"`
3. Android open-url routing:
   - `cd android && PIKA_ANDROID_URL_SCHEME=pikatest ./gradlew :app:installDebug`
   - `adb shell am start -a android.intent.action.VIEW -d "pikatest://chat/<npub-or-hex>"`

## Staging Backend Plan (Auto-Deploy on `master`)

If the goal is "staging auto-deployed from master", the smallest useful plan is:

1. Stand up a permanent staging backend stack (server + relays + MoQ + notif endpoint).
2. Add CI/CD on `master` to deploy staging and publish current staging endpoint set.
3. Add `environment=staging` profile in app/tooling config.
4. Default `test` channel to `staging`; keep `dev` default on `local`.
5. Add one command per platform to run `dev+staging` quickly for manual QA.

This gives you immediate value without introducing heavy architecture.
