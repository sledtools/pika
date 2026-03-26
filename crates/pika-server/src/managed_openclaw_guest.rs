use std::collections::BTreeMap;

use nostr_sdk::prelude::PublicKey;
use pika_cloud::RuntimeArtifactPaths;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub(crate) const GUEST_AUTOSTART_COMMAND: &str = "bash /workspace/pika-agent/start-agent.sh";
pub(crate) const GUEST_AUTOSTART_SCRIPT_PATH: &str = "workspace/pika-agent/start-agent.sh";
pub(crate) const GUEST_STARTUP_PLAN_PATH: &str = "workspace/pika-agent/startup-plan.json";
pub(crate) const GUEST_AUTOSTART_IDENTITY_PATH: &str = "workspace/pika-agent/state/identity.json";
pub(crate) const GUEST_LOG_PATH: &str = "workspace/pika-agent/agent.log";
pub(crate) const GUEST_PID_PATH: &str = "workspace/pika-agent/agent.pid";
pub(crate) const GUEST_OPENCLAW_CONFIG_PATH: &str = "workspace/pika-agent/openclaw/openclaw.json";
pub(crate) const GUEST_OPENCLAW_EXTENSION_ROOT: &str =
    "workspace/pika-agent/openclaw/extensions/pikachat-openclaw";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct OpenclawStartupPlan {
    daemon_state_dir: String,
    openclaw: OpenclawLaunchPlan,
    readiness: OpenclawReadinessPlan,
    #[serde(default)]
    artifacts: GuestStartupArtifacts,
    exit_failure_reason: String,
}

impl OpenclawStartupPlan {
    fn validate(&self) -> Result<(), String> {
        if self.openclaw.config_path != GUEST_OPENCLAW_CONFIG_PATH {
            return Err(format!(
                "openclaw startup plan config_path must use canonical path {:?}, got {:?}",
                GUEST_OPENCLAW_CONFIG_PATH, self.openclaw.config_path
            ));
        }
        if self.openclaw.gateway_port != DEFAULT_OPENCLAW_GATEWAY_PORT {
            return Err(format!(
                "openclaw startup plan gateway_port must stay pinned to {:?}, got {:?}",
                DEFAULT_OPENCLAW_GATEWAY_PORT, self.openclaw.gateway_port
            ));
        }
        self.artifacts.validate_canonical_paths()?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct OpenclawLaunchPlan {
    exec_command: String,
    state_dir: String,
    config_path: String,
    gateway_port: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct OpenclawReadinessPlan {
    url: String,
    ready_probe: String,
    timeout_failure_reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct GuestStartupArtifacts {
    startup_plan_path: String,
    identity_seed_path: String,
    #[serde(flatten)]
    lifecycle_artifacts: RuntimeArtifactPaths,
    #[serde(flatten)]
    service_artifacts: ManagedGuestServiceArtifacts,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ManagedGuestServiceArtifacts {
    #[serde(rename = "log_path")]
    service_log_path: String,
    pid_path: String,
}

impl Default for ManagedGuestServiceArtifacts {
    fn default() -> Self {
        Self {
            service_log_path: GUEST_LOG_PATH.to_string(),
            pid_path: GUEST_PID_PATH.to_string(),
        }
    }
}

impl ManagedGuestServiceArtifacts {
    fn validate_canonical_paths(&self, field_prefix: &str) -> Result<(), String> {
        let canonical = Self::default();
        for (field, actual, expected) in [
            (
                "log_path",
                self.service_log_path.as_str(),
                canonical.service_log_path.as_str(),
            ),
            (
                "pid_path",
                self.pid_path.as_str(),
                canonical.pid_path.as_str(),
            ),
        ] {
            if actual != expected {
                return Err(format!(
                    "{field_prefix}.{field} must use canonical path {expected:?}, got {actual:?}"
                ));
            }
        }
        Ok(())
    }
}

impl Default for GuestStartupArtifacts {
    fn default() -> Self {
        Self {
            startup_plan_path: GUEST_STARTUP_PLAN_PATH.to_string(),
            identity_seed_path: GUEST_AUTOSTART_IDENTITY_PATH.to_string(),
            lifecycle_artifacts: RuntimeArtifactPaths::default(),
            service_artifacts: ManagedGuestServiceArtifacts::default(),
        }
    }
}

impl GuestStartupArtifacts {
    fn validate_canonical_paths(&self) -> Result<(), String> {
        let canonical = Self::default();
        for (field, actual, expected) in [
            (
                "startup_plan_path",
                self.startup_plan_path.as_str(),
                canonical.startup_plan_path.as_str(),
            ),
            (
                "identity_seed_path",
                self.identity_seed_path.as_str(),
                canonical.identity_seed_path.as_str(),
            ),
        ] {
            if actual != expected {
                return Err(format!(
                    "guest startup plan artifacts.{field} must use canonical path {expected:?}, got {actual:?}"
                ));
            }
        }
        self.lifecycle_artifacts
            .validate_canonical_paths("artifacts")?;
        self.service_artifacts
            .validate_canonical_paths("artifacts")?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ManagedGuestAutostart {
    pub command: String,
    pub env: BTreeMap<String, String>,
    pub files: BTreeMap<String, String>,
}

pub const DEFAULT_OPENCLAW_EXEC_COMMAND: &str = "/opt/runtime-artifacts/openclaw/bin/openclaw";
pub const DEFAULT_OPENCLAW_GATEWAY_PORT: u16 = 18789;
const DEFAULT_DAEMON_STATE_DIR: &str = "/root/pika-agent/state";
const DEFAULT_OPENCLAW_STATE_DIR: &str = "/root/pika-agent/openclaw";
const DEFAULT_OPENCLAW_CONTROL_UI_ORIGIN: &str = "http://openclaw.localhost:19401";
const DEFAULT_OPENCLAW_CONTROL_UI_HTTPS_ORIGIN: &str = "https://openclaw.localhost:19401";
const DEFAULT_OPENCLAW_TRUSTED_PROXIES: &[&str] = &["127.0.0.1", "::1"];

#[derive(Debug, Clone, Copy)]
pub struct ManagedVmCreateInput<'a> {
    pub owner_pubkey: &'a PublicKey,
    pub relay_urls: &'a [String],
    pub bot_secret_hex: &'a str,
    pub bot_pubkey_hex: &'a str,
}

pub fn build_managed_guest_autostart(input: ManagedVmCreateInput<'_>) -> ManagedGuestAutostart {
    let startup_plan = openclaw_startup_plan();
    let mut env = BTreeMap::new();
    env.insert("PIKA_OWNER_PUBKEY".to_string(), input.owner_pubkey.to_hex());
    env.insert("PIKA_RELAY_URLS".to_string(), input.relay_urls.join(","));
    env.insert(
        "PIKA_BOT_PUBKEY".to_string(),
        input.bot_pubkey_hex.to_string(),
    );
    for key in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"] {
        if let Ok(value) = std::env::var(key) {
            if value.trim().is_empty() {
                continue;
            }
            env.insert(key.to_string(), value);
        }
    }

    let mut files = BTreeMap::new();
    files.insert(
        GUEST_AUTOSTART_SCRIPT_PATH.to_string(),
        guest_autostart_script(),
    );
    files.insert(
        GUEST_STARTUP_PLAN_PATH.to_string(),
        startup_plan_file(&startup_plan),
    );
    files.insert(
        GUEST_AUTOSTART_IDENTITY_PATH.to_string(),
        bot_identity_file(input.bot_secret_hex, input.bot_pubkey_hex),
    );
    files.insert(
        GUEST_OPENCLAW_CONFIG_PATH.to_string(),
        openclaw_gateway_config(input.relay_urls, &startup_plan),
    );
    files.extend(openclaw_extension_files());

    ManagedGuestAutostart {
        command: GUEST_AUTOSTART_COMMAND.to_string(),
        env,
        files,
    }
}

fn openclaw_startup_plan() -> OpenclawStartupPlan {
    OpenclawStartupPlan {
        daemon_state_dir: DEFAULT_DAEMON_STATE_DIR.to_string(),
        openclaw: OpenclawLaunchPlan {
            exec_command: resolved_openclaw_exec_command(),
            state_dir: DEFAULT_OPENCLAW_STATE_DIR.to_string(),
            config_path: GUEST_OPENCLAW_CONFIG_PATH.to_string(),
            gateway_port: DEFAULT_OPENCLAW_GATEWAY_PORT,
        },
        readiness: OpenclawReadinessPlan {
            url: format!("http://127.0.0.1:{DEFAULT_OPENCLAW_GATEWAY_PORT}/health"),
            ready_probe: "openclaw_gateway_health".to_string(),
            timeout_failure_reason: "timeout_waiting_for_openclaw_health".to_string(),
        },
        artifacts: GuestStartupArtifacts::default(),
        exit_failure_reason: "openclaw_gateway_exited".to_string(),
    }
}

fn resolved_openclaw_exec_command() -> String {
    std::env::var("PIKA_OPENCLAW_EXEC")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_OPENCLAW_EXEC_COMMAND.to_string())
}

fn startup_plan_file(startup_plan: &OpenclawStartupPlan) -> String {
    startup_plan
        .validate()
        .expect("startup plan must be internally consistent");
    let body = serde_json::to_string_pretty(startup_plan).expect("serialize startup plan");
    format!("{body}\n")
}

fn bot_identity_file(secret_hex: &str, pubkey_hex: &str) -> String {
    let body = serde_json::to_string_pretty(&json!({
        "secret_key_hex": secret_hex,
        "public_key_hex": pubkey_hex,
    }))
    .expect("identity json");
    format!("{body}\n")
}

fn guest_autostart_script() -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

STARTUP_PLAN_PATH="/{startup_plan_path}"
LIFECYCLE_HELPER="/run/current-system/sw/bin/pika-cloud-lifecycle"
service_exit_status=""
agent_pid=""
gateway_proxy_pid=""

cleanup_agent() {{
  if [[ -n "${{gateway_proxy_pid:-}}" ]]; then
    kill "$gateway_proxy_pid" 2>/dev/null || true
    wait "$gateway_proxy_pid" 2>/dev/null || true
  fi
  if [[ -n "${{agent_pid:-}}" ]]; then
    kill "$agent_pid" 2>/dev/null || true
    wait "$agent_pid" 2>/dev/null || true
  fi
}}

plan_value() {{
  jq -er "$1" "$STARTUP_PLAN_PATH"
}}

plan_path() {{
  local raw="$1"
  if [[ "$raw" == /* ]]; then
    printf '%s' "$raw"
  else
    printf '/%s' "$raw"
  fi
}}

runtime_status_path() {{
  plan_path "$(plan_value '.artifacts.status_path')"
}}

runtime_events_path() {{
  plan_path "$(plan_value '.artifacts.events_path')"
}}

runtime_result_path() {{
  plan_path "$(plan_value '.artifacts.result_path')"
}}

lifecycle_details_json() {{
  local probe="${{1:-}}"
  local service_probe_satisfied="${{2:-false}}"
  local failure_reason="${{3:-}}"
  jq -nc \
    --arg agent_kind "openclaw" \
    --arg backend_mode "native" \
    --arg service_kind "openclaw_gateway" \
    --arg probe "$probe" \
    --arg failure_reason "$failure_reason" \
    --argjson service_probe_satisfied "$service_probe_satisfied" \
    '
      {{
        agent_kind: $agent_kind,
        backend_mode: $backend_mode,
        service_kind: $service_kind,
        service_probe_satisfied: $service_probe_satisfied
      }}
      + (if $probe == "" then {{}} else {{ probe: $probe }} end)
      + (if $failure_reason == "" then {{}} else {{ failure_reason: $failure_reason }} end)
    '
}}

write_status() {{
  local state="$1"
  local message="$2"
  local details_json="${{3:-null}}"
  local -a args=(
    status
    --status-path "$(runtime_status_path)"
    --state "$state"
    --message "$message"
  )
  if [[ "$details_json" != "null" ]]; then
    args+=(--details-json "$details_json")
  fi
  "$LIFECYCLE_HELPER" "${{args[@]}}"
}}

append_event() {{
  local kind="$1"
  local message="$2"
  local details_json="${{3:-null}}"
  local -a args=(
    event
    --events-path "$(runtime_events_path)"
    --kind "$kind"
    --message "$message"
  )
  if [[ "$details_json" != "null" ]]; then
    args+=(--details-json "$details_json")
  fi
  "$LIFECYCLE_HELPER" "${{args[@]}}"
}}

write_result() {{
  local status="$1"
  local exit_code="$2"
  local message="$3"
  local details_json="${{4:-null}}"
  local -a args=(
    result
    --result-path "$(runtime_result_path)"
    --status "$status"
    --exit-code "$exit_code"
    --message "$message"
  )
  if [[ "$details_json" != "null" ]]; then
    args+=(--details-json "$details_json")
  fi
  "$LIFECYCLE_HELPER" "${{args[@]}}"
}}

mark_runtime_booted() {{
  local details_json
  details_json="$(lifecycle_details_json "" false "")"
  write_status "booted" "managed OpenClaw guest booted" "$details_json"
  append_event "booted" "managed OpenClaw guest booted" "$details_json"
}}

mark_waiting_for_service_ready() {{
  local details_json
  details_json="$(lifecycle_details_json "" false "")"
  write_status "starting" "waiting for OpenClaw health" "$details_json"
  append_event "starting" "waiting for OpenClaw health" "$details_json"
}}

mark_service_probe_ready() {{
  local probe="$1"
  local details_json
  details_json="$(lifecycle_details_json "$probe" true "")"
  write_status "starting" "OpenClaw health ready; publishing keypackage" "$details_json"
  append_event "starting" "OpenClaw health ready; publishing keypackage" "$details_json"
}}

mark_guest_ready() {{
  local probe="$1"
  local details_json
  details_json="$(lifecycle_details_json "$probe" true "")"
  write_status "ready" "managed OpenClaw guest ready" "$details_json"
  append_event "ready" "managed OpenClaw guest ready" "$details_json"
}}

mark_guest_completed() {{
  local message="$1"
  local exit_code="${{2:-0}}"
  local service_probe_satisfied="${{3:-false}}"
  local probe="${{4:-}}"
  local details_json
  details_json="$(lifecycle_details_json "$probe" "$service_probe_satisfied" "")"
  write_status "completed" "$message" "$details_json"
  append_event "completed" "$message" "$details_json"
  write_result "completed" "$exit_code" "$message" "$details_json"
}}

mark_guest_failed() {{
  local reason="$1"
  local exit_code="${{2:-1}}"
  local service_probe_satisfied="${{3:-false}}"
  local probe="${{4:-}}"
  local details_json
  details_json="$(lifecycle_details_json "$probe" "$service_probe_satisfied" "$reason")"
  write_status "failed" "$reason" "$details_json"
  append_event "failed" "$reason" "$details_json"
  write_result "failed" "$exit_code" "$reason" "$details_json"
}}

capture_service_exit_status() {{
  local service_pid="$1"
  if wait "$service_pid"; then
    service_exit_status=0
  else
    service_exit_status=$?
  fi
}}

start_openclaw_private_proxy() {{
  local listen_host="$1"
  local listen_port="$2"
  python3 - "$listen_host" "$listen_port" <<'PY' &
import asyncio
import sys

listen_host = sys.argv[1]
listen_port = int(sys.argv[2])
target_host = "127.0.0.1"
target_port = listen_port

async def pump(reader, writer):
    try:
        upstream_reader, upstream_writer = await asyncio.open_connection(
            target_host, target_port
        )
    except Exception:
        writer.close()
        await writer.wait_closed()
        return

    async def forward(src, dst):
        try:
            while True:
                chunk = await src.read(65536)
                if not chunk:
                    break
                dst.write(chunk)
                await dst.drain()
        except Exception:
            pass
        finally:
            dst.close()
            try:
                await dst.wait_closed()
            except Exception:
                pass

    await asyncio.gather(
        forward(reader, upstream_writer),
        forward(upstream_reader, writer),
    )

async def main():
    server = await asyncio.start_server(pump, listen_host, listen_port)
    async with server:
        await server.serve_forever()

asyncio.run(main())
PY
  gateway_proxy_pid=$!
  sleep 1
  if ! kill -0 "$gateway_proxy_pid" 2>/dev/null; then
    echo "[managed-openclaw] failed to start private gateway proxy on $listen_host:$listen_port" >&2
    exit 1
  fi
}}

trap cleanup_agent EXIT TERM INT

if ! command -v jq >/dev/null 2>&1; then
  echo "[managed-openclaw] missing jq in guest image" >&2
  exit 1
fi

if [[ ! -f "$STARTUP_PLAN_PATH" ]]; then
  echo "[managed-openclaw] missing startup plan at $STARTUP_PLAN_PATH" >&2
  exit 1
fi

daemon_state_dir="$(plan_value '.daemon_state_dir')"
identity_seed_path="$(plan_path "$(plan_value '.artifacts.identity_seed_path')")"
mkdir -p "$daemon_state_dir"
if [[ -f "$identity_seed_path" && ! -f "$daemon_state_dir/identity.json" ]]; then
  cp "$identity_seed_path" "$daemon_state_dir/identity.json"
fi
rm -f "$(runtime_result_path)"
mark_runtime_booted
mark_waiting_for_service_ready

if [[ -z "${{PIKA_OWNER_PUBKEY:-}}" ]]; then
  echo "[managed-openclaw] missing PIKA_OWNER_PUBKEY" >&2
  exit 1
fi
if [[ -z "${{PIKA_RELAY_URLS:-}}" ]]; then
  echo "[managed-openclaw] missing PIKA_RELAY_URLS" >&2
  exit 1
fi

relay_args=()
IFS=',' read -r -a relay_values <<< "${{PIKA_RELAY_URLS}}"
for relay in "${{relay_values[@]}}"; do
  relay="${{relay#"${{relay%%[![:space:]]*}}"}}"
  relay="${{relay%"${{relay##*[![:space:]]}}"}}"
  if [[ -n "$relay" ]]; then
    relay_args+=(--relay "$relay")
  fi
done
if [[ ${{#relay_args[@]}} -eq 0 ]]; then
  echo "[managed-openclaw] no valid relays in PIKA_RELAY_URLS" >&2
  exit 1
fi

bin=""
if [[ -n "${{PIKA_PIKACHAT_BIN:-}}" && -x "${{PIKA_PIKACHAT_BIN}}" ]]; then
  bin="${{PIKA_PIKACHAT_BIN}}"
elif command -v pikachat >/dev/null 2>&1; then
  bin="pikachat"
fi
if [[ -z "$bin" ]]; then
  echo "[managed-openclaw] could not find pikachat binary" >&2
  exit 1
fi
if [[ "$bin" != "pikachat" ]]; then
  export PATH="$(dirname "$bin"):$PATH"
fi

publish_daemon_keypackage() {{
  local publish_args=(--remote --state-dir "$daemon_state_dir")
  publish_args+=("${{relay_args[@]}}")
  "$bin" "${{publish_args[@]}}" publish-kp >/dev/null
}}

wait_for_service_ready() {{
  local service_pid="$1"
  local timeout_sec="${{PIKA_AGENT_READY_TIMEOUT_SECS:-120}}"
  local deadline=$((SECONDS + timeout_sec))
  local ready_probe="$(plan_value '.readiness.ready_probe')"
  local readiness_url="$(plan_value '.readiness.url')"
  local timeout_failure_reason="$(plan_value '.readiness.timeout_failure_reason')"

  while (( SECONDS < deadline )); do
    if ! kill -0 "$service_pid" 2>/dev/null; then
      capture_service_exit_status "$service_pid"
      return 2
    fi
    if curl -fsS --max-time 2 "$readiness_url" >/dev/null 2>&1; then
      printf '%s\n' "$ready_probe"
      return 0
    fi
    sleep 1
  done

  kill "$service_pid" 2>/dev/null || true
  wait "$service_pid" || true
  return 1
}}

wait_for_keypackage_publish() {{
  local service_pid="$1"
  local ready_probe="$2"
  local timeout_sec="${{PIKA_AGENT_READY_TIMEOUT_SECS:-120}}"
  local deadline=$((SECONDS + timeout_sec))

  while (( SECONDS < deadline )); do
    if ! kill -0 "$service_pid" 2>/dev/null; then
      capture_service_exit_status "$service_pid"
      return 2
    fi
    if publish_daemon_keypackage; then
      return 0
    fi
    sleep 1
  done

  kill "$service_pid" 2>/dev/null || true
  wait "$service_pid" || true
  return 1
}}

openclaw_exec="$(plan_value '.openclaw.exec_command')"
openclaw_state_dir="$(plan_value '.openclaw.state_dir')"
openclaw_config_path="$(plan_path "$(plan_value '.openclaw.config_path')")"
openclaw_workspace_root="$(dirname "$openclaw_config_path")"
openclaw_package_root="${{PIKA_OPENCLAW_PACKAGE_ROOT:-$(dirname "$(dirname "$openclaw_exec")")/lib/openclaw}}"
gateway_port="$(plan_value '.openclaw.gateway_port | tostring')"

mkdir -p "$openclaw_state_dir"
mkdir -p "$openclaw_state_dir/node_modules"
rm -rf "$openclaw_state_dir/node_modules/openclaw"
ln -s "$openclaw_package_root" "$openclaw_state_dir/node_modules/openclaw"
mkdir -p "$openclaw_workspace_root/node_modules"
rm -rf "$openclaw_workspace_root/node_modules/openclaw"
ln -s "$openclaw_package_root" "$openclaw_workspace_root/node_modules/openclaw"

export OPENCLAW_STATE_DIR="$openclaw_state_dir"
export OPENCLAW_CONFIG_PATH="$openclaw_config_path"
export NODE_PATH="$openclaw_state_dir/node_modules${{NODE_PATH:+:$NODE_PATH}}"
export PIKACHAT_DAEMON_CMD="$bin"
export PIKACHAT_SIDECAR_CMD="$bin"
export OPENCLAW_SKIP_BROWSER_CONTROL_SERVER=1
export OPENCLAW_SKIP_GMAIL_WATCHER=1
export OPENCLAW_SKIP_CANVAS_HOST=1
export OPENCLAW_SKIP_CRON=1
export PIKA_OPENCLAW_GATEWAY_PORT="$gateway_port"
: "${{PIKA_VM_IP:?missing PIKA_VM_IP}}"

"$openclaw_exec" gateway --allow-unconfigured &
agent_pid=$!
if [[ "${{PIKA_ENABLE_OPENCLAW_PRIVATE_PROXY:-1}}" != "0" ]]; then
  start_openclaw_private_proxy "$PIKA_VM_IP" "$gateway_port"
fi

if ready_probe="$(wait_for_service_ready "$agent_pid")"; then
  :
else
  wait_status=$?
  if [[ "$wait_status" -eq 2 ]]; then
    mark_guest_failed "openclaw_gateway_exited_before_service_ready" "${{service_exit_status:-1}}" false
  else
    mark_guest_failed "$(plan_value '.readiness.timeout_failure_reason')" 1 false
  fi
  exit 1
fi
mark_service_probe_ready "$ready_probe"
if wait_for_keypackage_publish "$agent_pid" "$ready_probe"; then
  :
else
  wait_status=$?
  if [[ "$wait_status" -eq 2 ]]; then
    mark_guest_failed "openclaw_gateway_exited_before_keypackage_publish" "${{service_exit_status:-1}}" true "$ready_probe"
  else
    mark_guest_failed "timeout_waiting_for_openclaw_keypackage_publish" 1 true "$ready_probe"
  fi
  exit 1
fi
mark_guest_ready "$ready_probe"
wait "$agent_pid"
status=$?
if [[ "$status" -eq 0 ]]; then
  mark_guest_completed "openclaw_gateway_exited" "$status" true "$ready_probe"
else
  mark_guest_failed "openclaw_gateway_exited" "$status" true "$ready_probe"
fi
exit $status
"#,
        startup_plan_path = GUEST_STARTUP_PLAN_PATH,
    )
}

fn openclaw_gateway_config(relay_urls: &[String], startup_plan: &OpenclawStartupPlan) -> String {
    startup_plan
        .validate()
        .expect("openclaw startup plan must be internally consistent");
    let entry_config = json!({
        "relays": relay_urls,
        "stateDir": startup_plan.daemon_state_dir,
        "autoAcceptWelcomes": true,
        "groupPolicy": "open",
        "daemonCmd": "pikachat",
        "daemonBackend": "native",
    });
    let mut gateway_config = json!({
        "mode": "local",
        "bind": "loopback",
        "port": DEFAULT_OPENCLAW_GATEWAY_PORT,
    });
    gateway_config
        .as_object_mut()
        .expect("openclaw gateway config object")
        .extend(
            managed_openclaw_gateway_security_config()
                .as_object()
                .expect("managed OpenClaw security config object")
                .clone(),
        );
    serde_json::to_string_pretty(&json!({
        "gateway": gateway_config,
        "plugins": {
            "enabled": true,
            "allow": ["pikachat-openclaw"],
            "load": {
                "paths": [format!("/{}", GUEST_OPENCLAW_EXTENSION_ROOT)]
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
            "pikachat-openclaw": entry_config
        }
    }))
    .expect("serialize openclaw config")
}

fn managed_openclaw_gateway_security_config() -> serde_json::Value {
    json!({
        "controlUi": {
            "allowInsecureAuth": true,
            "dangerouslyDisableDeviceAuth": true,
            "allowedOrigins": openclaw_control_ui_allowed_origins(),
        },
        "trustedProxies": DEFAULT_OPENCLAW_TRUSTED_PROXIES,
    })
}

fn openclaw_control_ui_allowed_origins() -> Vec<String> {
    let mut origins = vec![
        DEFAULT_OPENCLAW_CONTROL_UI_ORIGIN.to_string(),
        DEFAULT_OPENCLAW_CONTROL_UI_HTTPS_ORIGIN.to_string(),
    ];
    if let Ok(raw) = std::env::var("PIKA_OPENCLAW_CONTROL_UI_ALLOWED_ORIGINS") {
        for origin in raw
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
        {
            if !origins.iter().any(|existing| existing == origin) {
                origins.push(origin.to_string());
            }
        }
    }
    origins
}

fn openclaw_extension_files() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/package.json"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/package.json").to_string(),
        ),
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/openclaw.plugin.json"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/openclaw.plugin.json").to_string(),
        ),
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/index.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/index.ts").to_string(),
        ),
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/tsconfig.json"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/tsconfig.json").to_string(),
        ),
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/src/channel-behavior.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/channel-behavior.ts").to_string(),
        ),
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/src/channel.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/channel.ts").to_string(),
        ),
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/src/config-schema.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/config-schema.ts").to_string(),
        ),
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/src/config.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/config.ts").to_string(),
        ),
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/src/daemon-launch.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/daemon-launch.ts").to_string(),
        ),
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/src/daemon-protocol.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/daemon-protocol.ts").to_string(),
        ),
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/src/runtime.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/runtime.ts").to_string(),
        ),
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/src/sidecar-install.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/sidecar-install.ts").to_string(),
        ),
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/src/sidecar.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/sidecar.ts").to_string(),
        ),
        (
            format!("{GUEST_OPENCLAW_EXTENSION_ROOT}/src/types.ts"),
            include_str!("../../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/types.ts").to_string(),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use pika_cloud::{EVENTS_PATH, RESULT_PATH, STATUS_PATH};

    fn assert_contains_all(haystack: &str, needles: &[&str]) {
        for needle in needles {
            assert!(
                haystack.contains(needle),
                "expected generated guest script to contain `{needle}`"
            );
        }
    }

    #[test]
    fn guest_startup_artifacts_default_to_shared_paths() {
        let artifacts = GuestStartupArtifacts::default();

        assert_eq!(artifacts.startup_plan_path, GUEST_STARTUP_PLAN_PATH);
        assert_eq!(artifacts.identity_seed_path, GUEST_AUTOSTART_IDENTITY_PATH);
        assert_eq!(artifacts.lifecycle_artifacts.status_path, STATUS_PATH);
        assert_eq!(artifacts.lifecycle_artifacts.events_path, EVENTS_PATH);
        assert_eq!(artifacts.lifecycle_artifacts.result_path, RESULT_PATH);
        assert_eq!(artifacts.service_artifacts.service_log_path, GUEST_LOG_PATH);
        assert_eq!(artifacts.service_artifacts.pid_path, GUEST_PID_PATH);
    }

    #[test]
    fn guest_startup_plan_uses_shared_lifecycle_artifacts() {
        let plan = openclaw_startup_plan();

        assert_eq!(plan.artifacts.lifecycle_artifacts.status_path, STATUS_PATH);
        assert_eq!(plan.artifacts.lifecycle_artifacts.events_path, EVENTS_PATH);
        assert_eq!(plan.artifacts.lifecycle_artifacts.result_path, RESULT_PATH);
    }

    #[test]
    fn guest_startup_plan_file_round_trips_through_request_payload() {
        let plan = openclaw_startup_plan();
        let request = ManagedGuestAutostart {
            command: GUEST_AUTOSTART_COMMAND.to_string(),
            env: BTreeMap::from([("PIKA_OWNER_PUBKEY".to_string(), "owner".to_string())]),
            files: BTreeMap::from([(
                GUEST_STARTUP_PLAN_PATH.to_string(),
                startup_plan_file(&plan),
            )]),
        };

        let startup_plan = request
            .files
            .get(GUEST_STARTUP_PLAN_PATH)
            .expect("startup plan file present");
        let decoded: OpenclawStartupPlan =
            serde_json::from_str(startup_plan).expect("decode startup plan");

        assert_eq!(decoded, plan);
    }

    #[test]
    fn guest_startup_plan_serializes_openclaw_only_shape() {
        let plan = openclaw_startup_plan();
        let encoded = serde_json::to_value(&plan).expect("encode startup plan");

        assert!(encoded.get("openclaw").is_some());
        assert!(encoded.get("readiness").is_some());
        assert!(encoded.get("service_kind").is_none());
        assert!(encoded.get("backend_mode").is_none());
        assert!(encoded.get("service").is_none());
        assert!(encoded.get("readiness_check").is_none());
    }

    #[test]
    fn guest_startup_plan_validate_rejects_non_canonical_openclaw_config_path() {
        let mut plan = openclaw_startup_plan();
        plan.openclaw.config_path = "workspace/custom/openclaw.json".to_string();

        let err = plan
            .validate()
            .expect_err("plan should reject non-canonical config path");

        assert!(err.contains("config_path"));
        assert!(err.contains(GUEST_OPENCLAW_CONFIG_PATH));
    }

    #[test]
    fn guest_startup_plan_validate_rejects_non_canonical_artifact_paths() {
        let mut plan = openclaw_startup_plan();
        plan.artifacts = GuestStartupArtifacts {
            lifecycle_artifacts: RuntimeArtifactPaths {
                status_path: "/run/custom/status.json".to_string(),
                ..RuntimeArtifactPaths::default()
            },
            ..GuestStartupArtifacts::default()
        };

        let err = plan
            .validate()
            .expect_err("plan should reject non-canonical artifact paths");

        assert!(err.contains("artifacts.status_path"));
        assert!(err.contains(STATUS_PATH));
    }

    #[test]
    fn guest_autostart_script_uses_lifecycle_fields_not_marker_fields() {
        let script = guest_autostart_script();

        assert!(script.contains(".artifacts.status_path"));
        assert!(script.contains(".artifacts.events_path"));
        assert!(script.contains(".artifacts.result_path"));
        assert!(script.contains(".openclaw.exec_command"));
        assert!(script.contains(".openclaw.state_dir"));
        assert!(script.contains(".openclaw.config_path"));
        assert!(script.contains(".readiness.url"));
        assert!(script.contains(".readiness.ready_probe"));
        assert!(script.contains(".readiness.timeout_failure_reason"));
        assert!(!script.contains(".service."));
        assert!(!script.contains(".readiness_check."));
        assert!(!script.contains("ready_marker_path"));
        assert!(!script.contains("failed_marker_path"));
        assert!(!script.contains("service-ready.json"));
        assert!(!script.contains("service-failed.json"));
    }

    #[test]
    fn guest_autostart_script_pins_shared_status_and_event_contract() {
        let script = guest_autostart_script();

        assert_contains_all(
            &script,
            &[
                "write_status() {",
                "LIFECYCLE_HELPER=\"/run/current-system/sw/bin/pika-cloud-lifecycle\"",
                "\"$LIFECYCLE_HELPER\" \"${args[@]}\"",
                "--status-path \"$(runtime_status_path)\"",
                "--state \"$state\"",
                "--message \"$message\"",
                "args+=(--details-json \"$details_json\")",
                "write_status \"booted\" \"managed OpenClaw guest booted\"",
                "write_status \"starting\" \"waiting for OpenClaw health\"",
                "write_status \"starting\" \"OpenClaw health ready; publishing keypackage\"",
                "write_status \"ready\" \"managed OpenClaw guest ready\"",
                "write_status \"completed\" \"$message\" \"$details_json\"",
                "write_status \"failed\" \"$reason\" \"$details_json\"",
                "append_event() {",
                "--events-path \"$(runtime_events_path)\"",
                "--kind \"$kind\"",
                "--message \"$message\"",
                "append_event \"booted\" \"managed OpenClaw guest booted\"",
                "append_event \"starting\" \"waiting for OpenClaw health\"",
                "append_event \"starting\" \"OpenClaw health ready; publishing keypackage\"",
                "append_event \"ready\" \"managed OpenClaw guest ready\"",
                "append_event \"completed\" \"$message\" \"$details_json\"",
                "append_event \"failed\" \"$reason\" \"$details_json\"",
            ],
        );
        assert!(!script.contains("event_seq="));
        assert!(!script.contains("write_json_atomically"));
        assert!(!script.contains("current_boot_id"));
        assert!(!script.contains("\"status\": \"passed\""));
    }

    #[test]
    fn guest_autostart_script_pins_shared_terminal_result_contract() {
        let script = guest_autostart_script();

        assert_contains_all(
            &script,
            &[
                "write_result() {",
                "--result-path \"$(runtime_result_path)\"",
                "--status \"$status\"",
                "--exit-code \"$exit_code\"",
                "--message \"$message\"",
                "args+=(--details-json \"$details_json\")",
                "write_result \"completed\" \"$exit_code\" \"$message\" \"$details_json\"",
                "write_result \"failed\" \"$exit_code\" \"$reason\" \"$details_json\"",
            ],
        );
        assert!(!script.contains("write_result \"passed\""));
    }
}
