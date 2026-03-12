set working-directory := '..'

# Run pika_core tests.
test *ARGS:
    cargo test -p pika_core {{ ARGS }}

# Check formatting (cargo fmt).
fmt:
    cargo fmt --all --check

# Lint with clippy.
clippy *ARGS:
    cargo clippy --all-targets {{ ARGS }} -- -D warnings

# Local pre-commit checks (fmt + clippy + justfile + docs checks).
pre-commit: fmt clippy
    just --fmt --check --unstable
    npx --yes @justinmoon/agent-tools check-docs
    npx --yes @justinmoon/agent-tools check-justfile
    just clippy --lib --tests
    cargo clippy -p pikachat --tests -- -D warnings
    cargo clippy -p pikachat-sidecar --tests -- -D warnings
    cargo clippy -p pika-server --tests -- -D warnings
    cargo clippy -p pikahut -- -D warnings

# CI-safe pre-merge for the Pika app lane.
pre-merge-pika: fmt clippy
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ]; then
        nix run .#pikaci -- run pre-merge-pika-rust
    else
        # Keep the paging regression test available for manual/debug runs, but skip it in CI
        # until the underlying timing flake is fixed.
        cargo test -p pika_core --lib --tests -- --skip paging_loads_older_messages_in_pages
    fi
    cd android && ./gradlew :app:compileDebugAndroidTestKotlin
    cargo build -p pikachat
    just desktop-check
    actionlint
    npx --yes @justinmoon/agent-tools check-docs
    npx --yes @justinmoon/agent-tools check-justfile
    echo "pre-merge-pika complete"

# Advisory shadow CI lane for the staged Linux Rust remote microVM path.
pre-merge-pika-rust-shadow:
    just pikaci-remote-fulfill-pre-merge-pika-rust
    echo "pre-merge-pika-rust-shadow complete"

# Advisory shadow CI lane for the staged Linux notifications remote microVM path.
pre-merge-notifications-shadow:
    just pikaci-remote-fulfill-pre-merge-notifications
    echo "pre-merge-notifications-shadow complete"

# Advisory shadow CI lane for the staged Linux agent-contracts remote microVM path.
pre-merge-agent-contracts-shadow:
    just pikaci-remote-fulfill-pre-merge-agent-contracts
    echo "pre-merge-agent-contracts-shadow complete"

# Advisory shadow CI lane for the staged Linux RMP remote microVM path.
pre-merge-rmp-shadow:
    just pikaci-remote-fulfill-pre-merge-rmp
    echo "pre-merge-rmp-shadow complete"

# CI-safe pre-merge for the notification server lane.
pre-merge-notifications:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy -p pika-server -- -D warnings
    if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ]; then
        just pikaci-remote-fulfill-pre-merge-notifications
    else
        SD="$(mktemp -d /tmp/pikahut-notifications.XXXXXX)"
        cleanup() { cargo run -q -p pikahut -- down --state-dir "$SD" 2>/dev/null || true; rm -rf "$SD"; }
        trap cleanup EXIT
        cargo run -q -p pikahut -- up --profile postgres --background --state-dir "$SD" >/dev/null
        eval "$(cargo run -q -p pikahut -- env --state-dir "$SD")"
        cargo test -p pika-server -- --test-threads=1
    fi
    echo "pre-merge-notifications complete"

# CI-safe pre-merge for the pikachat lane (deterministic only).
[private]
pre-merge-pikachat-apple-followup:
    cargo clippy -p pikachat -- -D warnings
    cargo clippy -p pikachat-sidecar -- -D warnings
    cargo test -p pikahut --test integration_deterministic ui_e2e_local_desktop -- --ignored --nocapture
    npx --yes tsx --test pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/channel-behavior.test.ts

# CI-safe pre-merge for the pikachat lane (deterministic only).
pre-merge-pikachat:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ]; then
        nix run .#pikaci -- run pre-merge-pikachat-rust
        just --justfile just/checks.just pre-merge-pikachat-apple-followup
    else
        cargo clippy -p pikachat -- -D warnings
        cargo clippy -p pikachat-sidecar -- -D warnings
        cargo test -p pikachat
        cargo test -p pikachat-sidecar
        cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture
        cargo test -p pikahut --test integration_deterministic ui_e2e_local_desktop -- --ignored --nocapture
        cargo test -p pikahut --test integration_deterministic post_rebase_invalid_event_rejection_boundary -- --ignored --nocapture
        cargo test -p pikahut --test integration_deterministic post_rebase_logout_session_convergence_boundary -- --ignored --nocapture
        just openclaw-pikachat-deterministic
    fi
    echo "pre-merge-pikachat complete"

# Deterministic HTTP control-plane contracts (mocked MicroVM spawner).
pre-merge-agent-contracts:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ]; then
        just pikaci-remote-fulfill-pre-merge-agent-contracts
    else
        cargo test -p pika-agent-microvm
        cargo test -p pika-server -- agent_api::tests
        cargo test -p pika_core --lib core::agent::tests::run_agent_flow_signs_requests_with_nip98_authorization
    fi
    cargo test -p pikahut --test integration_deterministic agent_http_ensure_local -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic agent_http_cli_new_local -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic agent_http_cli_new_idempotent_local -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic agent_http_cli_new_me_recover_local -- --ignored --nocapture
    echo "pre-merge-agent-contracts complete"

# CI-safe pre-merge for the RMP tooling lane.
pre-merge-rmp:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ]; then
        just pikaci-remote-fulfill-pre-merge-rmp
    else
        just rmp-init-smoke-ci
    fi
    echo "pre-merge-rmp complete"

# Deterministic Apple-platform lane (iOS + macOS desktop) via Tart.
pre-merge-apple-deterministic:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
        echo "error: pre-merge-apple-deterministic currently requires Apple Silicon macOS" >&2
        exit 1
    fi
    PIKACI_TART_BASE_VM="${PIKACI_TART_BASE_VM:-pikaci-xcode16-base}" \
      nix run .#pikaci -- run pre-merge-apple-deterministic
    echo "pre-merge-apple-deterministic complete"

# CI-safe pre-merge for the pikahut tooling lane.
pre-merge-fixture:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy -p pikahut -- -D warnings
    if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ]; then
        nix run .#pikaci -- run pre-merge-fixture-rust
    else
        cargo test -p pikahut
    fi
    SD="$(mktemp -d /tmp/pikahut-smoke.XXXXXX)"
    cleanup() { cargo run -q -p pikahut -- down --state-dir "$SD" 2>/dev/null || true; rm -rf "$SD"; }
    trap cleanup EXIT
    cargo run -q -p pikahut -- up --profile relay --background --state-dir "$SD" --relay-port 0 >/dev/null
    cargo run -q -p pikahut -- wait --state-dir "$SD" --timeout 30
    cargo run -q -p pikahut -- status --state-dir "$SD" --json | python3 -c "import json,sys; d=json.load(sys.stdin); assert d.get('relay_url'), f'relay_url missing: {d}'"
    echo "pre-merge-fixture complete"

# Single CI entrypoint for the whole repo.
pre-merge:
    just pre-merge-pika
    just pre-merge-notifications
    just pre-merge-pikachat
    just pre-merge-fixture
    just pre-merge-rmp
    @echo "pre-merge complete"

# Nightly root task.
nightly:
    just pre-merge
    just nightly-pika-e2e
    just nightly-pikachat
    @echo "nightly complete"

# Nightly E2E (Rust): local-only deterministic + heavy call-path regression boundaries.
nightly-pika-e2e:
    cargo test -p pikahut --test integration_deterministic call_over_local_moq_relay_boundary -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic call_with_pikachat_daemon_boundary -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic cli_smoke_media_local -- --ignored --nocapture

# Nightly lane: full OpenClaw integration E2E (gateway + real sidecar wiring).
nightly-pikachat:
    just openclaw-pikachat-e2e

# Nightly lane: iOS interop smoke (nostrconnect:// route + Pika bridge emission).
nightly-primal-ios-interop:
    cargo test -p pikahut --test integration_primal primal_nostrconnect_smoke -- --ignored --nocapture

# openclaw pikachat deterministic contract suite (local relay + scenarios + focused tests).
openclaw-pikachat-deterministic:
    cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_rust_bot -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_daemon -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic openclaw_scenario_audio_echo -- --ignored --nocapture
    PIKACHAT_TTS_FIXTURE=1 cargo test -p pikachat-sidecar daemon::tests::tts_pcm_publish_reaches_subscriber -- --nocapture
    npx --yes tsx --test pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/channel-behavior.test.ts

# Full OpenClaw integration lane (nightly/manual).
openclaw-pikachat-e2e:
    cargo test -p pikahut --test integration_openclaw openclaw_gateway_e2e -- --ignored --nocapture

# Full QA: fmt, clippy, test, android build, iOS sim build.
qa: fmt clippy test
    just android-assemble
    just ios-build-sim
    @echo "QA complete"

# Deterministic E2E: local Nostr relay + local Rust bot (iOS + Android).
e2e-local-relay: ios-ui-e2e-local android-ui-e2e-local

# Android E2E: local Nostr relay + local Rust bot. Requires emulator.
android-ui-e2e-local:
    cargo test -p pikahut --test integration_deterministic ui_e2e_local_android -- --ignored --nocapture

# Desktop E2E: local Nostr relay + local Rust bot.
desktop-e2e-local:
    cargo test -p pikahut --test integration_deterministic ui_e2e_local_desktop -- --ignored --nocapture

# Run desktop tests (manager + UI wiring).
desktop-ui-test:
    cargo test -p pika-desktop

# iOS E2E: local Nostr relay + local Rust bot.
ios-ui-e2e-local:
    cargo test -p pikahut --test integration_deterministic ui_e2e_local_ios -- --ignored --nocapture

# Quick smoke test: two users, local relay, send+receive.
#
# Starts its own pika-relay automatically.
cli-smoke:
    cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture

# Minimum shared-runtime regression set for the app / CLI / daemon extraction boundary.
shared-runtime-regression:
    cargo test -p pika-marmot-runtime publish_welcome_rumors_
    cargo test -p pika-marmot-runtime create_group_and_publish_welcomes_returns_group_and_published_metadata
    cargo test -p pikachat-sidecar init_group_uses_shared_runtime_helper_and_keeps_expiration_tag
    cargo test -p pika_core app_background_publish_uses_shared_welcome_pairing
    cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_daemon -- --ignored --nocapture

# Quick smoke test including encrypted media upload/download over Blossom.
#
# Starts its own relay automatically. Requires internet for the default Blossom server.
cli-smoke-media:
    cargo test -p pikahut --test integration_deterministic cli_smoke_media_local -- --ignored --nocapture
