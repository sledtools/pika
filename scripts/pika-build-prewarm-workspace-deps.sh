#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
source "$script_dir/lib/pikaci-tools.sh"

load_remote_defaults() {
  resolve_pikaci_tools "$repo_root"
  eval "$("$PIKACI_BIN" staged-linux-remote-defaults)"
}

usage() {
  cat <<'EOF'
Usage: pika-build-prewarm-workspace-deps.sh [--store-uri URI] [--installable INSTALLABLE]

Prewarm the exact pre-compile closure for the staged x86_64 Linux workspaceDeps lane onto a
remote Nix store before running the real build. This targets the cold-start tax before cargo
begins on pika-build by asking the destination store to substitute what it can directly.

Options:
  --store-uri URI        Remote Nix store URI to prewarm. Default: ssh://pika-build
  --installable TARGET   Installable to inspect. Default: .#ci.x86_64-linux.workspaceDeps
  -h, --help             Show this help.
EOF
}

store_uri="${PIKACI_X86_64_REMOTE_STORE_URI:-}"
installable=".#ci.x86_64-linux.workspaceDeps"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --store-uri)
      store_uri="${2:?missing value for --store-uri}"
      shift 2
      ;;
    --installable)
      installable="${2:?missing value for --installable}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$store_uri" ]]; then
  load_remote_defaults
  store_uri="${default_store_uri}"
fi

cd "$repo_root"

tmp_json="$(mktemp)"
tmp_output_selectors="$(mktemp)"
tmp_outputs="$(mktemp)"
cleanup() {
  rm -f "$tmp_json" "$tmp_output_selectors" "$tmp_outputs"
}
trap cleanup EXIT

echo "==> inspecting $installable"
nix derivation show "$installable" >"$tmp_json"

drv_path="$(nix eval --raw "${installable}.drvPath")"
jq -r '
  .derivations
  | to_entries[0].value.inputs.drvs
  | to_entries[]
  | .key as $drv
  | .value.outputs[]
  | "/nix/store/\($drv)^\(.)"
' <"$tmp_json" >"$tmp_output_selectors"

jq -e '.derivations | length > 0' <"$tmp_json" >/dev/null

xargs nix build --no-link --print-out-paths <"$tmp_output_selectors" >"$tmp_outputs"
jq -r '.derivations | to_entries[0].value.env.cargoLock' <"$tmp_json" >>"$tmp_outputs"
sort -u -o "$tmp_outputs" "$tmp_outputs"

root_count="$(wc -l <"$tmp_outputs" | tr -d ' ')"
closure_count="$(xargs nix path-info -r <"$tmp_outputs" | wc -l | tr -d ' ')"

echo "==> prewarming remote store"
echo "    store: $store_uri"
echo "    derivation: $drv_path"
echo "    root paths: $root_count"
echo "    recursive closure paths: $closure_count"

nix copy --substitute-on-destination --to "$store_uri" $(cat "$tmp_outputs")
nix copy --derivation --to "$store_uri" "$drv_path"

echo "prewarm complete."
