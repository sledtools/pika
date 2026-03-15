#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <ssh-host>" >&2
  exit 2
fi

host="$1"
ssh_dir="${HOME}/.ssh"
known_hosts="${ssh_dir}/known_hosts"
scan_output="$(mktemp)"
scan_err="$(mktemp)"
trap 'rm -f "$scan_output" "$scan_err"' EXIT

mkdir -p "$ssh_dir"
chmod 700 "$ssh_dir"

for attempt in 1 2 3 4 5; do
  : >"$scan_output"
  : >"$scan_err"
  if ssh-keyscan -T 5 -H "$host" >"$scan_output" 2>"$scan_err"; then
    :
  fi

  if [[ -s "$scan_output" ]]; then
    cat "$scan_output" >>"$known_hosts"
    chmod 600 "$known_hosts"
    exit 0
  fi

  if [[ "$attempt" -lt 5 ]]; then
    sleep 1
  fi
done

echo "error: failed to collect SSH host keys for $host after 5 attempts" >&2
if [[ -s "$scan_err" ]]; then
  cat "$scan_err" >&2
fi
exit 1
