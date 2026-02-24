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

if [[ "$SPAWN_VARIANT" != "prebuilt" && "$SPAWN_VARIANT" != "prebuilt-cow" ]]; then
  echo "SPAWN_VARIANT must be prebuilt or prebuilt-cow for MVP demo."
  exit 1
fi

if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
  echo "ANTHROPIC_API_KEY is required."
  exit 1
fi

if ! curl -fsS "${SPAWNER_URL%/}/healthz" >/dev/null 2>&1; then
  echo "vm-spawner is unreachable at ${SPAWNER_URL}."
  echo "Open the tunnel first:"
  echo "  nix develop .#infra -c just -f infra/justfile build-vmspawner-tunnel"
  exit 1
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

exit "$status"
