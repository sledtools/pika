set shell := ["bash", "-c"]

# List available recipes.
default:
  @just --list

# Print developer-facing usage notes (targets, env vars, common flows).
info:
  @echo "Pika: run commands + target selection"
  @echo
  @echo "iOS"
  @echo "  Simulator:"
  @echo "    just run-ios"
  @echo "  Hardware device:"
  @echo "    just run-ios --device --udid <UDID>"
  @echo "  List targets (devices + simulators):"
  @echo "    ./tools/pika-run ios list-targets"
  @echo
  @echo "  Env equivalents:"
  @echo "    PIKA_IOS_DEVICE=1               (default to device)"
  @echo "    PIKA_IOS_DEVICE_UDID=<UDID>     (pick device)"
  @echo "    PIKA_IOS_DEVELOPMENT_TEAM=...   (required for device builds)"
  @echo "    PIKA_IOS_CONSOLE=1              (attach console on device)"
  @echo
  @echo "Android"
  @echo "  Emulator:"
  @echo "    just run-android"
  @echo "  Hardware device:"
  @echo "    just run-android --device --serial <serial>"
  @echo "  List targets (emulators + devices):"
  @echo "    ./tools/pika-run android list-targets"
  @echo
  @echo "  Env equivalents:"
  @echo "    PIKA_ANDROID_SERIAL=<serial>"

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
  npx --yes @justinmoon/agent-tools check-docs
  npx --yes @justinmoon/agent-tools check-justfile
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
  ./tools/ui-e2e-public --platform all

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
  ./tools/android-ensure-debug-installable
  SERIAL="$(./tools/android-pick-serial)"; \
  ANDROID_SERIAL="$SERIAL" cd android && ./gradlew :app:connectedDebugAndroidTest

# Android E2E: local docker relay + local Rust bot. Requires Docker + emulator.
android-ui-e2e-local:
  ./tools/ui-e2e-local --platform android

# Android E2E: public relays + deployed bot (nondeterministic). Requires emulator.
android-ui-e2e:
  ./tools/ui-e2e-public --platform android

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
  # Nix shells often set CC/CXX/SDKROOT/MACOSX_DEPLOYMENT_TARGET for macOS builds.
  # For iOS targets, force Xcode toolchain compilers + iOS SDK roots.
  set -euo pipefail; \
  DEV_DIR="$(./tools/xcode-dev-dir)"; \
  TOOLCHAIN_BIN="$DEV_DIR/Toolchains/XcodeDefault.xctoolchain/usr/bin"; \
  CC_BIN="$TOOLCHAIN_BIN/clang"; \
  CXX_BIN="$TOOLCHAIN_BIN/clang++"; \
  AR_BIN="$TOOLCHAIN_BIN/ar"; \
  RANLIB_BIN="$TOOLCHAIN_BIN/ranlib"; \
  IOS_MIN="17.0"; \
  SDKROOT_IOS="$(DEVELOPER_DIR="$DEV_DIR" /usr/bin/xcrun --sdk iphoneos --show-sdk-path)"; \
  SDKROOT_SIM="$(DEVELOPER_DIR="$DEV_DIR" /usr/bin/xcrun --sdk iphonesimulator --show-sdk-path)"; \
  base_env=(env -u LIBRARY_PATH -u SDKROOT -u MACOSX_DEPLOYMENT_TARGET -u CC -u CXX -u AR -u RANLIB \
    DEVELOPER_DIR="$DEV_DIR" CC="$CC_BIN" CXX="$CXX_BIN" AR="$AR_BIN" RANLIB="$RANLIB_BIN" IPHONEOS_DEPLOYMENT_TARGET="$IOS_MIN" \
    CARGO_TARGET_AARCH64_APPLE_IOS_LINKER="$CC_BIN" \
    CARGO_TARGET_AARCH64_APPLE_IOS_SIM_LINKER="$CC_BIN" \
    CARGO_TARGET_X86_64_APPLE_IOS_LINKER="$CC_BIN"); \
  "${base_env[@]}" SDKROOT="$SDKROOT_IOS" RUSTFLAGS="-C linker=$CC_BIN -C link-arg=-miphoneos-version-min=$IOS_MIN" cargo build -p pika_core --release --lib --target aarch64-apple-ios; \
  "${base_env[@]}" SDKROOT="$SDKROOT_SIM" RUSTFLAGS="-C linker=$CC_BIN -C link-arg=-mios-simulator-version-min=$IOS_MIN" cargo build -p pika_core --release --lib --target aarch64-apple-ios-sim; \
  "${base_env[@]}" SDKROOT="$SDKROOT_SIM" RUSTFLAGS="-C linker=$CC_BIN -C link-arg=-mios-simulator-version-min=$IOS_MIN" cargo build -p pika_core --release --lib --target x86_64-apple-ios

# Build PikaCore.xcframework (device + simulator slices).
ios-xcframework: ios-gen-swift ios-rust
  rm -rf ios/Frameworks/PikaCore.xcframework ios/.build
  mkdir -p ios/.build/headers ios/Frameworks
  cp ios/Bindings/pika_coreFFI.h ios/.build/headers/pika_coreFFI.h
  cp ios/Bindings/pika_coreFFI.modulemap ios/.build/headers/module.modulemap
  ./tools/xcode-run xcrun lipo -create \
    target/aarch64-apple-ios-sim/release/libpika_core.a \
    target/x86_64-apple-ios/release/libpika_core.a \
    -output ios/.build/libpika_core_sim.a; \
  ./tools/xcode-run xcodebuild -create-xcframework \
    -library target/aarch64-apple-ios/release/libpika_core.a -headers ios/.build/headers \
    -library ios/.build/libpika_core_sim.a -headers ios/.build/headers \
    -output ios/Frameworks/PikaCore.xcframework

# Generate Xcode project via xcodegen.
ios-xcodeproj:
  cd ios && rm -rf Pika.xcodeproj && xcodegen generate

# Build iOS app for simulator.
ios-build-sim: ios-xcframework ios-xcodeproj
  ./tools/xcode-run xcodebuild -project ios/Pika.xcodeproj -target Pika -configuration Debug -sdk iphonesimulator build CODE_SIGNING_ALLOWED=NO PRODUCT_BUNDLE_IDENTIFIER="${PIKA_IOS_BUNDLE_ID:-com.justinmoon.pika.dev}"

# Run iOS UI tests on simulator (skips E2E deployed-bot test).
ios-ui-test: ios-xcframework ios-xcodeproj
  udid="$(./tools/ios-sim-ensure | sed -n 's/^ok: ios simulator ready (udid=\(.*\))$/\1/p')"; \
  if [ -z "$udid" ]; then echo "error: could not determine simulator udid"; exit 1; fi; \
  ./tools/xcode-run xcodebuild -project ios/Pika.xcodeproj -scheme Pika -destination "id=$udid" test CODE_SIGNING_ALLOWED=NO PRODUCT_BUNDLE_IDENTIFIER="${PIKA_IOS_BUNDLE_ID:-com.justinmoon.pika.dev}" \
    -skip-testing:PikaUITests/PikaUITests/testE2E_deployedRustBot_pingPong

# iOS E2E: local docker relay + local Rust bot. Requires Docker.
ios-ui-e2e-local:
  ./tools/ui-e2e-local --platform ios

# iOS E2E: public relays + deployed bot (nondeterministic). Requires PIKA_UI_E2E=1.
ios-ui-e2e:
  ./tools/ui-e2e-public --platform ios

# Optional: device automation (npx). Not required for building.
device:
  ./tools/agent-device --help

# Show Android manual QA instructions.
android-manual-qa:
  @echo "Manual QA prompt: prompts/android-agent-device-manual-qa.md"
  @echo "Tip: run `npx --yes agent-device --platform android open com.justinmoon.pika.dev` then follow the prompt."

# Show iOS manual QA instructions.
ios-manual-qa:
  @echo "Manual QA prompt: prompts/ios-agent-device-manual-qa.md"
  @echo "Tip: run `./tools/agent-device --platform ios open com.justinmoon.pika.dev` then follow the prompt."

# Build, install, and launch Android app on emulator/device.
run-android *ARGS:
  ./tools/pika-run android run {{ARGS}}

# Build, install, and launch iOS app on simulator/device.
run-ios *ARGS:
  ./tools/pika-run ios run {{ARGS}}

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
  ./tools/cli-smoke --relay {{RELAY}}
