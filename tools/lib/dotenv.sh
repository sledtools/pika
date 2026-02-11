#!/usr/bin/env bash

# Load .env and .env.local without overriding existing environment variables.
# Intended for local dev/test runners (not production config parsing).
load_dotenv_no_override() {
  local root="${1:-.}"
  local f line key val

  for f in "$root/.env" "$root/.env.local"; do
    if [ ! -f "$f" ]; then
      continue
    fi
    while IFS= read -r line || [ -n "$line" ]; do
      # Trim whitespace.
      line="$(echo "$line" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
      if [ -z "${line:-}" ] || echo "$line" | grep -q '^#'; then
        continue
      fi
      if ! echo "$line" | grep -q '='; then
        continue
      fi
      key="${line%%=*}"
      val="${line#*=}"
      key="$(echo "$key" | tr -d '[:space:]')"
      # Allow `export KEY=...` in dev env files.
      if echo "$key" | grep -q '^export[[:space:]]'; then
        key="$(echo "$key" | sed -E 's/^export[[:space:]]+//')"
      fi
      if [ -z "${key:-}" ]; then
        continue
      fi
      # Strip a single pair of matching quotes.
      if [ "${#val}" -ge 2 ]; then
        case "$val" in
          \"*\")
            val="${val#\"}"
            val="${val%\"}"
            ;;
          \'*\')
            val="${val#\'}"
            val="${val%\'}"
            ;;
        esac
      fi
      # Indirect expansion: if already set, do not override.
      if eval "[ -z \"\${$key+x}\" ]"; then
        export "$key=$val"
      fi
    done <"$f"
  done
}
