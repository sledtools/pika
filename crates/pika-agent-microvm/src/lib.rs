use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{anyhow, Context};
use nostr_sdk::prelude::PublicKey;
use pika_agent_control_plane::{
    MicrovmAgentBackend, MicrovmAgentKind, MicrovmProvisionParams,
    SpawnerCreateVmRequest as CreateVmRequest,
    SpawnerGuestAutostartRequest as GuestAutostartRequest, SpawnerVmResponse as VmResponse,
};
use serde_json::json;

pub const DEFAULT_SPAWNER_URL: &str = "http://127.0.0.1:8080";

pub const AUTOSTART_COMMAND: &str = "bash /workspace/pika-agent/start-agent.sh";
pub const AUTOSTART_SCRIPT_PATH: &str = "workspace/pika-agent/start-agent.sh";
pub const AUTOSTART_IDENTITY_PATH: &str = "workspace/pika-agent/state/identity.json";
pub const AGENT_READY_PATH: &str = "workspace/pika-agent/service-ready.json";
pub const AGENT_FAILED_PATH: &str = "workspace/pika-agent/service-failed.json";
pub const OPENCLAW_CONFIG_PATH: &str = "workspace/pika-agent/openclaw/openclaw.json";
pub const OPENCLAW_EXTENSION_ROOT: &str =
    "workspace/pika-agent/openclaw/extensions/pikachat-openclaw";
pub const DEFAULT_ACP_EXEC_COMMAND: &str = "npx -y pi-acp";
pub const DEFAULT_ACP_CWD: &str = "/root/pika-agent/acp";
pub const DEFAULT_OPENCLAW_EXEC_COMMAND: &str = "npx -y openclaw";
pub const DEFAULT_OPENCLAW_GATEWAY_PORT: u16 = 18789;

const DEFAULT_CREATE_VM_TIMEOUT_SECS: u64 = 60;
const MIN_CREATE_VM_TIMEOUT_SECS: u64 = 10;
const DELETE_VM_TIMEOUT: Duration = Duration::from_secs(30);
const RECOVER_VM_TIMEOUT: Duration = Duration::from_secs(60);
const REQUEST_ID_HEADER: &str = "x-request-id";

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ResolvedMicrovmParams {
    pub spawner_url: String,
    pub kind: ResolvedMicrovmAgentKind,
    pub backend: ResolvedMicrovmAgentBackend,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ResolvedMicrovmAgentKind {
    Pi,
    Openclaw,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ResolvedMicrovmAgentBackend {
    Native,
    Acp { exec_command: String, cwd: String },
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
        self.create_vm_with_request_id(req, None).await
    }

    pub async fn create_vm_with_request_id(
        &self,
        req: &CreateVmRequest,
        request_id: Option<&str>,
    ) -> anyhow::Result<VmResponse> {
        let url = format!("{}/vms", self.base_url);
        let resp = with_request_id(
            self.client
                .post(&url)
                .json(req)
                .timeout(self.create_vm_timeout),
            request_id,
        )
        .send()
        .await
        .context("send create vm request")?;
        let status = resp.status();
        if !status.is_success() {
            let upstream_request_id = response_request_id(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                upstream_error_message(
                    "create vm",
                    None,
                    status,
                    upstream_request_id.as_deref(),
                    &text
                )
            );
        }
        resp.json().await.context("decode create vm response")
    }

    pub async fn delete_vm(&self, vm_id: &str) -> anyhow::Result<()> {
        self.delete_vm_with_request_id(vm_id, None).await
    }

    pub async fn delete_vm_with_request_id(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let url = format!("{}/vms/{vm_id}", self.base_url);
        let resp = with_request_id(
            self.client.delete(&url).timeout(DELETE_VM_TIMEOUT),
            request_id,
        )
        .send()
        .await
        .context("send delete vm request")?;
        let status = resp.status();
        if !status.is_success() {
            let upstream_request_id = response_request_id(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                upstream_error_message(
                    "delete vm",
                    Some(vm_id),
                    status,
                    upstream_request_id.as_deref(),
                    &text
                )
            );
        }
        Ok(())
    }

    pub async fn recover_vm(&self, vm_id: &str) -> anyhow::Result<VmResponse> {
        self.recover_vm_with_request_id(vm_id, None).await
    }

    pub async fn recover_vm_with_request_id(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<VmResponse> {
        let url = format!("{}/vms/{vm_id}/recover", self.base_url);
        let resp = with_request_id(
            self.client.post(&url).timeout(RECOVER_VM_TIMEOUT),
            request_id,
        )
        .send()
        .await
        .context("send recover vm request")?;
        let status = resp.status();
        if !status.is_success() {
            let upstream_request_id = response_request_id(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                upstream_error_message(
                    "recover vm",
                    Some(vm_id),
                    status,
                    upstream_request_id.as_deref(),
                    &text
                )
            );
        }
        resp.json().await.context("decode recover vm response")
    }

    pub async fn get_vm(&self, vm_id: &str) -> anyhow::Result<VmResponse> {
        self.get_vm_with_request_id(vm_id, None).await
    }

    pub async fn get_vm_with_request_id(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<VmResponse> {
        let url = format!("{}/vms/{vm_id}", self.base_url);
        let resp = with_request_id(self.client.get(&url).timeout(DELETE_VM_TIMEOUT), request_id)
            .send()
            .await
            .context("send get vm request")?;
        let status = resp.status();
        if !status.is_success() {
            let upstream_request_id = response_request_id(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                upstream_error_message(
                    "get vm",
                    Some(vm_id),
                    status,
                    upstream_request_id.as_deref(),
                    &text
                )
            );
        }
        resp.json().await.context("decode get vm response")
    }
}

fn with_request_id(
    request: reqwest::RequestBuilder,
    request_id: Option<&str>,
) -> reqwest::RequestBuilder {
    match request_id.map(str::trim).filter(|value| !value.is_empty()) {
        Some(request_id) => request.header(REQUEST_ID_HEADER, request_id),
        None => request,
    }
}

fn response_request_id(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn format_request_id_suffix(request_id: Option<&str>) -> String {
    request_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|request_id| format!(" (request_id={request_id})"))
        .unwrap_or_default()
}

fn sanitize_upstream_body(body: &str) -> Option<String> {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let collapsed = collapsed.trim();
    if collapsed.is_empty() {
        return None;
    }
    const MAX_LEN: usize = 240;
    if collapsed.len() <= MAX_LEN {
        return Some(collapsed.to_string());
    }
    let mut truncated = collapsed
        .char_indices()
        .take_while(|(byte_idx, _)| *byte_idx < MAX_LEN)
        .map(|(_, ch)| ch)
        .collect::<String>();
    truncated.push_str("...");
    Some(truncated)
}

fn upstream_error_message(
    action: &str,
    vm_id: Option<&str>,
    status: reqwest::StatusCode,
    request_id: Option<&str>,
    body: &str,
) -> String {
    let vm_suffix = vm_id.map(|vm_id| format!(" {vm_id}")).unwrap_or_default();
    let request_id_suffix = format_request_id_suffix(request_id);
    match sanitize_upstream_body(body) {
        Some(body) => {
            format!("failed to {action}{vm_suffix}: {status}{request_id_suffix} body={body}")
        }
        None => format!("failed to {action}{vm_suffix}: {status}{request_id_suffix}"),
    }
}

pub fn microvm_params_provided(params: &MicrovmProvisionParams) -> bool {
    params.spawner_url.is_some() || params.kind.is_some() || params.backend.is_some()
}

pub fn resolve_params(params: &MicrovmProvisionParams) -> ResolvedMicrovmParams {
    ResolvedMicrovmParams {
        spawner_url: params
            .spawner_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_SPAWNER_URL)
            .to_string(),
        kind: resolve_kind(params.kind),
        backend: resolve_backend(params.backend.as_ref()),
    }
}

pub fn validate_resolved_params(params: &ResolvedMicrovmParams) -> anyhow::Result<()> {
    if matches!(params.kind, ResolvedMicrovmAgentKind::Pi)
        && matches!(params.backend, ResolvedMicrovmAgentBackend::Native)
    {
        anyhow::bail!(
            "microvm agent kind `pi` requires ACP backend mode; use backend=acp or choose kind=openclaw for native daemon mode"
        );
    }
    Ok(())
}

fn resolve_kind(kind: Option<MicrovmAgentKind>) -> ResolvedMicrovmAgentKind {
    match kind.unwrap_or(MicrovmAgentKind::Pi) {
        MicrovmAgentKind::Pi => ResolvedMicrovmAgentKind::Pi,
        MicrovmAgentKind::Openclaw => ResolvedMicrovmAgentKind::Openclaw,
    }
}

fn resolve_backend(backend: Option<&MicrovmAgentBackend>) -> ResolvedMicrovmAgentBackend {
    match backend {
        Some(MicrovmAgentBackend::Native) | None => ResolvedMicrovmAgentBackend::Native,
        Some(MicrovmAgentBackend::Acp { exec_command, cwd }) => ResolvedMicrovmAgentBackend::Acp {
            exec_command: exec_command
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_ACP_EXEC_COMMAND)
                .to_string(),
            cwd: cwd
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_ACP_CWD)
                .to_string(),
        },
    }
}

pub fn build_create_vm_request(
    owner_pubkey: &PublicKey,
    relay_urls: &[String],
    bot_secret_hex: &str,
    bot_pubkey_hex: &str,
    params: &ResolvedMicrovmParams,
) -> CreateVmRequest {
    let mut env = BTreeMap::new();
    env.insert("PIKA_OWNER_PUBKEY".to_string(), owner_pubkey.to_hex());
    env.insert("PIKA_RELAY_URLS".to_string(), relay_urls.join(","));
    env.insert("PIKA_BOT_PUBKEY".to_string(), bot_pubkey_hex.to_string());
    env.insert(
        "PIKA_AGENT_KIND".to_string(),
        match params.kind {
            ResolvedMicrovmAgentKind::Pi => "pi".to_string(),
            ResolvedMicrovmAgentKind::Openclaw => "openclaw".to_string(),
        },
    );
    match &params.backend {
        ResolvedMicrovmAgentBackend::Native => {
            env.insert("PIKA_AGENT_BACKEND_MODE".to_string(), "native".to_string());
        }
        ResolvedMicrovmAgentBackend::Acp { exec_command, cwd } => {
            env.insert("PIKA_AGENT_BACKEND_MODE".to_string(), "acp".to_string());
            env.insert("PIKA_AGENT_ACP_EXEC".to_string(), exec_command.clone());
            env.insert("PIKA_AGENT_ACP_CWD".to_string(), cwd.clone());
        }
    }
    for key in [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "PI_MODEL",
        "PIKA_OPENCLAW_EXEC",
    ] {
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
        AUTOSTART_IDENTITY_PATH.to_string(),
        bot_identity_file(bot_secret_hex, bot_pubkey_hex),
    );
    if matches!(params.kind, ResolvedMicrovmAgentKind::Openclaw) {
        files.insert(
            OPENCLAW_CONFIG_PATH.to_string(),
            openclaw_gateway_config(relay_urls, &params.backend),
        );
        files.extend(openclaw_extension_files());
    }

    CreateVmRequest {
        guest_autostart: GuestAutostartRequest {
            command: AUTOSTART_COMMAND.to_string(),
            env,
            files,
        },
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

# Keep Marmot/MLS state under /root so VM restart/recovery preserves context.
STATE_DIR="/root/pika-agent/state"
READY_PATH="/workspace/pika-agent/service-ready.json"
FAILED_PATH="/workspace/pika-agent/service-failed.json"
mkdir -p "$STATE_DIR"
# Seed the durable state dir from the provisioned identity on first boot only.
if [[ -f "/workspace/pika-agent/state/identity.json" && ! -f "$STATE_DIR/identity.json" ]]; then
  cp "/workspace/pika-agent/state/identity.json" "$STATE_DIR/identity.json"
fi
rm -f "$READY_PATH" "$FAILED_PATH"

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
if [[ -n "${PIKA_PIKACHAT_BIN:-}" && -x "${PIKA_PIKACHAT_BIN}" ]]; then
  bin="${PIKA_PIKACHAT_BIN}"
elif command -v pikachat >/dev/null 2>&1; then
  bin="pikachat"
fi
if [[ -z "$bin" ]]; then
  echo "[microvm-agent] could not find pikachat binary" >&2
  exit 1
fi
if [[ "$bin" != "pikachat" ]]; then
  export PATH="$(dirname "$bin"):$PATH"
fi

agent_kind="${PIKA_AGENT_KIND:-pi}"
backend_mode="${PIKA_AGENT_BACKEND_MODE:-native}"

write_ready_marker() {
  local probe="$1"
  cat >"$READY_PATH" <<EOF
{
  "ready": true,
  "agent_kind": "${agent_kind}",
  "backend_mode": "${backend_mode}",
  "probe": "${probe}"
}
EOF
  rm -f "$FAILED_PATH"
}

write_failed_marker() {
  local reason="$1"
  cat >"$FAILED_PATH" <<EOF
{
  "ready": false,
  "agent_kind": "${agent_kind}",
  "backend_mode": "${backend_mode}",
  "reason": "${reason}"
}
EOF
  rm -f "$READY_PATH"
}

wait_for_pi_ready() {
  local agent_pid="$1"
  local timeout_sec="${PIKA_AGENT_READY_TIMEOUT_SECS:-120}"
  local deadline=$((SECONDS + timeout_sec))
  while (( SECONDS < deadline )); do
    if ! kill -0 "$agent_pid" 2>/dev/null; then
      wait "$agent_pid"
      return $?
    fi
    if [[ -f /workspace/pika-agent/agent.log ]] && grep -q '"type":"ready"' /workspace/pika-agent/agent.log; then
      write_ready_marker "daemon_ready_event"
      return 0
    fi
    sleep 1
  done
  write_failed_marker "timeout_waiting_for_daemon_ready"
  kill "$agent_pid" 2>/dev/null || true
  wait "$agent_pid" || true
  return 1
}

wait_for_openclaw_ready() {
  local agent_pid="$1"
  local port="${PIKA_OPENCLAW_GATEWAY_PORT:-18789}"
  local timeout_sec="${PIKA_AGENT_READY_TIMEOUT_SECS:-120}"
  local deadline=$((SECONDS + timeout_sec))
  while (( SECONDS < deadline )); do
    if ! kill -0 "$agent_pid" 2>/dev/null; then
      wait "$agent_pid"
      return $?
    fi
    if curl -fsS --max-time 2 "http://127.0.0.1:${port}/health" >/dev/null 2>&1; then
      write_ready_marker "openclaw_gateway_health"
      return 0
    fi
    sleep 1
  done
  write_failed_marker "timeout_waiting_for_openclaw_health"
  kill "$agent_pid" 2>/dev/null || true
  wait "$agent_pid" || true
  return 1
}

case "$agent_kind" in
  pi)
    echo "[microvm-agent] starting pi agent daemon via $bin" >&2
    daemon_args=(
      daemon
      --state-dir "$STATE_DIR"
      --auto-accept-welcomes
      --allow-pubkey "${PIKA_OWNER_PUBKEY}"
      "${relay_args[@]}"
    )
    case "$backend_mode" in
      native)
        ;;
      acp)
        acp_exec="${PIKA_AGENT_ACP_EXEC:-npx -y pi-acp}"
        acp_cwd="${PIKA_AGENT_ACP_CWD:-/root/pika-agent/acp}"
        if [[ -z "$acp_exec" ]]; then
          echo "[microvm-agent] ACP backend requires a non-empty ACP exec command" >&2
          exit 1
        fi
        mkdir -p "$acp_cwd"
        daemon_args+=(--acp-exec "$acp_exec" --acp-cwd "$acp_cwd")
        ;;
      *)
        echo "[microvm-agent] unsupported backend mode for pi agent (expected native or acp): $backend_mode" >&2
        exit 1
        ;;
    esac
    "$bin" "${daemon_args[@]}" &
    agent_pid=$!
    if ! wait_for_pi_ready "$agent_pid"; then
      exit 1
    fi
    wait "$agent_pid"
    status=$?
    rm -f "$READY_PATH"
    if [[ $status -ne 0 ]]; then
      write_failed_marker "pi_agent_exited"
    else
      rm -f "$FAILED_PATH"
    fi
    exit $status
    ;;
  openclaw)
    openclaw_exec="${PIKA_OPENCLAW_EXEC:-npx -y openclaw}"
    openclaw_state_dir="/root/pika-agent/openclaw"
    openclaw_config_path="/workspace/pika-agent/openclaw/openclaw.json"
    if [[ -z "$openclaw_exec" ]]; then
      echo "[microvm-agent] openclaw agent requires a non-empty OpenClaw exec command" >&2
      exit 1
    fi
    if [[ ! -f "$openclaw_config_path" ]]; then
      echo "[microvm-agent] missing OpenClaw config at $openclaw_config_path" >&2
      exit 1
    fi
    mkdir -p "$openclaw_state_dir"
    export OPENCLAW_STATE_DIR="$openclaw_state_dir"
    export OPENCLAW_CONFIG_PATH="$openclaw_config_path"
    export OPENCLAW_SKIP_BROWSER_CONTROL_SERVER=1
    export OPENCLAW_SKIP_GMAIL_WATCHER=1
    export OPENCLAW_SKIP_CANVAS_HOST=1
    export OPENCLAW_SKIP_CRON=1
    export PIKA_OPENCLAW_GATEWAY_PORT="${PIKA_OPENCLAW_GATEWAY_PORT:-18789}"
    echo "[microvm-agent] starting openclaw agent via $openclaw_exec" >&2
    # Use a login shell so npx/openclaw installed via profile-managed Node setups
    # are available in the guest PATH.
    bash -lc "$openclaw_exec gateway --allow-unconfigured" &
    agent_pid=$!
    if ! wait_for_openclaw_ready "$agent_pid"; then
      exit 1
    fi
    wait "$agent_pid"
    status=$?
    rm -f "$READY_PATH"
    if [[ $status -ne 0 ]]; then
      write_failed_marker "openclaw_gateway_exited"
    else
      rm -f "$FAILED_PATH"
    fi
    exit $status
    ;;
  *)
    echo "[microvm-agent] unsupported agent kind (expected pi or openclaw): $agent_kind" >&2
    exit 1
    ;;
esac
"#
}

fn openclaw_gateway_config(relay_urls: &[String], backend: &ResolvedMicrovmAgentBackend) -> String {
    let mut channel_config = json!({
        "relays": relay_urls,
        "stateDir": "/root/pika-agent/state",
        "autoAcceptWelcomes": true,
        "groupPolicy": "open",
        "daemonCmd": "pikachat",
    });
    match backend {
        ResolvedMicrovmAgentBackend::Native => {
            channel_config["daemonBackend"] = json!("native");
        }
        ResolvedMicrovmAgentBackend::Acp { exec_command, cwd } => {
            channel_config["daemonBackend"] = json!("acp");
            channel_config["daemonAcpExec"] = json!(exec_command);
            channel_config["daemonAcpCwd"] = json!(cwd);
        }
    }
    // Keep the plugin entry config and channel config identical so either OpenClaw
    // surface sees the same daemon launch settings.
    let entry_config = channel_config.clone();
    serde_json::to_string_pretty(&json!({
        "gateway": {
            "mode": "local",
            "bind": "loopback",
            "port": DEFAULT_OPENCLAW_GATEWAY_PORT,
        },
        "plugins": {
            "enabled": true,
            "allow": ["pikachat-openclaw"],
            "load": {
                "paths": [format!("/{}", OPENCLAW_EXTENSION_ROOT)]
            },
            "slots": {
                "memory": "none"
            },
            "entries": {
                "pikachat-openclaw": {
                    "enabled": true,
                    "config": entry_config
                }
            }
        },
        "channels": {
            "pikachat-openclaw": channel_config
        }
    }))
    .expect("serialize openclaw config")
}

fn openclaw_extension_files() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/package.json"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/package.json").to_string(),
        ),
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/openclaw.plugin.json"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/openclaw.plugin.json").to_string(),
        ),
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/index.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/index.ts").to_string(),
        ),
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/tsconfig.json"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/tsconfig.json").to_string(),
        ),
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/src/channel-behavior.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/channel-behavior.ts").to_string(),
        ),
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/src/channel.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/channel.ts").to_string(),
        ),
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/src/config-schema.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/config-schema.ts").to_string(),
        ),
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/src/config.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/config.ts").to_string(),
        ),
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/src/daemon-launch.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/daemon-launch.ts").to_string(),
        ),
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/src/daemon-protocol.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/daemon-protocol.ts").to_string(),
        ),
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/src/runtime.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/runtime.ts").to_string(),
        ),
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/src/sidecar-install.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/sidecar-install.ts").to_string(),
        ),
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/src/sidecar.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/sidecar.ts").to_string(),
        ),
        (
            format!("{OPENCLAW_EXTENSION_ROOT}/src/types.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/types.ts").to_string(),
        ),
    ])
}

#[cfg(test)]
fn expected_openclaw_extension_paths() -> &'static [&'static str] {
    &[
        "package.json",
        "openclaw.plugin.json",
        "index.ts",
        "tsconfig.json",
        "src/channel-behavior.ts",
        "src/channel.ts",
        "src/config-schema.ts",
        "src/config.ts",
        "src/daemon-launch.ts",
        "src/daemon-protocol.ts",
        "src/runtime.ts",
        "src/sidecar-install.ts",
        "src/sidecar.ts",
        "src/types.ts",
    ]
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
    use pika_test_utils::spawn_one_shot_server;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration as StdDuration;

    #[test]
    fn resolve_params_applies_defaults_and_overrides() {
        let defaults = resolve_params(&MicrovmProvisionParams::default());
        assert_eq!(defaults.spawner_url, DEFAULT_SPAWNER_URL);
        assert_eq!(defaults.kind, ResolvedMicrovmAgentKind::Pi);
        assert_eq!(defaults.backend, ResolvedMicrovmAgentBackend::Native);

        let overridden = resolve_params(&MicrovmProvisionParams {
            spawner_url: Some("http://10.0.0.5:8080".to_string()),
            kind: Some(MicrovmAgentKind::Openclaw),
            backend: Some(MicrovmAgentBackend::Acp {
                exec_command: Some("npx -y pi-acp".to_string()),
                cwd: Some("/tmp/acp".to_string()),
            }),
        });
        assert_eq!(overridden.spawner_url, "http://10.0.0.5:8080");
        assert_eq!(overridden.kind, ResolvedMicrovmAgentKind::Openclaw);
        assert_eq!(
            overridden.backend,
            ResolvedMicrovmAgentBackend::Acp {
                exec_command: "npx -y pi-acp".to_string(),
                cwd: "/tmp/acp".to_string(),
            }
        );
    }

    #[test]
    fn validate_resolved_params_rejects_pi_native_mode() {
        let err = validate_resolved_params(&ResolvedMicrovmParams {
            spawner_url: DEFAULT_SPAWNER_URL.to_string(),
            kind: ResolvedMicrovmAgentKind::Pi,
            backend: ResolvedMicrovmAgentBackend::Native,
        })
        .expect_err("pi native mode should be rejected");
        assert!(err.to_string().contains("requires ACP backend mode"));
    }

    #[test]
    fn build_create_vm_request_serializes_guest_autostart() {
        let keys = Keys::generate();
        let bot_keys = Keys::generate();
        let req = build_create_vm_request(
            &keys.public_key(),
            &[
                "wss://relay-a.example.com".to_string(),
                "wss://relay-b.example.com".to_string(),
            ],
            &bot_keys.secret_key().to_secret_hex(),
            &bot_keys.public_key().to_hex(),
            &ResolvedMicrovmParams {
                spawner_url: DEFAULT_SPAWNER_URL.to_string(),
                kind: ResolvedMicrovmAgentKind::Pi,
                backend: ResolvedMicrovmAgentBackend::Native,
            },
        );
        let value = serde_json::to_value(req).expect("serialize create vm request");

        assert_eq!(value["guest_autostart"]["command"], AUTOSTART_COMMAND);
        assert_eq!(
            value["guest_autostart"]["env"]["PIKA_OWNER_PUBKEY"],
            keys.public_key().to_hex()
        );
        assert_eq!(
            value["guest_autostart"]["env"]["PIKA_RELAY_URLS"],
            "wss://relay-a.example.com,wss://relay-b.example.com"
        );
        assert_eq!(value["guest_autostart"]["env"]["PIKA_AGENT_KIND"], "pi");
        assert_eq!(
            value["guest_autostart"]["env"]["PIKA_AGENT_BACKEND_MODE"],
            "native"
        );
        assert!(value["guest_autostart"]["files"][AUTOSTART_SCRIPT_PATH]
            .as_str()
            .expect("autostart script")
            .contains("starting pi agent daemon"));
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
    fn autostart_script_uses_root_backed_state_dir() {
        let script = microvm_autostart_script();
        assert!(
            script.contains("STATE_DIR=\"/root/pika-agent/state\""),
            "autostart script must keep state under /root for restart durability"
        );
        assert!(script.contains("READY_PATH=\"/workspace/pika-agent/service-ready.json\""));
        assert!(script.contains("FAILED_PATH=\"/workspace/pika-agent/service-failed.json\""));
        assert!(script.contains("--state-dir \"$STATE_DIR\""));
        assert!(script.contains("PIKA_PIKACHAT_BIN"));
        assert!(script.contains("PIKA_AGENT_KIND"));
        assert!(script.contains("PIKA_AGENT_BACKEND_MODE"));
        assert!(script.contains("wait_for_pi_ready"));
        assert!(script.contains("wait_for_openclaw_ready"));
        assert!(script.contains("curl -fsS --max-time 2 \"http://127.0.0.1:${port}/health\""));
        assert!(script.contains("openclaw)"));
        assert!(script.contains("--acp-exec \"$acp_exec\""));
        assert!(script.contains("npx -y openclaw"));
        assert!(
            !script.contains("marmotd"),
            "autostart script must only resolve pikachat daemon binary"
        );
    }

    #[test]
    fn microvm_params_provided_detects_presence() {
        assert!(!microvm_params_provided(&MicrovmProvisionParams::default()));
        assert!(microvm_params_provided(&MicrovmProvisionParams {
            spawner_url: Some("http://127.0.0.1:8080".to_string()),
            kind: None,
            backend: None,
        }));
        assert!(microvm_params_provided(&MicrovmProvisionParams {
            spawner_url: None,
            kind: Some(MicrovmAgentKind::Openclaw),
            backend: None,
        }));
        assert!(microvm_params_provided(&MicrovmProvisionParams {
            spawner_url: None,
            kind: None,
            backend: Some(MicrovmAgentBackend::Native),
        }));
        assert!(microvm_params_provided(&MicrovmProvisionParams {
            spawner_url: None,
            kind: None,
            backend: Some(MicrovmAgentBackend::Acp {
                exec_command: None,
                cwd: None,
            }),
        }));
    }

    #[test]
    fn build_create_vm_request_includes_acp_backend_env() {
        let keys = Keys::generate();
        let bot_keys = Keys::generate();
        let req = build_create_vm_request(
            &keys.public_key(),
            &["wss://relay-a.example.com".to_string()],
            &bot_keys.secret_key().to_secret_hex(),
            &bot_keys.public_key().to_hex(),
            &ResolvedMicrovmParams {
                spawner_url: DEFAULT_SPAWNER_URL.to_string(),
                kind: ResolvedMicrovmAgentKind::Pi,
                backend: ResolvedMicrovmAgentBackend::Acp {
                    exec_command: "npx -y pi-acp".to_string(),
                    cwd: "/root/pika-agent/acp".to_string(),
                },
            },
        );
        let value = serde_json::to_value(req).expect("serialize create vm request");
        assert_eq!(
            value["guest_autostart"]["env"]["PIKA_AGENT_BACKEND_MODE"],
            "acp"
        );
        assert_eq!(
            value["guest_autostart"]["env"]["PIKA_AGENT_ACP_EXEC"],
            "npx -y pi-acp"
        );
        assert_eq!(
            value["guest_autostart"]["env"]["PIKA_AGENT_ACP_CWD"],
            "/root/pika-agent/acp"
        );
    }

    #[test]
    fn build_create_vm_request_includes_openclaw_payload() {
        let keys = Keys::generate();
        let bot_keys = Keys::generate();
        let req = build_create_vm_request(
            &keys.public_key(),
            &["wss://relay-a.example.com".to_string()],
            &bot_keys.secret_key().to_secret_hex(),
            &bot_keys.public_key().to_hex(),
            &ResolvedMicrovmParams {
                spawner_url: DEFAULT_SPAWNER_URL.to_string(),
                kind: ResolvedMicrovmAgentKind::Openclaw,
                backend: ResolvedMicrovmAgentBackend::Native,
            },
        );
        let value = serde_json::to_value(req).expect("serialize create vm request");
        assert_eq!(
            value["guest_autostart"]["env"]["PIKA_AGENT_KIND"],
            "openclaw"
        );
        let openclaw_config = value["guest_autostart"]["files"][OPENCLAW_CONFIG_PATH]
            .as_str()
            .expect("openclaw config");
        let openclaw_json: serde_json::Value =
            serde_json::from_str(openclaw_config).expect("parse openclaw config");
        assert_eq!(openclaw_json["gateway"]["mode"], "local");
        assert_eq!(openclaw_json["gateway"]["bind"], "loopback");
        assert_eq!(
            openclaw_json["gateway"]["port"],
            serde_json::Value::Number(DEFAULT_OPENCLAW_GATEWAY_PORT.into())
        );
        assert_eq!(
            openclaw_json["channels"]["pikachat-openclaw"]["daemonBackend"],
            "native"
        );
        assert!(value["guest_autostart"]["files"]
            .as_object()
            .expect("files map")
            .contains_key(&format!("{OPENCLAW_EXTENSION_ROOT}/package.json")));
    }

    #[test]
    fn openclaw_extension_file_list_matches_source_tree() {
        fn collect_relative_files(root: &Path) -> BTreeSet<String> {
            fn visit(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
                for entry in fs::read_dir(dir).expect("read dir") {
                    let entry = entry.expect("dir entry");
                    let path = entry.path();
                    if entry.file_type().expect("file type").is_dir() {
                        visit(root, &path, out);
                    } else {
                        out.insert(
                            path.strip_prefix(root)
                                .expect("strip root")
                                .to_string_lossy()
                                .replace('\\', "/"),
                        );
                    }
                }
            }

            let mut files = BTreeSet::new();
            visit(root, root, &mut files);
            files
        }

        let extension_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw");
        let expected: BTreeSet<String> = expected_openclaw_extension_paths()
            .iter()
            .map(|path| path.to_string())
            .collect();
        let actual = collect_relative_files(&extension_root)
            .into_iter()
            .filter(|path| path != "CHANGELOG.md" && !path.ends_with(".test.ts"))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            actual, expected,
            "openclaw extension file list changed; update openclaw_extension_files()"
        );
    }

    #[tokio::test]
    async fn create_vm_contract_request_shape() {
        let (base_url, rx) =
            spawn_one_shot_server("200 OK", r#"{"id":"vm-123","status":"starting"}"#);
        let client = MicrovmSpawnerClient::new(base_url);
        let req = CreateVmRequest {
            guest_autostart: GuestAutostartRequest {
                command: "/workspace/pika-agent/start-agent.sh".to_string(),
                env: BTreeMap::from([("PIKA_OWNER_PUBKEY".to_string(), "pubkey123".to_string())]),
                files: BTreeMap::new(),
            },
        };

        let vm = client
            .create_vm_with_request_id(&req, Some("req-create-123"))
            .await
            .expect("create vm succeeds");
        assert_eq!(vm.id, "vm-123");
        assert_eq!(vm.status, "starting");

        let captured = rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("captured request");
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.path, "/vms");
        assert_eq!(
            captured.headers.get(REQUEST_ID_HEADER).map(String::as_str),
            Some("req-create-123")
        );

        let json: serde_json::Value =
            serde_json::from_str(&captured.body).expect("parse json body");
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
            .delete_vm_with_request_id("vm-delete-1", Some("req-delete-123"))
            .await
            .expect("delete vm succeeds");

        let captured = rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("captured request");
        assert_eq!(captured.method, "DELETE");
        assert_eq!(captured.path, "/vms/vm-delete-1");
        assert_eq!(
            captured.headers.get(REQUEST_ID_HEADER).map(String::as_str),
            Some("req-delete-123")
        );
        assert!(captured.body.is_empty());
    }

    #[tokio::test]
    async fn recover_vm_contract_request_shape() {
        let (base_url, rx) =
            spawn_one_shot_server("200 OK", r#"{"id":"vm-recover-1","status":"running"}"#);
        let client = MicrovmSpawnerClient::new(base_url);

        let recovered = client
            .recover_vm_with_request_id("vm-recover-1", Some("req-recover-123"))
            .await
            .expect("recover vm succeeds");
        assert_eq!(recovered.id, "vm-recover-1");
        assert_eq!(recovered.status, "running");

        let captured = rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("captured request");
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.path, "/vms/vm-recover-1/recover");
        assert_eq!(
            captured.headers.get(REQUEST_ID_HEADER).map(String::as_str),
            Some("req-recover-123")
        );
        assert!(captured.body.is_empty());
    }

    #[tokio::test]
    async fn get_vm_contract_response_carries_guest_ready() {
        let (base_url, rx) = spawn_one_shot_server(
            "200 OK",
            r#"{"id":"vm-123","status":"running","guest_ready":true}"#,
        );
        let client = MicrovmSpawnerClient::new(base_url);

        let vm = client
            .get_vm_with_request_id("vm-123", Some("req-get-123"))
            .await
            .expect("get vm succeeds");
        assert_eq!(vm.id, "vm-123");
        assert_eq!(vm.status, "running");
        assert!(vm.guest_ready);

        let captured = rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("captured request");
        assert_eq!(captured.method, "GET");
        assert_eq!(captured.path, "/vms/vm-123");
        assert_eq!(
            captured.headers.get(REQUEST_ID_HEADER).map(String::as_str),
            Some("req-get-123")
        );
        assert!(captured.body.is_empty());
    }

    #[tokio::test]
    async fn create_vm_surfaces_error_body() {
        let (base_url, _rx) = spawn_one_shot_server("503 Service Unavailable", "spawner down");
        let client = MicrovmSpawnerClient::new(base_url);
        let req = CreateVmRequest {
            guest_autostart: GuestAutostartRequest {
                command: "/workspace/pika-agent/start-agent.sh".to_string(),
                env: BTreeMap::new(),
                files: BTreeMap::new(),
            },
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

    #[tokio::test]
    async fn delete_vm_surfaces_error_body() {
        let (base_url, _rx) =
            spawn_one_shot_server("500 Internal Server Error", "vm stuck in cleanup");
        let client = MicrovmSpawnerClient::new(base_url);

        let err = client
            .delete_vm("vm-stuck")
            .await
            .expect_err("expected delete_vm failure");
        let msg = err.to_string();
        assert!(msg.contains("failed to delete vm vm-stuck"));
        assert!(msg.contains("500 Internal Server Error"));
        assert!(msg.contains("vm stuck in cleanup"));
    }

    #[tokio::test]
    async fn recover_vm_surfaces_error_body() {
        let (base_url, _rx) = spawn_one_shot_server("503 Service Unavailable", "vm reboot failed");
        let client = MicrovmSpawnerClient::new(base_url);

        let err = client
            .recover_vm("vm-bad")
            .await
            .expect_err("expected recover_vm failure");
        let msg = err.to_string();
        assert!(msg.contains("failed to recover vm vm-bad"));
        assert!(msg.contains("503 Service Unavailable"));
        assert!(msg.contains("vm reboot failed"));
    }

    #[test]
    fn resolve_params_trims_whitespace_and_ignores_empty() {
        let params = MicrovmProvisionParams {
            spawner_url: Some("  ".to_string()),
            kind: None,
            backend: None,
        };
        let resolved = resolve_params(&params);
        assert_eq!(resolved.spawner_url, DEFAULT_SPAWNER_URL);
    }

    #[test]
    fn spawner_client_strips_trailing_slashes() {
        let client = MicrovmSpawnerClient::new("http://localhost:8080///");
        assert_eq!(client.base_url(), "http://localhost:8080");
    }
}
