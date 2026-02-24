#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SPAWNER_URL="${SPAWNER_URL:-http://127.0.0.1:8080}"
SPAWN_VARIANT="${SPAWN_VARIANT:-prebuilt-cow}"
FLAKE_REF="${FLAKE_REF:-github:sledtools/pika}"
DEV_SHELL="${DEV_SHELL:-default}"
CPU="${CPU:-1}"
MEMORY_MB="${MEMORY_MB:-1024}"
TTL_SECONDS="${TTL_SECONDS:-7200}"
RELAY_PRIMARY="${RELAY_PRIMARY:-wss://us-east.nostr.pikachat.org}"
RELAY_FALLBACK="${RELAY_FALLBACK:-}"
KEEP_VM="${KEEP_VM:-0}"
AUTO_TUNNEL="${AUTO_TUNNEL:-1}"
AUTO_TUNNEL_TIMEOUT_SECONDS="${AUTO_TUNNEL_TIMEOUT_SECONDS:-20}"
TUNNEL_CMD="${TUNNEL_CMD:-nix develop .#infra -c just -f infra/justfile build-vmspawner-tunnel}"
DEFAULT_SPAWNER_URL="http://127.0.0.1:8080"
TUNNEL_PID=""
TUNNEL_LOG=""
STARTED_TUNNEL=0

if [[ "$SPAWN_VARIANT" != "prebuilt" && "$SPAWN_VARIANT" != "prebuilt-cow" ]]; then
  echo "SPAWN_VARIANT must be prebuilt or prebuilt-cow for MVP demo."
  exit 1
fi

if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
  echo "ANTHROPIC_API_KEY is required."
  exit 1
fi

cleanup() {
  if [[ -n "$TUNNEL_PID" ]]; then
    kill "$TUNNEL_PID" >/dev/null 2>&1 || true
    wait "$TUNNEL_PID" 2>/dev/null || true
  fi
  if [[ -n "$TUNNEL_LOG" ]]; then
    rm -f "$TUNNEL_LOG"
  fi
}
trap cleanup EXIT

spawner_reachable() {
  curl -fsS "${SPAWNER_URL%/}/healthz" >/dev/null 2>&1
}

if ! spawner_reachable; then
  if [[ "$AUTO_TUNNEL" == "1" && "$SPAWNER_URL" == "$DEFAULT_SPAWNER_URL" ]]; then
    echo "vm-spawner is unreachable at ${SPAWNER_URL}; starting tunnel..."
    TUNNEL_LOG="$(mktemp -t pika-microvm-tunnel.XXXXXX.log)"
    bash -lc "$TUNNEL_CMD" >"$TUNNEL_LOG" 2>&1 &
    TUNNEL_PID=$!
    STARTED_TUNNEL=1

    for _ in $(seq 1 "$((AUTO_TUNNEL_TIMEOUT_SECONDS * 2))"); do
      if spawner_reachable; then
        break
      fi
      if ! kill -0 "$TUNNEL_PID" >/dev/null 2>&1; then
        echo "failed to start vm-spawner tunnel (process exited early)."
        echo "Tunnel command: $TUNNEL_CMD"
        echo "Tunnel log (tail):"
        tail -n 120 "$TUNNEL_LOG" || true
        exit 1
      fi
      sleep 0.5
    done

    if ! spawner_reachable; then
      echo "timed out waiting for vm-spawner at ${SPAWNER_URL} after ${AUTO_TUNNEL_TIMEOUT_SECONDS}s."
      echo "Tunnel command: $TUNNEL_CMD"
      echo "Tunnel log (tail):"
      tail -n 120 "$TUNNEL_LOG" || true
      exit 1
    fi
    echo "vm-spawner tunnel is ready."
  else
    echo "vm-spawner is unreachable at ${SPAWNER_URL}."
    if [[ "$SPAWNER_URL" == "$DEFAULT_SPAWNER_URL" ]]; then
      echo "Open the tunnel first:"
      echo "  just agent-microvm-tunnel"
    fi
    exit 1
  fi
fi

cmd=(
  cargo run -q -p pikachat --
  --relay "$RELAY_PRIMARY"
)

if [[ -n "$RELAY_FALLBACK" ]]; then
  cmd+=(--relay "$RELAY_FALLBACK")
fi

cmd+=(
  agent new
  --provider microvm
  --brain pi
  --spawner-url "$SPAWNER_URL"
  --spawn-variant "$SPAWN_VARIANT"
  --flake-ref "$FLAKE_REF"
  --dev-shell "$DEV_SHELL"
  --cpu "$CPU"
  --memory-mb "$MEMORY_MB"
  --ttl-seconds "$TTL_SECONDS"
)

if [[ "$KEEP_VM" == "1" ]]; then
  cmd+=(--keep)
fi

cmd+=("$@")

echo "Running microVM agent demo..."
status=0
if "${cmd[@]}"; then
  status=0
else
  status=$?
fi

echo
if [[ "$KEEP_VM" == "1" ]]; then
  echo "VM was kept alive (--keep)."
  echo "List VMs:   curl ${SPAWNER_URL%/}/vms"
  echo "Delete VM:  curl -X DELETE ${SPAWNER_URL%/}/vms/<vm-id>"
else
  echo "VM teardown is automatic unless --keep is set."
fi
if [[ "$STARTED_TUNNEL" == "1" ]]; then
  echo "Closed auto-started vm-spawner tunnel."
fi

exit "$status"
