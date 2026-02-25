use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{anyhow, Context};
use nostr_sdk::prelude::PublicKey;
use pika_agent_control_plane::MicrovmProvisionParams;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const DEFAULT_SPAWNER_URL: &str = "http://127.0.0.1:8080";
pub const DEFAULT_FLAKE_REF: &str = "github:sledtools/pika";
pub const DEFAULT_DEV_SHELL: &str = "default";
pub const DEFAULT_CPU: u32 = 1;
pub const DEFAULT_MEMORY_MB: u32 = 1024;
pub const DEFAULT_TTL_SECONDS: u64 = 7200;
pub const DEFAULT_SPAWN_VARIANT: &str = "prebuilt-cow";

pub const AUTOSTART_COMMAND: &str = "bash /workspace/pika-agent/start-agent.sh";
pub const AUTOSTART_SCRIPT_PATH: &str = "workspace/pika-agent/start-agent.sh";
pub const AUTOSTART_BRIDGE_PATH: &str = "workspace/pika-agent/microvm-bridge.py";
pub const AUTOSTART_IDENTITY_PATH: &str = "workspace/pika-agent/state/identity.json";

const DEFAULT_CREATE_VM_TIMEOUT_SECS: u64 = 60;
const MIN_CREATE_VM_TIMEOUT_SECS: u64 = 10;
const DELETE_VM_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ResolvedMicrovmParams {
    pub spawner_url: String,
    pub spawn_variant: String,
    pub flake_ref: String,
    pub dev_shell: String,
    pub cpu: u32,
    pub memory_mb: u32,
    pub ttl_seconds: u64,
    pub keep: bool,
}

#[derive(Debug, Serialize)]
pub struct CreateVmRequest {
    pub flake_ref: Option<String>,
    pub dev_shell: Option<String>,
    pub cpu: Option<u32>,
    pub memory_mb: Option<u32>,
    pub ttl_seconds: Option<u64>,
    pub spawn_variant: Option<String>,
    pub guest_autostart: Option<GuestAutostartRequest>,
}

#[derive(Debug, Serialize, Clone)]
pub struct GuestAutostartRequest {
    pub command: String,
    pub env: BTreeMap<String, String>,
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct VmResponse {
    pub id: String,
    pub ip: String,
}

#[derive(Debug, Clone)]
pub struct MicrovmSpawnerClient {
    client: reqwest::Client,
    base_url: String,
    create_vm_timeout: Duration,
}

impl MicrovmSpawnerClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let mut base_url = base_url.into();
        while base_url.ends_with('/') {
            base_url.pop();
        }
        Self {
            client: reqwest::Client::new(),
            base_url,
            create_vm_timeout: create_vm_timeout(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn create_vm(&self, req: &CreateVmRequest) -> anyhow::Result<VmResponse> {
        let url = format!("{}/vms", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(req)
            .timeout(self.create_vm_timeout)
            .send()
            .await
            .context("send create vm request")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("failed to create vm: {status} {text}");
        }
        resp.json().await.context("decode create vm response")
    }

    pub async fn delete_vm(&self, vm_id: &str) -> anyhow::Result<()> {
        let url = format!("{}/vms/{vm_id}", self.base_url);
        let resp = self
            .client
            .delete(&url)
            .timeout(DELETE_VM_TIMEOUT)
            .send()
            .await
            .context("send delete vm request")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("failed to delete vm {vm_id}: {status} {text}");
        }
        Ok(())
    }
}

pub fn microvm_params_provided(params: &MicrovmProvisionParams) -> bool {
    params.spawner_url.is_some()
        || params.spawn_variant.is_some()
        || params.flake_ref.is_some()
        || params.dev_shell.is_some()
        || params.cpu.is_some()
        || params.memory_mb.is_some()
        || params.ttl_seconds.is_some()
}

pub fn resolve_params(params: &MicrovmProvisionParams, keep: bool) -> ResolvedMicrovmParams {
    ResolvedMicrovmParams {
        spawner_url: params
            .spawner_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_SPAWNER_URL)
            .to_string(),
        spawn_variant: params
            .spawn_variant
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_SPAWN_VARIANT)
            .to_string(),
        flake_ref: params
            .flake_ref
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_FLAKE_REF)
            .to_string(),
        dev_shell: params
            .dev_shell
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_DEV_SHELL)
            .to_string(),
        cpu: params.cpu.unwrap_or(DEFAULT_CPU),
        memory_mb: params.memory_mb.unwrap_or(DEFAULT_MEMORY_MB),
        ttl_seconds: params.ttl_seconds.unwrap_or(DEFAULT_TTL_SECONDS),
        keep,
    }
}

pub fn build_create_vm_request(
    resolved: &ResolvedMicrovmParams,
    owner_pubkey: &PublicKey,
    relay_urls: &[String],
    bot_secret_hex: &str,
    bot_pubkey_hex: &str,
) -> CreateVmRequest {
    let mut env = BTreeMap::new();
    env.insert("PIKA_OWNER_PUBKEY".to_string(), owner_pubkey.to_hex());
    env.insert("PIKA_RELAY_URLS".to_string(), relay_urls.join(","));
    env.insert("PIKA_BOT_PUBKEY".to_string(), bot_pubkey_hex.to_string());
    for key in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "PI_MODEL"] {
        if let Ok(value) = std::env::var(key) {
            if value.trim().is_empty() {
                continue;
            }
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

pub fn bot_identity_file(secret_hex: &str, pubkey_hex: &str) -> String {
    let body = serde_json::to_string_pretty(&json!({
        "secret_key_hex": secret_hex,
        "public_key_hex": pubkey_hex,
    }))
    .expect("identity json");
    format!("{body}\n")
}

pub fn microvm_autostart_script() -> &'static str {
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

pub fn microvm_bridge_script() -> &'static str {
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

fn create_vm_timeout() -> Duration {
    let secs = std::env::var("PIKA_MICROVM_CREATE_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_CREATE_VM_TIMEOUT_SECS)
        .max(MIN_CREATE_VM_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::Keys;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration as StdDuration;

    #[derive(Debug)]
    struct CapturedRequest {
        method: String,
        path: String,
        body: String,
    }

    fn spawn_one_shot_server(
        status_line: &str,
        response_body: &str,
    ) -> (String, mpsc::Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().expect("read mock server addr");
        let (tx, rx) = mpsc::channel();
        let status_line = status_line.to_string();
        let response_body = response_body.to_string();

        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept mock request");
            let req = read_http_request(&mut stream);
            tx.send(req).expect("send captured request");

            let response = format!(
                "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write mock response");
        });

        (format!("http://{addr}"), rx)
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
        let mut buf = Vec::new();
        let mut header_end = None;
        let mut content_length = 0usize;

        loop {
            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).expect("read request bytes");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if header_end.is_none() {
                header_end = buf
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|idx| idx + 4);
                if let Some(end) = header_end {
                    let headers = String::from_utf8_lossy(&buf[..end]);
                    for line in headers.lines() {
                        if let Some((key, value)) = line.split_once(':') {
                            if key.eq_ignore_ascii_case("content-length") {
                                content_length = value.trim().parse::<usize>().unwrap_or(0);
                            }
                        }
                    }
                }
            }
            if let Some(end) = header_end {
                if buf.len() >= end + content_length {
                    break;
                }
            }
        }

        let end = header_end.expect("request headers must be present");
        let headers_raw = String::from_utf8_lossy(&buf[..end]);
        let request_line = headers_raw.lines().next().expect("request line");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().expect("method").to_string();
        let path = parts.next().expect("path").to_string();
        let body = String::from_utf8(buf[end..end + content_length].to_vec()).expect("utf8 body");

        CapturedRequest { method, path, body }
    }

    #[test]
    fn resolve_params_applies_defaults_and_overrides() {
        let defaults = resolve_params(&MicrovmProvisionParams::default(), false);
        assert_eq!(defaults.spawner_url, DEFAULT_SPAWNER_URL);
        assert_eq!(defaults.spawn_variant, DEFAULT_SPAWN_VARIANT);
        assert_eq!(defaults.flake_ref, DEFAULT_FLAKE_REF);
        assert_eq!(defaults.dev_shell, DEFAULT_DEV_SHELL);
        assert_eq!(defaults.cpu, DEFAULT_CPU);
        assert_eq!(defaults.memory_mb, DEFAULT_MEMORY_MB);
        assert_eq!(defaults.ttl_seconds, DEFAULT_TTL_SECONDS);
        assert!(!defaults.keep);

        let overridden = resolve_params(
            &MicrovmProvisionParams {
                spawner_url: Some("http://10.0.0.5:8080".to_string()),
                spawn_variant: Some("prebuilt".to_string()),
                flake_ref: Some(".#nixpi".to_string()),
                dev_shell: Some("dev".to_string()),
                cpu: Some(2),
                memory_mb: Some(2048),
                ttl_seconds: Some(600),
            },
            true,
        );
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
        let resolved = resolve_params(&MicrovmProvisionParams::default(), false);
        let keys = Keys::generate();
        let bot_keys = Keys::generate();
        let req = build_create_vm_request(
            &resolved,
            &keys.public_key(),
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
        assert!(value["guest_autostart"]["files"][AUTOSTART_SCRIPT_PATH]
            .as_str()
            .expect("autostart script")
            .contains("starting daemon"));
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
    fn microvm_params_provided_detects_presence() {
        assert!(!microvm_params_provided(&MicrovmProvisionParams::default()));
        assert!(microvm_params_provided(&MicrovmProvisionParams {
            ttl_seconds: Some(123),
            ..MicrovmProvisionParams::default()
        }));
    }

    #[tokio::test]
    async fn create_vm_contract_request_shape() {
        let (base_url, rx) =
            spawn_one_shot_server("200 OK", r#"{"id":"vm-123","ip":"192.168.0.10"}"#);
        let client = MicrovmSpawnerClient::new(base_url);
        let req = CreateVmRequest {
            flake_ref: Some(".#nixpi".to_string()),
            dev_shell: Some("default".to_string()),
            cpu: Some(2),
            memory_mb: Some(1024),
            ttl_seconds: Some(600),
            spawn_variant: Some("prebuilt-cow".to_string()),
            guest_autostart: Some(GuestAutostartRequest {
                command: "/workspace/pika-agent/start-agent.sh".to_string(),
                env: BTreeMap::from([("PIKA_OWNER_PUBKEY".to_string(), "pubkey123".to_string())]),
                files: BTreeMap::new(),
            }),
        };

        let vm = client.create_vm(&req).await.expect("create vm succeeds");
        assert_eq!(vm.id, "vm-123");
        assert_eq!(vm.ip, "192.168.0.10");

        let captured = rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("captured request");
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.path, "/vms");

        let json: serde_json::Value =
            serde_json::from_str(&captured.body).expect("parse json body");
        assert_eq!(json["flake_ref"], ".#nixpi");
        assert_eq!(json["dev_shell"], "default");
        assert_eq!(json["cpu"], 2);
        assert_eq!(json["memory_mb"], 1024);
        assert_eq!(json["ttl_seconds"], 600);
        assert_eq!(json["spawn_variant"], "prebuilt-cow");
        assert_eq!(
            json["guest_autostart"]["command"],
            "/workspace/pika-agent/start-agent.sh"
        );
        assert_eq!(
            json["guest_autostart"]["env"]["PIKA_OWNER_PUBKEY"],
            "pubkey123"
        );
    }

    #[tokio::test]
    async fn delete_vm_contract_request_shape() {
        let (base_url, rx) = spawn_one_shot_server("204 No Content", "");
        let client = MicrovmSpawnerClient::new(base_url);

        client
            .delete_vm("vm-delete-1")
            .await
            .expect("delete vm succeeds");

        let captured = rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("captured request");
        assert_eq!(captured.method, "DELETE");
        assert_eq!(captured.path, "/vms/vm-delete-1");
        assert!(captured.body.is_empty());
    }

    #[tokio::test]
    async fn create_vm_surfaces_error_body() {
        let (base_url, _rx) = spawn_one_shot_server("503 Service Unavailable", "spawner down");
        let client = MicrovmSpawnerClient::new(base_url);
        let req = CreateVmRequest {
            flake_ref: None,
            dev_shell: None,
            cpu: None,
            memory_mb: None,
            ttl_seconds: None,
            spawn_variant: None,
            guest_autostart: None,
        };

        let err = client
            .create_vm(&req)
            .await
            .expect_err("expected create_vm failure");
        let msg = err.to_string();
        assert!(msg.contains("failed to create vm"));
        assert!(msg.contains("503 Service Unavailable"));
        assert!(msg.contains("spawner down"));
    }
}
