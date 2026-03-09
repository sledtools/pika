#!/usr/bin/env bash
set -euo pipefail

script_name="$(basename "$0")"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

service_label="org.nixos.linux-builder"
plist="/Library/LaunchDaemons/${service_label}.plist"
builder_work_dir="/private/var/lib/linux-builder"
builder_run_dir="/private/var/run/${service_label}"
builder_disk="${builder_work_dir}/nixos.qcow2"
builder_store_img="${builder_run_dir}/store.img"
builder_host_port="${LINUX_BUILDER_RECREATE_HOST_PORT:-31022}"
qemu_pattern="qemu-system-aarch64 .*hostfwd=tcp::${builder_host_port}-:22"

usage() {
  cat <<EOF
Usage: ${script_name}
       ${script_name} --help

Recreate the local nix-darwin linux-builder state in place.

This script:
- unloads the existing ${service_label} launchd service
- removes ${builder_work_dir} and ${builder_run_dir}
- recreates ${builder_work_dir}
- reloads ${plist} so the stock local builder is recreated in place

It operates only on the live local builder service and image state.
It does not modify flake/config sources or apply any pikaci-specific workaround.

Environment:
- LINUX_BUILDER_RECREATE_HOST_PORT: expected host ssh forward for the builder (default: ${builder_host_port})
EOF
}

die() {
  echo "$*" >&2
  exit 1
}

print_status() {
  echo "==> launchd status"
  launchctl print "system/${service_label}" | sed -n '1,24p' || true
  echo
  echo "==> builder paths"
  ls -ld "${builder_work_dir}" "${builder_run_dir}" 2>/dev/null || true
  ls -l "${builder_disk}" "${builder_store_img}" 2>/dev/null || true
  echo
}

wait_for_process_exit() {
  local deadline=$((SECONDS + 30))

  while (( SECONDS < deadline )); do
    if ! pgrep -f "${qemu_pattern}" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done

  return 1
}

wait_for_running() {
  local deadline=$((SECONDS + 60))

  while (( SECONDS < deadline )); do
    if launchctl print "system/${service_label}" 2>/dev/null | grep -q 'state = running'; then
      return 0
    fi
    sleep 1
  done

  return 1
}

wait_for_path() {
  local path="$1"
  local deadline=$((SECONDS + 60))

  while (( SECONDS < deadline )); do
    if [[ -e "${path}" ]]; then
      return 0
    fi
    sleep 1
  done

  return 1
}

for arg in "$@"; do
  case "$arg" in
    --help|-h)
      usage
      exit 0
      ;;
    *)
      usage >&2
      die "unexpected argument: ${arg}"
      ;;
  esac
done

if [[ ! -f "${plist}" ]]; then
  die "missing launchd plist: ${plist}"
fi

case "${builder_work_dir}" in
  /private/var/lib/linux-builder) ;;
  *) die "refusing to operate on unexpected builder work dir: ${builder_work_dir}" ;;
esac

case "${builder_run_dir}" in
  /private/var/run/${service_label}) ;;
  *) die "refusing to operate on unexpected builder run dir: ${builder_run_dir}" ;;
esac

if [[ $(id -u) -ne 0 ]]; then
  if sudo -n true >/dev/null 2>&1; then
    exec sudo -- "$0" "$@"
  fi

  cat >&2 <<EOF
${script_name} needs root to recreate the local linux-builder service and disk state.

Run this exact command from the repo root:
  cd ${repo_root} && sudo ./scripts/linux-builder-recreate.sh
EOF
  exit 1
fi

echo "==> linux-builder privileged recreate"
echo "    service: ${service_label}"
echo "    plist: ${plist}"
echo "    work dir: ${builder_work_dir}"
echo "    run dir: ${builder_run_dir}"
echo "    disk image: ${builder_disk}"
echo "    store image: ${builder_store_img}"
echo "    host ssh port: ${builder_host_port}"
echo

print_status

echo "==> unload launchd service"
if launchctl print "system/${service_label}" >/dev/null 2>&1; then
  launchctl bootout system "${plist}" 2>/dev/null || launchctl unload "${plist}" || true
fi

if ! wait_for_process_exit; then
  die "qemu process for ${service_label} did not exit cleanly after unload"
fi
echo

echo "==> remove builder state"
rm -rf -- "${builder_run_dir}" "${builder_work_dir}"
mkdir -p "${builder_work_dir}"
chown root:wheel "${builder_work_dir}"
chmod 0755 "${builder_work_dir}"
echo

echo "==> reload launchd service"
launchctl bootstrap system "${plist}" 2>/dev/null || launchctl load -w "${plist}"
launchctl enable "system/${service_label}" >/dev/null 2>&1 || true
launchctl kickstart -k "system/${service_label}" >/dev/null 2>&1 || true

if ! wait_for_running; then
  die "launchd service ${service_label} did not return to running state"
fi

if ! wait_for_path "${builder_disk}"; then
  die "builder disk image was not recreated at ${builder_disk}"
fi

if ! wait_for_path "${builder_store_img}"; then
  die "builder store image was not recreated at ${builder_store_img}"
fi
echo

print_status

echo "linux-builder recreate completed."
