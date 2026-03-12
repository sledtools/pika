set shell := ["bash", "-c"]
set dotenv-load := true

[private]
default:
    @echo "Pika just commands"
    @echo
    @just --list
    @echo
    @echo 'Common fast local smoke: `just pre-commit`'
    @echo 'Richer local follow-up: `just checks::pre-commit-full`'
    @echo
    @echo 'Humans: use `just info` for common run flows.'
    @echo 'Agents: use `./scripts/agent-brief` for expanded discovery.'
    @echo 'Full module tree: `JUST_UNSTABLE=1 just --list --list-submodules`.'

# Print developer-facing usage notes (targets, env vars, common flows).
info:
    @echo "Pika: run commands + target selection"
    @echo
    @echo "Discovery"
    @echo "  Curated root surface:"
    @echo "    just"
    @echo "    just --list"
    @echo "  Fast local smoke:"
    @echo "    just pre-commit"
    @echo "  Richer local follow-up:"
    @echo "    just checks::pre-commit-full"
    @echo "  Human-oriented workflow help:"
    @echo "    just info"
    @echo "  Expanded module tree:"
    @echo "    JUST_UNSTABLE=1 just --list --list-submodules"
    @echo "  Agent discovery:"
    @echo "    ./scripts/agent-brief"
    @echo
    @echo "iOS"
    @echo "  Simulator:"
    @echo "    just run-ios"
    @echo "    just run-swift        # skip Rust rebuild; reuse existing iOS artifacts"
    @echo "    just screenshot-booted-ios"
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
    @echo "    PIKA_CALL_MOQ_URL=...           (override MOQ relay URL)"
    @echo "    PIKA_MOQ_PROBE_ON_START=1       (log QUIC+TLS probe PASS/FAIL on startup)"
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
    @echo "    PIKA_CALL_MOQ_URL=...           (override MOQ relay URL)"
    @echo "    PIKA_MOQ_PROBE_ON_START=1       (log QUIC+TLS probe PASS/FAIL on startup)"
    @echo
    @echo "Desktop (ICED)"
    @echo "  Run desktop app:"
    @echo "    just run-desktop"
    @echo "  Build-check desktop app:"
    @echo "    just desktop-check"
    @echo
    @echo "Agent demos"
    @echo "  Local backend (postgres + relay + server):"
    @echo "    just pikahut-up"
    @echo "  Agent HTTP demo:"
    @echo "    just agent-microvm"
    @echo "    just agent-microvm-acp         # guest daemon boots with --acp-exec"
    @echo "  Agent chat demo (ensure/reuse + send + listen):"
    @echo "    just agent-microvm-chat \"hello\""
    @echo "    just agent-demo-acp \"hello\"   # full ACP-backed microVM demo"
    @echo "  Unified pikachat wrapper:"
    @echo "    just cli --help"
    @echo "    just cli agent new --nsec <nsec>"
    @echo
    @echo "pikaci remote fulfillment"
    @echo "  Deploy helper binaries to pika-build:"
    @echo "    just pikaci-remote-fulfill-deploy"
    @echo "  Run staged Rust lane with remote fulfillment on pika-build:"
    @echo "    just pikaci-remote-fulfill-pre-merge-pika-rust"
    @echo "  Realize both staged Linux Rust prepare outputs directly on pika-build:"
    @echo "    just pikaci-pre-merge-pika-rust-prepares-remote-build"
    @echo "  Repair the legacy local aarch64-linux linux-builder staged-Rust cargo-path failure:"
    @echo "    just linux-builder-repair"
    @echo
    @echo "RMP (new)"
    @echo "  Run iOS simulator:"
    @echo "    just rmp run ios"
    @echo "  Run Android emulator:"
    @echo "    just rmp run android"
    @echo "  Run desktop (ICED):"
    @echo "    just rmp run iced"
    @echo "  List devices:"
    @echo "    just rmp devices list"
    @echo "  Start Android emulator only:"
    @echo "    just rmp devices start android"
    @echo "  Start iOS simulator only:"
    @echo "    just rmp devices start ios"
    @echo "  Generate bindings:"
    @echo "    just rmp bindings all"
    @echo
    @echo "Command contract"
    @echo "  Root recipes stay small and high-signal."
    @echo "  New implementation should live in scripts/ or a real CLI."
    @echo "  Low-signal, manual, and debug helpers default to module-local or [private]."

# Command-surface contract:
# - Root recipes are rare, high-signal human entrypoints.
# - Real implementation should live in scripts/ or a dedicated CLI.
# - Low-signal/manual/debug helpers default to modules and usually `[private]`.
# - `./scripts/agent-brief` is the supported expanded discovery path for agents.

# Build, install, and launch iOS app on simulator/device.
run-ios *ARGS:
    @just build::run-ios {{ ARGS }}

# Build, install, and launch iOS app using existing Rust-generated artifacts.
run-swift *ARGS:
    @just build::run-swift {{ ARGS }}

# Build, install, and launch Android app on emulator/device.
run-android *ARGS:
    @just build::run-android {{ ARGS }}

# Run the desktop ICED app.
run-desktop *ARGS:
    @just build::run-desktop {{ ARGS }}

# Run pikachat with shared local-env defaults; forwards args verbatim.
cli *ARGS="":
    @just build::cli {{ ARGS }}

# Start local backend (postgres, pika-relay, pika-server with agent control).
pikahut-up: infra::pikahut-up

# Check formatting (cargo fmt).
fmt: checks::fmt

# Lint with clippy.
clippy *ARGS:
    @just checks::clippy {{ ARGS }}

# Run pika_core tests.
test *ARGS:
    @just checks::test {{ ARGS }}

# Full QA: fmt, clippy, test, android build, iOS sim build.
qa: checks::qa

# Single CI entrypoint for the whole repo.
pre-merge: checks::pre-merge

# Nightly root task.
nightly: checks::nightly

# Create + push version tag (pika/vX.Y.Z) after validating VERSION and clean tree.
release VERSION:
    @just ship::release {{ VERSION }}

# Run the new Rust `rmp` CLI.
rmp *ARGS:
    @just rmp_tools::rmp {{ ARGS }}

# Build, bindgen, and platform helper commands.
mod build 'just/build.just'
# QA aggregates, CI lanes, and test selectors.
mod checks 'just/checks.just'
# Visible lab entrypoints; manual prompts and niche helpers stay available via hidden aliases.
mod labs 'just/labs.just'
# Agent demos and microVM debugging helpers.
mod agent 'just/agent.just'
# Local runtime entrypoints; remote/news plumbing stays available via hidden aliases.
mod infra 'just/infra.just'
# Release, versioning, and Zapstore helpers.
mod ship 'just/ship.just'
# RMP CI/nightly entrypoints; local scaffold QA helpers stay available via hidden aliases.
mod rmp_tools 'just/rmp_tools.just'

# Hidden root aliases preserve existing docs/CI entrypoints without crowding `just --list`.
# CI/manual selector contracts remain listed here for guardrails and docs even
# though the executable recipe bodies now live in `just/checks.just` / `just/labs.just`.
# cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture
# cargo test -p pikahut --test integration_deterministic interop_rust_baseline -- --ignored --nocapture
# cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat -- --ignored --nocapture
# cargo test -p pikahut --test integration_deterministic post_rebase_invalid_event_rejection_boundary -- --ignored --nocapture
# cargo test -p pikahut --test integration_deterministic post_rebase_logout_session_convergence_boundary -- --ignored --nocapture
# cargo test -p pikahut --test integration_openclaw openclaw_gateway_e2e -- --ignored --nocapture
# cargo test -p pikahut --test integration_deterministic call_over_local_moq_relay_boundary -- --ignored --nocapture
# cargo test -p pikahut --test integration_deterministic call_with_pikachat_daemon_boundary -- --ignored --nocapture
# cargo test -p pikahut --test integration_primal primal_nostrconnect_smoke -- --ignored --nocapture
# cargo test -p pikahut --test integration_manual manual_interop_rust_runbook_contract -- --ignored --nocapture
# cargo test -p pikahut --test integration_manual manual_primal_lab_runbook_contract -- --ignored --nocapture

alias rust-build-host := build::rust-build-host
alias gen-kotlin := build::gen-kotlin
alias android-rust := build::android-rust
alias android-local-properties := build::android-local-properties
alias android-release := build::android-release
alias android-assemble := build::android-assemble
alias android-install := build::android-install
alias android-ui-test := build::android-ui-test
alias ios-gen-swift := build::ios-gen-swift
alias ios-rust := build::ios-rust
alias ios-xcframework := build::ios-xcframework
alias ios-xcodeproj := build::ios-xcodeproj
alias ios-appstore := build::ios-appstore
alias ios-build-sim := build::ios-build-sim
alias ios-build-swift-sim := build::ios-build-swift-sim
alias ios-ui-test := build::ios-ui-test
alias screenshot-booted-ios := build::screenshot-booted-ios
alias desktop-check := build::desktop-check
alias doctor-ios := build::doctor-ios
alias cli-build := build::cli-build
alias cli-release := build::cli-release
alias cli-identity := build::cli-identity
alias pre-commit := checks::pre-commit
alias pre-merge-pika := checks::pre-merge-pika
alias pre-merge-pika-rust-shadow := checks::pre-merge-pika-rust-shadow
alias pre-merge-notifications-shadow := checks::pre-merge-notifications-shadow
alias pre-merge-agent-contracts-shadow := checks::pre-merge-agent-contracts-shadow
alias pre-merge-rmp-shadow := checks::pre-merge-rmp-shadow
alias pre-merge-pikachat-rust-shadow := checks::pre-merge-pikachat-rust-shadow
alias pre-merge-pikachat-openclaw-e2e := checks::pre-merge-pikachat-openclaw-e2e
alias pre-merge-notifications := checks::pre-merge-notifications
alias pre-merge-pikachat := checks::pre-merge-pikachat
alias pre-merge-agent-contracts := checks::pre-merge-agent-contracts
alias pre-merge-rmp := checks::pre-merge-rmp
alias pre-merge-apple-deterministic := checks::pre-merge-apple-deterministic
alias pre-merge-fixture := checks::pre-merge-fixture
alias nightly-pika-e2e := checks::nightly-pika-e2e
alias nightly-pikachat := checks::nightly-pikachat
alias nightly-primal-ios-interop := checks::nightly-primal-ios-interop
alias openclaw-pikachat-deterministic := checks::openclaw-pikachat-deterministic
alias openclaw-pikachat-e2e := checks::openclaw-pikachat-e2e
alias e2e-local-relay := checks::e2e-local-relay
alias android-ui-e2e-local := checks::android-ui-e2e-local
alias ios-ui-e2e-local := checks::ios-ui-e2e-local
alias desktop-e2e-local := checks::desktop-e2e-local
alias desktop-ui-test := checks::desktop-ui-test
alias cli-smoke := checks::cli-smoke
alias shared-runtime-regression := checks::shared-runtime-regression
alias cli-smoke-media := checks::cli-smoke-media
alias android-device-start := labs::android-device-start
alias android-agent-open := labs::android-agent-open
alias integration-manual := labs::integration-manual
alias primal-ios-lab := labs::primal-ios-lab
alias primal-ios-lab-patch-primal := labs::primal-ios-lab-patch-primal
alias primal-ios-lab-seed-capture := labs::primal-ios-lab-seed-capture
alias primal-ios-lab-seed-reset := labs::primal-ios-lab-seed-reset
alias primal-ios-lab-dump-debug := labs::primal-ios-lab-dump-debug
alias interop-rust-baseline := labs::interop-rust-baseline
alias interop-rust-manual := labs::interop-rust-manual
alias device := labs::device
alias android-manual-qa := labs::android-manual-qa
alias ios-manual-qa := labs::ios-manual-qa
alias agent-microvm := agent::agent-microvm
alias agent-microvm-acp := agent::agent-microvm-acp
alias agent-pi-ensure := agent::agent-pi-ensure
alias agent-claw-ensure := agent::agent-claw-ensure
alias agent-microvm-chat := agent::agent-microvm-chat
alias agent-microvm-server-logs := agent::agent-microvm-server-logs
alias agent-microvm-vmspawner-logs := agent::agent-microvm-vmspawner-logs
alias agent-microvm-guest-logs := agent::agent-microvm-guest-logs
alias agent-demo := agent::agent-demo
alias agent-demo-acp := agent::agent-demo-acp
alias agent-pi := agent::agent-pi
alias agent-claw := agent::agent-claw
alias agent-microvm-tunnel := agent::agent-microvm-tunnel
alias run-relay := infra::run-relay
alias run-relay-dev := infra::run-relay-dev
alias relay-build := infra::relay-build
alias pikahut := infra::pikahut
alias pikaci-remote-fulfill-deploy := infra::pikaci-remote-fulfill-deploy
alias pikaci-remote-fulfill-pre-merge-pika-rust := infra::pikaci-remote-fulfill-pre-merge-pika-rust
alias pikaci-remote-fulfill-pre-merge-agent-contracts := infra::pikaci-remote-fulfill-pre-merge-agent-contracts
alias pikaci-remote-fulfill-pre-merge-notifications := infra::pikaci-remote-fulfill-pre-merge-notifications
alias pikaci-remote-fulfill-pre-merge-rmp := infra::pikaci-remote-fulfill-pre-merge-rmp
alias pikaci-remote-fulfill-pre-merge-pikachat-rust := infra::pikaci-remote-fulfill-pre-merge-pikachat-rust
alias pikaci-pre-merge-pika-rust-prepares-remote-build := infra::pikaci-pre-merge-pika-rust-prepares-remote-build
alias linux-builder-repair := infra::linux-builder-repair
alias linux-builder-recreate := infra::linux-builder-recreate
alias news := infra::news
alias release-bump := ship::release-bump
alias release-commit := ship::release-commit
alias zapstore-encrypt-signing := ship::zapstore-encrypt-signing
alias zapstore-check := ship::zapstore-check
alias zapstore-publish := ship::zapstore-publish
alias rmp-init-smoke := rmp_tools::rmp-init-smoke
alias rmp-init-run := rmp_tools::rmp-init-run
alias rmp-phase4-qa := rmp_tools::rmp-phase4-qa
alias rmp-init-smoke-ci := rmp_tools::rmp-init-smoke-ci
alias rmp-nightly-linux := rmp_tools::rmp-nightly-linux
alias rmp-nightly-macos := rmp_tools::rmp-nightly-macos
