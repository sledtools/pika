---
name: e2e-video
description: Run E2E UI tests with screen recording and upload videos to blossom
allowed-tools: Bash, Read, Grep, Glob
---

# E2E Video Recorder

Run local iOS/Android UI E2E tests with automatic MP4 screen recordings, then upload to blossom for PR descriptions.

Takes an optional platform argument: `ios`, `android`, or `both` (default: `both`).
Takes an optional test method argument to run a single test.

## Step 1: Run tests with recording

### iOS

```bash
PIKA_UI_E2E_RECORD_VIDEO=1 KEEP=1 just ios-ui-e2e-local
```

Single test method:
```bash
PIKA_UI_E2E_RECORD_VIDEO=1 KEEP=1 \
  PIKA_IOS_E2E_TEST_METHOD=<testMethodName> \
  just ios-ui-e2e-local
```

### Android (requires nix)

```bash
nix develop -c env PIKA_UI_E2E_RECORD_VIDEO=1 KEEP=1 just android-ui-e2e-local
```

Single test method:
```bash
nix develop -c env \
  PIKA_UI_E2E_RECORD_VIDEO=1 KEEP=1 \
  PIKA_ANDROID_E2E_TEST_CLASS='com.pika.app.PikaE2eUiTest#<methodName>' \
  just android-ui-e2e-local
```

## Step 2: Verify recordings

Recordings are written to `screen_recordings/` with timestamped filenames:
- `ios-ui-e2e-local-<unix_ms>.mp4`
- `android-ui-e2e-local-<unix_ms>.mp4`

Verify:
```bash
ffprobe screen_recordings/<file>.mp4
```

## Step 3: Upload to blossom

```bash
cargo run -p pikahut -- blossom-upload screen_recordings/*.mp4
```

URLs are printed to stdout, one per line. Use these in PR descriptions.

To upload a single file:
```bash
cargo run -p pikahut -- blossom-upload screen_recordings/<file>.mp4
```

## Available test methods

### iOS (`PIKA_IOS_E2E_TEST_METHOD`)
- `testE2E_deployedRustBot_pingPong`
- `testE2E_hypernoteDetailsAndCodeBlock`
- `testE2E_multiImageGrid`

### Android (`PIKA_ANDROID_E2E_TEST_CLASS`)
- `com.pika.app.PikaE2eUiTest#e2e_deployedRustBot_pingPong`
- `com.pika.app.PikaE2eUiTest#e2e_hypernoteDetailsAndCodeBlock`
