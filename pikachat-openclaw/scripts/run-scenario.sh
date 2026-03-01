#!/usr/bin/env bash
set -Eeuo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <scenario> [-- <extra-args...>]" >&2
  exit 2
fi

SCENARIO="$1"
shift || true

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

case "$SCENARIO" in
  invite-and-chat)
    selector="openclaw_scenario_invite_and_chat"
    ;;
  invite-and-chat-rust-bot)
    selector="openclaw_scenario_invite_and_chat_rust_bot"
    ;;
  invite-and-chat-daemon)
    selector="openclaw_scenario_invite_and_chat_daemon"
    ;;
  audio-echo)
    selector="openclaw_scenario_audio_echo"
    ;;
  *)
    echo "error: unknown scenario '$SCENARIO'" >&2
    echo "supported: invite-and-chat | invite-and-chat-rust-bot | invite-and-chat-daemon | audio-echo" >&2
    exit 2
    ;;
esac

if [[ -n "${STATE_DIR:-}" || -n "${RELAY_URL:-}" ]]; then
  echo "note: STATE_DIR/RELAY_URL are ignored by selector wrappers; configure environment expected by selectors instead." >&2
fi

if [[ $# -gt 0 ]]; then
  echo "note: extra scenario args are not supported by selector wrappers and will be ignored: $*" >&2
fi

exec cargo test -p pikahut --test integration_deterministic "$selector" -- --ignored --nocapture
