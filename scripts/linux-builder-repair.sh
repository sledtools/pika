#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

source_drv="${LINUX_BUILDER_REPAIR_CARGO_SRC_DRV:-/nix/store/q2l6kpdzbfb817ysvi6kkn6rhbxp0x95-cargo-src-linux-raw-sys-0.4.15.drv}"
package_drv="${LINUX_BUILDER_REPAIR_CARGO_PACKAGE_DRV:-/nix/store/ihwd4clfw242kgj9j1g80d51bag6a87y-cargo-package-linux-raw-sys-0.4.15.drv}"
workspace_target="${LINUX_BUILDER_REPAIR_WORKSPACE_TARGET:-.#ci.aarch64-linux.workspaceDeps}"

service_label="org.nixos.linux-builder"
builder_disk="/private/var/lib/linux-builder/nixos.qcow2"
builder_store_img="/private/var/run/org.nixos.linux-builder/store.img"

source_json="$(nix derivation show "$source_drv")"
source_path="$(jq -r '.derivations[] | (.env.out // ("/nix/store/" + .outputs.out.path))' <<<"$source_json")"
source_name="$(basename "$source_path")"
source_url="$(jq -r '.derivations[].structuredAttrs.urls[0]' <<<"$source_json")"
source_hash="$(jq -r '.derivations[].structuredAttrs.outputHash' <<<"$source_json")"

package_json="$(nix derivation show "$package_drv")"
package_path="$(jq -r '.derivations[] | (.env.out // ("/nix/store/" + .outputs.out.path))' <<<"$package_json")"

echo "==> linux-builder staged Rust repair"
echo "    service: $service_label"
echo "    source drv: $source_drv"
echo "    package drv: $package_drv"
echo "    workspace target: $workspace_target"
echo

echo "==> launchd status"
launchctl print "system/$service_label" | sed -n '1,24p'
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
cd "$repo_root"
if nix build --no-link -L "$workspace_target"; then
  echo
  echo "workspaceDeps succeeded after local repair."
  exit 0
fi

cat >&2 <<EOF

workspaceDeps still failed after the host-side repair.

What this repair covered:
- reseeded the exact cargo source fixed-output path locally
- deleted the known-invalid imported cargo-package path from the local store

What likely remains:
- builder-side corruption inside the live linux-builder VM
- or the malformed builder cache DB reported at /root/.cache/nix/binary-cache-v7.sqlite

Privileged local builder reset/recreate likely needs to touch:
- $builder_disk
- $builder_store_img
- launchd service $service_label
EOF

exit 1
