set working-directory := '..'

# ── pika-relay (local Nostr relay + Blossom server) ─────────────────────────
#
# Run pika-relay locally (relay on :3334, blossom on same port).
run-relay *ARGS:
    cd cmd/pika-relay && go run . {{ ARGS }}

# Run pika-relay with custom data/media dirs (persistent local dev).
run-relay-dev:
    ./scripts/run-relay-dev

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
    ./scripts/pikaci-staged-linux-remote.sh run pre-merge-pika-rust

# Realize both staged Linux Rust prepare outputs directly on pika-build without a Mac-side output bounce.
[private]
pikaci-pre-merge-pika-rust-prepares-remote-build:
    ./scripts/pikaci-staged-linux-remote.sh prepare pre-merge-pika-rust

# Run the staged `pre-merge-agent-contracts` lane with strict remote-authoritative fulfillment on pika-build.
[private]
pikaci-remote-fulfill-pre-merge-agent-contracts:
    ./scripts/pikaci-staged-linux-remote.sh run pre-merge-agent-contracts

# Run the staged `pre-merge-notifications` lane with strict remote-authoritative fulfillment on pika-build.
[private]
pikaci-remote-fulfill-pre-merge-notifications:
    ./scripts/pikaci-staged-linux-remote.sh run pre-merge-notifications

# Run the staged `pre-merge-rmp` lane with strict remote-authoritative fulfillment on pika-build.
[private]
pikaci-remote-fulfill-pre-merge-rmp:
    ./scripts/pikaci-staged-linux-remote.sh run pre-merge-rmp

# Run the staged `pre-merge-pikachat-rust` lane with strict remote-authoritative fulfillment on pika-build.
[private]
pikaci-remote-fulfill-pre-merge-pikachat-rust:
    ./scripts/pikaci-staged-linux-remote.sh run pre-merge-pikachat-rust

# Run pika-news with auto-rebuild on source changes.
[private]
news MAX_PRS="2":
    ./scripts/news {{ MAX_PRS }}
