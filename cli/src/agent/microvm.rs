use std::collections::BTreeMap;

use anyhow::anyhow;
use nostr_sdk::prelude::PublicKey;
use serde_json::json;

use crate::microvm_spawner::{CreateVmRequest, GuestAutostartRequest};
use crate::{AgentNewMicrovmArgs, MicrovmSpawnVariant};

pub const DEFAULT_SPAWNER_URL: &str = "http://127.0.0.1:8080";
const DEFAULT_FLAKE_REF: &str = "github:sledtools/pika";
const DEFAULT_DEV_SHELL: &str = "default";
const DEFAULT_CPU: u32 = 1;
const DEFAULT_MEMORY_MB: u32 = 1024;
const DEFAULT_TTL_SECONDS: u64 = 7200;
const AUTOSTART_COMMAND: &str = "bash /workspace/pika-agent/start-agent.sh";
const AUTOSTART_SCRIPT_PATH: &str = "workspace/pika-agent/start-agent.sh";
const AUTOSTART_BRIDGE_PATH: &str = "workspace/pika-agent/microvm-bridge.py";
const AUTOSTART_IDENTITY_PATH: &str = "workspace/pika-agent/state/identity.json";

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MicrovmResolvedArgs {
    pub spawner_url: String,
    pub spawn_variant: String,
    pub flake_ref: String,
    pub dev_shell: String,
    pub cpu: u32,
    pub memory_mb: u32,
    pub ttl_seconds: u64,
    pub keep: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MicrovmTeardownPolicy {
    DeleteOnExit,
    KeepVm,
}

pub fn resolve_args(args: &AgentNewMicrovmArgs) -> MicrovmResolvedArgs {
    MicrovmResolvedArgs {
        spawner_url: args
            .spawner_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_SPAWNER_URL)
            .to_string(),
        spawn_variant: spawn_variant_value(args.spawn_variant).to_string(),
        flake_ref: args
            .flake_ref
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_FLAKE_REF)
            .to_string(),
        dev_shell: args
            .dev_shell
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_DEV_SHELL)
            .to_string(),
        cpu: args.cpu.unwrap_or(DEFAULT_CPU),
        memory_mb: args.memory_mb.unwrap_or(DEFAULT_MEMORY_MB),
        ttl_seconds: args.ttl_seconds.unwrap_or(DEFAULT_TTL_SECONDS),
        keep: args.keep,
    }
}

pub fn teardown_policy(keep: bool) -> MicrovmTeardownPolicy {
    if keep {
        MicrovmTeardownPolicy::KeepVm
    } else {
        MicrovmTeardownPolicy::DeleteOnExit
    }
}

pub fn build_create_vm_request(
    resolved: &MicrovmResolvedArgs,
    owner_pubkey: PublicKey,
    relay_urls: &[String],
    bot_secret_hex: &str,
    bot_pubkey_hex: &str,
) -> CreateVmRequest {
    let mut env = BTreeMap::new();
    env.insert("PIKA_OWNER_PUBKEY".to_string(), owner_pubkey.to_hex());
    env.insert("PIKA_RELAY_URLS".to_string(), relay_urls.join(","));
    env.insert("PIKA_BOT_PUBKEY".to_string(), bot_pubkey_hex.to_string());
    for key in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "PI_MODEL"] {
        if let Ok(value) = std::env::var(key)
            && !value.trim().is_empty()
        {
            env.insert(key.to_string(), value);
        }
    }

    let mut files = BTreeMap::new();
    files.insert(
        AUTOSTART_SCRIPT_PATH.to_string(),
        microvm_autostart_script().to_string(),
    );
    files.insert(
        AUTOSTART_BRIDGE_PATH.to_string(),
        microvm_bridge_script().to_string(),
    );
    files.insert(
        AUTOSTART_IDENTITY_PATH.to_string(),
        bot_identity_file(bot_secret_hex, bot_pubkey_hex),
    );

    CreateVmRequest {
        flake_ref: Some(resolved.flake_ref.clone()),
        dev_shell: Some(resolved.dev_shell.clone()),
        cpu: Some(resolved.cpu),
        memory_mb: Some(resolved.memory_mb),
        ttl_seconds: Some(resolved.ttl_seconds),
        spawn_variant: Some(resolved.spawn_variant.clone()),
        guest_autostart: Some(GuestAutostartRequest {
            command: AUTOSTART_COMMAND.to_string(),
            env,
            files,
        }),
    }
}

pub fn spawner_create_error(spawner_url: &str, err: anyhow::Error) -> anyhow::Error {
    anyhow!(
        "failed to create microvm via vm-spawner at {}: {:#}\nhint: ensure vm-spawner is reachable (curl {}/healthz)\nif this is a remote host, open a tunnel:\n  just agent-microvm-tunnel",
        spawner_url,
        err,
        spawner_url.trim_end_matches('/')
    )
}

fn spawn_variant_value(value: Option<MicrovmSpawnVariant>) -> &'static str {
    match value.unwrap_or(MicrovmSpawnVariant::PrebuiltCow) {
        MicrovmSpawnVariant::Prebuilt => "prebuilt",
        MicrovmSpawnVariant::PrebuiltCow => "prebuilt-cow",
    }
}

fn bot_identity_file(secret_hex: &str, pubkey_hex: &str) -> String {
    let body = serde_json::to_string_pretty(&json!({
        "secret_key_hex": secret_hex,
        "public_key_hex": pubkey_hex,
    }))
    .expect("identity json");
    format!("{body}\n")
}

fn microvm_autostart_script() -> &'static str {
    r#"#!/usr/bin/env bash
set -euo pipefail

STATE_DIR="/workspace/pika-agent/state"
mkdir -p "$STATE_DIR"

if [[ -z "${PIKA_OWNER_PUBKEY:-}" ]]; then
  echo "[microvm-agent] missing PIKA_OWNER_PUBKEY" >&2
  exit 1
fi
if [[ -z "${PIKA_RELAY_URLS:-}" ]]; then
  echo "[microvm-agent] missing PIKA_RELAY_URLS" >&2
  exit 1
fi

relay_args=()
IFS=',' read -r -a relay_values <<< "${PIKA_RELAY_URLS}"
for relay in "${relay_values[@]}"; do
  relay="${relay#"${relay%%[![:space:]]*}"}"
  relay="${relay%"${relay##*[![:space:]]}"}"
  if [[ -n "$relay" ]]; then
    relay_args+=(--relay "$relay")
  fi
done
if [[ ${#relay_args[@]} -eq 0 ]]; then
  echo "[microvm-agent] no valid relays in PIKA_RELAY_URLS" >&2
  exit 1
fi

bin=""
if command -v pikachat >/dev/null 2>&1; then
  bin="pikachat"
elif [[ -n "${PIKA_MARMOTD_BIN:-}" ]]; then
  bin="${PIKA_MARMOTD_BIN}"
elif command -v marmotd >/dev/null 2>&1; then
  bin="marmotd"
fi
if [[ -z "$bin" ]]; then
  echo "[microvm-agent] could not find pikachat or marmotd binary" >&2
  exit 1
fi

echo "[microvm-agent] starting daemon via $bin" >&2
exec "$bin" daemon \
  --state-dir "$STATE_DIR" \
  --auto-accept-welcomes \
  --allow-pubkey "${PIKA_OWNER_PUBKEY}" \
  "${relay_args[@]}" \
  --exec "python3 /workspace/pika-agent/microvm-bridge.py"
"#
}

fn microvm_bridge_script() -> &'static str {
    r#"#!/usr/bin/env python3
import json
import os
import re
import shlex
import subprocess
import sys
from collections import deque
from urllib import error as urlerror
from urllib import request as urlrequest

owner = os.environ.get("PIKA_OWNER_PUBKEY", "").strip().lower()
relay_env = os.environ.get("PIKA_RELAY_URLS", "")
relays = [relay.strip() for relay in relay_env.split(",") if relay.strip()]
pi_cmd = os.environ.get("PIKA_PI_CMD", "pi -p").strip()
pi_timeout_ms = int(os.environ.get("PIKA_PI_TIMEOUT_MS", "120000"))
pi_history_turns = int(os.environ.get("PIKA_PI_HISTORY_TURNS", "8"))
pi_adapter_base_url = os.environ.get("PI_ADAPTER_BASE_URL", "").strip().rstrip("/")
pi_adapter_token = os.environ.get("PI_ADAPTER_TOKEN", "").strip()
anthropic_api_key = os.environ.get("ANTHROPIC_API_KEY", "").strip()
pi_model = os.environ.get("PI_MODEL", "claude-sonnet-4-6").strip()
agent_id = os.environ.get("PIKA_BOT_PUBKEY", "microvm-agent").strip()
if pi_timeout_ms < 1000:
    pi_timeout_ms = 1000
if pi_history_turns < 0:
    pi_history_turns = 0

ANSI_RE = re.compile(r"\x1B\[[0-?]*[ -/]*[@-~]")
seen_message_ids = deque(maxlen=256)
history_by_group = {}
anthropic_model_cache = None


def strip_ansi(text):
    return ANSI_RE.sub("", text)


def send(cmd):
    sys.stdout.write(json.dumps(cmd) + "\n")
    sys.stdout.flush()


def is_duplicate(message_id):
    if not message_id:
        return False
    if message_id in seen_message_ids:
        return True
    seen_message_ids.append(message_id)
    return False


def history_for_group(group_id):
    if pi_history_turns == 0:
        return None
    history = history_by_group.get(group_id)
    if history is None:
        history = deque(maxlen=pi_history_turns * 2)
        history_by_group[group_id] = history
    return history


def build_prompt(group_id, user_message):
    history = history_for_group(group_id)
    if history is None:
        return user_message
    lines = ["Conversation context:"]
    for role, content in history:
        lines.append(f"{role}: {content}")
    lines.append("assistant:")
    return "\n".join(lines)


def history_payload(group_id):
    history = history_for_group(group_id)
    if history is None:
        return []
    return [{"role": role, "content": content} for role, content in history]


def run_local_pi(prompt):
    if not pi_cmd:
        return None, "PIKA_PI_CMD is empty"
    try:
        proc = subprocess.run(
            shlex.split(pi_cmd),
            input=prompt + "\n",
            text=True,
            capture_output=True,
            timeout=pi_timeout_ms / 1000.0,
            check=False,
        )
    except FileNotFoundError:
        return None, f"pi command not found: {pi_cmd}"
    except subprocess.TimeoutExpired:
        return None, f"pi command timed out after {pi_timeout_ms}ms"
    except Exception as exc:
        return None, f"pi command failed: {exc}"

    stdout = strip_ansi(proc.stdout or "").strip()
    stderr = strip_ansi(proc.stderr or "").strip()
    if proc.returncode != 0:
        detail = stderr or stdout or f"exit code {proc.returncode}"
        return None, f"pi command failed ({detail})"

    lines = [line.strip() for line in stdout.splitlines() if line.strip()]
    if not lines:
        return None, "pi command returned empty output"
    return lines[-1], None


def extract_adapter_reply(parsed):
    if not isinstance(parsed, dict):
        return ""
    direct = str(parsed.get("reply") or (parsed.get("result") or {}).get("reply") or "").strip()
    if direct:
        return direct
    events = parsed.get("events")
    if not isinstance(events, list):
        return ""
    for event in events:
        if not isinstance(event, dict):
            continue
        for key in ("text", "delta", "reply", "message"):
            value = event.get(key)
            if isinstance(value, str) and value.strip():
                return value.strip()
        assistant = event.get("assistantMessageEvent")
        if isinstance(assistant, dict):
            delta = assistant.get("delta")
            if isinstance(delta, str) and delta.strip():
                return delta.strip()
    return ""


def run_pi_adapter(group_id, user_message):
    if not pi_adapter_base_url:
        return None, "PI_ADAPTER_BASE_URL is not set"
    payload = {
        "agent_id": agent_id,
        "message": user_message,
    }
    history = history_payload(group_id)
    if history:
        payload["history"] = history
    body = json.dumps(payload).encode("utf-8")
    headers = {"content-type": "application/json; charset=utf-8"}
    if pi_adapter_token:
        headers["authorization"] = f"Bearer {pi_adapter_token}"
    req = urlrequest.Request(
        f"{pi_adapter_base_url}/reply",
        data=body,
        headers=headers,
        method="POST",
    )
    try:
        with urlrequest.urlopen(req, timeout=pi_timeout_ms / 1000.0) as resp:
            text = resp.read().decode("utf-8", errors="replace")
    except urlerror.HTTPError as exc:
        err_body = exc.read().decode("utf-8", errors="replace")
        return None, f"pi-adapter HTTP {exc.code}: {err_body[:300]}"
    except Exception as exc:
        return None, f"pi-adapter request failed: {exc}"

    try:
        parsed = json.loads(text)
    except json.JSONDecodeError:
        return None, "pi-adapter returned invalid JSON"
    reply = extract_adapter_reply(parsed)
    if not reply:
        return None, "pi-adapter returned empty reply"
    return reply, None


def run_anthropic(prompt):
    if not anthropic_api_key:
        return None, "ANTHROPIC_API_KEY is not set"
    def anthropic_headers():
        return {
            "content-type": "application/json",
            "x-api-key": anthropic_api_key,
            "anthropic-version": "2023-06-01",
        }

    def call_model(model_id):
        body = json.dumps(
            {
                "model": model_id,
                "max_tokens": 512,
                "messages": [{"role": "user", "content": prompt}],
            }
        ).encode("utf-8")
        req = urlrequest.Request(
            "https://api.anthropic.com/v1/messages",
            data=body,
            headers=anthropic_headers(),
            method="POST",
        )
        try:
            with urlrequest.urlopen(req, timeout=pi_timeout_ms / 1000.0) as resp:
                text = resp.read().decode("utf-8", errors="replace")
        except urlerror.HTTPError as exc:
            err_body = exc.read().decode("utf-8", errors="replace")
            retry_model = exc.code == 404 and "model" in err_body.lower()
            return None, f"{model_id}: HTTP {exc.code}: {err_body[:300]}", retry_model
        except Exception as exc:
            return None, f"{model_id}: request failed: {exc}", False

        try:
            parsed = json.loads(text)
        except json.JSONDecodeError:
            return None, f"{model_id}: invalid JSON response", False
        content = parsed.get("content")
        if not isinstance(content, list):
            return None, f"{model_id}: response missing content", False
        for item in content:
            if not isinstance(item, dict):
                continue
            if item.get("type") != "text":
                continue
            text_value = str(item.get("text") or "").strip()
            if text_value:
                return text_value, None, False
        return None, f"{model_id}: no text output", False

    global anthropic_model_cache

    candidates = []
    if pi_model:
        candidates.append(pi_model)
    candidates.extend(
        [
            "claude-sonnet-4-6",
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4-20250514",
            "claude-3-haiku-20240307",
        ]
    )

    if anthropic_model_cache is None:
        req = urlrequest.Request(
            "https://api.anthropic.com/v1/models",
            headers=anthropic_headers(),
            method="GET",
        )
        try:
            with urlrequest.urlopen(req, timeout=pi_timeout_ms / 1000.0) as resp:
                payload = json.loads(resp.read().decode("utf-8", errors="replace"))
            models = payload.get("data")
            if isinstance(models, list):
                anthropic_model_cache = [
                    str(item.get("id") or "").strip()
                    for item in models
                    if isinstance(item, dict) and str(item.get("id") or "").strip()
                ]
            else:
                anthropic_model_cache = []
        except Exception:
            anthropic_model_cache = []
    candidates.extend(anthropic_model_cache)

    ordered_candidates = []
    seen = set()
    for model_id in candidates:
        trimmed = str(model_id).strip()
        if not trimmed or trimmed in seen:
            continue
        seen.add(trimmed)
        ordered_candidates.append(trimmed)

    last_error = "anthropic request failed"
    for model_id in ordered_candidates:
        reply, error_text, retry_model = call_model(model_id)
        if reply:
            return reply, None
        if error_text:
            last_error = error_text
        if not retry_model:
            return None, last_error
    return None, last_error


def generate_reply(group_id, user_message):
    prompt = build_prompt(group_id, user_message)
    errors = []

    if pi_adapter_base_url:
        reply, err = run_pi_adapter(group_id, user_message)
        if reply:
            return reply, None
        if err:
            errors.append(err)

    reply, err = run_local_pi(prompt)
    if reply:
        return reply, None
    if err:
        errors.append(err)

    if anthropic_api_key:
        reply, err = run_anthropic(prompt)
        if reply:
            return reply, None
        if err:
            errors.append(err)

    if not errors:
        errors.append("no pi backend configured")
    return None, "; ".join(errors)


for raw_line in sys.stdin:
    line = raw_line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except json.JSONDecodeError:
        continue

    kind = str(msg.get("type", ""))
    if kind == "ready":
        cmd = {"cmd": "publish_keypackage", "request_id": "microvm_boot_kp"}
        if relays:
            cmd["relays"] = relays
        send(cmd)
        continue

    if kind != "message_received":
        continue

    sender = str(msg.get("from_pubkey", "")).strip().lower()
    if owner and sender != owner:
        continue

    message_id = str(msg.get("message_id", "")).strip().lower()
    if is_duplicate(message_id):
        continue

    group_id = str(msg.get("nostr_group_id", "")).strip()
    content = str(msg.get("content", "")).strip()
    if not group_id or not content:
        continue

    history = history_for_group(group_id)
    if history is not None:
        history.append(("user", content))

    reply, err = generate_reply(group_id, content)
    if err:
        reply = f"[microvm] pi backend error: {err}"
    if history is not None and reply:
        history.append(("assistant", reply))

    send({
        "cmd": "send_message",
        "nostr_group_id": group_id,
        "content": reply,
    })
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::Keys;

    fn args_with_defaults() -> AgentNewMicrovmArgs {
        AgentNewMicrovmArgs {
            spawner_url: None,
            spawn_variant: None,
            flake_ref: None,
            dev_shell: None,
            cpu: None,
            memory_mb: None,
            ttl_seconds: None,
            keep: false,
        }
    }

    #[test]
    fn resolve_args_applies_defaults_and_overrides() {
        let defaults = resolve_args(&args_with_defaults());
        assert_eq!(defaults.spawner_url, DEFAULT_SPAWNER_URL);
        assert_eq!(defaults.spawn_variant, "prebuilt-cow");
        assert_eq!(defaults.flake_ref, DEFAULT_FLAKE_REF);
        assert_eq!(defaults.dev_shell, DEFAULT_DEV_SHELL);
        assert_eq!(defaults.cpu, DEFAULT_CPU);
        assert_eq!(defaults.memory_mb, DEFAULT_MEMORY_MB);
        assert_eq!(defaults.ttl_seconds, DEFAULT_TTL_SECONDS);
        assert!(!defaults.keep);

        let overridden = resolve_args(&AgentNewMicrovmArgs {
            spawner_url: Some("http://10.0.0.5:8080".to_string()),
            spawn_variant: Some(MicrovmSpawnVariant::Prebuilt),
            flake_ref: Some(".#nixpi".to_string()),
            dev_shell: Some("dev".to_string()),
            cpu: Some(2),
            memory_mb: Some(2048),
            ttl_seconds: Some(600),
            keep: true,
        });
        assert_eq!(overridden.spawner_url, "http://10.0.0.5:8080");
        assert_eq!(overridden.spawn_variant, "prebuilt");
        assert_eq!(overridden.flake_ref, ".#nixpi");
        assert_eq!(overridden.dev_shell, "dev");
        assert_eq!(overridden.cpu, 2);
        assert_eq!(overridden.memory_mb, 2048);
        assert_eq!(overridden.ttl_seconds, 600);
        assert!(overridden.keep);
    }

    #[test]
    fn build_create_vm_request_serializes_guest_autostart() {
        let resolved = resolve_args(&args_with_defaults());
        let keys = Keys::generate();
        let bot_keys = Keys::generate();
        let req = build_create_vm_request(
            &resolved,
            keys.public_key(),
            &[
                "wss://us-east.nostr.pikachat.org".to_string(),
                "wss://eu.nostr.pikachat.org".to_string(),
            ],
            &bot_keys.secret_key().to_secret_hex(),
            &bot_keys.public_key().to_hex(),
        );
        let value = serde_json::to_value(req).expect("serialize create vm request");

        assert_eq!(value["spawn_variant"], "prebuilt-cow");
        assert_eq!(value["guest_autostart"]["command"], AUTOSTART_COMMAND);
        assert_eq!(
            value["guest_autostart"]["env"]["PIKA_OWNER_PUBKEY"],
            keys.public_key().to_hex()
        );
        assert_eq!(
            value["guest_autostart"]["env"]["PIKA_RELAY_URLS"],
            "wss://us-east.nostr.pikachat.org,wss://eu.nostr.pikachat.org"
        );
        assert!(
            value["guest_autostart"]["files"][AUTOSTART_SCRIPT_PATH]
                .as_str()
                .expect("autostart script")
                .contains("starting daemon")
        );
        let bridge_script = value["guest_autostart"]["files"][AUTOSTART_BRIDGE_PATH]
            .as_str()
            .expect("bridge script");
        assert!(bridge_script.contains("publish_keypackage"));
        assert!(bridge_script.contains("run_pi"));
        assert!(!bridge_script.contains("microvm> {content}"));
        let identity_text = value["guest_autostart"]["files"][AUTOSTART_IDENTITY_PATH]
            .as_str()
            .expect("identity file");
        let identity_json: serde_json::Value =
            serde_json::from_str(identity_text).expect("parse identity file");
        assert_eq!(
            identity_json["public_key_hex"],
            serde_json::Value::String(bot_keys.public_key().to_hex())
        );
    }

    #[test]
    fn keep_flag_controls_teardown_policy() {
        assert_eq!(teardown_policy(false), MicrovmTeardownPolicy::DeleteOnExit);
        assert_eq!(teardown_policy(true), MicrovmTeardownPolicy::KeepVm);
    }
}
