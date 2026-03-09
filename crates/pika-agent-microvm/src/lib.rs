use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{anyhow, Context};
use nostr_sdk::prelude::PublicKey;
use pika_agent_control_plane::{
    MicrovmAgentBackend, MicrovmProvisionParams, SpawnerCreateVmRequest as CreateVmRequest,
    SpawnerGuestAutostartRequest as GuestAutostartRequest, SpawnerVmResponse as VmResponse,
};
use serde_json::json;

pub const DEFAULT_SPAWNER_URL: &str = "http://127.0.0.1:8080";

pub const AUTOSTART_COMMAND: &str = "bash /workspace/pika-agent/start-agent.sh";
pub const AUTOSTART_SCRIPT_PATH: &str = "workspace/pika-agent/start-agent.sh";
pub const AUTOSTART_IDENTITY_PATH: &str = "workspace/pika-agent/state/identity.json";
pub const DEFAULT_ACP_EXEC_COMMAND: &str = "npx -y pi-acp";
pub const DEFAULT_ACP_CWD: &str = "/root/pika-agent/acp";

const DEFAULT_CREATE_VM_TIMEOUT_SECS: u64 = 60;
const MIN_CREATE_VM_TIMEOUT_SECS: u64 = 10;
const DELETE_VM_TIMEOUT: Duration = Duration::from_secs(30);
const RECOVER_VM_TIMEOUT: Duration = Duration::from_secs(60);
const REQUEST_ID_HEADER: &str = "x-request-id";

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ResolvedMicrovmParams {
    pub spawner_url: String,
    pub backend: ResolvedMicrovmAgentBackend,
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
    params.spawner_url.is_some() || params.backend.is_some()
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
        backend: resolve_backend(params.backend.as_ref()),
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
        AUTOSTART_IDENTITY_PATH.to_string(),
        bot_identity_file(bot_secret_hex, bot_pubkey_hex),
    );

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
if [[ -n "${PIKA_PIKACHAT_BIN:-}" && -x "${PIKA_PIKACHAT_BIN}" ]]; then
  bin="${PIKA_PIKACHAT_BIN}"
elif command -v pikachat >/dev/null 2>&1; then
  bin="pikachat"
fi
if [[ -z "$bin" ]]; then
  echo "[microvm-agent] could not find pikachat binary" >&2
  exit 1
fi

echo "[microvm-agent] starting daemon via $bin" >&2
daemon_args=(
  daemon
  --state-dir "$STATE_DIR"
  --auto-accept-welcomes
  --allow-pubkey "${PIKA_OWNER_PUBKEY}"
  "${relay_args[@]}"
)

backend_mode="${PIKA_AGENT_BACKEND_MODE:-native}"
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
    echo "[microvm-agent] unsupported backend mode (expected native or acp): $backend_mode" >&2
    exit 1
    ;;
esac

exec "$bin" "${daemon_args[@]}"
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
    use pika_test_utils::spawn_one_shot_server;
    use std::time::Duration as StdDuration;

    #[test]
    fn resolve_params_applies_defaults_and_overrides() {
        let defaults = resolve_params(&MicrovmProvisionParams::default());
        assert_eq!(defaults.spawner_url, DEFAULT_SPAWNER_URL);
        assert_eq!(defaults.backend, ResolvedMicrovmAgentBackend::Native);

        let overridden = resolve_params(&MicrovmProvisionParams {
            spawner_url: Some("http://10.0.0.5:8080".to_string()),
            backend: Some(MicrovmAgentBackend::Acp {
                exec_command: Some("npx -y pi-acp".to_string()),
                cwd: Some("/tmp/acp".to_string()),
            }),
        });
        assert_eq!(overridden.spawner_url, "http://10.0.0.5:8080");
        assert_eq!(
            overridden.backend,
            ResolvedMicrovmAgentBackend::Acp {
                exec_command: "npx -y pi-acp".to_string(),
                cwd: "/tmp/acp".to_string(),
            }
        );
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
        assert_eq!(
            value["guest_autostart"]["env"]["PIKA_AGENT_BACKEND_MODE"],
            "native"
        );
        assert!(value["guest_autostart"]["files"][AUTOSTART_SCRIPT_PATH]
            .as_str()
            .expect("autostart script")
            .contains("starting daemon"));
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
        assert!(script.contains("--state-dir \"$STATE_DIR\""));
        assert!(script.contains("PIKA_PIKACHAT_BIN"));
        assert!(script.contains("PIKA_AGENT_BACKEND_MODE"));
        assert!(script.contains("native)"));
        assert!(script.contains("--acp-exec \"$acp_exec\""));
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
            backend: None,
        }));
        assert!(microvm_params_provided(&MicrovmProvisionParams {
            spawner_url: None,
            backend: Some(MicrovmAgentBackend::Native),
        }));
        assert!(microvm_params_provided(&MicrovmProvisionParams {
            spawner_url: None,
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
