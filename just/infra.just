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

# Run the staged `pre-merge-pika-rust` lane with strict remote-authoritative fulfillment on pika-build.
[private]
pikaci-remote-fulfill-pre-merge-pika-rust:
    ./scripts/pikaci-pre-merge-pika-rust-remote.sh run

# Realize both staged Linux Rust prepare outputs directly on pika-build without a Mac-side output bounce.
[private]
pikaci-pre-merge-pika-rust-prepares-remote-build:
    ./scripts/pikaci-pre-merge-pika-rust-remote.sh prepare

# Repair the legacy local aarch64-linux linux-builder staged Rust cargo-path corruption and rerun workspaceDeps.
[private]
linux-builder-repair:
    ./scripts/linux-builder-repair.sh

# Fully recreate the local linux-builder VM/store state via the existing launchd service after `just linux-builder-repair` still points at live builder corruption.
[private]
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
