#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/incus-dogfood-check.sh --api-base-url URL --nsec NSEC [options]

Options:
  --remote-host HOST     SSH host running Incus (default: pika-build)
  --project NAME         Incus project (default: pika-managed-agents)
  --storage-pool NAME    Incus storage pool (default: default)

This is a repeated dogfood helper for the internal Incus lane. It prints:
  - current managed-agent API state
  - VM ID
  - whether the Incus instance exists
  - whether the matching -state volume exists
  - the current guest ready marker, when available
EOF
}

api_base_url=""
nsec=""
remote_host="pika-build"
project="pika-managed-agents"
storage_pool="default"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --api-base-url)
      api_base_url="${2:-}"
      shift 2
      ;;
    --nsec)
      nsec="${2:-}"
      shift 2
      ;;
    --remote-host)
      remote_host="${2:-}"
      shift 2
      ;;
    --project)
      project="${2:-}"
      shift 2
      ;;
    --storage-pool)
      storage_pool="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "$api_base_url" || -z "$nsec" ]]; then
  usage >&2
  exit 1
fi

tmp_json="$(mktemp)"
trap 'rm -f "$tmp_json"' EXIT

cargo run -q -p pikachat -- agent me \
  --api-base-url "$api_base_url" \
  --nsec "$nsec" >"$tmp_json"

echo "== API state =="
cat "$tmp_json"
echo

vm_id="$(
  python3 - <<'PY' "$tmp_json"
import json, sys
with open(sys.argv[1], "r", encoding="utf-8") as fh:
    data = json.load(fh)
print((data.get("agent") or {}).get("vm_id") or "")
PY
)"

if [[ -z "$vm_id" ]]; then
  echo "No VM ID is currently assigned."
  exit 0
fi

remote_incus() {
  ssh "$remote_host" "sudo incus $*"
}

echo "== Incus instance =="
instance_status="$(remote_incus "list --project '$project' '$vm_id' --format csv -c ns" | tr -d '\r' || true)"
if [[ -z "$instance_status" ]]; then
  echo "missing"
else
  printf '%s\n' "$instance_status"
fi

echo
echo "== Incus state volume =="
volume_status="$(
  remote_incus "storage volume list '$storage_pool' --project '$project' --format csv -c nt" \
    | awk -F, -v target="${vm_id}-state" '$1 == target { print $0 }'
)"
if [[ -z "$volume_status" ]]; then
  echo "missing"
else
  printf '%s\n' "$volume_status"
fi

echo
echo "== Guest lifecycle status =="
remote_incus "file pull --project '$project' '$vm_id'/run/pika-cloud/status.json -" 2>/dev/null || echo "missing"
