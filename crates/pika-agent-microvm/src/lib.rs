use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{anyhow, Context};
use nostr_sdk::prelude::PublicKey;
use pika_agent_control_plane::{
    GuestAcpBackend, GuestOpenclawDaemonBackend, GuestServiceBackendMode, GuestServiceKind,
    GuestServiceLaunch, GuestServiceReadinessCheck, GuestStartupArtifacts, GuestStartupPlan,
    MicrovmAgentBackend, MicrovmAgentKind, MicrovmProvisionParams,
    SpawnerCreateVmRequest as CreateVmRequest,
    SpawnerGuestAutostartRequest as GuestAutostartRequest, SpawnerOpenClawLaunchAuth,
    SpawnerVmBackupStatus, SpawnerVmResponse as VmResponse,
};
use serde_json::json;

pub use pika_agent_control_plane::{
    GUEST_AUTOSTART_COMMAND as AUTOSTART_COMMAND,
    GUEST_AUTOSTART_IDENTITY_PATH as AUTOSTART_IDENTITY_PATH,
    GUEST_AUTOSTART_SCRIPT_PATH as AUTOSTART_SCRIPT_PATH,
    GUEST_FAILED_MARKER_PATH as AGENT_FAILED_PATH, GUEST_LOG_PATH as AGENT_LOG_PATH,
    GUEST_OPENCLAW_CONFIG_PATH as OPENCLAW_CONFIG_PATH,
    GUEST_OPENCLAW_EXTENSION_ROOT as OPENCLAW_EXTENSION_ROOT, GUEST_PID_PATH as AGENT_PID_PATH,
    GUEST_READY_MARKER_PATH as AGENT_READY_PATH, GUEST_STARTUP_PLAN_PATH as STARTUP_PLAN_PATH,
};

pub const DEFAULT_SPAWNER_URL: &str = "http://127.0.0.1:8080";

pub const DEFAULT_ACP_EXEC_COMMAND: &str = "npx -y pi-acp";
pub const DEFAULT_ACP_CWD: &str = "/root/pika-agent/acp";
pub const DEFAULT_OPENCLAW_EXEC_COMMAND: &str = "/opt/runtime-artifacts/openclaw/bin/openclaw";
pub const DEFAULT_OPENCLAW_GATEWAY_PORT: u16 = 18789;
pub const DEFAULT_DAEMON_STATE_DIR: &str = "/root/pika-agent/state";
pub const DEFAULT_OPENCLAW_STATE_DIR: &str = "/root/pika-agent/openclaw";
pub const DEFAULT_OPENCLAW_CONTROL_UI_ORIGIN: &str = "http://openclaw.localhost:19401";
pub const DEFAULT_OPENCLAW_CONTROL_UI_HTTPS_ORIGIN: &str = "https://openclaw.localhost:19401";
const DEFAULT_OPENCLAW_TRUSTED_PROXIES: &[&str] = &["127.0.0.1", "::1"];

const DEFAULT_CREATE_VM_TIMEOUT_SECS: u64 = 60;
const MIN_CREATE_VM_TIMEOUT_SECS: u64 = 10;
const DELETE_VM_TIMEOUT: Duration = Duration::from_secs(30);
const RECOVER_VM_TIMEOUT: Duration = Duration::from_secs(60);
const GET_VM_TIMEOUT: Duration = Duration::from_secs(10);
const GET_VM_BACKUP_STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const GET_OPENCLAW_LAUNCH_AUTH_TIMEOUT: Duration = Duration::from_secs(10);
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

#[derive(Debug, Clone, Copy)]
pub struct ManagedVmCreateInput<'a> {
    pub owner_pubkey: &'a PublicKey,
    pub relay_urls: &'a [String],
    pub bot_secret_hex: &'a str,
    pub bot_pubkey_hex: &'a str,
}

#[derive(Debug, Clone)]
pub struct MicrovmSpawnerClient {
    client: reqwest::Client,
    base_url: String,
    create_vm_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct MicrovmManagedVmProvider {
    client: MicrovmSpawnerClient,
    resolved: ResolvedMicrovmParams,
}

impl MicrovmManagedVmProvider {
    pub fn new(resolved: ResolvedMicrovmParams) -> Self {
        let client = MicrovmSpawnerClient::new(resolved.spawner_url.clone());
        Self { client, resolved }
    }

    pub fn spawner_base_url(&self) -> &str {
        self.client.base_url()
    }

    pub fn resolved(&self) -> &ResolvedMicrovmParams {
        &self.resolved
    }

    pub async fn create_managed_vm(
        &self,
        input: ManagedVmCreateInput<'_>,
        request_id: Option<&str>,
    ) -> anyhow::Result<VmResponse> {
        let request = build_create_vm_request(
            input.owner_pubkey,
            input.relay_urls,
            input.bot_secret_hex,
            input.bot_pubkey_hex,
            &self.resolved,
        );
        self.client
            .create_vm_with_request_id(&request, request_id)
            .await
            .map_err(|err| spawner_create_error(self.client.base_url(), err))
    }

    pub async fn get_vm_status(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<VmResponse> {
        self.client.get_vm_with_request_id(vm_id, request_id).await
    }

    pub async fn get_vm_backup_status(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerVmBackupStatus> {
        self.client
            .get_vm_backup_status_with_request_id(vm_id, request_id)
            .await
    }

    pub async fn recover_vm(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<VmResponse> {
        self.client
            .recover_vm_with_request_id(vm_id, request_id)
            .await
    }

    pub async fn restore_vm(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<VmResponse> {
        self.client
            .restore_vm_with_request_id(vm_id, request_id)
            .await
    }

    pub async fn delete_vm(&self, vm_id: &str, request_id: Option<&str>) -> anyhow::Result<()> {
        self.client
            .delete_vm_with_request_id(vm_id, request_id)
            .await
    }

    pub async fn get_openclaw_launch_auth(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerOpenClawLaunchAuth> {
        self.client
            .get_openclaw_launch_auth_with_request_id(vm_id, request_id)
            .await
    }
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

    pub async fn restore_vm(&self, vm_id: &str) -> anyhow::Result<VmResponse> {
        self.restore_vm_with_request_id(vm_id, None).await
    }

    pub async fn restore_vm_with_request_id(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<VmResponse> {
        let url = format!("{}/vms/{vm_id}/restore", self.base_url);
        // Durable-home restores can legitimately run for several minutes.
        let resp = with_request_id(self.client.post(&url), request_id)
            .send()
            .await
            .context("send restore vm request")?;
        let status = resp.status();
        if !status.is_success() {
            let upstream_request_id = response_request_id(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                upstream_error_message(
                    "restore vm",
                    Some(vm_id),
                    status,
                    upstream_request_id.as_deref(),
                    &text
                )
            );
        }
        resp.json().await.context("decode restore vm response")
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
        let resp = with_request_id(self.client.get(&url).timeout(GET_VM_TIMEOUT), request_id)
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

    pub async fn get_vm_backup_status(&self, vm_id: &str) -> anyhow::Result<SpawnerVmBackupStatus> {
        self.get_vm_backup_status_with_request_id(vm_id, None).await
    }

    pub async fn get_vm_backup_status_with_request_id(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerVmBackupStatus> {
        let url = format!("{}/vms/{vm_id}/backup-status", self.base_url);
        let resp = with_request_id(
            self.client.get(&url).timeout(GET_VM_BACKUP_STATUS_TIMEOUT),
            request_id,
        )
        .send()
        .await
        .context("send get vm backup status request")?;
        let status = resp.status();
        if !status.is_success() {
            let upstream_request_id = response_request_id(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                upstream_error_message(
                    "get vm backup status",
                    Some(vm_id),
                    status,
                    upstream_request_id.as_deref(),
                    &text
                )
            );
        }
        resp.json()
            .await
            .context("decode get vm backup status response")
    }

    pub async fn get_openclaw_launch_auth(
        &self,
        vm_id: &str,
    ) -> anyhow::Result<SpawnerOpenClawLaunchAuth> {
        self.get_openclaw_launch_auth_with_request_id(vm_id, None)
            .await
    }

    pub async fn get_openclaw_launch_auth_with_request_id(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerOpenClawLaunchAuth> {
        let url = format!("{}/vms/{vm_id}/openclaw/launch-auth", self.base_url);
        let resp = with_request_id(
            self.client
                .get(&url)
                .timeout(GET_OPENCLAW_LAUNCH_AUTH_TIMEOUT),
            request_id,
        )
        .send()
        .await
        .context("send get openclaw launch auth request")?;
        let status = resp.status();
        if !status.is_success() {
            let upstream_request_id = response_request_id(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                upstream_error_message(
                    "get openclaw launch auth",
                    Some(vm_id),
                    status,
                    upstream_request_id.as_deref(),
                    &text
                )
            );
        }
        resp.json()
            .await
            .context("decode get openclaw launch auth response")
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
    let kind = resolve_kind(params.kind);
    ResolvedMicrovmParams {
        spawner_url: params
            .spawner_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_SPAWNER_URL)
            .to_string(),
        kind,
        backend: resolve_backend(kind, params.backend.as_ref()),
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

fn resolve_backend(
    kind: ResolvedMicrovmAgentKind,
    backend: Option<&MicrovmAgentBackend>,
) -> ResolvedMicrovmAgentBackend {
    match backend {
        Some(MicrovmAgentBackend::Native) => ResolvedMicrovmAgentBackend::Native,
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
        None => match kind {
            ResolvedMicrovmAgentKind::Pi => ResolvedMicrovmAgentBackend::Acp {
                exec_command: DEFAULT_ACP_EXEC_COMMAND.to_string(),
                cwd: DEFAULT_ACP_CWD.to_string(),
            },
            ResolvedMicrovmAgentKind::Openclaw => ResolvedMicrovmAgentBackend::Native,
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
    let startup_plan = guest_startup_plan(params);
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
        microvm_autostart_script(),
    );
    files.insert(
        STARTUP_PLAN_PATH.to_string(),
        startup_plan_file(&startup_plan),
    );
    files.insert(
        AUTOSTART_IDENTITY_PATH.to_string(),
        bot_identity_file(bot_secret_hex, bot_pubkey_hex),
    );
    if matches!(params.kind, ResolvedMicrovmAgentKind::Openclaw) {
        files.insert(
            OPENCLAW_CONFIG_PATH.to_string(),
            openclaw_gateway_config(relay_urls, &startup_plan),
        );
        files.extend(openclaw_extension_files());
    }

    CreateVmRequest {
        guest_autostart: GuestAutostartRequest {
            command: AUTOSTART_COMMAND.to_string(),
            env,
            files,
            startup_plan,
        },
    }
}

fn guest_startup_plan(params: &ResolvedMicrovmParams) -> GuestStartupPlan {
    let artifacts = GuestStartupArtifacts::default();
    match (&params.kind, &params.backend) {
        (ResolvedMicrovmAgentKind::Pi, ResolvedMicrovmAgentBackend::Acp { exec_command, cwd }) => {
            GuestStartupPlan {
                agent_kind: MicrovmAgentKind::Pi,
                service_kind: GuestServiceKind::PikachatDaemon,
                backend_mode: GuestServiceBackendMode::Acp,
                daemon_state_dir: DEFAULT_DAEMON_STATE_DIR.to_string(),
                service: GuestServiceLaunch::PikachatDaemon {
                    acp_backend: Some(GuestAcpBackend {
                        exec_command: exec_command.clone(),
                        cwd: cwd.clone(),
                    }),
                },
                readiness_check: GuestServiceReadinessCheck::LogContains {
                    path: AGENT_LOG_PATH.to_string(),
                    pattern: "\"type\":\"ready\"".to_string(),
                    ready_probe: "daemon_ready_event".to_string(),
                    timeout_failure_reason: "timeout_waiting_for_daemon_ready".to_string(),
                },
                artifacts,
                exit_failure_reason: "pi_agent_exited".to_string(),
            }
        }
        (ResolvedMicrovmAgentKind::Pi, ResolvedMicrovmAgentBackend::Native) => GuestStartupPlan {
            agent_kind: MicrovmAgentKind::Pi,
            service_kind: GuestServiceKind::PikachatDaemon,
            backend_mode: GuestServiceBackendMode::Native,
            daemon_state_dir: DEFAULT_DAEMON_STATE_DIR.to_string(),
            service: GuestServiceLaunch::PikachatDaemon { acp_backend: None },
            readiness_check: GuestServiceReadinessCheck::LogContains {
                path: AGENT_LOG_PATH.to_string(),
                pattern: "\"type\":\"ready\"".to_string(),
                ready_probe: "daemon_ready_event".to_string(),
                timeout_failure_reason: "timeout_waiting_for_daemon_ready".to_string(),
            },
            artifacts,
            exit_failure_reason: "pi_agent_exited".to_string(),
        },
        (
            ResolvedMicrovmAgentKind::Openclaw,
            ResolvedMicrovmAgentBackend::Acp { exec_command, cwd },
        ) => GuestStartupPlan {
            agent_kind: MicrovmAgentKind::Openclaw,
            service_kind: GuestServiceKind::OpenclawGateway,
            backend_mode: GuestServiceBackendMode::Acp,
            daemon_state_dir: DEFAULT_DAEMON_STATE_DIR.to_string(),
            service: GuestServiceLaunch::OpenclawGateway {
                exec_command: resolved_openclaw_exec_command(),
                state_dir: DEFAULT_OPENCLAW_STATE_DIR.to_string(),
                config_path: OPENCLAW_CONFIG_PATH.to_string(),
                gateway_port: DEFAULT_OPENCLAW_GATEWAY_PORT,
                daemon_backend: GuestOpenclawDaemonBackend::Acp {
                    acp_backend: GuestAcpBackend {
                        exec_command: exec_command.clone(),
                        cwd: cwd.clone(),
                    },
                },
            },
            readiness_check: GuestServiceReadinessCheck::HttpGetOk {
                url: format!("http://127.0.0.1:{DEFAULT_OPENCLAW_GATEWAY_PORT}/health"),
                ready_probe: "openclaw_gateway_health".to_string(),
                timeout_failure_reason: "timeout_waiting_for_openclaw_health".to_string(),
            },
            artifacts,
            exit_failure_reason: "openclaw_gateway_exited".to_string(),
        },
        (ResolvedMicrovmAgentKind::Openclaw, ResolvedMicrovmAgentBackend::Native) => {
            GuestStartupPlan {
                agent_kind: MicrovmAgentKind::Openclaw,
                service_kind: GuestServiceKind::OpenclawGateway,
                backend_mode: GuestServiceBackendMode::Native,
                daemon_state_dir: DEFAULT_DAEMON_STATE_DIR.to_string(),
                service: GuestServiceLaunch::OpenclawGateway {
                    exec_command: resolved_openclaw_exec_command(),
                    state_dir: DEFAULT_OPENCLAW_STATE_DIR.to_string(),
                    config_path: OPENCLAW_CONFIG_PATH.to_string(),
                    gateway_port: DEFAULT_OPENCLAW_GATEWAY_PORT,
                    daemon_backend: GuestOpenclawDaemonBackend::Native,
                },
                readiness_check: GuestServiceReadinessCheck::HttpGetOk {
                    url: format!("http://127.0.0.1:{DEFAULT_OPENCLAW_GATEWAY_PORT}/health"),
                    ready_probe: "openclaw_gateway_health".to_string(),
                    timeout_failure_reason: "timeout_waiting_for_openclaw_health".to_string(),
                },
                artifacts,
                exit_failure_reason: "openclaw_gateway_exited".to_string(),
            }
        }
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

pub fn microvm_autostart_script() -> String {
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
    echo "[microvm-agent] failed to start OpenClaw private gateway proxy on $listen_host:$listen_port" >&2
    exit 1
  fi
}}

trap cleanup_agent EXIT TERM INT

if ! command -v jq >/dev/null 2>&1; then
  echo "[microvm-agent] missing jq in guest image; startup plan runner requires jq" >&2
  exit 1
fi

if [[ ! -f "$STARTUP_PLAN_PATH" ]]; then
  echo "[microvm-agent] missing startup plan at $STARTUP_PLAN_PATH" >&2
  exit 1
fi

agent_kind="$(plan_value '.agent_kind')"
service_kind="$(plan_value '.service_kind')"
backend_mode="$(plan_value '.backend_mode')"
daemon_state_dir="$(plan_value '.daemon_state_dir')"
ready_path="$(workspace_path "$(plan_value '.artifacts.ready_marker_path')")"
failed_path="$(workspace_path "$(plan_value '.artifacts.failed_marker_path')")"
identity_seed_path="$(workspace_path "$(plan_value '.artifacts.identity_seed_path')")"
exit_failure_reason="$(plan_value '.exit_failure_reason')"

mkdir -p "$daemon_state_dir"
if [[ -f "$identity_seed_path" && ! -f "$daemon_state_dir/identity.json" ]]; then
  cp "$identity_seed_path" "$daemon_state_dir/identity.json"
fi
rm -f "$ready_path" "$failed_path"

if [[ -z "${{PIKA_OWNER_PUBKEY:-}}" ]]; then
  echo "[microvm-agent] missing PIKA_OWNER_PUBKEY" >&2
  exit 1
fi
if [[ -z "${{PIKA_RELAY_URLS:-}}" ]]; then
  echo "[microvm-agent] missing PIKA_RELAY_URLS" >&2
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
  echo "[microvm-agent] no valid relays in PIKA_RELAY_URLS" >&2
  exit 1
fi

bin=""
if [[ -n "${{PIKA_PIKACHAT_BIN:-}}" && -x "${{PIKA_PIKACHAT_BIN}}" ]]; then
  bin="${{PIKA_PIKACHAT_BIN}}"
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
if [[ -n "${{PIKA_PI_CMD:-}}" ]]; then
  pi_exec="${{PIKA_PI_CMD%% *}}"
  if [[ -n "$pi_exec" && -x "$pi_exec" ]]; then
    export PATH="$(dirname "$pi_exec"):$PATH"
  fi
fi

write_ready_marker() {{
  local probe="$1"
  cat >"$ready_path" <<EOF
{{
  "ready": true,
  "agent_kind": "${{agent_kind}}",
  "backend_mode": "${{backend_mode}}",
  "service_kind": "${{service_kind}}",
  "probe": "${{probe}}"
}}
EOF
  rm -f "$failed_path"
}}

write_failed_marker() {{
  local reason="$1"
  cat >"$failed_path" <<EOF
{{
  "ready": false,
  "agent_kind": "${{agent_kind}}",
  "backend_mode": "${{backend_mode}}",
  "service_kind": "${{service_kind}}",
  "reason": "${{reason}}"
}}
EOF
  rm -f "$ready_path"
}}

publish_daemon_keypackage() {{
  case "$service_kind" in
    pikachat_daemon|openclaw_gateway)
      ;;
    *)
      return 0
      ;;
  esac

  local publish_args=(--remote --state-dir "$daemon_state_dir")
  publish_args+=("${{relay_args[@]}}")
  if "$bin" "${{publish_args[@]}}" publish-kp >/dev/null; then
    return 0
  fi

  echo "[microvm-agent] service ready but keypackage publish not confirmed yet; retrying" >&2
  return 1
}}

keypackage_publish_timeout_failure_reason() {{
  case "$service_kind" in
    pikachat_daemon)
      printf '%s\n' "timeout_waiting_for_daemon_keypackage_publish"
      ;;
    openclaw_gateway)
      printf '%s\n' "timeout_waiting_for_openclaw_keypackage_publish"
      ;;
    *)
      printf '%s\n' "timeout_waiting_for_keypackage_publish"
      ;;
  esac
}}

service_readiness_probe_succeeds() {{
  local readiness_kind="$1"
  local readiness_path="$2"
  local readiness_pattern="$3"
  local readiness_url="$4"

  case "$readiness_kind" in
    log_contains)
      [[ -f "$readiness_path" ]] && grep -q -- "$readiness_pattern" "$readiness_path"
      ;;
    http_get_ok)
      curl -fsS --max-time 2 "$readiness_url" >/dev/null 2>&1
      ;;
    *)
      return 1
      ;;
  esac
}}

wait_for_service_ready() {{
  local service_pid="$1"
  local timeout_sec="${{PIKA_AGENT_READY_TIMEOUT_SECS:-120}}"
  local deadline=$((SECONDS + timeout_sec))
  local readiness_kind
  local ready_probe
  local timeout_failure_reason
  local readiness_path=""
  local readiness_pattern=""
  local readiness_url=""

  readiness_kind="$(plan_value '.readiness_check.kind')"
  ready_probe="$(plan_value '.readiness_check.ready_probe')"
  timeout_failure_reason="$(plan_value '.readiness_check.timeout_failure_reason')"

  case "$readiness_kind" in
    log_contains)
      readiness_path="$(workspace_path "$(plan_value '.readiness_check.path')")"
      readiness_pattern="$(plan_value '.readiness_check.pattern')"
      ;;
    http_get_ok)
      readiness_url="$(plan_value '.readiness_check.url')"
      ;;
    *)
      echo "[microvm-agent] unsupported readiness check kind: $readiness_kind" >&2
      exit 1
      ;;
  esac

  while (( SECONDS < deadline )); do
    if ! kill -0 "$service_pid" 2>/dev/null; then
      wait "$service_pid"
      return $?
    fi
    if service_readiness_probe_succeeds \
      "$readiness_kind" \
      "$readiness_path" \
      "$readiness_pattern" \
      "$readiness_url"; then
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
  local publish_failure_reason
  local readiness_kind
  local readiness_path=""
  local readiness_pattern=""
  local readiness_url=""
  publish_failure_reason="$(keypackage_publish_timeout_failure_reason)"
  readiness_kind="$(plan_value '.readiness_check.kind')"

  case "$readiness_kind" in
    log_contains)
      readiness_path="$(workspace_path "$(plan_value '.readiness_check.path')")"
      readiness_pattern="$(plan_value '.readiness_check.pattern')"
      ;;
    http_get_ok)
      readiness_url="$(plan_value '.readiness_check.url')"
      ;;
    *)
      echo "[microvm-agent] unsupported readiness check kind: $readiness_kind" >&2
      exit 1
      ;;
  esac

  while (( SECONDS < deadline )); do
    if ! kill -0 "$service_pid" 2>/dev/null; then
      wait "$service_pid"
      return $?
    fi
    if service_readiness_probe_succeeds \
      "$readiness_kind" \
      "$readiness_path" \
      "$readiness_pattern" \
      "$readiness_url" && \
      publish_daemon_keypackage; then
      write_ready_marker "$ready_probe"
      return 0
    fi
    sleep 1
  done

  write_failed_marker "$publish_failure_reason"
  kill "$service_pid" 2>/dev/null || true
  wait "$service_pid" || true
  return 1
}}

start_service() {{
  case "$service_kind" in
    pikachat_daemon)
      echo "[microvm-agent] starting pikachat daemon via $bin" >&2
      daemon_args=(
        daemon
        --state-dir "$daemon_state_dir"
        --auto-accept-welcomes
        --allow-pubkey "${{PIKA_OWNER_PUBKEY}}"
        "${{relay_args[@]}}"
      )
      # backend_mode is authoritative for the daemon launch contract; the ACP payload
      # only supplies the extra launch arguments when ACP mode is selected.
      case "$backend_mode" in
        native)
          if ! jq -e '.service.acp_backend == null' "$STARTUP_PLAN_PATH" >/dev/null; then
            echo "[microvm-agent] invalid startup plan: backend_mode=native but ACP payload is present" >&2
            exit 1
          fi
          ;;
        acp)
          if ! jq -e '.service.acp_backend != null' "$STARTUP_PLAN_PATH" >/dev/null; then
            echo "[microvm-agent] invalid startup plan: backend_mode=acp but ACP payload is missing" >&2
            exit 1
          fi
          acp_exec="$(plan_value '.service.acp_backend.exec_command')"
          acp_cwd="$(plan_value '.service.acp_backend.cwd')"
          if [[ -z "$acp_exec" ]]; then
            echo "[microvm-agent] ACP backend requires a non-empty ACP exec command" >&2
            exit 1
          fi
          mkdir -p "$acp_cwd"
          daemon_args+=(--acp-exec "$acp_exec" --acp-cwd "$acp_cwd")
          ;;
        *)
          echo "[microvm-agent] unsupported daemon backend mode: $backend_mode" >&2
          exit 1
          ;;
      esac
      "$bin" "${{daemon_args[@]}}" &
      agent_pid=$!
      ;;
    openclaw_gateway)
      openclaw_exec="$(plan_value '.service.exec_command')"
      openclaw_state_dir="$(plan_value '.service.state_dir')"
      openclaw_config_path="$(workspace_path "$(plan_value '.service.config_path')")"
      openclaw_workspace_root="$(dirname "$openclaw_config_path")"
      openclaw_package_root="${{PIKA_OPENCLAW_PACKAGE_ROOT:-$(dirname "$(dirname "$openclaw_exec")")/lib/openclaw}}"
      gateway_port="$(plan_value '.service.gateway_port | tostring')"
      if [[ -z "$openclaw_exec" ]]; then
        echo "[microvm-agent] OpenClaw gateway requires a non-empty exec command" >&2
        exit 1
      fi
      if [[ "$openclaw_exec" == *[[:space:]]* ]]; then
        echo "[microvm-agent] OpenClaw gateway exec must be a binary path: $openclaw_exec" >&2
        exit 1
      fi
      if [[ ! -x "$openclaw_exec" ]]; then
        echo "[microvm-agent] OpenClaw gateway executable not found: $openclaw_exec" >&2
        exit 1
      fi
      if [[ ! -f "$openclaw_config_path" ]]; then
        echo "[microvm-agent] missing OpenClaw config at $openclaw_config_path" >&2
        exit 1
      fi
      if [[ ! -f "$openclaw_package_root/package.json" ]]; then
        echo "[microvm-agent] OpenClaw package root not found: $openclaw_package_root" >&2
        exit 1
      fi
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
      echo "[microvm-agent] starting OpenClaw gateway via $openclaw_exec" >&2
      "$openclaw_exec" gateway --allow-unconfigured &
      agent_pid=$!
      if [[ "${{PIKA_ENABLE_OPENCLAW_PRIVATE_PROXY:-1}}" != "0" ]]; then
        start_openclaw_private_proxy "$PIKA_VM_IP" "$gateway_port"
      fi
      ;;
    *)
      echo "[microvm-agent] unsupported startup service kind: $service_kind" >&2
      exit 1
      ;;
  esac
}}

start_service
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
  write_failed_marker "$exit_failure_reason"
else
  rm -f "$failed_path"
fi
exit $status
"#,
        startup_plan_path = STARTUP_PLAN_PATH,
    )
}

fn openclaw_gateway_config(relay_urls: &[String], startup_plan: &GuestStartupPlan) -> String {
    startup_plan
        .validate()
        .expect("openclaw startup plan must be internally consistent");
    let GuestServiceLaunch::OpenclawGateway { daemon_backend, .. } = &startup_plan.service else {
        panic!("openclaw_gateway_config requires OpenClaw startup plan");
    };
    let mut channel_config = json!({
        "relays": relay_urls,
        "stateDir": startup_plan.daemon_state_dir,
        "autoAcceptWelcomes": true,
        "groupPolicy": "open",
        "daemonCmd": "pikachat",
    });
    match daemon_backend {
        GuestOpenclawDaemonBackend::Native => {
            channel_config["daemonBackend"] = json!("native");
        }
        GuestOpenclawDaemonBackend::Acp { acp_backend } => {
            channel_config["daemonBackend"] = json!("acp");
            channel_config["daemonAcpExec"] = json!(acp_backend.exec_command);
            channel_config["daemonAcpCwd"] = json!(acp_backend.cwd);
        }
    }
    // Keep the plugin entry config and channel config identical so either OpenClaw
    // surface sees the same daemon launch settings.
    let entry_config = channel_config.clone();
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

fn managed_openclaw_gateway_security_config() -> serde_json::Value {
    let control_ui_allowed_origins = openclaw_control_ui_allowed_origins();
    // Managed OpenClaw is launched through the platform's authenticated dashboard,
    // ticket handoff, and scoped UI session. Guest device pairing is intentionally
    // disabled for this allowlisted flow because the platform boundary is the
    // intended security control, not direct guest-local browser trust.
    json!({
        "controlUi": {
            "allowInsecureAuth": true,
            "dangerouslyDisableDeviceAuth": true,
            "allowedOrigins": control_ui_allowed_origins,
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
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration as StdDuration;
    use std::time::Instant;

    #[derive(Copy, Clone)]
    enum KeypackageReadyScenario {
        Pi,
        Openclaw,
    }

    #[derive(Copy, Clone)]
    enum KeypackagePublishOutcome {
        SucceedsAfterRetry,
        SucceedsAfterReadinessRecovery,
        TimesOut { expected_reason: &'static str },
    }

    fn openclaw_origin_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct OpenClawOriginEnvGuard {
        prior: Option<String>,
    }

    impl OpenClawOriginEnvGuard {
        fn set(value: &str) -> Self {
            let prior = std::env::var("PIKA_OPENCLAW_CONTROL_UI_ALLOWED_ORIGINS").ok();
            unsafe {
                std::env::set_var("PIKA_OPENCLAW_CONTROL_UI_ALLOWED_ORIGINS", value);
            }
            Self { prior }
        }
    }

    impl Drop for OpenClawOriginEnvGuard {
        fn drop(&mut self) {
            match self.prior.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var("PIKA_OPENCLAW_CONTROL_UI_ALLOWED_ORIGINS", prior)
                },
                None => unsafe { std::env::remove_var("PIKA_OPENCLAW_CONTROL_UI_ALLOWED_ORIGINS") },
            }
        }
    }

    fn write_executable(path: &Path, body: &str) {
        fs::write(path, body).expect("write script");
        let mut perms = fs::metadata(path).expect("stat script").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod script");
    }

    fn poll_until(timeout: StdDuration, mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate() {
                return;
            }
            std::thread::sleep(StdDuration::from_millis(50));
        }
        assert!(predicate(), "condition not met within {:?}", timeout);
    }

    fn async_test_timeout() -> StdDuration {
        let secs = std::env::var("PIKA_AGENT_MICROVM_TEST_TIMEOUT_SECS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(15);
        StdDuration::from_secs(secs)
    }

    fn keypackage_timeout_test_secs() -> &'static str {
        "6"
    }

    fn test_guest_startup_plan() -> GuestStartupPlan {
        guest_startup_plan(&ResolvedMicrovmParams {
            spawner_url: DEFAULT_SPAWNER_URL.to_string(),
            kind: ResolvedMicrovmAgentKind::Pi,
            backend: ResolvedMicrovmAgentBackend::Acp {
                exec_command: DEFAULT_ACP_EXEC_COMMAND.to_string(),
                cwd: DEFAULT_ACP_CWD.to_string(),
            },
        })
    }

    fn read_counter(path: &Path) -> u32 {
        fs::read_to_string(path)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(0)
    }

    fn root_relative(path: &Path) -> String {
        path.strip_prefix("/")
            .expect("absolute path")
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn run_keypackage_ready_gating_scenario(
        scenario: KeypackageReadyScenario,
        outcome: KeypackagePublishOutcome,
    ) {
        let root = tempfile::tempdir().expect("tempdir");
        let bin_dir = root.path().join("bin");
        fs::create_dir_all(&bin_dir).expect("create bin dir");

        let workspace_dir = root.path().join("workspace/pika-agent");
        let daemon_state_dir = root.path().join("runtime/state");
        let acp_cwd = root.path().join("runtime/acp");
        let openclaw_state_dir = root.path().join("runtime/openclaw");
        let openclaw_package_root = root.path().join("runtime/openclaw-package");
        let openclaw_gateway_port = TcpListener::bind(("127.0.0.1", 0))
            .expect("bind ephemeral openclaw test port")
            .local_addr()
            .expect("read openclaw test port")
            .port();
        fs::create_dir_all(&workspace_dir).expect("create workspace dir");
        fs::create_dir_all(&daemon_state_dir).expect("create daemon state dir");
        fs::create_dir_all(&acp_cwd).expect("create acp cwd");
        fs::create_dir_all(&openclaw_state_dir).expect("create openclaw state dir");
        fs::create_dir_all(&openclaw_package_root).expect("create openclaw package root");

        let startup_plan_path = workspace_dir.join("startup-plan.json");
        let ready_marker_path = workspace_dir.join("service-ready.json");
        let failed_marker_path = workspace_dir.join("service-failed.json");
        let identity_seed_path = workspace_dir.join("state/identity.json");
        let ready_log_path = workspace_dir.join("agent.log");
        let openclaw_config_path = workspace_dir.join("openclaw/openclaw.json");
        let openclaw_extension_root = workspace_dir.join("openclaw/extensions/pikachat-openclaw");
        let publish_count_path = root.path().join("publish-count.txt");
        let publish_log_path = root.path().join("publish.log");
        let curl_count_path = root.path().join("curl-count.txt");
        let fake_openclaw_path = bin_dir.join("openclaw-fake");

        fs::create_dir_all(identity_seed_path.parent().expect("identity parent"))
            .expect("create identity dir");
        fs::create_dir_all(
            openclaw_config_path
                .parent()
                .expect("openclaw config parent"),
        )
        .expect("create openclaw config dir");
        fs::create_dir_all(&openclaw_extension_root).expect("create openclaw extension dir");
        fs::write(&startup_plan_path, "{}\n").expect("write startup plan");
        fs::write(
            &identity_seed_path,
            "{\n  \"secret_key_hex\": \"00\",\n  \"public_key_hex\": \"11\"\n}\n",
        )
        .expect("write identity seed");
        fs::write(&openclaw_config_path, "{\n  \"gateway\": {}\n}\n")
            .expect("write openclaw config");
        fs::write(
            openclaw_package_root.join("package.json"),
            "{\n  \"name\": \"openclaw\",\n  \"exports\": {}\n}\n",
        )
        .expect("write openclaw package metadata");

        let jq_path = bin_dir.join("jq");
        write_executable(
            &jq_path,
            r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "-er" || "${1:-}" == "-e" ]]; then
  shift
fi
query="${1:-}"
case "$query" in
  '.agent_kind') printf '%s\n' "${TEST_JQ_AGENT_KIND}" ;;
  '.service_kind') printf '%s\n' "${TEST_JQ_SERVICE_KIND}" ;;
  '.backend_mode') printf '%s\n' "${TEST_JQ_BACKEND_MODE}" ;;
  '.daemon_state_dir') printf '%s\n' "${TEST_JQ_DAEMON_STATE_DIR}" ;;
  '.artifacts.ready_marker_path') printf '%s\n' "${TEST_JQ_READY_MARKER_PATH}" ;;
  '.artifacts.failed_marker_path') printf '%s\n' "${TEST_JQ_FAILED_MARKER_PATH}" ;;
  '.artifacts.identity_seed_path') printf '%s\n' "${TEST_JQ_IDENTITY_SEED_PATH}" ;;
  '.exit_failure_reason') printf '%s\n' "${TEST_JQ_EXIT_FAILURE_REASON}" ;;
  '.readiness_check.kind') printf '%s\n' "${TEST_JQ_READINESS_KIND}" ;;
  '.readiness_check.ready_probe') printf '%s\n' "${TEST_JQ_READY_PROBE}" ;;
  '.readiness_check.timeout_failure_reason') printf '%s\n' "${TEST_JQ_TIMEOUT_FAILURE_REASON}" ;;
  '.readiness_check.path') printf '%s\n' "${TEST_JQ_READINESS_PATH}" ;;
  '.readiness_check.pattern') printf '%s\n' "${TEST_JQ_READINESS_PATTERN}" ;;
  '.readiness_check.url') printf '%s\n' "${TEST_JQ_READINESS_URL}" ;;
  '.service.acp_backend.exec_command') printf '%s\n' "${TEST_JQ_ACP_EXEC_COMMAND}" ;;
  '.service.acp_backend.cwd') printf '%s\n' "${TEST_JQ_ACP_CWD}" ;;
  '.service.exec_command') printf '%s\n' "${TEST_JQ_SERVICE_EXEC_COMMAND}" ;;
  '.service.state_dir') printf '%s\n' "${TEST_JQ_SERVICE_STATE_DIR}" ;;
  '.service.config_path') printf '%s\n' "${TEST_JQ_SERVICE_CONFIG_PATH}" ;;
  '.service.gateway_port | tostring') printf '%s\n' "${TEST_JQ_GATEWAY_PORT}" ;;
  '.service.acp_backend == null')
    [[ "${TEST_JQ_SERVICE_ACP_PRESENT:-0}" == "0" ]]
    ;;
  '.service.acp_backend != null')
    [[ "${TEST_JQ_SERVICE_ACP_PRESENT:-0}" == "1" ]]
    ;;
  *)
    echo "unsupported jq query: $query" >&2
    exit 1
    ;;
esac
"#,
        );

        let pikachat_path = bin_dir.join("pikachat");
        write_executable(
            &pikachat_path,
            r#"#!/usr/bin/env bash
set -euo pipefail
cmd="${1:-}"
if [[ "$cmd" == "daemon" ]]; then
  state_dir=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --state-dir)
        state_dir="$2"
        shift 2
        ;;
      *)
        shift
        ;;
    esac
  done
  mkdir -p "$state_dir"
  if [[ -n "${PIKA_TEST_DAEMON_READY_LOG_PATH:-}" ]]; then
    printf '%s\n' "daemon-ready" >> "${PIKA_TEST_DAEMON_READY_LOG_PATH}"
  fi
  printf '%s\n' '{"type":"ready","pubkey":"test","npub":"npub1test"}' >> "${PIKA_TEST_READY_LOG_PATH}"
  trap 'exit 0' TERM INT
  while :; do
    sleep 1
  done
elif [[ "$cmd" == "--remote" ]]; then
  shift
  state_dir=""
  relays=()
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --state-dir)
        state_dir="$2"
        shift 2
        ;;
      --relay)
        relays+=("$2")
        shift 2
        ;;
      publish-kp)
        break
        ;;
      *)
        shift
        ;;
    esac
  done
  count=0
  if [[ -f "${PIKA_TEST_PUBLISH_COUNT_FILE}" ]]; then
    count="$(cat "${PIKA_TEST_PUBLISH_COUNT_FILE}")"
  fi
  count=$((count + 1))
  printf '%s\n' "$count" > "${PIKA_TEST_PUBLISH_COUNT_FILE}"
  printf 'state_dir=%s relays=%s\n' "$state_dir" "${relays[*]:-}" >> "${PIKA_TEST_PUBLISH_LOG_FILE}"
  if [[ "${PIKA_TEST_PUBLISH_ALWAYS_FAIL:-0}" == "1" ]]; then
    exit 1
  fi
  succeed_after="${PIKA_TEST_PUBLISH_SUCCEED_AFTER:-2}"
  if [[ "$count" -lt "$succeed_after" ]]; then
    exit 1
  fi
  printf '%s\n' '{"event_id":"kp-test","kind":443}'
else
  echo "unsupported fake pikachat invocation: $*" >&2
  exit 1
fi
"#,
        );

        let curl_path = bin_dir.join("curl");
        write_executable(
            &curl_path,
            r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "${*}" >> "${PIKA_TEST_CURL_LOG_FILE}"
count=0
if [[ -f "${PIKA_TEST_CURL_COUNT_FILE}" ]]; then
  count="$(cat "${PIKA_TEST_CURL_COUNT_FILE}")"
fi
count=$((count + 1))
printf '%s\n' "$count" > "${PIKA_TEST_CURL_COUNT_FILE}"
IFS=',' read -r -a statuses <<< "${PIKA_TEST_CURL_RESULTS:-}"
if [[ "${#statuses[@]}" -gt 0 ]]; then
  index=$((count - 1))
  if (( index >= ${#statuses[@]} )); then
    index=$((${#statuses[@]} - 1))
  fi
  if [[ "${statuses[$index]}" != "0" ]]; then
    exit 0
  fi
  exit 1
fi
exit 0
"#,
        );

        write_executable(
            &fake_openclaw_path,
            r#"#!/usr/bin/env bash
set -euo pipefail
if [[ -n "${PIKA_TEST_OPENCLAW_READY_LOG_PATH:-}" ]]; then
  printf '%s\n' "openclaw-ready" >> "${PIKA_TEST_OPENCLAW_READY_LOG_PATH}"
fi
trap 'exit 0' TERM INT
while :; do
  sleep 1
done
"#,
        );

        let mut script = microvm_autostart_script();
        script = script.replace(
            &format!("STARTUP_PLAN_PATH=\"/{STARTUP_PLAN_PATH}\""),
            &format!("STARTUP_PLAN_PATH=\"{}\"", startup_plan_path.display()),
        );
        script = script.replace(
            &format!("OPENCLAW_EXTENSION_ROOT=\"/{OPENCLAW_EXTENSION_ROOT}\""),
            &format!(
                "OPENCLAW_EXTENSION_ROOT=\"{}\"",
                openclaw_extension_root.display()
            ),
        );
        let script_path = root.path().join("start-agent.sh");
        write_executable(&script_path, &script);

        let mut command = Command::new("bash");
        command
            .arg(&script_path)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    bin_dir.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("PIKA_PIKACHAT_BIN", &pikachat_path)
            .env(
                "PIKA_OWNER_PUBKEY",
                "2284fc7b932b5dbbdaa2185c76a4e17a2ef928d4a82e29b812986b454b957f8f",
            )
            .env(
                "PIKA_RELAY_URLS",
                "wss://relay-one.example.com,wss://relay-two.example.com",
            )
            // The OpenClaw startup path now always boots the private loopback proxy,
            // so the self-contained test harness must provide a guest IP.
            .env("PIKA_VM_IP", "127.0.0.1")
            .env("PIKA_TEST_READY_LOG_PATH", &ready_log_path)
            .env("PIKA_TEST_PUBLISH_COUNT_FILE", &publish_count_path)
            .env("PIKA_TEST_PUBLISH_LOG_FILE", &publish_log_path)
            .env("PIKA_TEST_CURL_LOG_FILE", root.path().join("curl.log"))
            .env("PIKA_TEST_CURL_COUNT_FILE", &curl_count_path)
            .env(
                "TEST_JQ_READY_MARKER_PATH",
                root_relative(&ready_marker_path),
            )
            .env(
                "TEST_JQ_FAILED_MARKER_PATH",
                root_relative(&failed_marker_path),
            )
            .env(
                "TEST_JQ_IDENTITY_SEED_PATH",
                root_relative(&identity_seed_path),
            )
            .env("TEST_JQ_DAEMON_STATE_DIR", daemon_state_dir.as_os_str())
            .env("TEST_JQ_EXIT_FAILURE_REASON", "service_exited")
            .env(
                "TEST_JQ_TIMEOUT_FAILURE_REASON",
                "timeout_waiting_for_service",
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        match outcome {
            KeypackagePublishOutcome::SucceedsAfterRetry => {
                command.env("PIKA_TEST_PUBLISH_SUCCEED_AFTER", "2");
            }
            KeypackagePublishOutcome::SucceedsAfterReadinessRecovery => {
                command
                    .env("PIKA_TEST_PUBLISH_SUCCEED_AFTER", "1")
                    .env("PIKA_TEST_CURL_RESULTS", "1,0,1");
            }
            KeypackagePublishOutcome::TimesOut { .. } => {
                command.env("PIKA_TEST_PUBLISH_ALWAYS_FAIL", "1").env(
                    "PIKA_AGENT_READY_TIMEOUT_SECS",
                    keypackage_timeout_test_secs(),
                );
            }
        }

        match scenario {
            KeypackageReadyScenario::Pi => {
                command
                    .env("TEST_JQ_AGENT_KIND", "pi")
                    .env("TEST_JQ_SERVICE_KIND", "pikachat_daemon")
                    .env("TEST_JQ_BACKEND_MODE", "acp")
                    .env("TEST_JQ_SERVICE_ACP_PRESENT", "1")
                    .env("TEST_JQ_ACP_EXEC_COMMAND", "npx -y pi-acp")
                    .env("TEST_JQ_ACP_CWD", acp_cwd.as_os_str())
                    .env("TEST_JQ_SERVICE_EXEC_COMMAND", "")
                    .env("TEST_JQ_SERVICE_STATE_DIR", "")
                    .env("TEST_JQ_SERVICE_CONFIG_PATH", "")
                    .env("TEST_JQ_GATEWAY_PORT", "");
                if matches!(outcome, KeypackagePublishOutcome::TimesOut { .. }) {
                    command
                        .env("TEST_JQ_READY_PROBE", "daemon_ready_event")
                        .env("TEST_JQ_READINESS_KIND", "log_contains")
                        .env("TEST_JQ_READINESS_PATH", root_relative(&ready_log_path))
                        .env("TEST_JQ_READINESS_PATTERN", "daemon-ready")
                        .env("TEST_JQ_READINESS_URL", "")
                        .env("PIKA_TEST_DAEMON_READY_LOG_PATH", &ready_log_path);
                } else {
                    command
                        .env("TEST_JQ_READY_PROBE", "daemon_ready_event")
                        .env("TEST_JQ_READINESS_KIND", "log_contains")
                        .env("TEST_JQ_READINESS_PATH", root_relative(&ready_log_path))
                        .env("TEST_JQ_READINESS_PATTERN", "\"type\":\"ready\"")
                        .env("TEST_JQ_READINESS_URL", "");
                }
            }
            KeypackageReadyScenario::Openclaw => {
                let command = command
                    .env("TEST_JQ_AGENT_KIND", "openclaw")
                    .env("TEST_JQ_SERVICE_KIND", "openclaw_gateway")
                    .env("TEST_JQ_BACKEND_MODE", "native")
                    .env("TEST_JQ_SERVICE_ACP_PRESENT", "0")
                    .env("TEST_JQ_ACP_EXEC_COMMAND", "")
                    .env("TEST_JQ_ACP_CWD", "")
                    .env(
                        "TEST_JQ_SERVICE_EXEC_COMMAND",
                        fake_openclaw_path.as_os_str(),
                    )
                    .env(
                        "PIKA_OPENCLAW_PACKAGE_ROOT",
                        openclaw_package_root.as_os_str(),
                    )
                    .env("TEST_JQ_SERVICE_STATE_DIR", openclaw_state_dir.as_os_str())
                    .env(
                        "TEST_JQ_SERVICE_CONFIG_PATH",
                        root_relative(&openclaw_config_path),
                    )
                    .env("TEST_JQ_GATEWAY_PORT", openclaw_gateway_port.to_string())
                    .env("TEST_JQ_READY_PROBE", "openclaw_gateway_health")
                    .env("TEST_JQ_READINESS_KIND", "http_get_ok")
                    .env("TEST_JQ_READINESS_PATH", "")
                    .env("TEST_JQ_READINESS_PATTERN", "")
                    .env(
                        "TEST_JQ_READINESS_URL",
                        format!("http://127.0.0.1:{openclaw_gateway_port}/health"),
                    );
                if matches!(outcome, KeypackagePublishOutcome::TimesOut { .. }) {
                    command.env("PIKA_TEST_CURL_RESULTS", "1");
                }
            }
        }

        let mut child = command.spawn().expect("spawn autostart script");

        match outcome {
            KeypackagePublishOutcome::SucceedsAfterRetry => {
                poll_until(async_test_timeout(), || {
                    read_counter(&publish_count_path) >= 1
                });
                assert!(
                    !ready_marker_path.exists(),
                    "ready marker must not exist after a failed first keypackage publish attempt"
                );
                poll_until(async_test_timeout(), || {
                    read_counter(&publish_count_path) >= 2 && ready_marker_path.exists()
                });

                let publish_log = fs::read_to_string(&publish_log_path).expect("read publish log");
                assert!(
                    publish_log.contains("state_dir="),
                    "publish log should capture the remote state-dir invocation"
                );
                assert!(
                    publish_log.contains("wss://relay-one.example.com wss://relay-two.example.com"),
                    "publish log should include the full relay list"
                );

                Command::new("kill")
                    .arg("-TERM")
                    .arg(child.id().to_string())
                    .status()
                    .expect("terminate autostart script");
                let _ = child.wait().expect("wait for autostart script shutdown");
            }
            KeypackagePublishOutcome::SucceedsAfterReadinessRecovery => {
                poll_until(async_test_timeout(), || read_counter(&curl_count_path) >= 2);
                assert_eq!(
                    read_counter(&publish_count_path),
                    0,
                    "keypackage publish should wait for readiness to recover before retrying"
                );
                assert!(
                    !ready_marker_path.exists(),
                    "ready marker must not exist while readiness has regressed"
                );

                poll_until(async_test_timeout(), || {
                    read_counter(&publish_count_path) >= 1 && ready_marker_path.exists()
                });

                let publish_log = fs::read_to_string(&publish_log_path).expect("read publish log");
                assert!(
                    publish_log.contains("state_dir="),
                    "publish log should capture the remote state-dir invocation"
                );

                Command::new("kill")
                    .arg("-TERM")
                    .arg(child.id().to_string())
                    .status()
                    .expect("terminate autostart script");
                let _ = child.wait().expect("wait for autostart script shutdown");
            }
            KeypackagePublishOutcome::TimesOut { expected_reason } => {
                poll_until(async_test_timeout(), || {
                    read_counter(&publish_count_path) >= 1 || failed_marker_path.exists()
                });
                let publish_count = read_counter(&publish_count_path);
                assert!(
                    publish_count >= 1,
                    "timeout harness should reach keypackage publish before failing; failed marker contents: {}",
                    fs::read_to_string(&failed_marker_path).unwrap_or_default()
                );
                let status = child.wait().expect("wait for autostart script failure");
                assert!(
                    !ready_marker_path.exists(),
                    "ready marker must stay absent when keypackage publish never succeeds"
                );
                let failed_marker =
                    fs::read_to_string(&failed_marker_path).expect("read failed marker");
                if matches!(scenario, KeypackageReadyScenario::Openclaw) {
                    assert!(
                        read_counter(&curl_count_path) >= 1,
                        "OpenClaw timeout harness should still exercise the health-check path"
                    );
                    assert!(
                        !failed_marker.contains("timeout_waiting_for_openclaw_health"),
                        "keypackage timeout test must not fail through the service-timeout path; got {failed_marker}"
                    );
                }
                assert!(
                    failed_marker.contains(expected_reason),
                    "failed marker should report the dedicated keypackage publish timeout reason; got {failed_marker}"
                );
                assert!(
                    !status.success(),
                    "autostart script should exit non-zero when keypackage publication times out"
                );
            }
        }
    }

    #[test]
    fn resolve_params_applies_defaults_and_overrides() {
        let defaults = resolve_params(&MicrovmProvisionParams::default());
        assert_eq!(defaults.spawner_url, DEFAULT_SPAWNER_URL);
        assert_eq!(defaults.kind, ResolvedMicrovmAgentKind::Pi);
        assert_eq!(
            defaults.backend,
            ResolvedMicrovmAgentBackend::Acp {
                exec_command: DEFAULT_ACP_EXEC_COMMAND.to_string(),
                cwd: DEFAULT_ACP_CWD.to_string(),
            }
        );

        let openclaw_defaults = resolve_params(&MicrovmProvisionParams {
            kind: Some(MicrovmAgentKind::Openclaw),
            ..MicrovmProvisionParams::default()
        });
        assert_eq!(openclaw_defaults.kind, ResolvedMicrovmAgentKind::Openclaw);
        assert_eq!(
            openclaw_defaults.backend,
            ResolvedMicrovmAgentBackend::Native
        );

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
                backend: ResolvedMicrovmAgentBackend::Acp {
                    exec_command: DEFAULT_ACP_EXEC_COMMAND.to_string(),
                    cwd: DEFAULT_ACP_CWD.to_string(),
                },
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
        assert_eq!(value["guest_autostart"]["startup_plan"]["agent_kind"], "pi");
        assert_eq!(
            value["guest_autostart"]["startup_plan"]["service_kind"],
            "pikachat_daemon"
        );
        assert_eq!(
            value["guest_autostart"]["startup_plan"]["backend_mode"],
            "acp"
        );
        assert!(value["guest_autostart"]["files"][STARTUP_PLAN_PATH]
            .as_str()
            .expect("startup plan")
            .contains("\"service_kind\": \"pikachat_daemon\""));
        assert!(value["guest_autostart"]["files"][AUTOSTART_SCRIPT_PATH]
            .as_str()
            .expect("autostart script")
            .contains("service_kind"));
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
    fn autostart_script_uses_deterministic_openclaw_binary_path() {
        let script = microvm_autostart_script();
        assert!(
            script.contains("daemon_state_dir"),
            "autostart script must keep state under /root for restart durability"
        );
        assert!(script.contains("STARTUP_PLAN_PATH=\"/workspace/pika-agent/startup-plan.json\""));
        assert!(script.contains("ready_path"));
        assert!(script.contains("failed_path"));
        assert!(script.contains("trap cleanup_agent EXIT TERM INT"));
        assert!(script.contains("startup plan runner requires jq"));
        assert!(script.contains("--state-dir \"$daemon_state_dir\""));
        assert!(script.contains("PIKA_PIKACHAT_BIN"));
        assert!(script.contains("plan_value '.agent_kind'"));
        assert!(script.contains("plan_value '.backend_mode'"));
        assert!(script.contains("wait_for_service_ready"));
        assert!(script.contains("case \"$backend_mode\" in"));
        assert!(script.contains("case \"$readiness_kind\""));
        assert!(script.contains("curl -fsS --max-time 2 \"$readiness_url\""));
        assert!(script.contains("openclaw_gateway)"));
        assert!(script.contains("PIKA_PI_CMD"));
        assert!(script.contains("pi_exec=\"${PIKA_PI_CMD%% *}\""));
        assert!(script.contains("export PATH=\"$(dirname \"$pi_exec\"):$PATH\""));
        assert!(script.contains("openclaw_exec=\"$(plan_value '.service.exec_command')\""));
        assert!(!script.contains("PIKA_OPENCLAW_CMD"));
        assert!(script.contains("openclaw_package_root=\"${PIKA_OPENCLAW_PACKAGE_ROOT:-$(dirname \"$(dirname \"$openclaw_exec\")\")/lib/openclaw}\""));
        assert!(script.contains("OpenClaw package root not found"));
        assert!(script.contains("mkdir -p \"$openclaw_state_dir/node_modules\""));
        assert!(script.contains("rm -rf \"$openclaw_state_dir/node_modules/openclaw\""));
        assert!(script.contains(
            "ln -s \"$openclaw_package_root\" \"$openclaw_state_dir/node_modules/openclaw\""
        ));
        assert!(script.contains("mkdir -p \"$openclaw_workspace_root/node_modules\""));
        assert!(script.contains("rm -rf \"$openclaw_workspace_root/node_modules/openclaw\""));
        assert!(script.contains(
            "ln -s \"$openclaw_package_root\" \"$openclaw_workspace_root/node_modules/openclaw\""
        ));
        assert!(script.contains(
            "export NODE_PATH=\"$openclaw_state_dir/node_modules${NODE_PATH:+:$NODE_PATH}\""
        ));
        assert!(script.contains("export PIKACHAT_DAEMON_CMD=\"$bin\""));
        assert!(script.contains("export PIKACHAT_SIDECAR_CMD=\"$bin\""));
        assert!(script.contains("OpenClaw gateway exec must be a binary path"));
        assert!(script.contains("\"$openclaw_exec\" gateway --allow-unconfigured"));
        assert!(!script.contains("bash -lc \"$openclaw_exec gateway --allow-unconfigured\""));
        assert!(!script.contains("npm_cache_dir="));
        assert!(!script.contains("NPM_CONFIG_CACHE"));
        assert!(script.contains("plan_value '.service.acp_backend.exec_command'"));
        assert!(
            script.contains("invalid startup plan: backend_mode=acp but ACP payload is missing")
        );
        assert!(script.contains("plan_value '.service.exec_command'"));
        assert!(script.contains("start_openclaw_private_proxy"));
        assert!(script.contains(": \"${PIKA_VM_IP:?missing PIKA_VM_IP}\""));
        assert!(
            !script.contains("marmotd"),
            "autostart script must only resolve pikachat daemon binary"
        );
    }

    #[test]
    fn autostart_script_publishes_keypackage_before_marking_ready() {
        let script = microvm_autostart_script();
        assert!(
            script.contains("publish_daemon_keypackage"),
            "autostart script must publish a keypackage before reporting ready"
        );
        assert!(
            script.contains("publish_args=(--remote --state-dir \"$daemon_state_dir\")"),
            "autostart script must publish through the daemon socket"
        );
        assert!(
            script.contains("publish_args+=(\"${relay_args[@]}\")"),
            "autostart script must forward the full relay list when publishing the keypackage"
        );
        assert!(
            script.contains("\"${publish_args[@]}\" publish-kp >/dev/null"),
            "autostart script must invoke remote publish-kp before ready"
        );
        assert!(
            script.contains("if ! ready_probe=\"$(wait_for_service_ready \"$agent_pid\")\"; then"),
            "autostart script must wait for service readiness before publishing the keypackage"
        );
        assert!(
            script
                .contains("if ! wait_for_keypackage_publish \"$agent_pid\" \"$ready_probe\"; then"),
            "autostart script must treat keypackage publication as a distinct startup phase"
        );
        assert!(
            script.contains(
                "service_readiness_probe_succeeds \\\n      \"$readiness_kind\" \\\n      \"$readiness_path\" \\\n      \"$readiness_pattern\" \\\n      \"$readiness_url\" && \\\n      publish_daemon_keypackage"
            ),
            "autostart script must require the readiness probe to still pass before final ready"
        );
        assert!(
            script.contains("timeout_waiting_for_openclaw_keypackage_publish"),
            "autostart script must report a dedicated OpenClaw keypackage publish timeout"
        );
        assert!(
            script.contains("timeout_waiting_for_daemon_keypackage_publish"),
            "autostart script must report a dedicated daemon keypackage publish timeout"
        );
    }

    #[test]
    fn pi_autostart_waits_for_keypackage_publish_before_ready_marker() {
        run_keypackage_ready_gating_scenario(
            KeypackageReadyScenario::Pi,
            KeypackagePublishOutcome::SucceedsAfterRetry,
        );
    }

    #[test]
    fn openclaw_autostart_waits_for_keypackage_publish_before_ready_marker() {
        run_keypackage_ready_gating_scenario(
            KeypackageReadyScenario::Openclaw,
            KeypackagePublishOutcome::SucceedsAfterRetry,
        );
    }

    #[test]
    fn openclaw_autostart_rechecks_health_before_marking_ready() {
        run_keypackage_ready_gating_scenario(
            KeypackageReadyScenario::Openclaw,
            KeypackagePublishOutcome::SucceedsAfterReadinessRecovery,
        );
    }

    #[test]
    fn pi_autostart_reports_keypackage_publish_timeout_separately_from_service_timeout() {
        run_keypackage_ready_gating_scenario(
            KeypackageReadyScenario::Pi,
            KeypackagePublishOutcome::TimesOut {
                expected_reason: "timeout_waiting_for_daemon_keypackage_publish",
            },
        );
    }

    #[test]
    fn openclaw_autostart_reports_keypackage_publish_timeout_separately_from_service_timeout() {
        run_keypackage_ready_gating_scenario(
            KeypackageReadyScenario::Openclaw,
            KeypackagePublishOutcome::TimesOut {
                expected_reason: "timeout_waiting_for_openclaw_keypackage_publish",
            },
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
    fn guest_startup_plan_selects_pi_acp_readiness() {
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
        let startup_plan = req.guest_autostart.startup_plan;
        assert_eq!(startup_plan.service_kind, GuestServiceKind::PikachatDaemon);
        assert_eq!(startup_plan.backend_mode, GuestServiceBackendMode::Acp);
        assert_eq!(
            startup_plan.readiness_check,
            GuestServiceReadinessCheck::LogContains {
                path: AGENT_LOG_PATH.to_string(),
                pattern: "\"type\":\"ready\"".to_string(),
                ready_probe: "daemon_ready_event".to_string(),
                timeout_failure_reason: "timeout_waiting_for_daemon_ready".to_string(),
            }
        );
        assert_eq!(
            startup_plan.service,
            GuestServiceLaunch::PikachatDaemon {
                acp_backend: Some(GuestAcpBackend {
                    exec_command: "npx -y pi-acp".to_string(),
                    cwd: "/root/pika-agent/acp".to_string(),
                }),
            }
        );
    }

    #[test]
    fn build_create_vm_request_includes_openclaw_native_payload() {
        let _lock = openclaw_origin_env_lock()
            .lock()
            .expect("lock openclaw origin env");
        let _env = OpenClawOriginEnvGuard::set("");
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
        let startup_plan = req.guest_autostart.startup_plan.clone();
        assert_eq!(startup_plan.backend_mode, GuestServiceBackendMode::Native);
        assert_eq!(
            startup_plan.service,
            GuestServiceLaunch::OpenclawGateway {
                exec_command: DEFAULT_OPENCLAW_EXEC_COMMAND.to_string(),
                state_dir: DEFAULT_OPENCLAW_STATE_DIR.to_string(),
                config_path: OPENCLAW_CONFIG_PATH.to_string(),
                gateway_port: DEFAULT_OPENCLAW_GATEWAY_PORT,
                daemon_backend: GuestOpenclawDaemonBackend::Native,
            }
        );

        let value = serde_json::to_value(req).expect("serialize create vm request");
        assert_eq!(
            value["guest_autostart"]["startup_plan"]["agent_kind"],
            "openclaw"
        );
        assert_eq!(
            value["guest_autostart"]["startup_plan"]["service_kind"],
            "openclaw_gateway"
        );
        assert_eq!(
            value["guest_autostart"]["startup_plan"]["service"]["exec_command"],
            DEFAULT_OPENCLAW_EXEC_COMMAND
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
            openclaw_json["gateway"]["controlUi"]["allowedOrigins"],
            serde_json::json!([
                DEFAULT_OPENCLAW_CONTROL_UI_ORIGIN,
                DEFAULT_OPENCLAW_CONTROL_UI_HTTPS_ORIGIN
            ])
        );
        assert_eq!(
            openclaw_json["gateway"]["controlUi"]["allowInsecureAuth"],
            serde_json::json!(true)
        );
        assert_eq!(
            openclaw_json["gateway"]["controlUi"]["dangerouslyDisableDeviceAuth"],
            serde_json::json!(true)
        );
        assert_eq!(
            openclaw_json["gateway"]["trustedProxies"],
            serde_json::json!(["127.0.0.1", "::1"])
        );
        assert_eq!(
            openclaw_json["channels"]["pikachat-openclaw"]["daemonBackend"],
            "native"
        );
        assert_eq!(
            value["guest_autostart"]["startup_plan"]["readiness_check"]["kind"],
            "http_get_ok"
        );
        assert!(value["guest_autostart"]["files"]
            .as_object()
            .expect("files map")
            .contains_key(&format!("{OPENCLAW_EXTENSION_ROOT}/package.json")));
    }

    #[test]
    fn build_create_vm_request_includes_openclaw_acp_payload() {
        let _lock = openclaw_origin_env_lock()
            .lock()
            .expect("lock openclaw origin env");
        let _env = OpenClawOriginEnvGuard::set("");
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
                backend: ResolvedMicrovmAgentBackend::Acp {
                    exec_command: "npx -y pi-acp".to_string(),
                    cwd: "/root/pika-agent/acp".to_string(),
                },
            },
        );
        let startup_plan = req.guest_autostart.startup_plan.clone();
        assert_eq!(startup_plan.backend_mode, GuestServiceBackendMode::Acp);
        assert_eq!(
            startup_plan.service,
            GuestServiceLaunch::OpenclawGateway {
                exec_command: DEFAULT_OPENCLAW_EXEC_COMMAND.to_string(),
                state_dir: DEFAULT_OPENCLAW_STATE_DIR.to_string(),
                config_path: OPENCLAW_CONFIG_PATH.to_string(),
                gateway_port: DEFAULT_OPENCLAW_GATEWAY_PORT,
                daemon_backend: GuestOpenclawDaemonBackend::Acp {
                    acp_backend: GuestAcpBackend {
                        exec_command: "npx -y pi-acp".to_string(),
                        cwd: "/root/pika-agent/acp".to_string(),
                    },
                },
            }
        );

        let value = serde_json::to_value(req).expect("serialize create vm request");
        assert_eq!(
            value["guest_autostart"]["startup_plan"]["service"]["exec_command"],
            DEFAULT_OPENCLAW_EXEC_COMMAND
        );
        let openclaw_config = value["guest_autostart"]["files"][OPENCLAW_CONFIG_PATH]
            .as_str()
            .expect("openclaw config");
        let openclaw_json: serde_json::Value =
            serde_json::from_str(openclaw_config).expect("parse openclaw config");
        assert_eq!(
            openclaw_json["gateway"]["controlUi"]["allowedOrigins"],
            serde_json::json!([
                DEFAULT_OPENCLAW_CONTROL_UI_ORIGIN,
                DEFAULT_OPENCLAW_CONTROL_UI_HTTPS_ORIGIN
            ])
        );
        assert_eq!(
            openclaw_json["gateway"]["controlUi"]["allowInsecureAuth"],
            serde_json::json!(true)
        );
        assert_eq!(
            openclaw_json["gateway"]["controlUi"]["dangerouslyDisableDeviceAuth"],
            serde_json::json!(true)
        );
        assert_eq!(
            openclaw_json["gateway"]["trustedProxies"],
            serde_json::json!(["127.0.0.1", "::1"])
        );
        assert_eq!(
            openclaw_json["channels"]["pikachat-openclaw"]["daemonBackend"],
            "acp"
        );
        assert_eq!(
            openclaw_json["channels"]["pikachat-openclaw"]["daemonAcpExec"],
            "npx -y pi-acp"
        );
        assert_eq!(
            openclaw_json["channels"]["pikachat-openclaw"]["daemonAcpCwd"],
            "/root/pika-agent/acp"
        );
    }

    #[test]
    fn openclaw_gateway_config_includes_env_control_ui_origin_overrides() {
        let _lock = openclaw_origin_env_lock()
            .lock()
            .expect("lock openclaw origin env");
        let _env = OpenClawOriginEnvGuard::set("https://openclaw.api.pikachat.org");
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

        let openclaw_config = req.guest_autostart.files[OPENCLAW_CONFIG_PATH].as_str();
        let openclaw_json: serde_json::Value =
            serde_json::from_str(openclaw_config).expect("parse openclaw config");
        assert_eq!(
            openclaw_json["gateway"]["controlUi"]["allowedOrigins"],
            serde_json::json!([
                DEFAULT_OPENCLAW_CONTROL_UI_ORIGIN,
                DEFAULT_OPENCLAW_CONTROL_UI_HTTPS_ORIGIN,
                "https://openclaw.api.pikachat.org"
            ])
        );
        assert_eq!(
            openclaw_json["gateway"]["controlUi"]["allowInsecureAuth"],
            serde_json::json!(true)
        );
        assert_eq!(
            openclaw_json["gateway"]["controlUi"]["dangerouslyDisableDeviceAuth"],
            serde_json::json!(true)
        );
        assert_eq!(
            openclaw_json["gateway"]["trustedProxies"],
            serde_json::json!(["127.0.0.1", "::1"])
        );
    }

    #[test]
    fn openclaw_extension_file_list_matches_source_tree() {
        fn openclaw_extension_source_root() -> PathBuf {
            std::env::var_os("PIKACI_OPENCLAW_EXTENSION_SOURCE_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../../pikachat-openclaw/openclaw/extensions/pikachat-openclaw")
                })
        }

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

        let expected: BTreeSet<String> = expected_openclaw_extension_paths()
            .iter()
            .map(|path| path.to_string())
            .collect();
        let actual = collect_relative_files(&openclaw_extension_source_root())
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
                startup_plan: test_guest_startup_plan(),
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
    async fn restore_vm_contract_request_shape() {
        let (base_url, rx) =
            spawn_one_shot_server("200 OK", r#"{"id":"vm-restore-1","status":"starting"}"#);
        let client = MicrovmSpawnerClient::new(base_url);

        let restored = client
            .restore_vm_with_request_id("vm-restore-1", Some("req-restore-123"))
            .await
            .expect("restore vm succeeds");
        assert_eq!(restored.id, "vm-restore-1");
        assert_eq!(restored.status, "starting");

        let captured = rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("captured request");
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.path, "/vms/vm-restore-1/restore");
        assert_eq!(
            captured.headers.get(REQUEST_ID_HEADER).map(String::as_str),
            Some("req-restore-123")
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
        assert!(!vm.startup_probe_satisfied);
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
    async fn get_vm_backup_status_contract_request_shape() {
        let (base_url, rx) = spawn_one_shot_server(
            "200 OK",
            r#"{"vm_id":"vm-123","backup_unit_kind":"durable_home","backup_target":"/var/lib/microvms/vm-123/home","recovery_point_kind":"metadata_record","freshness":"healthy","latest_recovery_point_name":null,"latest_successful_backup_at":"2026-03-11T00:00:00Z","observed_at":"2026-03-11T00:00:00Z"}"#,
        );
        let client = MicrovmSpawnerClient::new(base_url);

        let status = client
            .get_vm_backup_status_with_request_id("vm-123", Some("req-backup-123"))
            .await
            .expect("get backup status succeeds");
        assert_eq!(status.vm_id, "vm-123");
        assert_eq!(status.backup_target, "/var/lib/microvms/vm-123/home");

        let captured = rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("captured request");
        assert_eq!(captured.method, "GET");
        assert_eq!(captured.path, "/vms/vm-123/backup-status");
        assert_eq!(
            captured.headers.get(REQUEST_ID_HEADER).map(String::as_str),
            Some("req-backup-123")
        );
        assert!(captured.body.is_empty());
    }

    #[tokio::test]
    async fn get_openclaw_launch_auth_contract_request_shape() {
        let (base_url, rx) = spawn_one_shot_server(
            "200 OK",
            r#"{"vm_id":"vm-123","gateway_auth_token":"launch-token-123"}"#,
        );
        let client = MicrovmSpawnerClient::new(base_url);

        let auth = client
            .get_openclaw_launch_auth_with_request_id("vm-123", Some("req-launch-auth-123"))
            .await
            .expect("get launch auth succeeds");
        assert_eq!(auth.vm_id, "vm-123");
        assert_eq!(auth.gateway_auth_token.as_deref(), Some("launch-token-123"));

        let captured = rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("captured request");
        assert_eq!(captured.method, "GET");
        assert_eq!(captured.path, "/vms/vm-123/openclaw/launch-auth");
        assert_eq!(
            captured.headers.get(REQUEST_ID_HEADER).map(String::as_str),
            Some("req-launch-auth-123")
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
                startup_plan: test_guest_startup_plan(),
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

    #[tokio::test]
    async fn restore_vm_surfaces_error_body() {
        let (base_url, _rx) =
            spawn_one_shot_server("500 Internal Server Error", "restic restore failed");
        let client = MicrovmSpawnerClient::new(base_url);

        let err = client
            .restore_vm("vm-bad")
            .await
            .expect_err("expected restore_vm failure");
        let msg = err.to_string();
        assert!(msg.contains("failed to restore vm vm-bad"));
        assert!(msg.contains("500 Internal Server Error"));
        assert!(msg.contains("restic restore failed"));
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
