#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/incus-managed-agent-smoke.sh \
    --api-base-url URL \
    --nsec NSEC \
    --incus-endpoint URL \
    --incus-project NAME \
    --incus-profile NAME \
    --incus-storage-pool NAME \
    --incus-image-alias ALIAS \
    [--agent-kind openclaw|pi] \
    [--incus-insecure-tls]

Request-scope smoke flow for the Incus dev lane:
  1. provision via pika-server with provider=incus
  2. poll until the agent reports ready
  3. print the final agent JSON

This requires a fresh test owner with no existing managed environment. If `agent new` reports
`created=false`, the script fails instead of silently reusing an existing VM.

This intentionally does not attempt cleanup because pika-server still has no public delete endpoint.
Use the dashboard reset flow or Incus operator commands for teardown after validation.
EOF
}

api_base_url=""
nsec=""
incus_endpoint=""
incus_project=""
incus_profile=""
incus_storage_pool=""
incus_image_alias=""
agent_kind="openclaw"
incus_insecure_tls=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --api-base-url) api_base_url="$2"; shift 2 ;;
    --nsec) nsec="$2"; shift 2 ;;
    --incus-endpoint) incus_endpoint="$2"; shift 2 ;;
    --incus-project) incus_project="$2"; shift 2 ;;
    --incus-profile) incus_profile="$2"; shift 2 ;;
    --incus-storage-pool) incus_storage_pool="$2"; shift 2 ;;
    --incus-image-alias) incus_image_alias="$2"; shift 2 ;;
    --agent-kind) agent_kind="$2"; shift 2 ;;
    --incus-insecure-tls) incus_insecure_tls=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage; exit 2 ;;
  esac
done

for required in api_base_url nsec incus_endpoint incus_project incus_profile incus_storage_pool incus_image_alias; do
  if [[ -z "${!required}" ]]; then
    echo "missing required argument: --${required//_/-}" >&2
    usage
    exit 2
  fi
done

cmd=(
  cargo run -q -p pikachat -- agent new
  --api-base-url "$api_base_url"
  --nsec "$nsec"
  --provider incus
  --runtime-kind "$agent_kind"
  --incus-endpoint "$incus_endpoint"
  --incus-project "$incus_project"
  --incus-profile "$incus_profile"
  --incus-storage-pool "$incus_storage_pool"
  --incus-image-alias "$incus_image_alias"
)
if [[ "$incus_insecure_tls" -eq 1 ]]; then
  cmd+=(--incus-insecure-tls)
fi
new_json="$("${cmd[@]}")"
created="$(printf '%s\n' "$new_json" | jq -er '.created')"
if [[ "$created" != "true" ]]; then
  echo "Incus smoke requires a fresh owner with no existing managed environment; got created=$created" >&2
  printf '%s\n' "$new_json" >&2
  exit 1
fi

for _ in $(seq 1 90); do
  state_json="$(cargo run -q -p pikachat -- agent me --api-base-url "$api_base_url" --nsec "$nsec")"
  state="$(printf '%s\n' "$state_json" | jq -r '.agent.state')"
  phase="$(printf '%s\n' "$state_json" | jq -r '.agent.startup_phase')"
  if [[ "$state" == "ready" ]]; then
    printf '%s\n' "$state_json"
    exit 0
  fi
  echo "waiting for ready: state=$state phase=$phase" >&2
  sleep 2
done

echo "timed out waiting for Incus managed agent readiness" >&2
exit 1
