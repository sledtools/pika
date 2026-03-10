#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Legacy helper for the local aarch64-linux linux-builder path. The preferred
# staged Linux Rust target is now ci.x86_64-linux, but the current vfkit execute
# guest still hard-codes aarch64-linux and may require this repair flow.
workspace_target="${LINUX_BUILDER_REPAIR_WORKSPACE_TARGET:-.#ci.aarch64-linux.workspaceDeps}"
service_label="org.nixos.linux-builder"
builder_disk="/private/var/lib/linux-builder/nixos.qcow2"
builder_store_img="/private/var/run/org.nixos.linux-builder/store.img"

find_drv_for_name() {
  local name="$1"
  local matches=()

  while IFS= read -r match; do
    matches+=("$match")
  done < <(find /nix/store -maxdepth 1 -name "*-${name}.drv" -print 2>/dev/null | sort)

  if [ "${#matches[@]}" -eq 0 ]; then
    return 1
  fi

  printf '%s\n' "${matches[0]}"
}

find_drv_for_output() {
  local expected_output="$1"
  local name="$2"
  local drv
  local actual_output

  while IFS= read -r drv; do
    actual_output="$(nix derivation show "$drv" | jq -r '.derivations[] | (.env.out // ("/nix/store/" + .outputs.out.path))')"
    if [ "$actual_output" = "$expected_output" ]; then
      printf '%s\n' "$drv"
      return 0
    fi
  done < <(find /nix/store -maxdepth 1 -name "*-${name}.drv" -print 2>/dev/null | sort)

  return 1
}

parse_current_failure() {
  local log_file="$1"
  local failing_path
  local failing_name

  failing_path="$(rg -o "hash mismatch importing path '/nix/store/[^']+'" "$log_file" | sed -E "s/^hash mismatch importing path '([^']+)'$/\\1/" | tail -n 1 || true)"
  if [ -z "$failing_path" ]; then
    return 1
  fi

  failing_name="$(basename "$failing_path")"

  case "$failing_name" in
    cargo-src-*)
      source_path="$failing_path"
      source_name="$failing_name"
      package_name="${failing_name/cargo-src-/cargo-package-}"
      ;;
    cargo-package-*)
      package_path="$failing_path"
      package_name="$failing_name"
      source_name="${failing_name/cargo-package-/cargo-src-}"
      ;;
    *)
      return 1
      ;;
  esac

  if [ -z "${source_path:-}" ]; then
    source_drv="$(find_drv_for_name "$source_name")"
    source_json="$(nix derivation show "$source_drv")"
    source_path="$(jq -r '.derivations[] | (.env.out // ("/nix/store/" + .outputs.out.path))' <<<"$source_json")"
  else
    source_drv="$(find_drv_for_output "$source_path" "$source_name")"
    source_json="$(nix derivation show "$source_drv")"
  fi

  if [ -z "${package_path:-}" ]; then
    package_drv="$(find_drv_for_name "$package_name")"
    package_json="$(nix derivation show "$package_drv")"
    package_path="$(jq -r '.derivations[] | (.env.out // ("/nix/store/" + .outputs.out.path))' <<<"$package_json")"
  else
    package_drv="$(find_drv_for_output "$package_path" "$package_name")"
    package_json="$(nix derivation show "$package_drv")"
  fi

  source_url="$(jq -r '.derivations[].structuredAttrs.urls[0]' <<<"$source_json")"
  source_hash="$(jq -r '.derivations[].structuredAttrs.outputHash' <<<"$source_json")"
}

echo "==> linux-builder staged Rust repair"
echo "    service: $service_label"
echo "    workspace target: $workspace_target"
echo

echo "==> launchd status"
launchctl print "system/$service_label" | sed -n '1,24p'
echo

tmp_log="$(mktemp "${TMPDIR:-/tmp}/linux-builder-repair.XXXXXX.log")"
cleanup() {
  rm -f "$tmp_log"
}
trap cleanup EXIT

echo "==> reproduce current workspaceDeps failure to identify the offending cargo path"
cd "$repo_root"
if nix build --no-link -L "$workspace_target" 2>&1 | tee "$tmp_log"; then
  echo
  echo "workspaceDeps already succeeds; no repair needed."
  exit 0
fi
echo

source_path=""
package_path=""
source_drv="${LINUX_BUILDER_REPAIR_CARGO_SRC_DRV:-}"
package_drv="${LINUX_BUILDER_REPAIR_CARGO_PACKAGE_DRV:-}"

if [ -n "$source_drv" ] && [ -n "$package_drv" ]; then
  source_json="$(nix derivation show "$source_drv")"
  package_json="$(nix derivation show "$package_drv")"
  source_path="$(jq -r '.derivations[] | (.env.out // ("/nix/store/" + .outputs.out.path))' <<<"$source_json")"
  package_path="$(jq -r '.derivations[] | (.env.out // ("/nix/store/" + .outputs.out.path))' <<<"$package_json")"
  source_name="$(basename "$source_path")"
  package_name="$(basename "$package_path")"
  source_url="$(jq -r '.derivations[].structuredAttrs.urls[0]' <<<"$source_json")"
  source_hash="$(jq -r '.derivations[].structuredAttrs.outputHash' <<<"$source_json")"
else
  parse_current_failure "$tmp_log"
fi

echo "==> identified current failing cargo paths"
echo "    source drv: $source_drv"
echo "    source path: $source_path"
echo "    package drv: $package_drv"
echo "    package path: $package_path"
echo

echo "==> ensure exact cargo source path exists locally"
if nix path-info "$source_path" >/dev/null 2>&1; then
  echo "    source path already valid: $source_path"
else
  echo "    prefetching $source_url"
  nix-prefetch-url --type sha256 --name "$source_name" "$source_url" >/dev/null
fi
echo "    source hash: $source_hash"
echo

echo "==> drop invalid local cargo-package copy if present"
if [ -e "$package_path" ] && ! nix path-info "$package_path" >/dev/null 2>&1; then
  nix-store --delete "$package_path"
  echo "    deleted invalid local path: $package_path"
elif nix path-info "$package_path" >/dev/null 2>&1; then
  echo "    package path already valid locally: $package_path"
else
  echo "    package path not present locally: $package_path"
fi
echo

echo "==> rerun staged workspaceDeps"
if nix build --no-link -L "$workspace_target"; then
  echo
  echo "workspaceDeps succeeded after local repair."
  exit 0
fi

cat >&2 <<EOF

workspaceDeps still failed after the host-side repair.

What this repair covered:
- reproduced the current workspaceDeps failure to discover the current bad cargo path
- reseeded the exact cargo source fixed-output path locally
- deleted the known-invalid imported cargo-package path from the local store

What likely remains:
- builder-side corruption inside the live linux-builder VM
- or the malformed builder cache DB reported at /root/.cache/nix/binary-cache-v7.sqlite

Privileged local builder reset/recreate likely needs to touch:
- $builder_disk
- $builder_store_img
- launchd service $service_label

Next step:
- just linux-builder-recreate
EOF

exit 1
