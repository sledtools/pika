---
summary: Run local iOS/Android UI E2E tests with automatic MP4 screen recordings
read_when:
  - running deterministic ui_e2e_local scenarios and needing visual artifacts
  - debugging local UI E2E behavior with simulator/emulator recordings
---

# How To Run UI E2E Tests With Video

This doc explains how to run the local-fixture UI E2E selectors and automatically save screen recordings.
These commands are for local relay/bot fixtures, not public-relay canaries.

## Video Output

Recordings are written to:

- `screen_recordings/`

Filenames are timestamped for uniqueness:

- `ios-ui-e2e-local-<unix_ms>.mp4`
- `android-ui-e2e-local-<unix_ms>.mp4`

`screen_recordings/` is gitignored.

## Required Flags

- `PIKA_UI_E2E_RECORD_VIDEO=1` enables recording.
- `KEEP=1` preserves local test state/artifacts after success.

## iOS

Run:

```bash
PIKA_UI_E2E_RECORD_VIDEO=1 KEEP=1 just ios-ui-e2e-local
```

## Android (recommended via nix)

Run:

```bash
nix develop -c env PIKA_UI_E2E_RECORD_VIDEO=1 KEEP=1 just android-ui-e2e-local
```

## iOS: Run Specific E2E Methods

Use `PIKA_IOS_E2E_TEST_METHOD` to run a single test, for example multi-image grid only:

```bash
PIKA_UI_E2E_RECORD_VIDEO=1 KEEP=1 \
  PIKA_IOS_E2E_TEST_METHOD=testE2E_multiImageGrid \
  just ios-ui-e2e-local
```

Or ping-pong only:

```bash
PIKA_IOS_E2E_TEST_METHOD=testE2E_deployedRustBot_pingPong just ios-ui-e2e-local
```

The iOS `testE2E_*` names are legacy selector hooks. `just ios-ui-e2e-local` still runs them against a local fixture.

## Android: Run Specific E2E Methods

Use `PIKA_ANDROID_E2E_TEST_CLASS` to limit runtime, for example ping-only:

```bash
nix develop -c env \
  PIKA_UI_E2E_RECORD_VIDEO=1 \
  KEEP=1 \
  PIKA_ANDROID_E2E_TEST_CLASS='com.pika.app.PikaE2eUiTest#e2e_deployedRustBot_pingPong' \
  just android-ui-e2e-local
```

Or hypernote-only:

```bash
nix develop -c env \
  PIKA_UI_E2E_RECORD_VIDEO=1 \
  KEEP=1 \
  PIKA_ANDROID_E2E_TEST_CLASS='com.pika.app.PikaE2eUiTest#e2e_hypernoteDetailsAndCodeBlock' \
  just android-ui-e2e-local
```

The Android `PikaE2eUiTest` class name is also legacy. `just android-ui-e2e-local` injects a local relay/bot fixture rather than public infrastructure.

## Verify A Recording

```bash
ffprobe screen_recordings/<file>.mp4
```

If `ffprobe` prints stream and duration metadata, the container is valid.

## Upload Recordings To Blossom

Upload all recordings and get back URLs (for PR descriptions, etc.):

```bash
cargo run -p pikahut -- blossom-upload screen_recordings/*.mp4
```

Or upload a single file:

```bash
cargo run -p pikahut -- blossom-upload screen_recordings/ios-ui-e2e-local-1234567890.mp4
```

URLs are printed to stdout, one per line. An ephemeral nostr key is generated automatically — no configuration needed.

To override the default blossom servers:

```bash
cargo run -p pikahut -- blossom-upload --server https://my-blossom.example.com screen_recordings/*.mp4
```
