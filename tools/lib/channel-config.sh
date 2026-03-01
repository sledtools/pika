#!/usr/bin/env bash
set -euo pipefail

_channel_config_root() {
  cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd
}

channel_config_file() {
  echo "$(_channel_config_root)/config/channels.json"
}

_channel_config_query() {
  local query="$1"
  shift
  local config_file
  config_file="$(channel_config_file)"
  python3 - "$config_file" "$query" "$@" <<'PY'
import json
import sys

config_path, query, *args = sys.argv[1:]
with open(config_path, "r", encoding="utf-8") as f:
    data = json.load(f)

channels = {entry["id"]: entry for entry in data.get("channels", [])}

def fail() -> None:
    raise SystemExit(1)

def print_value(value):
    if not value:
        fail()
    print(value)

if query == "default":
    print_value(data.get("default_channel"))
elif query == "scheme":
    print_value(channels.get(args[0], {}).get("url_scheme"))
elif query == "ios_bundle_id":
    print_value(channels.get(args[0], {}).get("ios_bundle_id"))
elif query == "android_application_id":
    print_value(channels.get(args[0], {}).get("android_application_id"))
elif query == "from_ios_bundle_id":
    bundle_id = args[0]
    for channel in data.get("channels", []):
        if channel.get("ios_bundle_id") == bundle_id:
            print_value(channel.get("id"))
            break
    else:
        fail()
elif query == "from_android_application_id":
    app_id = args[0]
    for channel in data.get("channels", []):
        if channel.get("android_application_id") == app_id:
            print_value(channel.get("id"))
            break
    else:
        fail()
else:
    fail()
PY
}

channel_default() {
  _channel_config_query default
}

channel_scheme() {
  local channel="$1"
  _channel_config_query scheme "$channel"
}

channel_ios_bundle_id() {
  local channel="$1"
  _channel_config_query ios_bundle_id "$channel"
}

channel_android_application_id() {
  local channel="$1"
  _channel_config_query android_application_id "$channel"
}

channel_from_ios_bundle_id() {
  local bundle_id="$1"
  _channel_config_query from_ios_bundle_id "$bundle_id"
}

channel_from_android_application_id() {
  local app_id="$1"
  _channel_config_query from_android_application_id "$app_id"
}
