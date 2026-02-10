set shell := ["bash", "-lc"]

# List available recipes.
default:
  @just --list

# Run pika_core tests.
test *ARGS:
  cargo test -p pika_core {{ARGS}}

# Check formatting (cargo fmt).
fmt:
  cargo fmt --all --check

# Lint with clippy.
clippy *ARGS:
  cargo clippy -p pika_core {{ARGS}} -- -D warnings

# CI-safe pre-merge: skips cdylib/staticlib (OOM on 7GB GitHub runners).
pre-merge: fmt
  just clippy --lib --tests
  just test --lib --tests
  cargo build -p pika-cli
  npx -y @justinmoon/agent-tools check-docs
  npx -y @justinmoon/agent-tools check-justfile
  @echo "pre-merge complete"

# Full QA: fmt, clippy, test, android build, iOS sim build.
qa: fmt clippy test android-assemble ios-build-sim
  @echo "QA complete"

# Deterministic E2E: local docker relay + local Rust bot (iOS + Android).
e2e-local-relay:
  just ios-ui-e2e-local
  just android-ui-e2e-local

# E2E against public relays + deployed bot (nondeterministic).
e2e-public-relays:
  #!/usr/bin/env bash
  set -euo pipefail
  # Load local secrets if present (gitignored).
  if [ -f .env ]; then
    set -a
    # shellcheck disable=SC1091
    . ./.env
    set +a
  fi
  : "${PIKA_TEST_NSEC:?missing (put it in ./.env)}"
  : "${PIKA_UI_E2E_BOT_NPUB:?missing (put it in ./.env)}"
  : "${PIKA_UI_E2E_RELAYS:?missing (put it in ./.env)}"
  : "${PIKA_UI_E2E_KP_RELAYS:?missing (put it in ./.env)}"
  just e2e-public
  just ios-ui-e2e
  just android-ui-e2e

# Rust-level E2E smoke test against public relays (nondeterministic).
e2e-public:
  PIKA_E2E_PUBLIC=1 cargo test -p pika_core --test e2e_public_relays -- --ignored --nocapture

# Build Rust core for the host platform.
rust-build-host:
  cargo build -p pika_core --release

# Generate Kotlin bindings via UniFFI.
gen-kotlin: rust-build-host
  mkdir -p android/app/src/main/java/com/pika/app/rust
  # Resolve the host cdylib extension (dylib on macOS, so on Linux).
  LIB=$(ls -1 target/release/libpika_core.dylib target/release/libpika_core.so target/release/libpika_core.dll 2>/dev/null | head -n 1); \
  if [ -z "$LIB" ]; then echo "Missing built library: target/release/libpika_core.*"; exit 1; fi; \
  cargo run -q -p uniffi-bindgen -- generate \
    --library "$LIB" \
    --language kotlin \
    --out-dir android/app/src/main/java \
    --no-format \
    --config rust/uniffi.toml

# Cross-compile Rust core for Android (arm64, armv7, x86_64).
android-rust:
  mkdir -p android/app/src/main/jniLibs
  cargo ndk -o android/app/src/main/jniLibs \
    -t arm64-v8a -t armeabi-v7a -t x86_64 \
    build -p pika_core --release

# Write android/local.properties with SDK path.
android-local-properties:
  SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"; \
  if [ -z "$SDK" ]; then echo "ANDROID_HOME/ANDROID_SDK_ROOT not set (run inside nix develop)"; exit 1; fi; \
  printf "sdk.dir=%s\n" "$SDK" > android/local.properties

# Build Android debug APK.
android-assemble: gen-kotlin android-rust android-local-properties
  cd android && ./gradlew :app:assembleDebug

# Build and install Android debug APK on connected device.
android-install: gen-kotlin android-rust android-local-properties
  cd android && ./gradlew :app:installDebug

# Run Android instrumentation tests (requires running emulator/device).
android-ui-test: gen-kotlin android-rust android-local-properties
  cd android && ./gradlew :app:connectedDebugAndroidTest

# Android E2E: local docker relay + local Rust bot. Requires Docker + emulator.
android-ui-e2e-local:
  ./tools/ui-e2e-local --platform android

# Android E2E: public relays + deployed bot (nondeterministic). Requires emulator.
android-ui-e2e: gen-kotlin android-rust android-local-properties
  #!/usr/bin/env bash
  set -euo pipefail
  # Load local secrets if present (gitignored).
  if [ -f .env ]; then
    set -a
    . ./.env
    set +a
  fi

  : "${PIKA_TEST_NSEC:?missing (put it in ./.env)}"
  : "${PIKA_UI_E2E_BOT_NPUB:?missing (put it in ./.env)}"
  : "${PIKA_UI_E2E_RELAYS:?missing (put it in ./.env)}"
  : "${PIKA_UI_E2E_KP_RELAYS:?missing (put it in ./.env)}"

  peer="${PIKA_UI_E2E_BOT_NPUB}"
  relays="${PIKA_UI_E2E_RELAYS}"
  kp_relays="${PIKA_UI_E2E_KP_RELAYS}"
  nsec="${PIKA_UI_E2E_NSEC:-${PIKA_TEST_NSEC}}"

  ./tools/android-emulator-ensure

  cd android && ./gradlew :app:connectedDebugAndroidTest \
    -Pandroid.testInstrumentationRunnerArguments.class=com.pika.app.PikaE2eUiTest \
    -Pandroid.testInstrumentationRunnerArguments.pika_e2e=1 \
    -Pandroid.testInstrumentationRunnerArguments.pika_disable_network=false \
    -Pandroid.testInstrumentationRunnerArguments.pika_reset=1 \
    -Pandroid.testInstrumentationRunnerArguments.pika_peer_npub="$peer" \
    -Pandroid.testInstrumentationRunnerArguments.pika_relay_urls="$relays" \
    -Pandroid.testInstrumentationRunnerArguments.pika_key_package_relay_urls="$kp_relays" \
    -Pandroid.testInstrumentationRunnerArguments.pika_nsec="$nsec"

# Generate Swift bindings via UniFFI.
ios-gen-swift: rust-build-host
  mkdir -p ios/Bindings
  cargo run -q -p uniffi-bindgen -- generate \
    --library target/release/libpika_core.dylib \
    --language swift \
    --out-dir ios/Bindings \
    --config rust/uniffi.toml

# Cross-compile Rust core for iOS (device + simulator).
ios-rust:
  set -euo pipefail; \
  DEV_DIR=$(ls -d /Applications/Xcode*.app/Contents/Developer 2>/dev/null | sort | tail -n 1); \
  if [ -z "$DEV_DIR" ]; then echo "Xcode not found under /Applications (needed for iOS SDK)"; exit 1; fi; \
  env -u LIBRARY_PATH DEVELOPER_DIR="$DEV_DIR" RUSTFLAGS="-C link-arg=-miphoneos-version-min=17.0" cargo build -p pika_core --release --lib --target aarch64-apple-ios; \
  env -u LIBRARY_PATH DEVELOPER_DIR="$DEV_DIR" RUSTFLAGS="-C link-arg=-mios-simulator-version-min=17.0" cargo build -p pika_core --release --lib --target aarch64-apple-ios-sim; \
  env -u LIBRARY_PATH DEVELOPER_DIR="$DEV_DIR" RUSTFLAGS="-C link-arg=-mios-simulator-version-min=17.0" cargo build -p pika_core --release --lib --target x86_64-apple-ios

# Build PikaCore.xcframework (device + simulator slices).
ios-xcframework: ios-gen-swift ios-rust
  rm -rf ios/Frameworks/PikaCore.xcframework ios/.build
  mkdir -p ios/.build/headers ios/Frameworks
  cp ios/Bindings/pika_coreFFI.h ios/.build/headers/pika_coreFFI.h
  cp ios/Bindings/pika_coreFFI.modulemap ios/.build/headers/module.modulemap
  DEV_DIR=$(ls -d /Applications/Xcode*.app/Contents/Developer 2>/dev/null | sort | tail -n 1); \
  if [ -z "$DEV_DIR" ]; then echo "Xcode not found under /Applications"; exit 1; fi; \
  DEVELOPER_DIR="$DEV_DIR" xcrun lipo -create \
    target/aarch64-apple-ios-sim/release/libpika_core.a \
    target/x86_64-apple-ios/release/libpika_core.a \
    -output ios/.build/libpika_core_sim.a; \
  DEVELOPER_DIR="$DEV_DIR" xcodebuild -create-xcframework \
    -library target/aarch64-apple-ios/release/libpika_core.a -headers ios/.build/headers \
    -library ios/.build/libpika_core_sim.a -headers ios/.build/headers \
    -output ios/Frameworks/PikaCore.xcframework

# Generate Xcode project via xcodegen.
ios-xcodeproj:
  cd ios && xcodegen generate

# Build iOS app for simulator.
ios-build-sim: ios-xcframework ios-xcodeproj
  DEV_DIR=$(ls -d /Applications/Xcode*.app/Contents/Developer 2>/dev/null | sort | tail -n 1); \
  if [ -z "$DEV_DIR" ]; then echo "Xcode not found under /Applications"; exit 1; fi; \
  env -u LD -u CC -u CXX DEVELOPER_DIR="$DEV_DIR" xcodebuild -project ios/Pika.xcodeproj -target Pika -configuration Debug -sdk iphonesimulator build CODE_SIGNING_ALLOWED=NO

# Run iOS UI tests on simulator (skips E2E deployed-bot test).
ios-ui-test: ios-xcframework ios-xcodeproj
  DEV_DIR=$(ls -d /Applications/Xcode*.app/Contents/Developer 2>/dev/null | sort | tail -n 1); \
  if [ -z "$DEV_DIR" ]; then echo "Xcode not found under /Applications"; exit 1; fi; \
  udid="$(./tools/ios-sim-ensure | sed -n 's/^ok: ios simulator ready (udid=\(.*\))$/\1/p')"; \
  if [ -z "$udid" ]; then echo "error: could not determine simulator udid"; exit 1; fi; \
  env -u LD -u CC -u CXX DEVELOPER_DIR="$DEV_DIR" xcodebuild -project ios/Pika.xcodeproj -scheme Pika -destination "id=$udid" test CODE_SIGNING_ALLOWED=NO \
    -skip-testing:PikaUITests/PikaUITests/testE2E_deployedRustBot_pingPong

# iOS E2E: local docker relay + local Rust bot. Requires Docker.
ios-ui-e2e-local:
  ./tools/ui-e2e-local --platform ios

# iOS E2E: public relays + deployed bot (nondeterministic). Requires PIKA_UI_E2E=1.
ios-ui-e2e: ios-xcframework ios-xcodeproj
  #!/usr/bin/env bash
  set -euo pipefail
  # Load local secrets if present (gitignored).
  if [ -f .env ]; then
    set -a
    # shellcheck disable=SC1091
    . ./.env
    set +a
  fi
  : "${PIKA_TEST_NSEC:?missing (put it in ./.env)}"
  : "${PIKA_UI_E2E_BOT_NPUB:?missing (put it in ./.env)}"
  : "${PIKA_UI_E2E_RELAYS:?missing (put it in ./.env)}"
  : "${PIKA_UI_E2E_KP_RELAYS:?missing (put it in ./.env)}"

  DEV_DIR=$(ls -d /Applications/Xcode*.app/Contents/Developer 2>/dev/null | sort | tail -n 1)
  if [ -z "$DEV_DIR" ]; then echo "Xcode not found under /Applications"; exit 1; fi

  # Ensure the simulator test runner can see this value (tools/ios-sim-ensure propagates it).
  nsec="${PIKA_UI_E2E_NSEC:-${PIKA_TEST_NSEC}}"
  udid="$(PIKA_UI_E2E_NSEC="$nsec" ./tools/ios-sim-ensure | sed -n 's/^ok: ios simulator ready (udid=\(.*\))$/\1/p')"
  if [ -z "$udid" ]; then echo "error: could not determine simulator udid"; exit 1; fi

  PIKA_UI_E2E=1 \
    PIKA_UI_E2E_BOT_NPUB="${PIKA_UI_E2E_BOT_NPUB}" \
    PIKA_UI_E2E_RELAYS="${PIKA_UI_E2E_RELAYS}" \
    PIKA_UI_E2E_KP_RELAYS="${PIKA_UI_E2E_KP_RELAYS}" \
    PIKA_UI_E2E_NSEC="$nsec" \
    env -u LD -u CC -u CXX DEVELOPER_DIR="$DEV_DIR" xcodebuild -project ios/Pika.xcodeproj -scheme Pika -destination "id=$udid" test CODE_SIGNING_ALLOWED=NO \
      -only-testing:PikaUITests/PikaUITests/testE2E_deployedRustBot_pingPong

# Optional: device automation (npx). Not required for building.
device:
  ./tools/agent-device --help

# Show Android manual QA instructions.
android-manual-qa:
  @echo "Manual QA prompt: prompts/android-agent-device-manual-qa.md"
  @echo "Tip: run `npx --yes agent-device --platform android open com.pika.app` then follow the prompt."

# Show iOS manual QA instructions.
ios-manual-qa:
  @echo "Manual QA prompt: prompts/ios-agent-device-manual-qa.md"
  @echo "Tip: run `./tools/agent-device --platform ios open com.pika.app` then follow the prompt."

# Build, install, and launch Android app on connected device.
run-android:
  ./tools/run-android

# Build, install, and launch iOS app on simulator.
run-ios:
  ./tools/run-ios

# Check iOS dev environment (Xcode, simulators, runtimes).
doctor-ios:
  ./tools/ios-runtime-doctor

# Interop baseline: local Rust bot. Requires Docker + ~/code/marmot-interop-lab-rust.
interop-rust-baseline:
  ./tools/interop-rust-baseline

# Interactive interop test (manual send/receive with local bot).
interop-rust-manual:
  ./tools/interop-rust-baseline --manual

# ── pika-cli (Marmot protocol CLI) ──────────────────────────────────────────

# Build pika-cli (debug).
cli-build:
  cargo build -p pika-cli

# Build pika-cli (release).
cli-release:
  cargo build -p pika-cli --release

# Show (or create) an identity in the given state dir.
cli-identity STATE_DIR=".pika-cli" RELAY="ws://127.0.0.1:7777":
  cargo run -p pika-cli -- --state-dir {{STATE_DIR}} --relay {{RELAY}} identity

# Quick smoke test: two users, local relay, send+receive.
# Requires a Nostr relay running at RELAY (e.g. `strfry` or `nostr-rs-relay`).
cli-smoke RELAY="ws://127.0.0.1:7777":
  #!/usr/bin/env bash
  set -euo pipefail
  TMPDIR=$(mktemp -d)
  trap 'rm -rf "$TMPDIR"' EXIT
  CLI="cargo run -q -p pika-cli --"

  echo "=== Alice: create identity ==="
  ALICE=$($CLI --state-dir "$TMPDIR/alice" --relay {{RELAY}} identity)
  ALICE_PK=$(echo "$ALICE" | python3 -c "import sys,json; print(json.load(sys.stdin)['pubkey'])")
  echo "Alice pubkey: $ALICE_PK"

  echo "=== Bob: create identity ==="
  BOB=$($CLI --state-dir "$TMPDIR/bob" --relay {{RELAY}} identity)
  BOB_PK=$(echo "$BOB" | python3 -c "import sys,json; print(json.load(sys.stdin)['pubkey'])")
  echo "Bob pubkey: $BOB_PK"

  echo "=== Both: publish key packages ==="
  $CLI --state-dir "$TMPDIR/alice" --relay {{RELAY}} publish-kp
  $CLI --state-dir "$TMPDIR/bob" --relay {{RELAY}} publish-kp

  echo "=== Alice: invite Bob ==="
  INVITE=$($CLI --state-dir "$TMPDIR/alice" --relay {{RELAY}} invite --peer "$BOB_PK")
  GROUP=$(echo "$INVITE" | python3 -c "import sys,json; print(json.load(sys.stdin)['nostr_group_id'])")
  echo "Group: $GROUP"

  echo "=== Bob: check welcomes ==="
  WELCOMES=$($CLI --state-dir "$TMPDIR/bob" --relay {{RELAY}} welcomes)
  echo "$WELCOMES"
  WRAPPER=$(echo "$WELCOMES" | python3 -c "import sys,json; print(json.load(sys.stdin)['welcomes'][0]['wrapper_event_id'])")

  echo "=== Bob: accept welcome ==="
  $CLI --state-dir "$TMPDIR/bob" --relay {{RELAY}} accept-welcome --wrapper-event-id "$WRAPPER"

  echo "=== Alice: send message ==="
  $CLI --state-dir "$TMPDIR/alice" --relay {{RELAY}} send --group "$GROUP" --content "hello from alice"

  echo "=== Bob: read messages ==="
  $CLI --state-dir "$TMPDIR/bob" --relay {{RELAY}} messages --group "$GROUP"

  echo "=== SMOKE TEST PASSED ==="
