use std::collections::BTreeMap;

use nostr_sdk::prelude::PublicKey;
use pika_cloud::{
    GuestOpenclawDaemonBackend, GuestServiceBackendMode, GuestServiceKind, GuestServiceLaunch,
    GuestServiceReadinessCheck, GuestStartupArtifacts, GuestStartupPlan, ManagedVmCreateRequest,
    ManagedVmGuestAutostartRequest, GUEST_AUTOSTART_COMMAND, GUEST_AUTOSTART_IDENTITY_PATH,
    GUEST_AUTOSTART_SCRIPT_PATH, GUEST_OPENCLAW_CONFIG_PATH, GUEST_OPENCLAW_EXTENSION_ROOT,
    GUEST_STARTUP_PLAN_PATH,
};
use serde_json::json;

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

pub fn build_managed_vm_create_request(input: ManagedVmCreateInput<'_>) -> ManagedVmCreateRequest {
    let startup_plan = guest_startup_plan();
    let mut env = BTreeMap::new();
    env.insert("PIKA_OWNER_PUBKEY".to_string(), input.owner_pubkey.to_hex());
    env.insert("PIKA_RELAY_URLS".to_string(), input.relay_urls.join(","));
    env.insert(
        "PIKA_BOT_PUBKEY".to_string(),
        input.bot_pubkey_hex.to_string(),
    );
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

    ManagedVmCreateRequest {
        guest_autostart: ManagedVmGuestAutostartRequest {
            command: GUEST_AUTOSTART_COMMAND.to_string(),
            env,
            files,
            startup_plan,
        },
    }
}

fn guest_startup_plan() -> GuestStartupPlan {
    GuestStartupPlan {
        service_kind: GuestServiceKind::OpenclawGateway,
        backend_mode: GuestServiceBackendMode::Native,
        daemon_state_dir: DEFAULT_DAEMON_STATE_DIR.to_string(),
        service: GuestServiceLaunch::OpenclawGateway {
            exec_command: resolved_openclaw_exec_command(),
            state_dir: DEFAULT_OPENCLAW_STATE_DIR.to_string(),
            config_path: GUEST_OPENCLAW_CONFIG_PATH.to_string(),
            gateway_port: DEFAULT_OPENCLAW_GATEWAY_PORT,
            daemon_backend: GuestOpenclawDaemonBackend::Native,
        },
        readiness_check: GuestServiceReadinessCheck::HttpGetOk {
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

fn startup_plan_file(startup_plan: &GuestStartupPlan) -> String {
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

workspace_path() {{
  printf '/%s' "$1"
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

write_ready_marker() {{
  local probe="$1"
  local ready_path="$(workspace_path "$(plan_value '.artifacts.ready_marker_path')")"
  local failed_path="$(workspace_path "$(plan_value '.artifacts.failed_marker_path')")"
  local boot_id=""
  if [[ -r /proc/sys/kernel/random/boot_id ]]; then
    boot_id="$(tr -d '\n' < /proc/sys/kernel/random/boot_id)"
  fi
  cat >"$ready_path" <<EOF
{{
  "ready": true,
  "agent_kind": "openclaw",
  "backend_mode": "native",
  "service_kind": "openclaw_gateway",
  "probe": "${{probe}}",
  "boot_id": "${{boot_id}}"
}}
EOF
  rm -f "$failed_path"
}}

write_failed_marker() {{
  local reason="$1"
  local ready_path="$(workspace_path "$(plan_value '.artifacts.ready_marker_path')")"
  local failed_path="$(workspace_path "$(plan_value '.artifacts.failed_marker_path')")"
  cat >"$failed_path" <<EOF
{{
  "ready": false,
  "agent_kind": "openclaw",
  "backend_mode": "native",
  "service_kind": "openclaw_gateway",
  "reason": "${{reason}}"
}}
EOF
  rm -f "$ready_path"
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
ready_path="$(workspace_path "$(plan_value '.artifacts.ready_marker_path')")"
failed_path="$(workspace_path "$(plan_value '.artifacts.failed_marker_path')")"
identity_seed_path="$(workspace_path "$(plan_value '.artifacts.identity_seed_path')")"
mkdir -p "$daemon_state_dir"
if [[ -f "$identity_seed_path" && ! -f "$daemon_state_dir/identity.json" ]]; then
  cp "$identity_seed_path" "$daemon_state_dir/identity.json"
fi
rm -f "$ready_path" "$failed_path"

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
  local ready_probe="$(plan_value '.readiness_check.ready_probe')"
  local readiness_url="$(plan_value '.readiness_check.url')"
  local timeout_failure_reason="$(plan_value '.readiness_check.timeout_failure_reason')"

  while (( SECONDS < deadline )); do
    if ! kill -0 "$service_pid" 2>/dev/null; then
      wait "$service_pid"
      return $?
    fi
    if curl -fsS --max-time 2 "$readiness_url" >/dev/null 2>&1; then
      printf '%s\n' "$ready_probe"
      return 0
    fi
    sleep 1
  done

  write_failed_marker "$timeout_failure_reason"
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
      wait "$service_pid"
      return $?
    fi
    if publish_daemon_keypackage; then
      write_ready_marker "$ready_probe"
      return 0
    fi
    sleep 1
  done

  write_failed_marker "timeout_waiting_for_openclaw_keypackage_publish"
  kill "$service_pid" 2>/dev/null || true
  wait "$service_pid" || true
  return 1
}}

openclaw_exec="$(plan_value '.service.exec_command')"
openclaw_state_dir="$(plan_value '.service.state_dir')"
openclaw_config_path="$(workspace_path "$(plan_value '.service.config_path')")"
openclaw_workspace_root="$(dirname "$openclaw_config_path")"
openclaw_package_root="${{PIKA_OPENCLAW_PACKAGE_ROOT:-$(dirname "$(dirname "$openclaw_exec")")/lib/openclaw}}"
gateway_port="$(plan_value '.service.gateway_port | tostring')"

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

if ! ready_probe="$(wait_for_service_ready "$agent_pid")"; then
  exit 1
fi
if ! wait_for_keypackage_publish "$agent_pid" "$ready_probe"; then
  exit 1
fi
wait "$agent_pid"
status=$?
rm -f "$ready_path"
if [[ $status -ne 0 ]]; then
  write_failed_marker "openclaw_gateway_exited"
else
  rm -f "$failed_path"
fi
exit $status
"#,
        startup_plan_path = GUEST_STARTUP_PLAN_PATH,
    )
}

fn openclaw_gateway_config(relay_urls: &[String], startup_plan: &GuestStartupPlan) -> String {
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
