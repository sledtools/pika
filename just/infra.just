set working-directory := '..'

# ── pika-relay (local Nostr relay + Blossom server) ─────────────────────────
#
# Run pika-relay locally (relay on :3334, blossom on same port).
run-relay *ARGS:
    cd cmd/pika-relay && go run . {{ ARGS }}

# Run pika-relay with custom data/media dirs (persistent local dev).
run-relay-dev:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p .pika-relay/data .pika-relay/media
    cd cmd/pika-relay && \
      DATA_DIR=../../.pika-relay/data \
      MEDIA_DIR=../../.pika-relay/media \
      SERVICE_URL=http://localhost:3334 \
      go run .

# Build pika-relay binary.
[private]
relay-build:
    cd cmd/pika-relay && go build -o ../../target/pika-relay .

# ── Local backend (postgres + relay + pika-server) ──────────────────────────
# Start local backend (postgres, pika-relay, pika-server with agent control).
#
# State persists in .pikahut/. Press Ctrl-C to stop all services.
[private]
pikahut-up:
    cargo run -p pikahut -- up --profile backend

# TUI: pikahut-up + component logs + interactive shell via mprocs.
[private]
pikahut:
    mprocs

# Deploy the `pikaci` helper and launcher binaries onto pika-build via the host NixOS config.
[private]
pikaci-remote-fulfill-deploy:
    nix develop .#infra -c just -f infra/justfile build-deploy

# Run the staged `pre-merge-pika-rust` lane with remote prepared-output fulfillment on pika-build.
# Assumptions:
# - pika-build has `/run/current-system/sw/bin/pikaci-launch-fulfill-prepared-output`
# - pika-build has `/run/current-system/sw/bin/pikaci-fulfill-prepared-output`
# - ssh access to `{{env_var_or_default("PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST", "pika-build")}}` works
#
# - nix can copy the realized prepared output to that host
[private]
pikaci-remote-fulfill-pre-merge-pika-rust:
    #!/usr/bin/env bash
    set -euo pipefail

    export PIKACI_PRE_MERGE_PIKA_RUST_SUBPROCESS_FULFILL=1
    export PIKACI_PREPARED_OUTPUT_FULFILL_INVOCATION=external_wrapper_command_v1
    export PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_TRANSPORT=ssh_launcher_transport_v1
    export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST:-pika-build}"
    export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_LAUNCHER_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_LAUNCHER_BINARY:-/run/current-system/sw/bin/pikaci-launch-fulfill-prepared-output}"
    export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_HELPER_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_HELPER_BINARY:-/run/current-system/sw/bin/pikaci-fulfill-prepared-output}"
    export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR:-/var/tmp/pikaci-prepared-output}"

    cargo build -p pikaci --bins
    export PIKACI_PREPARED_OUTPUT_FULFILL_BINARY="$PWD/target/debug/pikaci-fulfill-prepared-output"
    export PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY="$PWD/target/debug/pikaci-launch-fulfill-prepared-output"
    exec "$PWD/target/debug/pikaci" run pre-merge-pika-rust

# Prewarm the exact `ci.x86_64-linux.workspaceDeps` pre-compile closure onto pika-build.
pikaci-workspace-deps-prewarm:
    ./scripts/pika-build-prewarm-workspace-deps.sh

# Prewarm + build the exact `ci.x86_64-linux.workspaceDeps` lane on pika-build with an explicit x86_64 builder override.
pikaci-workspace-deps-remote-build:
    ./scripts/pika-build-run-workspace-deps.sh

# Fast-build both staged Linux Rust prepare outputs for the real pre-merge pika Rust lane on pika-build.
pikaci-pre-merge-pika-rust-prepares-remote-build:
    ./scripts/pika-build-run-workspace-deps.sh --installable .#ci.x86_64-linux.workspaceDeps
    ./scripts/pika-build-run-workspace-deps.sh --installable .#ci.x86_64-linux.workspaceBuild

# Repair the legacy local aarch64-linux linux-builder staged Rust cargo-path corruption and rerun workspaceDeps.
linux-builder-repair:
    ./scripts/linux-builder-repair.sh

# Fully recreate the local linux-builder VM/store state via the existing launchd service after `just linux-builder-repair` still points at live builder corruption.
linux-builder-recreate:
    ./scripts/linux-builder-recreate.sh

# Run pika-news with auto-rebuild on source changes.
[private]
news MAX_PRS="2":
    #!/usr/bin/env bash
    set -euo pipefail
    set -a; source .env; set +a
    export GITHUB_TOKEN="$(gh auth token)"
    cargo watch -w crates/pika-news -x "run -p pika-news -- serve \
      --config crates/pika-news/pika-news.toml \
      --db .tmp/pika-news.db \
      --max-prs {{ MAX_PRS }}"
