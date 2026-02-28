# TODO: Channel/Environment Bootstrap for Deep Links

## Spec

Deep-link routing is currently brittle because "which installed app should open this URL"
is handled with ad-hoc scheme logic (`pika`, `pikadev`, `pikatest`, bundle-id suffix rules).
We need one explicit notion of **channel** for app identity and URL scheme selection, so
`pikachat qr` and mobile app callback handling stay aligned.

This implementation focuses on the smallest end-to-end slice:

- Add a source-of-truth channel manifest (`prod`, `dev`, `test`).
- Make `pikachat qr` channel-aware by default.
- Remove iOS callback scheme derivation from bundle-id hacks and use the app's registered scheme.
- Parameterize Android callback/chat deep-link scheme so it is not hardcoded in Kotlin + manifest.

Environment profiles (`prod/staging/local/ephemeral`) remain documented, but we are not yet
wiring full backend-environment switching in this change.

## Plan

### 1. Add channel manifest and helper plumbing

Files: `config/channels.json`, `tools/lib/channel-config.sh` (new), docs touched as needed.

- Define canonical channels (`prod`, `dev`, `test`) with URL scheme + iOS/Android identifiers.
- Add shell helpers to read channel metadata and resolve a channel from bundle/app id.

Acceptance: manifest exists and helpers can return scheme/bundle/app-id for a known channel.

### 2. Make `pikachat qr` channel-aware

Files: `cli/src/main.rs` (and any small helper module if needed).

- Replace ad-hoc scheme default with channel selection (`--channel`, default `prod`).
- Preserve `--scheme` as an explicit override.
- Build deep links from resolved channel metadata.

Acceptance: `pikachat qr` defaults to `pika://...`; `pikachat qr --channel test` emits `pikatest://...`.

### 3. Fix iOS scheme handling to use explicit channel data

Files: `ios/Sources/IOSExternalSignerBridge.swift`, `ios/Sources/AppManager.swift`, `tools/run-ios`, tests/docs as needed.

- Read callback scheme from runtime-registered URL scheme instead of bundle-id suffix logic.
- In `tools/run-ios`, resolve scheme from explicit channel mapping (with safe fallback behavior).
- Keep existing callback/deep-link behavior intact for prod.

Acceptance: iOS callback/deep-link scheme selection is no longer inferred from `bundle_id.last_segment`.

### 4. Parameterize Android deep-link/callback scheme

Files: `android/app/build.gradle.kts`, `android/app/src/main/AndroidManifest.xml`, `android/app/src/main/java/com/pika/app/AppManager.kt`, tests as needed.

- Introduce a single Gradle-configured scheme value.
- Use it for manifest intent filters and Kotlin callback/chat parsing.

Acceptance: Android scheme can be overridden via build config and logic uses one source value.

### 5. Verification + manual QA gate

- Run focused tests/build checks for edited areas.
- Document manual QA commands to validate channel behavior (`prod` vs `test`) for `pikachat qr` and mobile deep-link opens.

Acceptance: checks pass or are explicitly documented as deferred, with manual QA steps listed.
