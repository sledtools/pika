---
summary: Run local iOS/Android UI E2E tests with automatic MP4 screen recordings
read_when:
  - running deterministic ui_e2e_local scenarios and needing visual artifacts
  - debugging local UI E2E behavior with simulator/emulator recordings
---

# How To Run UI E2E Tests With Video

This doc explains how to run local UI E2E tests and automatically save screen recordings.

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

## Verify A Recording

```bash
ffprobe screen_recordings/<file>.mp4
```

If `ffprobe` prints stream and duration metadata, the container is valid.
