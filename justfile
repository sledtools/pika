set shell := ["bash", "-lc"]

default:
  @just --list

test *ARGS:
  cargo test -p pika_core {{ARGS}}

fmt:
  cargo fmt --all --check

clippy *ARGS:
  cargo clippy -p pika_core {{ARGS}} -- -D warnings

# CI-safe pre-merge: skips cdylib/staticlib (OOM on 7GB GitHub runners).
pre-merge: fmt
  just clippy --lib --tests
  just test --lib --tests
  cargo build -p pika-cli
  @echo "pre-merge complete"

qa: fmt clippy test android-assemble ios-build-sim
  @echo "QA complete"

# Manual-only nondeterministic smoke test using public relays.
# Optional:
#   PIKA_E2E_RELAYS="wss://relay.damus.io,wss://relay.primal.net" just e2e-public
#   PIKA_E2E_KP_RELAYS="wss://nostr-pub.wellorder.net,wss://nostr-01.yakihonne.com,..." just e2e-public
e2e-public:
  PIKA_E2E_PUBLIC=1 cargo test -p pika_core --test e2e_public_relays -- --ignored --nocapture

rust-build-host:
  cargo build -p pika_core --release

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

android-rust:
  mkdir -p android/app/src/main/jniLibs
  cargo ndk -o android/app/src/main/jniLibs \
    -t arm64-v8a -t armeabi-v7a -t x86_64 \
    build -p pika_core --release

android-local-properties:
  SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"; \
  if [ -z "$SDK" ]; then echo "ANDROID_HOME/ANDROID_SDK_ROOT not set (run inside nix develop)"; exit 1; fi; \
  printf "sdk.dir=%s\n" "$SDK" > android/local.properties

android-assemble: gen-kotlin android-rust android-local-properties
  cd android && ./gradlew :app:assembleDebug

android-install: gen-kotlin android-rust android-local-properties
  cd android && ./gradlew :app:installDebug

android-ui-test: gen-kotlin android-rust android-local-properties
  # Requires a running emulator/device (instrumentation tests).
  cd android && ./gradlew :app:connectedDebugAndroidTest

# Opt-in E2E: runs against public relays + deployed OpenClaw rust marmot bot.
# This is intentionally NOT part of `just qa` because it is nondeterministic by nature.
android-ui-e2e: gen-kotlin android-rust android-local-properties
  # Requires a running emulator/device.
  cd android && ./gradlew :app:connectedDebugAndroidTest \
    -Pandroid.testInstrumentationRunnerArguments.class=com.pika.app.PikaE2eUiTest \
    -Pandroid.testInstrumentationRunnerArguments.pika_e2e=1 \
    -Pandroid.testInstrumentationRunnerArguments.pika_disable_network=false \
    -Pandroid.testInstrumentationRunnerArguments.pika_reset=1 \
    -Pandroid.testInstrumentationRunnerArguments.pika_peer_npub=1ac66317246750fe862cf47156b200051aa219d9a37ca018690bb3483065d3b8 \
    -Pandroid.testInstrumentationRunnerArguments.pika_relay_urls=wss://relay.primal.net,wss://nos.lol,wss://relay.damus.io \
    -Pandroid.testInstrumentationRunnerArguments.pika_key_package_relay_urls=wss://nostr-pub.wellorder.net,wss://nostr-01.yakihonne.com,wss://nostr-02.yakihonne.com,wss://relay.satlantis.io

# iOS (Xcode build happens outside Nix; Nix helps with Rust + xcodegen).
ios-gen-swift: rust-build-host
  mkdir -p ios/Bindings
  cargo run -q -p uniffi-bindgen -- generate \
    --library target/release/libpika_core.dylib \
    --language swift \
    --out-dir ios/Bindings \
    --config rust/uniffi.toml

ios-rust:
  set -euo pipefail; \
  DEV_DIR=$(ls -d /Applications/Xcode*.app/Contents/Developer 2>/dev/null | sort | tail -n 1); \
  if [ -z "$DEV_DIR" ]; then echo "Xcode not found under /Applications (needed for iOS SDK)"; exit 1; fi; \
  env -u LIBRARY_PATH DEVELOPER_DIR="$DEV_DIR" RUSTFLAGS="-C link-arg=-miphoneos-version-min=17.0" cargo build -p pika_core --release --lib --target aarch64-apple-ios; \
  env -u LIBRARY_PATH DEVELOPER_DIR="$DEV_DIR" RUSTFLAGS="-C link-arg=-mios-simulator-version-min=17.0" cargo build -p pika_core --release --lib --target aarch64-apple-ios-sim; \
  env -u LIBRARY_PATH DEVELOPER_DIR="$DEV_DIR" RUSTFLAGS="-C link-arg=-mios-simulator-version-min=17.0" cargo build -p pika_core --release --lib --target x86_64-apple-ios

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

ios-xcodeproj:
  cd ios && xcodegen generate

ios-build-sim: ios-xcframework ios-xcodeproj
  DEV_DIR=$(ls -d /Applications/Xcode*.app/Contents/Developer 2>/dev/null | sort | tail -n 1); \
  if [ -z "$DEV_DIR" ]; then echo "Xcode not found under /Applications"; exit 1; fi; \
  env -u LD -u CC -u CXX DEVELOPER_DIR="$DEV_DIR" xcodebuild -project ios/Pika.xcodeproj -target Pika -configuration Debug -sdk iphonesimulator build CODE_SIGNING_ALLOWED=NO

ios-ui-test: ios-xcframework ios-xcodeproj
  DEV_DIR=$(ls -d /Applications/Xcode*.app/Contents/Developer 2>/dev/null | sort | tail -n 1); \
  if [ -z "$DEV_DIR" ]; then echo "Xcode not found under /Applications"; exit 1; fi; \
  udid="$(./tools/ios-sim-ensure | sed -n 's/^ok: ios simulator ready (udid=\(.*\))$/\1/p')"; \
  if [ -z "$udid" ]; then echo "error: could not determine simulator udid"; exit 1; fi; \
  env -u LD -u CC -u CXX DEVELOPER_DIR="$DEV_DIR" xcodebuild -project ios/Pika.xcodeproj -scheme Pika -destination "id=$udid" test CODE_SIGNING_ALLOWED=NO \
    -skip-testing:PikaUITests/PikaUITests/testE2E_deployedRustBot_pingPong

# Opt-in E2E: runs the XCUITest that hits public relays + deployed OpenClaw rust marmot bot.
# Enable by setting PIKA_UI_E2E=1 (and optional overrides).
ios-ui-e2e: ios-xcframework ios-xcodeproj
  DEV_DIR=$(ls -d /Applications/Xcode*.app/Contents/Developer 2>/dev/null | sort | tail -n 1); \
  if [ -z "$DEV_DIR" ]; then echo "Xcode not found under /Applications"; exit 1; fi; \
  udid="$(./tools/ios-sim-ensure | sed -n 's/^ok: ios simulator ready (udid=\(.*\))$/\1/p')"; \
  if [ -z "$udid" ]; then echo "error: could not determine simulator udid"; exit 1; fi; \
  PIKA_UI_E2E=1 \
    PIKA_UI_E2E_BOT_NPUB="npub1rtrxx9eyvag0ap3v73c4dvsqq5d2yxwe5d72qxrfpwe5svr96wuqed4p38" \
    PIKA_UI_E2E_RELAYS="wss://relay.primal.net,wss://nos.lol,wss://relay.damus.io" \
    PIKA_UI_E2E_KP_RELAYS="wss://nostr-pub.wellorder.net,wss://nostr-01.yakihonne.com,wss://nostr-02.yakihonne.com,wss://relay.satlantis.io" \
    env -u LD -u CC -u CXX DEVELOPER_DIR="$DEV_DIR" xcodebuild -project ios/Pika.xcodeproj -scheme Pika -destination "id=$udid" test CODE_SIGNING_ALLOWED=NO \
      -only-testing:PikaUITests/PikaUITests/testE2E_deployedRustBot_pingPong

# Optional: device automation (npx). Not required for building.
device:
  ./tools/agent-device --help

android-manual-qa:
  @echo "Manual QA prompt: prompts/android-agent-device-manual-qa.md"
  @echo "Tip: run `npx --yes agent-device --platform android open com.pika.app` then follow the prompt."

ios-manual-qa:
  @echo "Manual QA prompt: prompts/ios-agent-device-manual-qa.md"
  @echo "Tip: run `./tools/agent-device --platform ios open com.pika.app` then follow the prompt."

run-android:
  ./tools/run-android

run-ios:
  ./tools/run-ios

doctor-ios:
  ./tools/ios-runtime-doctor

# Local-first interop baseline with the Rust OpenClaw-style bot (marmot-interop-lab-rust).
# Opt-in: requires Docker and ~/code/marmot-interop-lab-rust (override with MARMOT_INTEROP_RUST_DIR).
interop-rust-baseline:
  ./tools/interop-rust-baseline

interop-rust-manual:
  ./tools/interop-rust-baseline --manual

# ── pika-cli (Marmot protocol CLI) ──────────────────────────────────────────

cli-build:
  cargo build -p pika-cli

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
