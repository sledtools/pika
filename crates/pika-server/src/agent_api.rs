use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{anyhow, Context};
use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::{response::IntoResponse, Json};
use base64::Engine;
use diesel::result::{DatabaseErrorKind, Error as DieselError};
use diesel::Connection;
use diesel::PgConnection;
use ipnet::Ipv4Net;
use nostr_sdk::prelude::{Keys, PublicKey};
use nostr_sdk::ToBech32;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::agent_api_v1_contract::{
    AgentApiErrorCode, AgentAppState, V1_AGENTS_ENSURE_PATH, V1_AGENTS_ME_PATH,
    V1_AGENTS_RECOVER_PATH,
};
use crate::managed_openclaw_guest::{
    build_managed_vm_create_request, ManagedVmCreateInput as ManagedRuntimeCreateInput,
    DEFAULT_OPENCLAW_GATEWAY_PORT,
};
use crate::models::agent_allowlist::AgentAllowlistEntry;
use crate::models::agent_instance::{
    AgentInstance, AGENT_PHASE_CREATING, AGENT_PHASE_ERROR, AGENT_PHASE_READY,
};
use crate::models::managed_environment_event::ManagedEnvironmentEvent;
use crate::nostr_auth::{
    event_from_authorization_header, expected_host_from_headers, verify_nip98_event,
};
use crate::{RequestContext, State};
use pika_cloud::{
    incus_mount_device_config, incus_runtime_config, AgentProvisionRequest, AgentStartupPhase,
    IncusProvisionParams, IncusRuntimeConfig, IncusRuntimePlan, LifecycleState,
    ManagedOpenClawLaunchAuth, ManagedRuntimeBackupStatus, ManagedRuntimeStatus,
    ManagedVmProvisionParams as ManagedRuntimeProvisionParams, MountKind, MountMode,
    RuntimeArtifacts, RuntimeIdentity, RuntimeMount, RuntimeResources, RuntimeSpec,
    RuntimeStatusSnapshot, VmBackupFreshness, VmBackupUnitKind, VmRecoveryPointKind, STATUS_PATH,
};
use pika_relay_profiles::default_message_relays;

const AGENT_OWNER_ACTIVE_INDEX: &str = "agent_instances_owner_active_idx";
const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const INCUS_ENDPOINT_ENV: &str = "PIKA_AGENT_INCUS_ENDPOINT";
const INCUS_PROJECT_ENV: &str = "PIKA_AGENT_INCUS_PROJECT";
const INCUS_PROFILE_ENV: &str = "PIKA_AGENT_INCUS_PROFILE";
const INCUS_STORAGE_POOL_ENV: &str = "PIKA_AGENT_INCUS_STORAGE_POOL";
const INCUS_IMAGE_ALIAS_ENV: &str = "PIKA_AGENT_INCUS_IMAGE_ALIAS";
const INCUS_INSECURE_TLS_ENV: &str = "PIKA_AGENT_INCUS_INSECURE_TLS";
const INCUS_CLIENT_CERT_PATH_ENV: &str = "PIKA_AGENT_INCUS_CLIENT_CERT_PATH";
const INCUS_CLIENT_KEY_PATH_ENV: &str = "PIKA_AGENT_INCUS_CLIENT_KEY_PATH";
const INCUS_SERVER_CERT_PATH_ENV: &str = "PIKA_AGENT_INCUS_SERVER_CERT_PATH";
const INCUS_OPENCLAW_GUEST_IPV4_CIDR_ENV: &str = "PIKA_AGENT_INCUS_OPENCLAW_GUEST_IPV4_CIDR";
const INCUS_OPENCLAW_PROXY_HOST_ENV: &str = "PIKA_AGENT_INCUS_OPENCLAW_PROXY_HOST";
const INCUS_VM_KIND: &str = "virtual-machine";
const INCUS_PERSISTENT_VOLUME_TYPE: &str = "custom";
const INCUS_PERSISTENT_VOLUME_CONTENT_TYPE: &str = "filesystem";
const INCUS_PERSISTENT_VOLUME_PATH: &str = "/mnt/pika-state";
const INCUS_DEV_VM_MEMORY_MIB: u32 = 2048;
const INCUS_CLOUD_INIT_USER_DATA_KEY: &str = "cloud-init.user-data";
const INCUS_OPENCLAW_PROXY_DEVICE_NAME: &str = "pikaopenclaw";
const INCUS_OPENCLAW_PROXY_PORT_START: u16 = 24000;
const INCUS_OPENCLAW_PROXY_PORT_SPAN: u16 = 10000;
const INCUS_OPENCLAW_PROXY_HOST_CONFIG_KEY: &str = "user.pika.openclaw_proxy_host";
const INCUS_OPENCLAW_PROXY_PORT_CONFIG_KEY: &str = "user.pika.openclaw_proxy_port";
const INCUS_OPENCLAW_GUEST_IPV4_CONFIG_KEY: &str = "user.pika.openclaw_guest_ipv4";
const INCUS_PRIMARY_NIC_DEVICE_NAME: &str = "eth0";
const INCUS_BOOTSTRAP_LAUNCHER_PATH: &str = "/workspace/pika-agent/incus-launcher.sh";
const INCUS_STATE_VOLUME_SETUP_PATH: &str = "/workspace/pika-agent/incus-state-volume-setup.sh";
const INCUS_PERSISTENT_AGENT_STATE_ROOT: &str = "/mnt/pika-state/pika-agent";
const INCUS_PERSISTENT_DAEMON_STATE_DIR: &str = "/mnt/pika-state/pika-agent/state";
const INCUS_PERSISTENT_OPENCLAW_STATE_DIR: &str = "/mnt/pika-state/pika-agent/openclaw";
const INCUS_OPERATION_WAIT_TIMEOUT_SECS: i64 = 60;
const INCUS_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const INCUS_BACKUP_HEALTHY_MAX_AGE_HOURS: i64 = 24;
const INCUS_GUEST_BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";
const EVENT_PROVISION_REQUESTED: &str = "provision_requested";
const EVENT_PROVISION_ACCEPTED: &str = "provision_accepted";
const EVENT_RECOVER_REQUESTED: &str = "recover_requested";
const EVENT_RECOVER_SUCCEEDED: &str = "recover_succeeded";
const EVENT_RECOVER_FELL_BACK_TO_FRESH: &str = "recover_fell_back_to_fresh";
const EVENT_RESET_REQUESTED: &str = "reset_requested";
const EVENT_RESET_DESTROYED_OLD_VM: &str = "reset_destroyed_old_vm";
const EVENT_RESET_CONTINUED_MISSING_VM: &str = "reset_continued_missing_vm";
const EVENT_RESTORE_REQUESTED: &str = "restore_requested";
const EVENT_RESTORE_SUCCEEDED: &str = "restore_succeeded";
const EVENT_RESTORE_FAILED: &str = "restore_failed";
const EVENT_READINESS_REFRESH_MISSING_VM: &str = "readiness_refresh_missing_vm";

#[derive(Debug)]
pub struct AgentApiError {
    status: StatusCode,
    code: AgentApiErrorCode,
    request_id: Option<String>,
}

impl AgentApiError {
    fn from_code(code: AgentApiErrorCode) -> Self {
        Self {
            status: code.status_code(),
            code,
            request_id: None,
        }
    }

    fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    pub(crate) fn status_code(&self) -> StatusCode {
        self.status
    }

    pub(crate) fn error_code(&self) -> &'static str {
        self.code.as_str()
    }

    pub(crate) fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }
}

impl IntoResponse for AgentApiError {
    fn into_response(self) -> axum::response::Response {
        if let Some(request_id) = self.request_id.as_deref() {
            let code = self.code.as_str();
            if self.status.is_server_error() {
                tracing::error!(
                    request_id,
                    status = self.status.as_u16(),
                    error_code = code,
                    "agent api request failed"
                );
            } else {
                tracing::warn!(
                    request_id,
                    status = self.status.as_u16(),
                    error_code = code,
                    "agent api request failed"
                );
            }
        }
        let body = Json(serde_json::json!({
            "error": self.code.as_str(),
            "request_id": self.request_id,
        }));
        (self.status, body).into_response()
    }
}

#[derive(Debug, Clone)]
struct RequesterIdentity {
    owner_npub: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentStateResponse {
    agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    vm_id: Option<String>,
    state: AgentAppState,
    startup_phase: AgentStartupPhase,
}

#[derive(Debug, Clone)]
pub(crate) struct ManagedEnvironmentAction {
    pub row: AgentInstance,
    pub startup_phase: AgentStartupPhase,
}

#[derive(Debug, Clone)]
pub(crate) struct ManagedEnvironmentStatus {
    pub row: Option<AgentInstance>,
    pub app_state: Option<AgentAppState>,
    pub startup_phase: Option<AgentStartupPhase>,
    pub environment_exists: bool,
    pub status_copy: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ManagedEnvironmentBackupFreshness {
    NotProvisioned,
    Healthy,
    Stale,
    Missing,
    Unavailable,
}

#[derive(Debug, Clone)]
pub(crate) struct ManagedEnvironmentBackupStatus {
    pub freshness: ManagedEnvironmentBackupFreshness,
    pub backup_target: Option<String>,
    pub backup_target_label: String,
    pub latest_recovery_point_name: Option<String>,
    pub latest_successful_backup_at: Option<String>,
    pub status_copy: String,
    pub reset_requires_confirmation: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ManagedEnvironmentHandle {
    pub owner_npub: String,
    pub agent_id: String,
    pub vm_id: String,
    pub incus: ManagedRuntimeProvisionParams,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct OpenClawProxyTarget {
    pub base_url: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ResolvedIncusParams {
    endpoint: String,
    project: String,
    profile: String,
    storage_pool: String,
    image_alias: String,
    insecure_tls: bool,
    openclaw_guest_ipv4_cidr: Option<String>,
    openclaw_proxy_host: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ResolvedIncusTlsConfig {
    client_cert_path: Option<String>,
    client_key_path: Option<String>,
    server_cert_path: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum ResolvedManagedRuntimeProviderConfig {
    Incus(ResolvedIncusParams),
}

#[derive(Debug, Clone)]
enum ManagedRuntimeProvider {
    Incus(IncusManagedRuntimeProvider),
}

#[derive(Debug, Clone)]
struct IncusManagedRuntimeProvider {
    client: reqwest::Client,
    resolved: ResolvedIncusParams,
}

#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct IncusResponseEnvelope<T> {
    #[serde(rename = "type")]
    _response_type: String,
    #[serde(default)]
    metadata: Option<T>,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct IncusOperationMetadata {
    #[serde(default)]
    err: String,
}

#[derive(Debug, Default, Deserialize)]
struct IncusExecOperationMetadata {
    #[serde(default)]
    err: String,
    #[serde(default)]
    metadata: IncusExecOperationResult,
}

#[derive(Debug, Default, Deserialize)]
struct IncusExecOperationResult {
    #[serde(default, rename = "return")]
    return_code: Option<i64>,
    #[serde(default)]
    output: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct IncusInstanceState {
    status: String,
    #[serde(default)]
    network: Option<BTreeMap<String, IncusInstanceNetwork>>,
}

#[derive(Debug, Clone, Deserialize)]
struct IncusStorageVolumeSnapshot {
    name: String,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct IncusInstanceNetwork {
    #[serde(default)]
    addresses: Vec<IncusInstanceNetworkAddress>,
}

#[derive(Debug, Default, Deserialize)]
struct IncusInstanceNetworkAddress {
    #[serde(default)]
    address: String,
    #[serde(default)]
    family: String,
    #[serde(default)]
    scope: String,
}

#[derive(Debug, Default, Deserialize)]
struct IncusInstanceDetails {
    #[serde(default)]
    name: String,
    #[serde(default)]
    config: BTreeMap<String, String>,
    #[serde(default)]
    devices: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct ManagedGuestLifecycleDetails {
    #[serde(default)]
    agent_kind: Option<String>,
    #[serde(default)]
    backend_mode: Option<String>,
    #[serde(default)]
    service_kind: Option<String>,
    #[serde(default)]
    probe: Option<String>,
    #[serde(default)]
    service_probe_satisfied: bool,
    #[serde(default)]
    failure_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
struct ManagedGuestLifecycleSignal {
    startup_probe_satisfied: bool,
    guest_ready: bool,
    failed: bool,
}

#[derive(Debug, Default, Deserialize)]
struct IncusOpenClawConfigFile {
    #[serde(default)]
    gateway: Option<IncusOpenClawGatewayConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct IncusOpenClawGatewayConfig {
    #[serde(default)]
    auth: Option<IncusOpenClawAuthConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct IncusOpenClawAuthConfig {
    #[serde(default)]
    token: Option<String>,
}

fn materialized_managed_runtime_params(
    config: &ResolvedManagedRuntimeProviderConfig,
) -> ManagedRuntimeProvisionParams {
    match config {
        ResolvedManagedRuntimeProviderConfig::Incus(resolved) => ManagedRuntimeProvisionParams {
            incus: IncusProvisionParams {
                endpoint: Some(resolved.endpoint.clone()),
                project: Some(resolved.project.clone()),
                profile: Some(resolved.profile.clone()),
                storage_pool: Some(resolved.storage_pool.clone()),
                image_alias: Some(resolved.image_alias.clone()),
                insecure_tls: Some(resolved.insecure_tls),
                openclaw_guest_ipv4_cidr: resolved.openclaw_guest_ipv4_cidr.clone(),
                openclaw_proxy_host: resolved.openclaw_proxy_host.clone(),
            },
        },
    }
}

fn serialize_managed_runtime_incus_config(
    config: &ResolvedManagedRuntimeProviderConfig,
) -> anyhow::Result<String> {
    serde_json::to_string(&materialized_managed_runtime_params(config).incus)
        .context("serialize managed runtime incus config")
}

fn row_requires_manual_cleanup(row: &AgentInstance) -> bool {
    row.incus_config.is_none()
}

fn managed_runtime_params_from_row(
    row: &AgentInstance,
) -> anyhow::Result<ManagedRuntimeProvisionParams> {
    match row.incus_config.as_deref() {
        Some(serialized) => serde_json::from_str::<IncusProvisionParams>(serialized)
            .map(|incus| ManagedRuntimeProvisionParams { incus })
            .context("decode managed runtime incus config from row"),
        None => Err(anyhow!(
            "managed-agent row {} lacks incus_config; pre-cut legacy rows must be cleaned up manually before deploy",
            row.agent_id
        )),
    }
}

fn merge_incus_provision_params(
    base: Option<IncusProvisionParams>,
    requested: Option<&IncusProvisionParams>,
) -> Option<IncusProvisionParams> {
    let mut merged = base.unwrap_or_default();
    let mut changed = false;
    if let Some(requested) = requested {
        if requested
            .endpoint
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            merged.endpoint = requested.endpoint.clone();
            changed = true;
        }
        if requested
            .project
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            merged.project = requested.project.clone();
            changed = true;
        }
        if requested
            .profile
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            merged.profile = requested.profile.clone();
            changed = true;
        }
        if requested
            .storage_pool
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            merged.storage_pool = requested.storage_pool.clone();
            changed = true;
        }
        if requested
            .image_alias
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            merged.image_alias = requested.image_alias.clone();
            changed = true;
        }
        if requested.insecure_tls.is_some() {
            merged.insecure_tls = requested.insecure_tls;
            changed = true;
        }
        if requested
            .openclaw_guest_ipv4_cidr
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            merged.openclaw_guest_ipv4_cidr = requested.openclaw_guest_ipv4_cidr.clone();
            changed = true;
        }
        if requested
            .openclaw_proxy_host
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            merged.openclaw_proxy_host = requested.openclaw_proxy_host.clone();
            changed = true;
        }
    }
    if changed
        || merged.endpoint.is_some()
        || merged.project.is_some()
        || merged.profile.is_some()
        || merged.storage_pool.is_some()
        || merged.image_alias.is_some()
        || merged.insecure_tls.is_some()
        || merged.openclaw_proxy_host.is_some()
    {
        Some(merged)
    } else {
        None
    }
}

fn managed_runtime_params_for_existing_row(
    row: &AgentInstance,
    requested: Option<&ManagedRuntimeProvisionParams>,
) -> anyhow::Result<ManagedRuntimeProvisionParams> {
    let mut params = managed_runtime_params_from_row(row)?;
    if row.incus_config.is_some() || requested.is_none() {
        return Ok(params);
    }

    let requested = requested.expect("checked above");
    params.incus = merge_incus_provision_params(Some(params.incus), Some(&requested.incus))
        .unwrap_or_default();
    Ok(params)
}

fn managed_runtime_provider_for_row(
    row: &AgentInstance,
    requested: Option<&ManagedRuntimeProvisionParams>,
) -> anyhow::Result<ManagedRuntimeProvider> {
    let params = managed_runtime_params_for_existing_row(row, requested)?;
    managed_runtime_provider(Some(&params))
}

fn record_managed_environment_event(
    conn: &mut PgConnection,
    owner_npub: &str,
    agent_id: Option<&str>,
    vm_id: Option<&str>,
    event_kind: &str,
    message: &str,
    request_id: &str,
) -> Result<ManagedEnvironmentEvent, AgentApiError> {
    insert_managed_environment_event(
        conn,
        owner_npub,
        agent_id,
        vm_id,
        event_kind,
        message,
        Some(request_id),
    )
    .map_err(|err| {
        tracing::error!(
            request_id = %request_id,
            owner_npub = %owner_npub,
            agent_id,
            vm_id,
            event_kind = %event_kind,
            error = %err,
            "failed to record managed environment event"
        );
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })
}

fn insert_managed_environment_event(
    conn: &mut PgConnection,
    owner_npub: &str,
    agent_id: Option<&str>,
    vm_id: Option<&str>,
    event_kind: &str,
    message: &str,
    request_id: Option<&str>,
) -> anyhow::Result<ManagedEnvironmentEvent> {
    ManagedEnvironmentEvent::record(
        conn, owner_npub, agent_id, vm_id, event_kind, message, request_id,
    )
}

pub(crate) fn list_recent_managed_environment_events(
    state: &State,
    owner_npub: &str,
    limit: i64,
    request_id: &str,
) -> Result<Vec<ManagedEnvironmentEvent>, AgentApiError> {
    let mut conn = state.db_pool.get().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    ManagedEnvironmentEvent::list_recent_by_owner(&mut conn, owner_npub, limit).map_err(|err| {
        tracing::error!(
            request_id = %request_id,
            owner_npub = %owner_npub,
            error = %err,
            "failed to load recent managed environment events"
        );
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })
}

fn non_empty_env_var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn default_incus_params_from_env() -> IncusProvisionParams {
    IncusProvisionParams {
        endpoint: non_empty_env_var(INCUS_ENDPOINT_ENV),
        project: non_empty_env_var(INCUS_PROJECT_ENV),
        profile: non_empty_env_var(INCUS_PROFILE_ENV),
        storage_pool: non_empty_env_var(INCUS_STORAGE_POOL_ENV),
        image_alias: non_empty_env_var(INCUS_IMAGE_ALIAS_ENV),
        insecure_tls: std::env::var(INCUS_INSECURE_TLS_ENV)
            .ok()
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true")),
        openclaw_guest_ipv4_cidr: non_empty_env_var(INCUS_OPENCLAW_GUEST_IPV4_CIDR_ENV),
        openclaw_proxy_host: non_empty_env_var(INCUS_OPENCLAW_PROXY_HOST_ENV),
    }
}

fn build_incus_http_client(resolved: &ResolvedIncusParams) -> anyhow::Result<reqwest::Client> {
    let tls = resolved_incus_tls_config(resolved)?;
    let mut builder = reqwest::Client::builder()
        .timeout(INCUS_HTTP_TIMEOUT)
        .use_rustls_tls()
        .danger_accept_invalid_certs(resolved.insecure_tls);

    if let Some(server_cert_path) = tls.server_cert_path.as_deref() {
        let server_cert_pem = fs::read(server_cert_path)
            .with_context(|| format!("read incus server certificate from {server_cert_path}"))?;
        let server_cert = reqwest::Certificate::from_pem(&server_cert_pem)
            .with_context(|| format!("parse incus server certificate from {server_cert_path}"))?;
        builder = builder.add_root_certificate(server_cert);
    }

    if let (Some(client_cert_path), Some(client_key_path)) = (
        tls.client_cert_path.as_deref(),
        tls.client_key_path.as_deref(),
    ) {
        let mut identity_pem = fs::read(client_cert_path)
            .with_context(|| format!("read incus client certificate from {client_cert_path}"))?;
        if !identity_pem.ends_with(b"\n") {
            identity_pem.push(b'\n');
        }
        let client_key_pem = fs::read(client_key_path)
            .with_context(|| format!("read incus client key from {client_key_path}"))?;
        identity_pem.extend_from_slice(&client_key_pem);
        let identity = reqwest::Identity::from_pem(&identity_pem).with_context(|| {
            format!(
                "parse incus client identity from {} and {}",
                client_cert_path, client_key_path
            )
        })?;
        builder = builder.identity(identity);
    }

    builder.build().context("build incus client")
}

fn resolved_incus_tls_config(
    resolved: &ResolvedIncusParams,
) -> anyhow::Result<ResolvedIncusTlsConfig> {
    let client_cert_path = non_empty_env_var(INCUS_CLIENT_CERT_PATH_ENV);
    let client_key_path = non_empty_env_var(INCUS_CLIENT_KEY_PATH_ENV);
    anyhow::ensure!(
        client_cert_path.is_some() == client_key_path.is_some(),
        "{INCUS_CLIENT_CERT_PATH_ENV} and {INCUS_CLIENT_KEY_PATH_ENV} must either both be set or both be unset"
    );
    anyhow::ensure!(
        client_cert_path.is_some() || !resolved.endpoint.starts_with("https://"),
        "incus https endpoint {} requires both {INCUS_CLIENT_CERT_PATH_ENV} and {INCUS_CLIENT_KEY_PATH_ENV}",
        resolved.endpoint
    );
    Ok(ResolvedIncusTlsConfig {
        client_cert_path,
        client_key_path,
        server_cert_path: non_empty_env_var(INCUS_SERVER_CERT_PATH_ENV),
    })
}

fn incus_params_provided(params: &IncusProvisionParams) -> bool {
    [
        params.endpoint.as_deref(),
        params.project.as_deref(),
        params.profile.as_deref(),
        params.storage_pool.as_deref(),
        params.image_alias.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .any(|value| !value.is_empty())
        || params.insecure_tls.is_some()
        || params.openclaw_guest_ipv4_cidr.is_some()
        || params.openclaw_proxy_host.is_some()
}

fn required_non_empty_field(
    value: Option<String>,
    field_name: &str,
    env_name: &str,
) -> anyhow::Result<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .with_context(|| format!("missing {field_name}; set request.{field_name} or {env_name}"))
}

impl ManagedRuntimeProvider {
    fn ensure_customer_openclaw_flow_supported(&self) -> anyhow::Result<()> {
        match self {
            Self::Incus(provider) => provider.ensure_customer_openclaw_flow_supported(),
        }
    }

    async fn get_openclaw_proxy_target(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<OpenClawProxyTarget> {
        match self {
            Self::Incus(provider) => provider.get_openclaw_proxy_target(vm_id, request_id).await,
        }
    }

    async fn create_managed_vm(
        &self,
        input: ManagedRuntimeCreateInput<'_>,
        request_id: Option<&str>,
    ) -> anyhow::Result<ManagedRuntimeStatus> {
        match self {
            Self::Incus(provider) => provider.create_managed_vm(input, request_id).await,
        }
    }

    async fn get_vm_status(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<ManagedRuntimeStatus> {
        match self {
            Self::Incus(provider) => provider.get_vm_status(vm_id, request_id).await,
        }
    }

    async fn get_vm_backup_status(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<ManagedRuntimeBackupStatus> {
        match self {
            Self::Incus(provider) => provider.get_vm_backup_status(vm_id, request_id).await,
        }
    }

    async fn recover_vm(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<ManagedRuntimeStatus> {
        match self {
            Self::Incus(provider) => provider.recover_vm(vm_id, request_id).await,
        }
    }

    async fn restore_vm(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<ManagedRuntimeStatus> {
        match self {
            Self::Incus(provider) => provider.restore_vm(vm_id, request_id).await,
        }
    }

    async fn delete_vm(&self, vm_id: &str, request_id: Option<&str>) -> anyhow::Result<()> {
        match self {
            Self::Incus(provider) => provider.delete_vm(vm_id, request_id).await,
        }
    }

    async fn get_openclaw_launch_auth(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<ManagedOpenClawLaunchAuth> {
        match self {
            Self::Incus(provider) => provider.get_openclaw_launch_auth(vm_id, request_id).await,
        }
    }
}

impl IncusManagedRuntimeProvider {
    fn new(resolved: ResolvedIncusParams) -> anyhow::Result<Self> {
        let client = build_incus_http_client(&resolved)?;
        Ok(Self { client, resolved })
    }

    async fn healthcheck(&self) -> anyhow::Result<()> {
        let _: serde_json::Value = self
            .get_json(
                &["1.0", "projects", &self.resolved.project],
                false,
                None,
                "load configured incus project",
            )
            .await?;
        let profile: serde_json::Value = self
            .get_json(
                &["1.0", "profiles", &self.resolved.profile],
                true,
                None,
                "load configured incus profile",
            )
            .await?;
        anyhow::ensure!(
            incus_profile_has_nic_device(&profile),
            "configured incus profile {} in project {} must include at least one nic device",
            self.resolved.profile,
            self.resolved.project
        );
        let _: serde_json::Value = self
            .get_json(
                &["1.0", "storage-pools", &self.resolved.storage_pool],
                true,
                None,
                "load configured incus storage pool",
            )
            .await?;
        let _: serde_json::Value = self
            .get_json(
                &["1.0", "images", "aliases", &self.resolved.image_alias],
                true,
                None,
                "load configured incus image alias",
            )
            .await?;
        self.ensure_customer_openclaw_flow_supported()
            .context("validate incus OpenClaw dashboard support")?;
        Ok(())
    }

    fn ensure_customer_openclaw_flow_supported(&self) -> anyhow::Result<()> {
        self.openclaw_guest_ipv4_network()
            .context("configured Incus customer flow requires a static guest IPv4 subnet")?;
        self.openclaw_proxy_host_ipv4()
            .context("configured Incus customer flow requires an explicit proxy host IPv4")?;
        Ok(())
    }

    async fn get_openclaw_launch_auth(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<ManagedOpenClawLaunchAuth> {
        let config = self
            .load_openclaw_config(vm_id, request_id)
            .await
            .with_context(|| format!("load incus OpenClaw config for VM {vm_id}"))?;
        let gateway_auth_token = config
            .gateway
            .and_then(|gateway| gateway.auth)
            .and_then(|auth| auth.token)
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
        Ok(ManagedOpenClawLaunchAuth {
            vm_id: vm_id.to_string(),
            gateway_auth_token,
        })
    }

    async fn get_openclaw_proxy_target(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<OpenClawProxyTarget> {
        let (proxy_host, proxy_port) =
            self.ensure_openclaw_proxy_device(vm_id, request_id)
                .await
                .with_context(|| format!("prepare incus OpenClaw proxy target for VM {vm_id}"))?;
        Ok(OpenClawProxyTarget {
            base_url: format!("http://{proxy_host}:{proxy_port}"),
        })
    }

    async fn load_openclaw_config(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<IncusOpenClawConfigFile> {
        let path = format!("/{}", pika_cloud::GUEST_OPENCLAW_CONFIG_PATH);
        let bytes = self
            .get_instance_file(vm_id, &path, request_id, "load incus OpenClaw config")
            .await?
            .with_context(|| format!("incus OpenClaw config file was missing for VM {vm_id}"))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("parse incus OpenClaw config for VM {vm_id}"))
    }

    async fn get_instance_details(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<IncusInstanceDetails> {
        self.get_json(
            &["1.0", "instances", vm_id],
            true,
            request_id,
            "load incus instance details",
        )
        .await
        .map_err(|err| self.rewrite_not_found(err, format!("incus vm not found: {vm_id}")))
    }

    fn openclaw_proxy_host_ipv4(&self) -> anyhow::Result<Ipv4Addr> {
        let raw = self
            .resolved
            .openclaw_proxy_host
            .as_deref()
            .with_context(|| {
                format!(
                    "missing incus.openclaw_proxy_host; set request.incus.openclaw_proxy_host or {INCUS_OPENCLAW_PROXY_HOST_ENV}"
                )
            })?;
        raw.parse::<Ipv4Addr>().with_context(|| {
            format!("invalid incus.openclaw_proxy_host {raw:?}; expected an IPv4 address")
        })
    }

    fn openclaw_guest_ipv4_network(&self) -> anyhow::Result<Ipv4Net> {
        let cidr = self
            .resolved
            .openclaw_guest_ipv4_cidr
            .as_deref()
            .with_context(|| {
                format!(
                    "missing incus.openclaw_guest_ipv4_cidr; set request.incus.openclaw_guest_ipv4_cidr or {INCUS_OPENCLAW_GUEST_IPV4_CIDR_ENV}"
                )
            })?;
        let network = cidr
            .parse::<Ipv4Net>()
            .with_context(|| format!("invalid incus.openclaw_guest_ipv4_cidr {cidr:?}"))?;
        let host_start = u32::from(network.network()) + 2;
        let host_end = u32::from(network.broadcast()) - 1;
        anyhow::ensure!(
            host_start <= host_end,
            "incus.openclaw_guest_ipv4_cidr {cidr:?} must leave at least one guest address after reserving the gateway"
        );
        Ok(network)
    }

    fn deterministic_openclaw_proxy_port(&self, vm_id: &str) -> u16 {
        let digest = Sha256::digest(vm_id.as_bytes());
        let offset = u16::from_be_bytes([digest[0], digest[1]]) % INCUS_OPENCLAW_PROXY_PORT_SPAN;
        INCUS_OPENCLAW_PROXY_PORT_START + offset
    }

    fn deterministic_openclaw_guest_ipv4(&self, vm_id: &str) -> anyhow::Result<Ipv4Addr> {
        let network = self.openclaw_guest_ipv4_network()?;
        let host_start = u32::from(network.network()) + 2;
        let host_end = u32::from(network.broadcast()) - 1;
        let host_count = host_end - host_start + 1;
        let digest = Sha256::digest(vm_id.as_bytes());
        let offset = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % host_count;
        Ok(Ipv4Addr::from(host_start + offset))
    }

    async fn list_project_instances(
        &self,
        request_id: Option<&str>,
    ) -> anyhow::Result<Vec<IncusInstanceDetails>> {
        let response = self
            .request(
                reqwest::Method::GET,
                &["1.0", "instances"],
                true,
                request_id,
            )?
            .query(&[("recursion", "1")])
            .send()
            .await
            .context("list incus instances in project")?;
        self.parse_json_response(response, "list incus instances in project")
            .await
    }

    fn openclaw_guest_ipv4_from_details(details: &IncusInstanceDetails) -> Option<Ipv4Addr> {
        details
            .config
            .get(INCUS_OPENCLAW_GUEST_IPV4_CONFIG_KEY)
            .and_then(|value| value.parse::<Ipv4Addr>().ok())
            .or_else(|| {
                details
                    .devices
                    .get(INCUS_PRIMARY_NIC_DEVICE_NAME)
                    .and_then(|device| device.get("ipv4.address"))
                    .and_then(|value| value.parse::<Ipv4Addr>().ok())
            })
    }

    fn openclaw_proxy_port_from_details(details: &IncusInstanceDetails) -> Option<u16> {
        details
            .config
            .get(INCUS_OPENCLAW_PROXY_PORT_CONFIG_KEY)
            .and_then(|value| value.parse::<u16>().ok())
    }

    fn select_openclaw_guest_ipv4(
        &self,
        vm_id: &str,
        current: Option<Ipv4Addr>,
        used: &BTreeSet<Ipv4Addr>,
    ) -> anyhow::Result<Ipv4Addr> {
        let network = self.openclaw_guest_ipv4_network()?;
        if current.is_some_and(|address| network.contains(&address) && !used.contains(&address)) {
            return Ok(current.expect("checked above"));
        }

        let preferred = self.deterministic_openclaw_guest_ipv4(vm_id)?;
        let host_start = u32::from(network.network()) + 2;
        let host_end = u32::from(network.broadcast()) - 1;
        let host_count = host_end - host_start + 1;
        let preferred_offset = u32::from(preferred) - host_start;
        for step in 0..host_count {
            let candidate = Ipv4Addr::from(host_start + ((preferred_offset + step) % host_count));
            if !used.contains(&candidate) {
                return Ok(candidate);
            }
        }
        anyhow::bail!(
            "no free guest IPv4 remains in incus.openclaw_guest_ipv4_cidr {}",
            network
        )
    }

    fn select_openclaw_proxy_port(
        &self,
        vm_id: &str,
        current: Option<u16>,
        used: &BTreeSet<u16>,
    ) -> anyhow::Result<u16> {
        let valid_proxy_port_range = INCUS_OPENCLAW_PROXY_PORT_START
            ..INCUS_OPENCLAW_PROXY_PORT_START + INCUS_OPENCLAW_PROXY_PORT_SPAN;
        if current
            .is_some_and(|port| valid_proxy_port_range.contains(&port) && !used.contains(&port))
        {
            return Ok(current.expect("checked above"));
        }

        let preferred = self.deterministic_openclaw_proxy_port(vm_id);
        let preferred_offset = preferred - INCUS_OPENCLAW_PROXY_PORT_START;
        for step in 0..INCUS_OPENCLAW_PROXY_PORT_SPAN {
            let candidate = INCUS_OPENCLAW_PROXY_PORT_START
                + ((preferred_offset + step) % INCUS_OPENCLAW_PROXY_PORT_SPAN);
            if !used.contains(&candidate) {
                return Ok(candidate);
            }
        }
        anyhow::bail!("no free Incus OpenClaw proxy port remains in the configured host port range")
    }

    async fn allocate_openclaw_proxy_binding(
        &self,
        vm_id: &str,
        current_proxy_port: Option<u16>,
        current_guest_ipv4: Option<Ipv4Addr>,
        request_id: Option<&str>,
    ) -> anyhow::Result<(u16, Ipv4Addr)> {
        let instances = self.list_project_instances(request_id).await?;
        let mut used_guest_ipv4s = BTreeSet::new();
        let mut used_proxy_ports = BTreeSet::new();
        for instance in instances {
            if instance.name == vm_id {
                continue;
            }
            if let Some(address) = Self::openclaw_guest_ipv4_from_details(&instance) {
                used_guest_ipv4s.insert(address);
            }
            if let Some(port) = Self::openclaw_proxy_port_from_details(&instance) {
                used_proxy_ports.insert(port);
            }
        }
        let proxy_port =
            self.select_openclaw_proxy_port(vm_id, current_proxy_port, &used_proxy_ports)?;
        let guest_ipv4 =
            self.select_openclaw_guest_ipv4(vm_id, current_guest_ipv4, &used_guest_ipv4s)?;
        Ok((proxy_port, guest_ipv4))
    }

    async fn load_primary_nic_network_name(
        &self,
        request_id: Option<&str>,
    ) -> anyhow::Result<String> {
        let profile: serde_json::Value = self
            .get_json(
                &["1.0", "profiles", &self.resolved.profile],
                true,
                request_id,
                "load configured incus profile for primary nic",
            )
            .await?;
        profile
            .get("devices")
            .and_then(serde_json::Value::as_object)
            .and_then(|devices| {
                devices.values().find_map(|device| {
                    (device.get("type").and_then(serde_json::Value::as_str) == Some("nic"))
                        .then(|| {
                            device
                                .get("network")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string)
                        })
                        .flatten()
                })
            })
            .with_context(|| {
                format!(
                    "configured incus profile {} in project {} must include a nic device with a managed network",
                    self.resolved.profile, self.resolved.project
                )
            })
    }

    async fn ensure_openclaw_proxy_device(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<(Ipv4Addr, u16)> {
        let mut details = self
            .get_instance_details(vm_id, request_id)
            .await
            .with_context(|| format!("load incus instance details for VM {vm_id}"))?;
        let nic_network = self
            .load_primary_nic_network_name(request_id)
            .await
            .with_context(|| format!("load primary incus nic network for VM {vm_id}"))?;
        let proxy_host = details
            .config
            .get(INCUS_OPENCLAW_PROXY_HOST_CONFIG_KEY)
            .and_then(|value| value.parse::<Ipv4Addr>().ok())
            .unwrap_or(self.openclaw_proxy_host_ipv4()?);
        let current_guest_ipv4 = self
            .guest_ipv4_from_instance_state(vm_id, request_id)
            .await
            .ok()
            .filter(|address| {
                self.openclaw_guest_ipv4_network()
                    .map(|network| network.contains(address))
                    .unwrap_or(false)
            });
        let current_configured_guest_ipv4 =
            Self::openclaw_guest_ipv4_from_details(&details).or(current_guest_ipv4);
        let current_proxy_port = Self::openclaw_proxy_port_from_details(&details);
        let (proxy_port, guest_ipv4) = self
            .allocate_openclaw_proxy_binding(
                vm_id,
                current_proxy_port,
                current_configured_guest_ipv4,
                request_id,
            )
            .await?;
        let expected_nic = BTreeMap::from([
            ("type".to_string(), "nic".to_string()),
            ("network".to_string(), nic_network),
            (
                "name".to_string(),
                INCUS_PRIMARY_NIC_DEVICE_NAME.to_string(),
            ),
            ("ipv4.address".to_string(), guest_ipv4.to_string()),
        ]);
        let expected_device = BTreeMap::from([
            ("type".to_string(), "proxy".to_string()),
            ("bind".to_string(), "host".to_string()),
            (
                "listen".to_string(),
                format!("tcp:{proxy_host}:{proxy_port}"),
            ),
            (
                "connect".to_string(),
                format!("tcp:{guest_ipv4}:{}", DEFAULT_OPENCLAW_GATEWAY_PORT),
            ),
            ("nat".to_string(), "true".to_string()),
        ]);
        let expected_host = proxy_host.to_string();
        let expected_port = proxy_port.to_string();
        let expected_guest_ipv4 = guest_ipv4.to_string();
        let proxy_device_matches = details
            .devices
            .get(INCUS_OPENCLAW_PROXY_DEVICE_NAME)
            .is_some_and(|device| device == &expected_device);
        let nic_device_matches = details
            .devices
            .get(INCUS_PRIMARY_NIC_DEVICE_NAME)
            .is_some_and(|device| device == &expected_nic);
        let proxy_metadata_matches = details
            .config
            .get(INCUS_OPENCLAW_PROXY_HOST_CONFIG_KEY)
            .is_some_and(|value| value == &expected_host)
            && details
                .config
                .get(INCUS_OPENCLAW_PROXY_PORT_CONFIG_KEY)
                .is_some_and(|value| value == &expected_port)
            && details
                .config
                .get(INCUS_OPENCLAW_GUEST_IPV4_CONFIG_KEY)
                .is_some_and(|value| value == &expected_guest_ipv4);
        if proxy_device_matches && proxy_metadata_matches && nic_device_matches {
            return Ok((proxy_host, proxy_port));
        }

        details
            .devices
            .insert(INCUS_PRIMARY_NIC_DEVICE_NAME.to_string(), expected_nic);
        details.devices.insert(
            INCUS_OPENCLAW_PROXY_DEVICE_NAME.to_string(),
            expected_device,
        );
        details.config.insert(
            INCUS_OPENCLAW_PROXY_HOST_CONFIG_KEY.to_string(),
            expected_host,
        );
        details.config.insert(
            INCUS_OPENCLAW_PROXY_PORT_CONFIG_KEY.to_string(),
            expected_port,
        );
        details.config.insert(
            INCUS_OPENCLAW_GUEST_IPV4_CONFIG_KEY.to_string(),
            expected_guest_ipv4,
        );
        let body = serde_json::json!({
            "config": details.config,
            "devices": details.devices,
        });
        self.patch_expect_operation(
            &["1.0", "instances", vm_id],
            true,
            &body,
            request_id,
            "update incus OpenClaw proxy device",
        )
        .await?;
        Ok((proxy_host, proxy_port))
    }

    async fn guest_ipv4_from_instance_state(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<Ipv4Addr> {
        let state: IncusInstanceState = self
            .get_json(
                &["1.0", "instances", vm_id, "state"],
                true,
                request_id,
                "load incus instance state for OpenClaw proxy",
            )
            .await
            .map_err(|err| self.rewrite_not_found(err, format!("incus vm not found: {vm_id}")))?;
        state
            .network
            .unwrap_or_default()
            .values()
            .flat_map(|interface| interface.addresses.iter())
            .find_map(|address| {
                (address.family == "inet" && address.scope == "global")
                    .then(|| address.address.parse::<Ipv4Addr>().ok())
                    .flatten()
            })
            .with_context(|| format!("incus VM {vm_id} did not report a global IPv4 address"))
    }

    async fn create_managed_vm(
        &self,
        input: ManagedRuntimeCreateInput<'_>,
        request_id: Option<&str>,
    ) -> anyhow::Result<ManagedRuntimeStatus> {
        let vm_id = self.instance_name_for_input(&input);
        let volume_name = self.persistent_volume_name(&vm_id);

        self.create_persistent_volume(&volume_name, request_id)
            .await
            .with_context(|| format!("create incus persistent volume for VM {vm_id}"))?;

        if let Err(err) = self
            .create_instance(&vm_id, &volume_name, &input, request_id)
            .await
        {
            let instance_cleanup = self.delete_instance(&vm_id, request_id).await;
            if let Err(cleanup_err) = instance_cleanup {
                if !is_incus_not_found_error(&cleanup_err) {
                    tracing::error!(
                        vm_id = %vm_id,
                        error = %cleanup_err,
                        "failed to clean up incus instance after create failure"
                    );
                }
            }
            let volume_cleanup = self
                .delete_persistent_volume(&volume_name, request_id)
                .await;
            if let Err(cleanup_err) = volume_cleanup {
                if !is_incus_not_found_error(&cleanup_err) {
                    tracing::error!(
                        vm_id = %vm_id,
                        volume_name = %volume_name,
                        error = %cleanup_err,
                        "failed to clean up incus persistent volume after instance create failure"
                    );
                }
            }
            return Err(err);
        }

        match self.get_vm_status(&vm_id, request_id).await {
            Ok(status) => Ok(status),
            Err(err) => {
                tracing::warn!(
                    vm_id = %vm_id,
                    error = %err,
                    "created incus VM but failed to fetch immediate status; returning conservative starting state"
                );
                Ok(ManagedRuntimeStatus {
                    id: vm_id,
                    status: "starting".to_string(),
                    startup_probe_satisfied: false,
                    guest_ready: false,
                })
            }
        }
    }

    async fn get_vm_status(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<ManagedRuntimeStatus> {
        let state: IncusInstanceState = self
            .get_json(
                &["1.0", "instances", vm_id, "state"],
                true,
                request_id,
                "load incus instance state",
            )
            .await
            .map_err(|err| self.rewrite_not_found(err, format!("incus vm not found: {vm_id}")))?;

        let status = match state.status.trim() {
            "Running" => "running",
            "Error" => "failed",
            "Starting" => "starting",
            "Stopping" | "Stopped" | "Frozen" | "Freezing" | "Thawed" => "starting",
            _ => "starting",
        };
        let guest_signal = if status == "running" {
            self.guest_lifecycle_signal(vm_id, request_id).await
        } else {
            None
        };
        let startup_probe_satisfied = guest_signal
            .as_ref()
            .is_some_and(|signal| signal.startup_probe_satisfied);
        let guest_ready = guest_signal
            .as_ref()
            .is_some_and(|signal| signal.guest_ready);
        let status = if guest_signal.as_ref().is_some_and(|signal| signal.failed) {
            "failed"
        } else {
            status
        };
        Ok(ManagedRuntimeStatus {
            id: vm_id.to_string(),
            status: status.to_string(),
            startup_probe_satisfied,
            guest_ready,
        })
    }

    async fn get_vm_backup_status(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<ManagedRuntimeBackupStatus> {
        let volume_name = self.persistent_volume_name(vm_id);
        let backup_target = format!("{}/{}", self.resolved.storage_pool, volume_name);
        let snapshots = self
            .list_persistent_volume_snapshots(&volume_name, request_id)
            .await
            .with_context(|| format!("list incus state-volume snapshots for VM {vm_id}"))?;
        let Some(latest_snapshot) = latest_incus_snapshot(&snapshots) else {
            return Ok(ManagedRuntimeBackupStatus {
                vm_id: vm_id.to_string(),
                backup_unit_kind: VmBackupUnitKind::PersistentStateVolume,
                backup_target,
                recovery_point_kind: VmRecoveryPointKind::VolumeSnapshot,
                freshness: VmBackupFreshness::Missing,
                latest_recovery_point_name: None,
                latest_successful_backup_at: None,
                observed_at: Some(chrono::Utc::now().to_rfc3339()),
            });
        };

        let latest_at = latest_snapshot.created_at.clone().with_context(|| {
            format!("latest incus snapshot for VM {vm_id} did not include created_at metadata")
        })?;
        let latest_parsed = chrono::DateTime::parse_from_rfc3339(&latest_at)
            .with_context(|| format!("parse incus snapshot created_at for VM {vm_id}"))?
            .with_timezone(&chrono::Utc);
        let freshness = if chrono::Utc::now().signed_duration_since(latest_parsed)
            <= chrono::Duration::hours(INCUS_BACKUP_HEALTHY_MAX_AGE_HOURS)
        {
            VmBackupFreshness::Healthy
        } else {
            VmBackupFreshness::Stale
        };

        Ok(ManagedRuntimeBackupStatus {
            vm_id: vm_id.to_string(),
            backup_unit_kind: VmBackupUnitKind::PersistentStateVolume,
            backup_target,
            recovery_point_kind: VmRecoveryPointKind::VolumeSnapshot,
            freshness,
            latest_recovery_point_name: Some(incus_snapshot_leaf_name(&latest_snapshot.name)),
            latest_successful_backup_at: Some(latest_at),
            observed_at: Some(chrono::Utc::now().to_rfc3339()),
        })
    }

    async fn recover_vm(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<ManagedRuntimeStatus> {
        let state: IncusInstanceState = self
            .get_json(
                &["1.0", "instances", vm_id, "state"],
                true,
                request_id,
                "load incus instance state for recover",
            )
            .await
            .map_err(|err| self.rewrite_not_found(err, format!("incus vm not found: {vm_id}")))?;

        let action = if state.status.trim() == "Running" {
            "restart"
        } else {
            "start"
        };
        self.change_instance_state(vm_id, action, request_id)
            .await
            .with_context(|| format!("recover incus VM {vm_id}"))?;
        self.get_vm_status(vm_id, request_id).await
    }

    async fn restore_vm(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<ManagedRuntimeStatus> {
        let state: IncusInstanceState = self
            .get_json(
                &["1.0", "instances", vm_id, "state"],
                true,
                request_id,
                "load incus instance state for restore",
            )
            .await
            .map_err(|err| self.rewrite_not_found(err, format!("incus vm not found: {vm_id}")))?;
        let volume_name = self.persistent_volume_name(vm_id);
        let snapshots = self
            .list_persistent_volume_snapshots(&volume_name, request_id)
            .await
            .with_context(|| format!("list incus state-volume snapshots for VM {vm_id}"))?;
        let latest_snapshot = latest_incus_snapshot(&snapshots).ok_or_else(|| {
            anyhow!("incus restore requires at least one state-volume snapshot for VM {vm_id}")
        })?;
        let snapshot_name = incus_snapshot_leaf_name(&latest_snapshot.name);

        if !matches!(state.status.trim(), "Stopped" | "Frozen") {
            self.stop_instance(vm_id, request_id)
                .await
                .with_context(|| format!("stop incus VM {vm_id} before restore"))?;
        }
        self.restore_persistent_volume(&volume_name, &snapshot_name, request_id)
            .await
            .with_context(|| format!("restore incus state volume for VM {vm_id}"))?;
        self.start_instance(vm_id, request_id)
            .await
            .with_context(|| format!("start incus VM {vm_id} after restore"))?;
        self.get_vm_status(vm_id, request_id).await
    }

    async fn delete_vm(&self, vm_id: &str, request_id: Option<&str>) -> anyhow::Result<()> {
        let volume_name = self.persistent_volume_name(vm_id);
        let state = self
            .get_json::<IncusInstanceState>(
                &["1.0", "instances", vm_id, "state"],
                true,
                request_id,
                "load incus instance state for delete",
            )
            .await;
        if let Ok(state) = state {
            if !matches!(state.status.trim(), "Stopped" | "Frozen") {
                self.stop_instance(vm_id, request_id)
                    .await
                    .with_context(|| format!("stop incus VM {vm_id} before delete"))?;
            }
        } else if !state.as_ref().err().is_some_and(is_incus_not_found_error) {
            return Err(state
                .expect_err("incus delete state probe should fail")
                .context(format!("load incus instance state for delete {vm_id}")));
        }
        let instance_delete = self.delete_instance(vm_id, request_id).await;
        let volume_delete = self
            .delete_persistent_volume(&volume_name, request_id)
            .await;

        let instance_missing = instance_delete
            .as_ref()
            .err()
            .is_some_and(is_incus_not_found_error);
        let volume_missing = volume_delete
            .as_ref()
            .err()
            .is_some_and(is_incus_not_found_error);

        if let Err(err) = instance_delete {
            if !instance_missing {
                return Err(err);
            }
        }
        if let Err(err) = volume_delete {
            if !volume_missing {
                return Err(err);
            }
        }
        if instance_missing {
            anyhow::bail!("incus vm not found: {vm_id}");
        }
        Ok(())
    }

    async fn create_persistent_volume(
        &self,
        volume_name: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "name": volume_name,
            "content_type": INCUS_PERSISTENT_VOLUME_CONTENT_TYPE,
            "description": format!("Persistent managed-agent state volume for {volume_name}"),
        });
        self.post_expect_sync_or_operation(
            &[
                "1.0",
                "storage-pools",
                &self.resolved.storage_pool,
                "volumes",
                INCUS_PERSISTENT_VOLUME_TYPE,
            ],
            true,
            &body,
            request_id,
            "create incus persistent volume",
        )
        .await
    }

    async fn create_instance(
        &self,
        vm_id: &str,
        volume_name: &str,
        input: &ManagedRuntimeCreateInput<'_>,
        request_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let runtime_plan = self.build_runtime_plan(vm_id, volume_name)?;
        let mut devices = BTreeMap::new();
        devices.insert(
            "root".to_string(),
            serde_json::json!({
                "type": "disk",
                "path": "/",
                "pool": self.resolved.storage_pool.as_str(),
            }),
        );
        for mount in &runtime_plan.mounts {
            let mut device = serde_json::Map::from_iter(
                incus_mount_device_config(mount)
                    .into_iter()
                    .map(|(key, value)| (key, serde_json::Value::String(value))),
            );
            device.insert(
                "pool".to_string(),
                serde_json::Value::String(self.resolved.storage_pool.clone()),
            );
            devices.insert(mount.device_name.clone(), serde_json::Value::Object(device));
        }
        let openclaw_nic_network = self.load_primary_nic_network_name(request_id).await?;

        let cloud_init_user_data = self
            .cloud_init_user_data(input)
            .context("build incus bootstrap user-data")?;
        let mut instance_config = serde_json::Map::from_iter([
            (
                INCUS_CLOUD_INIT_USER_DATA_KEY.to_string(),
                serde_json::Value::String(cloud_init_user_data),
            ),
            (
                "user.pika.provider".to_string(),
                serde_json::Value::String("incus".to_string()),
            ),
            (
                "user.pika.state_volume".to_string(),
                serde_json::Value::String(volume_name.to_string()),
            ),
            (
                "user.pika.agent_kind".to_string(),
                serde_json::Value::String("openclaw".to_string()),
            ),
        ]);
        for (key, value) in incus_runtime_config(&runtime_plan) {
            instance_config.insert(key, serde_json::Value::String(value));
        }
        let proxy_host = self.openclaw_proxy_host_ipv4()?;
        let (proxy_port, guest_ipv4) = self
            .allocate_openclaw_proxy_binding(vm_id, None, None, request_id)
            .await?;
        instance_config.insert(
            INCUS_OPENCLAW_PROXY_HOST_CONFIG_KEY.to_string(),
            serde_json::Value::String(proxy_host.to_string()),
        );
        instance_config.insert(
            INCUS_OPENCLAW_PROXY_PORT_CONFIG_KEY.to_string(),
            serde_json::Value::String(proxy_port.to_string()),
        );
        instance_config.insert(
            INCUS_OPENCLAW_GUEST_IPV4_CONFIG_KEY.to_string(),
            serde_json::Value::String(guest_ipv4.to_string()),
        );
        devices.insert(
            INCUS_PRIMARY_NIC_DEVICE_NAME.to_string(),
            serde_json::json!({
                "type": "nic",
                "network": openclaw_nic_network,
                "name": INCUS_PRIMARY_NIC_DEVICE_NAME,
                "ipv4.address": guest_ipv4.to_string(),
            }),
        );
        let body = serde_json::json!({
            "name": vm_id,
            "type": INCUS_VM_KIND,
            "start": true,
            "profiles": [runtime_plan.incus.profile.as_str()],
            "source": {
                "type": "image",
                "alias": runtime_plan.incus.image_alias.as_str(),
            },
            "devices": devices,
            "config": instance_config,
        });
        self.post_expect_operation(
            &["1.0", "instances"],
            true,
            &body,
            request_id,
            "create incus instance",
        )
        .await
    }

    async fn delete_instance(&self, vm_id: &str, request_id: Option<&str>) -> anyhow::Result<()> {
        self.delete_expect_operation(
            &["1.0", "instances", vm_id],
            true,
            request_id,
            format!("delete incus instance {vm_id}"),
        )
        .await
        .map_err(|err| self.rewrite_not_found(err, format!("incus vm not found: {vm_id}")))
    }

    async fn delete_persistent_volume(
        &self,
        volume_name: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.delete_expect_empty(
            &[
                "1.0",
                "storage-pools",
                &self.resolved.storage_pool,
                "volumes",
                INCUS_PERSISTENT_VOLUME_TYPE,
                volume_name,
            ],
            true,
            request_id,
            format!("delete incus persistent volume {volume_name}"),
        )
        .await
        .map_err(|err| {
            self.rewrite_not_found(
                err,
                format!("incus persistent volume not found: {volume_name}"),
            )
        })
    }

    async fn list_persistent_volume_snapshots(
        &self,
        volume_name: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<Vec<IncusStorageVolumeSnapshot>> {
        let response = self
            .request(
                reqwest::Method::GET,
                &[
                    "1.0",
                    "storage-pools",
                    &self.resolved.storage_pool,
                    "volumes",
                    INCUS_PERSISTENT_VOLUME_TYPE,
                    volume_name,
                    "snapshots",
                ],
                true,
                request_id,
            )?
            .query(&[("recursion", "1")])
            .send()
            .await
            .context("load incus storage volume snapshots")?;
        self.parse_json_response(response, "load incus storage volume snapshots")
            .await
    }

    async fn restore_persistent_volume(
        &self,
        volume_name: &str,
        snapshot_name: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "restore": snapshot_name,
        });
        self.put_expect_empty(
            &[
                "1.0",
                "storage-pools",
                &self.resolved.storage_pool,
                "volumes",
                INCUS_PERSISTENT_VOLUME_TYPE,
                volume_name,
            ],
            true,
            &body,
            request_id,
            format!("restore incus persistent volume {volume_name} from snapshot {snapshot_name}"),
        )
        .await
    }

    async fn change_instance_state(
        &self,
        vm_id: &str,
        action: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "action": action,
            "force": true,
            "timeout": INCUS_OPERATION_WAIT_TIMEOUT_SECS,
        });
        self.put_expect_operation(
            &["1.0", "instances", vm_id, "state"],
            true,
            &body,
            request_id,
            &format!("set incus VM {vm_id} state to {action}"),
        )
        .await
    }

    async fn start_instance(&self, vm_id: &str, request_id: Option<&str>) -> anyhow::Result<()> {
        self.change_instance_state(vm_id, "start", request_id).await
    }

    async fn stop_instance(&self, vm_id: &str, request_id: Option<&str>) -> anyhow::Result<()> {
        self.change_instance_state(vm_id, "stop", request_id).await
    }

    fn instance_name_for_input(&self, input: &ManagedRuntimeCreateInput<'_>) -> String {
        let mut hasher = Sha256::new();
        hasher.update(input.bot_pubkey_hex.as_bytes());
        let digest = hasher.finalize();
        format!("pika-agent-{}", &hex::encode(digest)[..20])
    }

    fn persistent_volume_name(&self, vm_id: &str) -> String {
        format!("{vm_id}-state")
    }

    fn build_runtime_plan(
        &self,
        vm_id: &str,
        volume_name: &str,
    ) -> anyhow::Result<IncusRuntimePlan> {
        RuntimeSpec::for_incus(
            RuntimeIdentity {
                runtime_id: vm_id.to_string(),
                instance_name: vm_id.to_string(),
            },
            IncusRuntimeConfig {
                project: self.resolved.project.clone(),
                profile: self.resolved.profile.clone(),
                image_alias: self.resolved.image_alias.clone(),
            },
            RuntimeResources {
                vcpu_count: None,
                memory_mib: Some(INCUS_DEV_VM_MEMORY_MIB),
                root_disk_gib: None,
            },
            vec![RuntimeMount {
                kind: MountKind::PersistentVolume,
                guest_path: INCUS_PERSISTENT_VOLUME_PATH.to_string(),
                source: volume_name.to_string(),
                mode: MountMode::ReadWrite,
                required: true,
            }],
        )
        .build_incus_plan()
        .context("build managed Incus runtime plan")
    }

    fn cloud_init_user_data(
        &self,
        input: &ManagedRuntimeCreateInput<'_>,
    ) -> anyhow::Result<String> {
        let bootstrap_request = build_managed_vm_create_request(*input);
        let guest_autostart = bootstrap_request.guest_autostart;
        let mut launcher_env = guest_autostart.env.clone();
        if let Ok(value) = std::env::var(ANTHROPIC_API_KEY_ENV) {
            if !value.trim().is_empty() {
                launcher_env.insert(ANTHROPIC_API_KEY_ENV.to_string(), value);
            }
        }
        let mut files = BTreeMap::new();
        for (path, content) in guest_autostart.files {
            files.insert(
                format!("/{path}"),
                (bootstrap_file_permissions(&path), content),
            );
        }
        files.insert(
            INCUS_BOOTSTRAP_LAUNCHER_PATH.to_string(),
            (
                "0755",
                incus_bootstrap_launcher_script(&launcher_env, &guest_autostart.command),
            ),
        );
        files.insert(
            INCUS_STATE_VOLUME_SETUP_PATH.to_string(),
            ("0755", incus_state_volume_setup_script()),
        );

        let mut cloud_init = String::from("#cloud-config\nwrite_files:\n");
        for (path, (permissions, content)) in files {
            cloud_init.push_str("  - path: ");
            cloud_init.push_str(&path);
            cloud_init.push('\n');
            cloud_init.push_str("    permissions: '");
            cloud_init.push_str(permissions);
            cloud_init.push_str("'\n");
            cloud_init.push_str("    encoding: b64\n");
            cloud_init.push_str("    content: ");
            cloud_init.push_str(&base64::engine::general_purpose::STANDARD.encode(content));
            cloud_init.push('\n');
        }
        cloud_init.push_str("runcmd:\n");
        cloud_init.push_str("  - [systemctl, --no-block, restart, pika-managed-agent.service]\n");
        Ok(cloud_init)
    }

    async fn load_guest_lifecycle_status(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> Option<RuntimeStatusSnapshot> {
        let status_path = STATUS_PATH.to_string();
        let status_bytes = match self
            .get_instance_file(
                vm_id,
                &status_path,
                request_id,
                "load incus guest lifecycle status",
            )
            .await
        {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return None,
            Err(err) => {
                tracing::warn!(
                    vm_id = %vm_id,
                    error = %err,
                    "failed to load incus guest lifecycle status; reporting guest as not ready"
                );
                return None;
            }
        };
        match RuntimeArtifacts::decode_status_artifact(&status_path, &status_bytes) {
            Ok(status) => Some(status),
            Err(err) => {
                tracing::warn!(
                    vm_id = %vm_id,
                    error = %err,
                    "incus guest lifecycle status was malformed; reporting guest as not ready"
                );
                None
            }
        }
    }

    async fn current_guest_boot_id(&self, vm_id: &str, request_id: Option<&str>) -> Option<String> {
        match self
            .exec_instance_stdout(
                vm_id,
                &["cat", INCUS_GUEST_BOOT_ID_PATH],
                request_id,
                "load incus guest boot id",
            )
            .await
        {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(value) => Some(value.trim().to_string()),
                Err(err) => {
                    tracing::warn!(
                        vm_id = %vm_id,
                        error = %err,
                        "incus guest boot id was not valid utf-8; reporting guest as not ready"
                    );
                    None
                }
            },
            Err(err) => {
                tracing::warn!(
                    vm_id = %vm_id,
                    error = %err,
                    "failed to load incus guest boot id; reporting guest as not ready"
                );
                None
            }
        }
    }

    async fn guest_lifecycle_signal(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> Option<ManagedGuestLifecycleSignal> {
        let status = self.load_guest_lifecycle_status(vm_id, request_id).await?;
        let current_boot_id = self.current_guest_boot_id(vm_id, request_id).await?;
        match managed_guest_lifecycle_signal(&status, &current_boot_id) {
            Ok(signal) => Some(signal),
            Err(reason) => {
                tracing::warn!(
                    vm_id = %vm_id,
                    state = ?status.state,
                    reason,
                    "incus guest lifecycle status did not satisfy managed guest contract; reporting guest as not ready"
                );
                None
            }
        }
    }

    fn request(
        &self,
        method: reqwest::Method,
        path_segments: &[&str],
        include_project: bool,
        request_id: Option<&str>,
    ) -> anyhow::Result<reqwest::RequestBuilder> {
        let mut url = Url::parse(&self.resolved.endpoint)
            .with_context(|| format!("parse incus endpoint URL {}", self.resolved.endpoint))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow!("incus endpoint must be a base URL without path segments"))?;
            segments.clear();
            for segment in path_segments {
                segments.push(segment);
            }
        }
        if include_project {
            url.query_pairs_mut()
                .append_pair("project", &self.resolved.project);
        }

        let mut request = self
            .client
            .request(method, url)
            .header(axum::http::header::ACCEPT, "application/json");
        if let Some(request_id) = request_id {
            request = request.header("x-request-id", request_id);
        }
        Ok(request)
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path_segments: &[&str],
        include_project: bool,
        request_id: Option<&str>,
        context: &str,
    ) -> anyhow::Result<T> {
        let response = self
            .request(
                reqwest::Method::GET,
                path_segments,
                include_project,
                request_id,
            )?
            .send()
            .await
            .with_context(|| context.to_string())?;
        self.parse_json_response(response, context).await
    }

    async fn get_instance_file(
        &self,
        vm_id: &str,
        path: &str,
        request_id: Option<&str>,
        context: &str,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let response = self
            .request(
                reqwest::Method::GET,
                &["1.0", "instances", vm_id, "files"],
                true,
                request_id,
            )?
            .header(axum::http::header::ACCEPT, "*/*")
            .query(&[("path", path)])
            .send()
            .await
            .with_context(|| context.to_string())?;
        match response.status() {
            reqwest::StatusCode::OK => Ok(Some(
                response
                    .bytes()
                    .await
                    .with_context(|| format!("{context}: read Incus guest file body"))?
                    .to_vec(),
            )),
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            _ => Ok(None),
        }
    }

    async fn exec_instance_stdout(
        &self,
        vm_id: &str,
        command: &[&str],
        request_id: Option<&str>,
        context: &str,
    ) -> anyhow::Result<Vec<u8>> {
        let body = serde_json::json!({
            "command": command,
            "interactive": false,
            "wait-for-websocket": false,
            "record-output": true,
        });
        let response = self
            .request(
                reqwest::Method::POST,
                &["1.0", "instances", vm_id, "exec"],
                true,
                request_id,
            )?
            .json(&body)
            .send()
            .await
            .with_context(|| format!("{context}: start Incus exec"))?;
        if !response.status().is_success() {
            return Err(self.error_from_response(response, context).await);
        }
        let envelope: IncusResponseEnvelope<serde_json::Value> = response
            .json()
            .await
            .with_context(|| format!("{context}: decode Incus exec operation envelope"))?;
        let operation_path = envelope
            .operation
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .with_context(|| format!("{context}: missing Incus exec operation handle"))?;
        let operation_url = self.operation_wait_url(operation_path)?;
        let mut wait_request = self
            .client
            .get(operation_url)
            .header(axum::http::header::ACCEPT, "application/json");
        if let Some(request_id) = request_id {
            wait_request = wait_request.header("x-request-id", request_id);
        }
        let wait_response = wait_request
            .send()
            .await
            .with_context(|| format!("{context}: wait for Incus exec operation"))?;
        if !wait_response.status().is_success() {
            return Err(self.error_from_response(wait_response, context).await);
        }
        let wait_envelope: IncusResponseEnvelope<IncusExecOperationMetadata> = wait_response
            .json()
            .await
            .with_context(|| format!("{context}: decode waited Incus exec operation"))?;
        let waited = wait_envelope
            .metadata
            .with_context(|| format!("{context}: missing waited Incus exec metadata"))?;
        if let Some(err) = Some(waited.err.trim()).filter(|err| !err.is_empty()) {
            anyhow::bail!("{context}: Incus exec failed: {err}");
        }
        match waited.metadata.return_code {
            Some(0) => {}
            Some(code) => anyhow::bail!("{context}: Incus exec returned non-zero exit code {code}"),
            None => anyhow::bail!("{context}: Incus exec omitted return code"),
        }
        let stdout_path = waited
            .metadata
            .output
            .get("1")
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .with_context(|| format!("{context}: Incus exec omitted stdout log path"))?;
        let stdout_segments = stdout_path
            .trim_start_matches('/')
            .split('/')
            .collect::<Vec<_>>();
        let stdout_response = self
            .request(reqwest::Method::GET, &stdout_segments, true, request_id)?
            .header(axum::http::header::ACCEPT, "*/*")
            .send()
            .await
            .with_context(|| format!("{context}: load Incus exec stdout log"))?;
        if !stdout_response.status().is_success() {
            return Err(self.error_from_response(stdout_response, context).await);
        }
        stdout_response
            .bytes()
            .await
            .with_context(|| format!("{context}: read Incus exec stdout log"))
            .map(|bytes| bytes.to_vec())
    }

    async fn post_expect_sync_or_operation(
        &self,
        path_segments: &[&str],
        include_project: bool,
        body: &serde_json::Value,
        request_id: Option<&str>,
        context: &str,
    ) -> anyhow::Result<()> {
        let response = self
            .request(
                reqwest::Method::POST,
                path_segments,
                include_project,
                request_id,
            )?
            .json(body)
            .send()
            .await
            .with_context(|| context.to_string())?;
        self.finish_mutating_response(response, request_id, context)
            .await
    }

    async fn post_expect_operation(
        &self,
        path_segments: &[&str],
        include_project: bool,
        body: &serde_json::Value,
        request_id: Option<&str>,
        context: &str,
    ) -> anyhow::Result<()> {
        let response = self
            .request(
                reqwest::Method::POST,
                path_segments,
                include_project,
                request_id,
            )?
            .json(body)
            .send()
            .await
            .with_context(|| context.to_string())?;
        self.finish_operation_response(response, request_id, context)
            .await
    }

    async fn put_expect_operation(
        &self,
        path_segments: &[&str],
        include_project: bool,
        body: &serde_json::Value,
        request_id: Option<&str>,
        context: &str,
    ) -> anyhow::Result<()> {
        let response = self
            .request(
                reqwest::Method::PUT,
                path_segments,
                include_project,
                request_id,
            )?
            .json(body)
            .send()
            .await
            .with_context(|| context.to_string())?;
        self.finish_operation_response(response, request_id, context)
            .await
    }

    async fn patch_expect_operation(
        &self,
        path_segments: &[&str],
        include_project: bool,
        body: &serde_json::Value,
        request_id: Option<&str>,
        context: &str,
    ) -> anyhow::Result<()> {
        let response = self
            .request(
                reqwest::Method::PATCH,
                path_segments,
                include_project,
                request_id,
            )?
            .json(body)
            .send()
            .await
            .with_context(|| context.to_string())?;
        self.finish_mutating_response(response, request_id, context)
            .await
    }

    async fn delete_expect_operation(
        &self,
        path_segments: &[&str],
        include_project: bool,
        request_id: Option<&str>,
        context: impl Into<String>,
    ) -> anyhow::Result<()> {
        let context = context.into();
        let response = self
            .request(
                reqwest::Method::DELETE,
                path_segments,
                include_project,
                request_id,
            )?
            .send()
            .await
            .with_context(|| context.clone())?;
        self.finish_operation_response(response, request_id, &context)
            .await
    }

    async fn delete_expect_empty(
        &self,
        path_segments: &[&str],
        include_project: bool,
        request_id: Option<&str>,
        context: impl Into<String>,
    ) -> anyhow::Result<()> {
        let context = context.into();
        let response = self
            .request(
                reqwest::Method::DELETE,
                path_segments,
                include_project,
                request_id,
            )?
            .send()
            .await
            .with_context(|| context.clone())?;
        self.finish_mutating_response(response, request_id, &context)
            .await
    }

    async fn put_expect_empty(
        &self,
        path_segments: &[&str],
        include_project: bool,
        body: &serde_json::Value,
        request_id: Option<&str>,
        context: impl Into<String>,
    ) -> anyhow::Result<()> {
        let context = context.into();
        let response = self
            .request(
                reqwest::Method::PUT,
                path_segments,
                include_project,
                request_id,
            )?
            .json(body)
            .send()
            .await
            .with_context(|| context.clone())?;
        self.finish_mutating_response(response, request_id, &context)
            .await
    }

    async fn finish_mutating_response(
        &self,
        response: reqwest::Response,
        request_id: Option<&str>,
        context: &str,
    ) -> anyhow::Result<()> {
        if response.status() == reqwest::StatusCode::ACCEPTED {
            return self
                .finish_operation_response(response, request_id, context)
                .await;
        }
        if response.status().is_success() {
            return Ok(());
        }
        Err(self.error_from_response(response, context).await)
    }

    async fn finish_operation_response(
        &self,
        response: reqwest::Response,
        request_id: Option<&str>,
        context: &str,
    ) -> anyhow::Result<()> {
        if !response.status().is_success() {
            return Err(self.error_from_response(response, context).await);
        }
        let envelope: IncusResponseEnvelope<IncusOperationMetadata> = response
            .json()
            .await
            .with_context(|| format!("{context}: decode Incus async response"))?;
        let operation_path = envelope
            .operation
            .as_deref()
            .filter(|value| !value.is_empty())
            .with_context(|| format!("{context}: missing Incus operation handle"))?;
        self.wait_for_operation(operation_path, request_id, context)
            .await
    }

    async fn wait_for_operation(
        &self,
        operation_path: &str,
        request_id: Option<&str>,
        context: &str,
    ) -> anyhow::Result<()> {
        let operation_url = self.operation_wait_url(operation_path)?;
        let mut request = self
            .client
            .get(operation_url)
            .header(axum::http::header::ACCEPT, "application/json");
        if let Some(request_id) = request_id {
            request = request.header("x-request-id", request_id);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("{context}: wait for Incus operation"))?;
        if !response.status().is_success() {
            return Err(self.error_from_response(response, context).await);
        }
        let envelope: IncusResponseEnvelope<IncusOperationMetadata> = response
            .json()
            .await
            .with_context(|| format!("{context}: decode waited Incus operation"))?;
        if let Some(err) = envelope
            .metadata
            .as_ref()
            .map(|metadata| metadata.err.trim())
            .filter(|err| !err.is_empty())
        {
            anyhow::bail!("{context}: Incus operation failed: {err}");
        }
        Ok(())
    }

    fn operation_wait_url(&self, operation_path: &str) -> anyhow::Result<Url> {
        let mut url = match Url::parse(operation_path) {
            Ok(url) => url,
            Err(_) => {
                let base = Url::parse(&self.resolved.endpoint).with_context(|| {
                    format!("parse incus endpoint URL {}", self.resolved.endpoint)
                })?;
                let trimmed = operation_path.trim_start_matches('/');
                base.join(trimmed)
                    .with_context(|| format!("join Incus operation URL {operation_path}"))?
            }
        };
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow!("incus operation URL must be hierarchical"))?;
            segments.push("wait");
        }
        url.query_pairs_mut()
            .append_pair("timeout", &INCUS_OPERATION_WAIT_TIMEOUT_SECS.to_string());
        Ok(url)
    }

    async fn parse_json_response<T: DeserializeOwned>(
        &self,
        response: reqwest::Response,
        context: &str,
    ) -> anyhow::Result<T> {
        if !response.status().is_success() {
            return Err(self.error_from_response(response, context).await);
        }
        let envelope: IncusResponseEnvelope<T> = response
            .json()
            .await
            .with_context(|| format!("{context}: decode Incus response"))?;
        if let Some(error) = envelope
            .error
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            anyhow::bail!("{context}: Incus returned error: {error}");
        }
        envelope
            .metadata
            .with_context(|| format!("{context}: missing Incus response metadata"))
    }

    async fn error_from_response(
        &self,
        response: reqwest::Response,
        context: &str,
    ) -> anyhow::Error {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<failed to read body>".to_string());
        let body_trimmed = body.trim();
        let detail = if body_trimmed.is_empty() {
            format!("status {status}")
        } else {
            format!("status {status}: {body_trimmed}")
        };
        anyhow!("{context}: {detail}")
    }

    fn rewrite_not_found(&self, err: anyhow::Error, replacement: String) -> anyhow::Error {
        if is_incus_not_found_error(&err) {
            anyhow!(replacement)
        } else {
            err
        }
    }
}

fn is_incus_not_found_error(err: &anyhow::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("404") && message.contains("not found")
}

fn incus_profile_has_nic_device(profile: &serde_json::Value) -> bool {
    profile
        .get("devices")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|devices| {
            devices
                .values()
                .any(|device| device.get("type").and_then(serde_json::Value::as_str) == Some("nic"))
        })
}

fn bootstrap_file_permissions(path: &str) -> &'static str {
    if path == pika_cloud::GUEST_AUTOSTART_SCRIPT_PATH {
        "0755"
    } else {
        "0644"
    }
}

fn incus_bootstrap_launcher_script(env: &BTreeMap<String, String>, command: &str) -> String {
    let mut script = String::from("#!/usr/bin/env bash\nset -euo pipefail\n");
    for (key, value) in env {
        script.push_str("export ");
        script.push_str(key);
        script.push('=');
        script.push_str(&shell_single_quote(value));
        script.push('\n');
    }
    script.push_str("export PIKA_ENABLE_OPENCLAW_PRIVATE_PROXY=1\n");
    script.push_str("export PIKACHAT_SKIP_RELAY_READY_CHECK=1\n");
    script.push_str(
        "if [[ -z \"${PIKA_VM_IP:-}\" ]]; then\n\
PIKA_VM_IP=\"$(python3 - <<'PY'\n\
import socket\n\
\n\
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)\n\
try:\n    sock.connect((\"1.1.1.1\", 80))\n    print(sock.getsockname()[0])\n\
except OSError:\n    pass\n\
finally:\n    sock.close()\n\
PY\n\
)\"\n\
fi\n\
export PIKA_VM_IP=\"${PIKA_VM_IP:-127.0.0.1}\"\n",
    );
    script.push_str("exec ");
    script.push_str(command);
    script.push('\n');
    script
}

fn incus_state_volume_setup_script() -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

volume_root="{volume_root}"
daemon_state_target="{daemon_state_target}"
openclaw_state_target="{openclaw_state_target}"
agent_root="/root/pika-agent"

link_state_dir() {{
  local source_path="$1"
  local target_path="$2"

  mkdir -p "$target_path"
  if [[ -L "$source_path" ]]; then
    if [[ "$(readlink "$source_path")" == "$target_path" ]]; then
      return
    fi
    rm -f "$source_path"
  elif [[ -d "$source_path" ]]; then
    cp -a "$source_path"/. "$target_path"/
    rm -rf "$source_path"
  elif [[ -e "$source_path" ]]; then
    rm -rf "$source_path"
  fi

  ln -s "$target_path" "$source_path"
}}

mkdir -p "$volume_root" "$daemon_state_target" "$openclaw_state_target" "$agent_root"
link_state_dir "$agent_root/state" "$daemon_state_target"
link_state_dir "$agent_root/openclaw" "$openclaw_state_target"
"#,
        volume_root = INCUS_PERSISTENT_AGENT_STATE_ROOT,
        daemon_state_target = INCUS_PERSISTENT_DAEMON_STATE_DIR,
        openclaw_state_target = INCUS_PERSISTENT_OPENCLAW_STATE_DIR,
    )
}

fn latest_incus_snapshot(
    snapshots: &[IncusStorageVolumeSnapshot],
) -> Option<IncusStorageVolumeSnapshot> {
    let mut snapshots = snapshots.to_vec();
    snapshots.sort_by(|left, right| {
        let left_created = left
            .created_at
            .as_deref()
            .and_then(parse_rfc3339_utc)
            .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
        let right_created = right
            .created_at
            .as_deref()
            .and_then(parse_rfc3339_utc)
            .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
        right_created
            .cmp(&left_created)
            .then_with(|| right.name.cmp(&left.name))
    });
    snapshots.into_iter().next()
}

fn parse_rfc3339_utc(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc))
}

fn incus_snapshot_leaf_name(name: &str) -> String {
    name.rsplit('/')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(name)
        .to_string()
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn authenticated_requester_npub(
    headers: &HeaderMap,
    expected_method: &str,
    expected_path: &str,
    trust_forwarded_host: bool,
) -> Result<String, AgentApiError> {
    let event = event_from_authorization_header(headers)
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))?;
    let expected_host = expected_host_from_headers(headers, trust_forwarded_host)
        .ok_or_else(|| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))?;
    verify_nip98_event(
        &event,
        expected_method,
        expected_path,
        Some(expected_host.as_str()),
        None,
    )
    .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))
}

fn require_whitelisted_requester(
    conn: &mut PgConnection,
    headers: &HeaderMap,
    expected_method: &str,
    expected_path: &str,
    trust_forwarded_host: bool,
) -> Result<RequesterIdentity, AgentApiError> {
    let owner_npub = authenticated_requester_npub(
        headers,
        expected_method,
        expected_path,
        trust_forwarded_host,
    )?;
    let is_active = AgentAllowlistEntry::is_active(conn, &owner_npub)
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;
    if !is_active {
        return Err(AgentApiError::from_code(AgentApiErrorCode::NotWhitelisted));
    }
    Ok(RequesterIdentity { owner_npub })
}

fn phase_to_state(phase: &str) -> Option<AgentAppState> {
    match phase {
        AGENT_PHASE_CREATING => Some(AgentAppState::Creating),
        AGENT_PHASE_READY => Some(AgentAppState::Ready),
        AGENT_PHASE_ERROR => Some(AgentAppState::Error),
        _ => None,
    }
}

fn startup_phase_from_row_phase(phase: &str) -> Option<AgentStartupPhase> {
    match phase {
        AGENT_PHASE_CREATING => Some(AgentStartupPhase::ProvisioningVm),
        AGENT_PHASE_READY => Some(AgentStartupPhase::Ready),
        AGENT_PHASE_ERROR => Some(AgentStartupPhase::Failed),
        _ => None,
    }
}

fn map_row_to_response(
    row: AgentInstance,
    startup_phase: AgentStartupPhase,
) -> Result<AgentStateResponse, AgentApiError> {
    let Some(state) = phase_to_state(&row.phase) else {
        return Err(AgentApiError::from_code(AgentApiErrorCode::Internal));
    };
    Ok(AgentStateResponse {
        agent_id: row.agent_id,
        vm_id: row.vm_id,
        state,
        startup_phase,
    })
}

fn json_response(
    row: AgentInstance,
    startup_phase: AgentStartupPhase,
    request_id: &str,
) -> Result<Json<AgentStateResponse>, AgentApiError> {
    Ok(Json(map_row_to_response(row, startup_phase).map_err(
        |err| err.with_request_id(request_id.to_string()),
    )?))
}

fn managed_environment_status_copy(
    row: Option<&AgentInstance>,
    startup_phase: Option<AgentStartupPhase>,
) -> String {
    match (row, startup_phase) {
        (None, None) => "No managed OpenClaw environment has been provisioned yet.".to_string(),
        (Some(_), Some(AgentStartupPhase::Requested)) => {
            "Provision request recorded. Waiting for a managed OpenClaw VM to be assigned."
                .to_string()
        }
        (Some(_), Some(AgentStartupPhase::ProvisioningVm)) => {
            "Provisioning a managed OpenClaw environment.".to_string()
        }
        (Some(_), Some(AgentStartupPhase::BootingGuest)) => {
            "The VM is booting and OpenClaw is starting inside the guest.".to_string()
        }
        (Some(_), Some(AgentStartupPhase::WaitingForServiceReady)) => {
            "The VM is up. Waiting for the managed OpenClaw service to report ready."
                .to_string()
        }
        (Some(_), Some(AgentStartupPhase::WaitingForKeypackagePublish)) => {
            "The managed OpenClaw startup probe passed. Waiting for its key package to publish."
                .to_string()
        }
        (Some(_), Some(AgentStartupPhase::Ready)) => {
            "Managed OpenClaw is running and ready.".to_string()
        }
        (Some(row), Some(AgentStartupPhase::Failed)) if row.vm_id.is_some() => {
            "Managed OpenClaw needs recovery. Recover first tries to bring the VM back and preserve the current persistent state; if that VM is gone, Recover provisions a fresh environment instead."
                .to_string()
        }
        (Some(_), Some(AgentStartupPhase::Failed)) => {
            "Managed OpenClaw needs recovery. No recoverable VM is available, so Recover provisions a fresh environment."
                .to_string()
        }
        (Some(_), None) => "Managed OpenClaw status is unavailable.".to_string(),
        (None, Some(_)) => "Managed OpenClaw status is unavailable.".to_string(),
    }
}

fn managed_environment_backup_status_from_provider(
    backup: ManagedRuntimeBackupStatus,
) -> ManagedEnvironmentBackupStatus {
    let freshness = match backup.freshness {
        VmBackupFreshness::Healthy => ManagedEnvironmentBackupFreshness::Healthy,
        VmBackupFreshness::Stale => ManagedEnvironmentBackupFreshness::Stale,
        VmBackupFreshness::Missing => ManagedEnvironmentBackupFreshness::Missing,
        VmBackupFreshness::Unavailable => ManagedEnvironmentBackupFreshness::Unavailable,
    };
    let backup_target = (!backup.backup_target.trim().is_empty()).then_some(backup.backup_target);
    let backup_target_label = match backup.backup_unit_kind {
        VmBackupUnitKind::DurableHome => "Legacy Recovery Target".to_string(),
        VmBackupUnitKind::PersistentStateVolume => "State Volume".to_string(),
    };
    let recovery_point_label = match backup.recovery_point_kind {
        VmRecoveryPointKind::MetadataRecord => "recovery record",
        VmRecoveryPointKind::VolumeSnapshot => "state-volume snapshot",
    };
    let latest_successful_backup_at = backup
        .latest_successful_backup_at
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let latest_recovery_point_name = backup
        .latest_recovery_point_name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let status_copy = match freshness {
        ManagedEnvironmentBackupFreshness::Healthy => {
            format!("A recent {recovery_point_label} is available for this managed environment.")
        }
        ManagedEnvironmentBackupFreshness::Stale => {
            format!(
                "Recovery-point protection is stale. The latest {recovery_point_label} is older than the healthy window, so destructive reset now requires explicit confirmation."
            )
        }
        ManagedEnvironmentBackupFreshness::Missing => {
            format!(
                "No {recovery_point_label} is known yet. Treat destructive reset as unsafe until the first recovery point exists."
            )
        }
        ManagedEnvironmentBackupFreshness::Unavailable => {
            "Recovery-point protection could not be verified from the control plane right now. Destructive reset now requires explicit confirmation."
                .to_string()
        }
        ManagedEnvironmentBackupFreshness::NotProvisioned => {
            "No managed environment exists yet, so recovery-point protection is not tracked."
                .to_string()
        }
    };

    ManagedEnvironmentBackupStatus {
        freshness,
        backup_target,
        backup_target_label,
        latest_recovery_point_name,
        latest_successful_backup_at,
        reset_requires_confirmation: !matches!(
            freshness,
            ManagedEnvironmentBackupFreshness::Healthy
                | ManagedEnvironmentBackupFreshness::NotProvisioned
        ),
        status_copy,
    }
}

fn unavailable_backup_status(status_copy: impl Into<String>) -> ManagedEnvironmentBackupStatus {
    ManagedEnvironmentBackupStatus {
        freshness: ManagedEnvironmentBackupFreshness::Unavailable,
        backup_target: None,
        backup_target_label: "Recovery Target".to_string(),
        latest_recovery_point_name: None,
        latest_successful_backup_at: None,
        reset_requires_confirmation: true,
        status_copy: status_copy.into(),
    }
}

fn managed_guest_lifecycle_signal(
    status: &RuntimeStatusSnapshot,
    current_boot_id: &str,
) -> Result<ManagedGuestLifecycleSignal, &'static str> {
    let expected_boot_id = status
        .boot_id
        .as_deref()
        .map(str::trim)
        .filter(|boot_id| !boot_id.is_empty())
        .ok_or("missing boot_id")?;
    let current_boot_id = current_boot_id.trim();
    if current_boot_id.is_empty() || current_boot_id != expected_boot_id {
        return Err("boot_id mismatch");
    }

    let details: ManagedGuestLifecycleDetails = match status.details.clone() {
        Some(details) => serde_json::from_value(details).map_err(|_| "malformed details")?,
        None => return Err("missing details"),
    };
    if details.agent_kind.as_deref() != Some("openclaw") {
        return Err("mismatched agent_kind");
    }
    if details.backend_mode.as_deref() != Some("native") {
        return Err("mismatched backend_mode");
    }
    if details.service_kind.as_deref() != Some("openclaw_gateway") {
        return Err("mismatched service_kind");
    }
    if matches!(status.state, LifecycleState::Failed)
        && details
            .failure_reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .is_none()
    {
        return Err("missing failure_reason");
    }

    let guest_ready = match status.state {
        LifecycleState::Ready => details
            .probe
            .as_deref()
            .is_some_and(|probe| !probe.trim().is_empty()),
        _ => false,
    };
    if matches!(status.state, LifecycleState::Ready) && !guest_ready {
        return Err("missing readiness probe");
    }

    Ok(ManagedGuestLifecycleSignal {
        startup_probe_satisfied: details.service_probe_satisfied,
        guest_ready,
        failed: matches!(
            status.state,
            LifecycleState::Failed | LifecycleState::Completed
        ),
    })
}

fn phase_from_runtime_status(vm: &ManagedRuntimeStatus) -> &'static str {
    match (vm.status.as_str(), vm.guest_ready) {
        ("failed", _) => AGENT_PHASE_ERROR,
        ("running", true) => AGENT_PHASE_READY,
        _ => AGENT_PHASE_CREATING,
    }
}

fn startup_phase_from_runtime_status(vm: &ManagedRuntimeStatus) -> AgentStartupPhase {
    match (
        vm.status.as_str(),
        vm.guest_ready,
        vm.startup_probe_satisfied,
    ) {
        ("failed", _, _) => AgentStartupPhase::Failed,
        ("running", true, _) => AgentStartupPhase::Ready,
        ("running", false, true) => AgentStartupPhase::WaitingForKeypackagePublish,
        ("running", false, false) => AgentStartupPhase::WaitingForServiceReady,
        ("starting", _, _) => AgentStartupPhase::BootingGuest,
        _ => AgentStartupPhase::ProvisioningVm,
    }
}

fn is_pending_initial_provision(row: &AgentInstance) -> bool {
    row.phase == AGENT_PHASE_CREATING && row.vm_id.is_none()
}

fn select_visible_agent_row(
    active: Option<AgentInstance>,
    latest: Option<AgentInstance>,
) -> Option<AgentInstance> {
    active.or_else(|| {
        latest.filter(|row| row.phase == AGENT_PHASE_ERROR && !row_requires_manual_cleanup(row))
    })
}

fn load_visible_agent_row(
    conn: &mut PgConnection,
    owner_npub: &str,
) -> Result<Option<AgentInstance>, AgentApiError> {
    let active = AgentInstance::find_active_by_owner(conn, owner_npub)
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;
    let latest = if active.is_none() {
        AgentInstance::find_latest_by_owner(conn, owner_npub)
            .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?
    } else {
        None
    };
    let selected = select_visible_agent_row(active, latest);
    if selected.as_ref().is_some_and(row_requires_manual_cleanup) {
        return Err(AgentApiError::from_code(AgentApiErrorCode::InvalidRequest));
    }
    Ok(selected)
}

fn normalize_loaded_agent_row(
    conn: &mut PgConnection,
    row: AgentInstance,
    request_id: &str,
) -> Result<AgentInstance, AgentApiError> {
    match row.vm_id.as_deref() {
        Some(_) => Ok(row),
        None if row.phase == AGENT_PHASE_READY => mark_agent_errored(conn, &row.agent_id)
            .map_err(|err| err.with_request_id(request_id.to_string())),
        None => Ok(row),
    }
}

struct RefreshedManagedEnvironment {
    row: AgentInstance,
    startup_phase: AgentStartupPhase,
}

async fn refresh_agent_from_runtime(
    conn: &mut PgConnection,
    row: AgentInstance,
    request_id: &str,
) -> Result<RefreshedManagedEnvironment, AgentApiError> {
    let Some(vm_id) = row.vm_id.as_deref() else {
        return Ok(RefreshedManagedEnvironment {
            startup_phase: startup_phase_from_row_phase(&row.phase)
                .unwrap_or(AgentStartupPhase::Requested),
            row,
        });
    };
    let provider = match managed_runtime_provider_for_row(&row, None) {
        Ok(provider) => provider,
        Err(err) => {
            tracing::warn!(
                request_id,
                agent_id = %row.agent_id,
                vm_id,
                error = %err,
                "failed to resolve managed runtime provider while refreshing managed environment readiness"
            );
            return Ok(RefreshedManagedEnvironment {
                startup_phase: startup_phase_from_row_phase(&row.phase)
                    .unwrap_or(AgentStartupPhase::ProvisioningVm),
                row,
            });
        }
    };
    let vm = match provider.get_vm_status(vm_id, Some(request_id)).await {
        Ok(vm) => vm,
        Err(err) if is_vm_not_found_error(&err) => {
            tracing::warn!(
                request_id,
                agent_id = %row.agent_id,
                vm_id,
                error = %err,
                "agent vm missing during readiness refresh; marking row errored"
            );
            if row.phase == AGENT_PHASE_ERROR {
                return Ok(RefreshedManagedEnvironment {
                    row,
                    startup_phase: AgentStartupPhase::Failed,
                });
            }
            let errored = conn
                .transaction::<AgentInstance, anyhow::Error, _>(|conn| {
                    let errored =
                        AgentInstance::update_phase(conn, &row.agent_id, AGENT_PHASE_ERROR, None)?;
                    let message = format!(
                        "A readiness check found that VM {vm_id} was missing. Managed OpenClaw was marked failed and now needs recovery."
                    );
                    insert_managed_environment_event(
                        conn,
                        &row.owner_npub,
                        Some(&row.agent_id),
                        Some(vm_id),
                        EVENT_READINESS_REFRESH_MISSING_VM,
                        &message,
                        Some(request_id),
                    )?;
                    Ok(errored)
                })
                .map_err(|err| {
                    tracing::error!(
                        request_id,
                        agent_id = %row.agent_id,
                        vm_id,
                        error = %err,
                        "failed to mark missing vm row errored"
                    );
                    AgentApiError::from_code(AgentApiErrorCode::Internal)
                        .with_request_id(request_id.to_string())
                })?;
            return Ok(RefreshedManagedEnvironment {
                row: errored,
                startup_phase: AgentStartupPhase::Failed,
            });
        }
        Err(err) => {
            tracing::warn!(
                request_id,
                agent_id = %row.agent_id,
                vm_id,
                error = %err,
                "failed to refresh managed environment readiness from provider; keeping existing phase"
            );
            return Ok(RefreshedManagedEnvironment {
                startup_phase: startup_phase_from_row_phase(&row.phase)
                    .unwrap_or(AgentStartupPhase::ProvisioningVm),
                row,
            });
        }
    };

    let next_phase = phase_from_runtime_status(&vm);
    let startup_phase = startup_phase_from_runtime_status(&vm);
    if row.phase == next_phase && row.vm_id.as_deref() == Some(vm.id.as_str()) {
        return Ok(RefreshedManagedEnvironment { row, startup_phase });
    }

    let updated = AgentInstance::update_phase(conn, &row.agent_id, next_phase, Some(&vm.id))
        .map_err(|_| {
            AgentApiError::from_code(AgentApiErrorCode::Internal)
                .with_request_id(request_id.to_string())
        })?;
    Ok(RefreshedManagedEnvironment {
        row: updated,
        startup_phase,
    })
}

pub(crate) async fn load_managed_environment_status(
    state: &State,
    owner_npub: &str,
    request_id: &str,
) -> Result<ManagedEnvironmentStatus, AgentApiError> {
    let mut conn = state.db_pool.get().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    let Some(row) = load_visible_agent_row(&mut conn, owner_npub)
        .map_err(|err| err.with_request_id(request_id.to_string()))?
    else {
        return Ok(ManagedEnvironmentStatus {
            row: None,
            app_state: None,
            startup_phase: None,
            environment_exists: false,
            status_copy: managed_environment_status_copy(None, None),
        });
    };
    let normalized = normalize_loaded_agent_row(&mut conn, row, request_id)?;
    let refreshed = refresh_agent_from_runtime(&mut conn, normalized, request_id).await?;
    let app_state = phase_to_state(&refreshed.row.phase);
    Ok(ManagedEnvironmentStatus {
        environment_exists: refreshed.row.vm_id.is_some(),
        status_copy: managed_environment_status_copy(
            Some(&refreshed.row),
            Some(refreshed.startup_phase),
        ),
        row: Some(refreshed.row),
        app_state,
        startup_phase: Some(refreshed.startup_phase),
    })
}

pub(crate) async fn load_managed_environment_backup_status(
    status: &ManagedEnvironmentStatus,
    request_id: &str,
) -> ManagedEnvironmentBackupStatus {
    let Some(row) = status.row.as_ref() else {
        return ManagedEnvironmentBackupStatus {
            freshness: ManagedEnvironmentBackupFreshness::NotProvisioned,
            backup_target: None,
            backup_target_label: "Recovery Target".to_string(),
            latest_recovery_point_name: None,
            latest_successful_backup_at: None,
            reset_requires_confirmation: false,
            status_copy:
                "No managed environment exists yet, so recovery-point protection is not tracked."
                    .to_string(),
        };
    };

    let Some(vm_id) = row.vm_id.as_deref() else {
        return unavailable_backup_status(
            "No current VM assignment is available, so recovery-point protection cannot be verified from the control plane.",
        );
    };

    let provider = match managed_runtime_provider_for_row(row, None) {
        Ok(provider) => provider,
        Err(err) => {
            tracing::warn!(
                request_id = %request_id,
                agent_id = %row.agent_id,
                vm_id = %vm_id,
                error = %err,
                "failed to resolve managed runtime provider while loading backup status"
            );
            return unavailable_backup_status(
                "Recovery-point protection could not be verified because the managed-environment control path is unavailable.",
            );
        }
    };
    match provider.get_vm_backup_status(vm_id, Some(request_id)).await {
        Ok(backup) => managed_environment_backup_status_from_provider(backup),
        Err(err) => {
            tracing::warn!(
                request_id = %request_id,
                agent_id = %row.agent_id,
                vm_id = %vm_id,
                error = %err,
                "failed to load backup status from managed runtime provider"
            );
            unavailable_backup_status(
                "Recovery-point protection could not be verified from the control plane right now.",
            )
        }
    }
}

pub(crate) async fn load_launchable_managed_environment(
    state: &State,
    owner_npub: &str,
    request_id: &str,
) -> Result<ManagedEnvironmentHandle, AgentApiError> {
    let status = load_managed_environment_status(state, owner_npub, request_id).await?;
    let Some(row) = status.row else {
        return Err(
            AgentApiError::from_code(AgentApiErrorCode::AgentNotFound).with_request_id(request_id)
        );
    };
    let Some(vm_id) = row.vm_id.clone() else {
        return Err(
            AgentApiError::from_code(AgentApiErrorCode::InvalidRequest).with_request_id(request_id)
        );
    };
    if status.app_state != Some(AgentAppState::Ready)
        || status.startup_phase != Some(AgentStartupPhase::Ready)
    {
        return Err(
            AgentApiError::from_code(AgentApiErrorCode::InvalidRequest).with_request_id(request_id)
        );
    }
    let managed_runtime = managed_runtime_params_from_row(&row).map_err(|err| {
        tracing::error!(
            request_id = %request_id,
            agent_id = %row.agent_id,
            error = %err,
            "failed to decode managed runtime incus config from row"
        );
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    Ok(ManagedEnvironmentHandle {
        owner_npub: row.owner_npub,
        agent_id: row.agent_id,
        vm_id,
        incus: managed_runtime,
    })
}

pub(crate) async fn load_openclaw_proxy_target(
    managed_runtime: &ManagedRuntimeProvisionParams,
    vm_id: &str,
    request_id: &str,
) -> Result<OpenClawProxyTarget, AgentApiError> {
    let provider = managed_runtime_provider(Some(managed_runtime)).map_err(|err| {
        tracing::error!(
            request_id = %request_id,
            vm_id = %vm_id,
            error = %err,
            "failed to resolve managed runtime provider for OpenClaw proxy target"
        );
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    provider
        .get_openclaw_proxy_target(vm_id, Some(request_id))
        .await
        .map_err(|err| {
            tracing::error!(
                request_id = %request_id,
                vm_id = %vm_id,
                error = %err,
                error_chain = %format!("{err:#}"),
                error_debug = ?err,
                "failed to resolve managed OpenClaw proxy target"
            );
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })
}

pub(crate) async fn load_openclaw_launch_auth(
    managed_runtime: &ManagedRuntimeProvisionParams,
    vm_id: &str,
    request_id: &str,
) -> Result<ManagedOpenClawLaunchAuth, AgentApiError> {
    let provider = managed_runtime_provider(Some(managed_runtime)).map_err(|err| {
        tracing::error!(
            request_id = %request_id,
            vm_id = %vm_id,
            error = %err,
            "failed to resolve managed runtime provider for openclaw launch auth"
        );
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    provider
        .get_openclaw_launch_auth(vm_id, Some(request_id))
        .await
        .map_err(|err| {
            tracing::warn!(
                request_id = %request_id,
                vm_id = %vm_id,
                error = %err,
                "failed to load managed openclaw launch auth"
            );
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })
}

fn is_vm_not_found_error(err: &anyhow::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    (message.contains("vm not found") || (message.contains("404") && message.contains("not found")))
        && message.contains("vm")
}

fn is_owner_active_unique_violation(err: &anyhow::Error) -> bool {
    if let Some(DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, info)) =
        err.downcast_ref::<DieselError>()
    {
        return info
            .constraint_name()
            .map(|name| name == AGENT_OWNER_ACTIVE_INDEX)
            .unwrap_or(false);
    }
    err.to_string().contains(AGENT_OWNER_ACTIVE_INDEX)
}

#[derive(Debug, Clone)]
struct ProvisioningBotIdentity {
    pubkey_npub: String,
    pubkey_hex: String,
    secret_hex: String,
}

fn generate_provisioning_bot_identity() -> anyhow::Result<ProvisioningBotIdentity> {
    let bot_keys = Keys::generate();
    Ok(ProvisioningBotIdentity {
        pubkey_npub: bot_keys.public_key().to_bech32()?,
        pubkey_hex: bot_keys.public_key().to_hex(),
        secret_hex: bot_keys.secret_key().to_secret_hex(),
    })
}

fn mark_agent_errored(
    conn: &mut PgConnection,
    agent_id: &str,
) -> Result<AgentInstance, AgentApiError> {
    AgentInstance::update_phase(conn, agent_id, AGENT_PHASE_ERROR, None)
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))
}

fn mark_agent_errored_preserving_vm(
    conn: &mut PgConnection,
    active: &AgentInstance,
) -> Result<AgentInstance, AgentApiError> {
    AgentInstance::update_phase(
        conn,
        &active.agent_id,
        AGENT_PHASE_ERROR,
        active.vm_id.as_deref(),
    )
    .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))
}

fn prepare_agent_for_reprovision(
    conn: &mut PgConnection,
    active: &AgentInstance,
) -> Result<(), AgentApiError> {
    if active.phase != AGENT_PHASE_ERROR {
        mark_agent_errored(conn, &active.agent_id)?;
    }
    Ok(())
}

fn resolved_incus_params(
    requested: Option<&ManagedRuntimeProvisionParams>,
) -> anyhow::Result<ResolvedIncusParams> {
    let mut params = default_incus_params_from_env();
    if let Some(requested) = requested {
        let requested = &requested.incus;
        if requested
            .endpoint
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            params.endpoint = requested.endpoint.clone();
        }
        if requested
            .project
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            params.project = requested.project.clone();
        }
        if requested
            .profile
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            params.profile = requested.profile.clone();
        }
        if requested
            .storage_pool
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            params.storage_pool = requested.storage_pool.clone();
        }
        if requested
            .image_alias
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            params.image_alias = requested.image_alias.clone();
        }
        if requested.insecure_tls.is_some() {
            params.insecure_tls = requested.insecure_tls;
        }
        if requested
            .openclaw_guest_ipv4_cidr
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            params.openclaw_guest_ipv4_cidr = requested.openclaw_guest_ipv4_cidr.clone();
        }
        if requested
            .openclaw_proxy_host
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            params.openclaw_proxy_host = requested.openclaw_proxy_host.clone();
        }
    }
    let endpoint = required_non_empty_field(params.endpoint, "incus.endpoint", INCUS_ENDPOINT_ENV)?;
    let mut endpoint_url = Url::parse(&endpoint)
        .with_context(|| format!("incus.endpoint must be a valid URL, got {endpoint:?}"))?;
    anyhow::ensure!(
        matches!(endpoint_url.scheme(), "http" | "https"),
        "incus.endpoint must use http or https, got {:?}",
        endpoint_url.scheme()
    );
    endpoint_url.set_query(None);
    endpoint_url.set_fragment(None);
    Ok(ResolvedIncusParams {
        endpoint: endpoint_url.to_string().trim_end_matches('/').to_string(),
        project: required_non_empty_field(params.project, "incus.project", INCUS_PROJECT_ENV)?,
        profile: required_non_empty_field(params.profile, "incus.profile", INCUS_PROFILE_ENV)?,
        storage_pool: required_non_empty_field(
            params.storage_pool,
            "incus.storage_pool",
            INCUS_STORAGE_POOL_ENV,
        )?,
        image_alias: required_non_empty_field(
            params.image_alias,
            "incus.image_alias",
            INCUS_IMAGE_ALIAS_ENV,
        )?,
        insecure_tls: params.insecure_tls.unwrap_or(false),
        openclaw_guest_ipv4_cidr: params.openclaw_guest_ipv4_cidr,
        openclaw_proxy_host: params.openclaw_proxy_host,
    })
}

fn resolve_managed_runtime_provider_config(
    requested: Option<&ManagedRuntimeProvisionParams>,
) -> anyhow::Result<ResolvedManagedRuntimeProviderConfig> {
    Ok(ResolvedManagedRuntimeProviderConfig::Incus(
        resolved_incus_params(requested)?,
    ))
}

fn managed_runtime_provider_from_resolved(
    config: ResolvedManagedRuntimeProviderConfig,
) -> anyhow::Result<ManagedRuntimeProvider> {
    match config {
        ResolvedManagedRuntimeProviderConfig::Incus(resolved) => Ok(ManagedRuntimeProvider::Incus(
            IncusManagedRuntimeProvider::new(resolved)?,
        )),
    }
}

fn managed_runtime_provider(
    requested: Option<&ManagedRuntimeProvisionParams>,
) -> anyhow::Result<ManagedRuntimeProvider> {
    managed_runtime_provider_from_resolved(resolve_managed_runtime_provider_config(requested)?)
}

async fn provision_runtime_for_owner(
    owner_npub: &str,
    bot_identity: &ProvisioningBotIdentity,
    request_id: &str,
    provider: &ManagedRuntimeProvider,
) -> anyhow::Result<ManagedRuntimeStatus> {
    let owner_pubkey = PublicKey::parse(owner_npub).context("parse owner npub")?;
    provider
        .create_managed_vm(
            ManagedRuntimeCreateInput {
                owner_pubkey: &owner_pubkey,
                relay_urls: &default_message_relays(),
                bot_secret_hex: &bot_identity.secret_hex,
                bot_pubkey_hex: &bot_identity.pubkey_hex,
            },
            Some(request_id),
        )
        .await
}

async fn provision_agent_for_owner(
    state: &State,
    owner_npub: &str,
    request_id: &str,
    requested: Option<&ManagedRuntimeProvisionParams>,
) -> Result<AgentInstance, AgentApiError> {
    let bot_identity = generate_provisioning_bot_identity().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    let resolved_provider = resolve_managed_runtime_provider_config(requested).map_err(|err| {
        tracing::error!(
            request_id = %request_id,
            owner_npub = %owner_npub,
            error = %err,
            "failed to resolve managed runtime provider for provision"
        );
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    let provider =
        managed_runtime_provider_from_resolved(resolved_provider.clone()).map_err(|err| {
            tracing::error!(
                request_id = %request_id,
                owner_npub = %owner_npub,
                error = %err,
                "failed to initialize managed runtime provider for provision"
            );
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
    let incus_config =
        serialize_managed_runtime_incus_config(&resolved_provider).map_err(|err| {
            tracing::error!(
                request_id = %request_id,
                owner_npub = %owner_npub,
                error = %err,
                "failed to serialize managed runtime incus config for provision"
            );
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;

    let created = {
        let mut conn = state.db_pool.get().map_err(|_| {
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
        conn.transaction::<AgentInstance, anyhow::Error, _>(|conn| {
            let created = AgentInstance::create_with_incus_config(
                conn,
                owner_npub,
                &bot_identity.pubkey_npub,
                None,
                Some(&incus_config),
                AGENT_PHASE_CREATING,
            )?;
            insert_managed_environment_event(
                conn,
                owner_npub,
                Some(&created.agent_id),
                None,
                EVENT_PROVISION_REQUESTED,
                "Provision requested for a new Managed OpenClaw environment.",
                Some(request_id),
            )?;
            Ok(created)
        })
        .map_err(|err| {
            if is_owner_active_unique_violation(&err) {
                AgentApiError::from_code(AgentApiErrorCode::AgentExists).with_request_id(request_id)
            } else {
                tracing::error!(
                    request_id,
                    owner_npub = %owner_npub,
                    error = %err,
                    "failed to create agent instance row"
                );
                AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
            }
        })?
    };

    let vm =
        match provision_runtime_for_owner(owner_npub, &bot_identity, request_id, &provider).await {
            Ok(vm) => vm,
            Err(err) => {
                tracing::error!(
                    request_id,
                    owner_npub = %owner_npub,
                    error = %err,
                    "failed to provision managed runtime for agent"
                );
                if let Ok(mut conn) = state.db_pool.get() {
                    let _ = mark_agent_errored(&mut conn, &created.agent_id);
                }
                return Err(AgentApiError::from_code(AgentApiErrorCode::Internal)
                    .with_request_id(request_id));
            }
        };

    let mut conn = state.db_pool.get().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    let updated = conn
        .transaction::<AgentInstance, anyhow::Error, _>(|conn| {
            let updated = AgentInstance::update_phase(
                conn,
                &created.agent_id,
                phase_from_runtime_status(&vm),
                Some(&vm.id),
            )?;
            let message = format!(
                "Provision accepted. Managed OpenClaw is starting on VM {}.",
                vm.id
            );
            insert_managed_environment_event(
                conn,
                owner_npub,
                Some(&updated.agent_id),
                Some(&vm.id),
                EVENT_PROVISION_ACCEPTED,
                &message,
                Some(request_id),
            )?;
            Ok(updated)
        })
        .map_err(|err| {
            tracing::error!(
                request_id,
                agent_id = %created.agent_id,
                error = %err,
                "failed to update agent phase after provision"
            );
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
    tracing::info!(
        request_id,
        agent_id = %updated.agent_id,
        vm_id = %vm.id,
        vm_status = %vm.status,
        owner_npub = %owner_npub,
        "provisioned agent managed runtime"
    );
    Ok(updated)
}

async fn provision_or_existing_managed_environment(
    state: &State,
    owner_npub: &str,
    request_id: &str,
    requested: Option<&ManagedRuntimeProvisionParams>,
) -> Result<ManagedEnvironmentAction, AgentApiError> {
    match provision_agent_for_owner(state, owner_npub, request_id, requested).await {
        Ok(row) => Ok(ManagedEnvironmentAction {
            row,
            startup_phase: AgentStartupPhase::ProvisioningVm,
        }),
        Err(err) if err.code == AgentApiErrorCode::AgentExists => {
            let status = load_managed_environment_status(state, owner_npub, request_id).await?;
            match (status.row, status.startup_phase) {
                (Some(row), Some(startup_phase)) => {
                    Ok(ManagedEnvironmentAction { row, startup_phase })
                }
                _ => Err(err),
            }
        }
        Err(err) => Err(err),
    }
}

pub(crate) async fn provision_managed_environment_if_missing(
    state: &State,
    owner_npub: &str,
    request_id: &str,
    requested: Option<&ManagedRuntimeProvisionParams>,
) -> Result<ManagedEnvironmentAction, AgentApiError> {
    let status = load_managed_environment_status(state, owner_npub, request_id).await?;
    if let (Some(row), Some(startup_phase)) = (status.row, status.startup_phase) {
        return Ok(ManagedEnvironmentAction { row, startup_phase });
    }

    provision_or_existing_managed_environment(state, owner_npub, request_id, requested).await
}

pub async fn ensure_agent(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    headers: HeaderMap,
    body: Option<Json<AgentProvisionRequest>>,
) -> Result<(StatusCode, Json<AgentStateResponse>), AgentApiError> {
    let mut conn = state.db_pool.get().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal)
            .with_request_id(request_context.request_id.clone())
    })?;
    let requester = require_whitelisted_requester(
        &mut conn,
        &headers,
        "POST",
        V1_AGENTS_ENSURE_PATH,
        state.trust_forwarded_host,
    )
    .map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
    drop(conn);

    let requested = body.as_ref().map(|body| body.0.managed_vm_params());
    let updated = provision_agent_for_owner(
        &state,
        &requester.owner_npub,
        &request_context.request_id,
        requested.as_ref(),
    )
    .await?;

    Ok((
        StatusCode::ACCEPTED,
        json_response(
            updated,
            AgentStartupPhase::ProvisioningVm,
            &request_context.request_id,
        )?,
    ))
}

pub async fn get_my_agent(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    headers: HeaderMap,
) -> Result<Json<AgentStateResponse>, AgentApiError> {
    let mut conn = state.db_pool.get().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal)
            .with_request_id(request_context.request_id.clone())
    })?;
    let requester = require_whitelisted_requester(
        &mut conn,
        &headers,
        "GET",
        V1_AGENTS_ME_PATH,
        state.trust_forwarded_host,
    )
    .map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
    let Some(active) = load_visible_agent_row(&mut conn, &requester.owner_npub)
        .map_err(|err| err.with_request_id(request_context.request_id.clone()))?
    else {
        return Err(AgentApiError::from_code(AgentApiErrorCode::AgentNotFound)
            .with_request_id(request_context.request_id.clone()));
    };
    let normalized = normalize_loaded_agent_row(&mut conn, active, &request_context.request_id)?;
    let refreshed =
        refresh_agent_from_runtime(&mut conn, normalized, &request_context.request_id).await?;
    json_response(
        refreshed.row,
        refreshed.startup_phase,
        &request_context.request_id,
    )
}

pub(crate) async fn recover_agent_for_owner(
    state: &State,
    owner_npub: &str,
    request_id: &str,
    requested: Option<&ManagedRuntimeProvisionParams>,
) -> Result<ManagedEnvironmentAction, AgentApiError> {
    let mut conn = state.db_pool.get().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    let Some(active) = load_visible_agent_row(&mut conn, owner_npub)
        .map_err(|err| err.with_request_id(request_id.to_string()))?
    else {
        return Err(
            AgentApiError::from_code(AgentApiErrorCode::AgentNotFound).with_request_id(request_id)
        );
    };
    let recover_requested_message = match active.vm_id.as_deref() {
        Some(vm_id) => format!("Recover requested for Managed OpenClaw on VM {vm_id}."),
        None => "Recover requested for Managed OpenClaw without a recoverable VM.".to_string(),
    };
    if is_pending_initial_provision(&active) {
        return Ok(ManagedEnvironmentAction {
            row: active,
            startup_phase: AgentStartupPhase::ProvisioningVm,
        });
    }
    record_managed_environment_event(
        &mut conn,
        owner_npub,
        Some(&active.agent_id),
        active.vm_id.as_deref(),
        EVENT_RECOVER_REQUESTED,
        &recover_requested_message,
        request_id,
    )?;
    if active.vm_id.is_none() {
        prepare_agent_for_reprovision(&mut conn, &active)
            .map_err(|err| err.with_request_id(request_id.to_string()))?;
        record_managed_environment_event(
            &mut conn,
            owner_npub,
            Some(&active.agent_id),
            None,
            EVENT_RECOVER_FELL_BACK_TO_FRESH,
            "Recover could not preserve the previous persistent state because no recoverable VM was available. Provisioning a fresh Managed OpenClaw environment.",
            request_id,
        )?;
        drop(conn);
        return provision_or_existing_managed_environment(state, owner_npub, request_id, requested)
            .await;
    }
    let vm_id = active.vm_id.clone().ok_or_else(|| {
        AgentApiError::from_code(AgentApiErrorCode::RecoverFailed).with_request_id(request_id)
    })?;

    let provider = managed_runtime_provider_for_row(&active, requested).map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::RecoverFailed).with_request_id(request_id)
    })?;
    let recovered = match provider.recover_vm(&vm_id, Some(request_id)).await {
        Ok(recovered) => recovered,
        Err(err) if is_vm_not_found_error(&err) => {
            tracing::error!(
                request_id = %request_id,
                owner_npub = %owner_npub,
                agent_id = %active.agent_id,
                vm_id = %vm_id,
                error = %err,
                "recover requested for missing vm; marking stale agent errored and reprovisioning"
            );
            prepare_agent_for_reprovision(&mut conn, &active)
                .map_err(|err| err.with_request_id(request_id.to_string()))?;
            let message = format!(
                "Recover could not preserve the previous persistent state because VM {vm_id} was missing. Provisioning a fresh Managed OpenClaw environment."
            );
            record_managed_environment_event(
                &mut conn,
                owner_npub,
                Some(&active.agent_id),
                Some(&vm_id),
                EVENT_RECOVER_FELL_BACK_TO_FRESH,
                &message,
                request_id,
            )?;
            drop(conn);
            return provision_or_existing_managed_environment(
                state, owner_npub, request_id, requested,
            )
            .await;
        }
        Err(err) => {
            tracing::error!(
                request_id = %request_id,
                agent_id = %active.agent_id,
                vm_id = %vm_id,
                owner_npub = %owner_npub,
                error = %err,
                "failed to recover agent managed runtime"
            );
            return Err(AgentApiError::from_code(AgentApiErrorCode::RecoverFailed)
                .with_request_id(request_id));
        }
    };

    let startup_phase = startup_phase_from_runtime_status(&recovered);
    let updated = conn
        .transaction::<AgentInstance, anyhow::Error, _>(|conn| {
            let updated = AgentInstance::update_phase(
                conn,
                &active.agent_id,
                phase_from_runtime_status(&recovered),
                Some(&recovered.id),
            )?;
            let message = format!(
                "Recover succeeded. Managed OpenClaw is starting again on VM {}.",
                recovered.id
            );
            insert_managed_environment_event(
                conn,
                owner_npub,
                Some(&updated.agent_id),
                Some(&recovered.id),
                EVENT_RECOVER_SUCCEEDED,
                &message,
                Some(request_id),
            )?;
            Ok(updated)
        })
        .map_err(|_| {
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
    Ok(ManagedEnvironmentAction {
        row: updated,
        startup_phase,
    })
}

pub(crate) async fn reset_agent_for_owner(
    state: &State,
    owner_npub: &str,
    request_id: &str,
    requested: Option<&ManagedRuntimeProvisionParams>,
) -> Result<ManagedEnvironmentAction, AgentApiError> {
    let existing = {
        let mut conn = state.db_pool.get().map_err(|_| {
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
        let existing = load_visible_agent_row(&mut conn, owner_npub)
            .map_err(|err| err.with_request_id(request_id.to_string()))?;
        if let Some(existing) = existing
            .as_ref()
            .filter(|row| is_pending_initial_provision(row))
        {
            return Ok(ManagedEnvironmentAction {
                row: existing.clone(),
                startup_phase: AgentStartupPhase::ProvisioningVm,
            });
        }
        let reset_requested_message = match existing.as_ref() {
            Some(_) => {
                "Destructive reset requested. The current managed environment will be replaced."
                    .to_string()
            }
            None => "Destructive reset requested without an existing managed environment. Provisioning a fresh Managed OpenClaw environment."
                .to_string(),
        };
        record_managed_environment_event(
            &mut conn,
            owner_npub,
            existing.as_ref().map(|row| row.agent_id.as_str()),
            existing.as_ref().and_then(|row| row.vm_id.as_deref()),
            EVENT_RESET_REQUESTED,
            &reset_requested_message,
            request_id,
        )?;
        existing
    };

    if let Some(vm_id) = existing.as_ref().and_then(|row| row.vm_id.as_deref()) {
        // Reset intentionally tears down the existing environment using the row's stored provider
        // and only then provisions the replacement with the requested provider policy.
        let provider =
            managed_runtime_provider_for_row(existing.as_ref().expect("existing row"), None)
                .map_err(|err| {
                    tracing::error!(
                        request_id = %request_id,
                        owner_npub = %owner_npub,
                        error = %err,
                        "failed to resolve stored reset managed runtime provider"
                    );
                    AgentApiError::from_code(AgentApiErrorCode::Internal)
                        .with_request_id(request_id)
                })?;
        match provider.delete_vm(vm_id, Some(request_id)).await {
            Ok(()) => {
                tracing::info!(
                    request_id = %request_id,
                    owner_npub = %owner_npub,
                    vm_id = %vm_id,
                    "destroyed managed environment during reset"
                );
                let mut conn = state.db_pool.get().map_err(|_| {
                    AgentApiError::from_code(AgentApiErrorCode::Internal)
                        .with_request_id(request_id)
                })?;
                let message = format!(
                    "Destructive reset removed VM {vm_id}. Provisioning a fresh Managed OpenClaw environment."
                );
                record_managed_environment_event(
                    &mut conn,
                    owner_npub,
                    existing.as_ref().map(|row| row.agent_id.as_str()),
                    Some(vm_id),
                    EVENT_RESET_DESTROYED_OLD_VM,
                    &message,
                    request_id,
                )?;
            }
            Err(err) if is_vm_not_found_error(&err) => {
                tracing::warn!(
                    request_id = %request_id,
                    owner_npub = %owner_npub,
                    vm_id = %vm_id,
                    error = %err,
                    "reset requested for missing vm; continuing with fresh provision"
                );
                let mut conn = state.db_pool.get().map_err(|_| {
                    AgentApiError::from_code(AgentApiErrorCode::Internal)
                        .with_request_id(request_id)
                })?;
                let message = format!(
                    "Destructive reset continued with a fresh environment because VM {vm_id} was already missing."
                );
                record_managed_environment_event(
                    &mut conn,
                    owner_npub,
                    existing.as_ref().map(|row| row.agent_id.as_str()),
                    Some(vm_id),
                    EVENT_RESET_CONTINUED_MISSING_VM,
                    &message,
                    request_id,
                )?;
            }
            Err(err) => {
                tracing::error!(
                    request_id = %request_id,
                    owner_npub = %owner_npub,
                    vm_id = %vm_id,
                    error = %err,
                    "failed to destroy managed environment during reset"
                );
                return Err(AgentApiError::from_code(AgentApiErrorCode::Internal)
                    .with_request_id(request_id));
            }
        }
    } else if existing.is_some() {
        let mut conn = state.db_pool.get().map_err(|_| {
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
        record_managed_environment_event(
            &mut conn,
            owner_npub,
            existing.as_ref().map(|row| row.agent_id.as_str()),
            None,
            EVENT_RESET_CONTINUED_MISSING_VM,
            "Destructive reset continued with a fresh environment because no recoverable VM was available.",
            request_id,
        )?;
    }

    if let Some(existing) = existing {
        let mut conn = state.db_pool.get().map_err(|_| {
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
        prepare_agent_for_reprovision(&mut conn, &existing)
            .map_err(|err| err.with_request_id(request_id.to_string()))?;
    }

    provision_or_existing_managed_environment(state, owner_npub, request_id, requested).await
}

pub(crate) async fn restore_managed_environment_from_backup(
    state: &State,
    owner_npub: &str,
    request_id: &str,
    requested: Option<&ManagedRuntimeProvisionParams>,
) -> Result<ManagedEnvironmentAction, AgentApiError> {
    let mut conn = state.db_pool.get().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    let Some(active) = load_visible_agent_row(&mut conn, owner_npub)
        .map_err(|err| err.with_request_id(request_id.to_string()))?
    else {
        return Err(
            AgentApiError::from_code(AgentApiErrorCode::AgentNotFound).with_request_id(request_id)
        );
    };
    if is_pending_initial_provision(&active) {
        return Err(
            AgentApiError::from_code(AgentApiErrorCode::InvalidRequest).with_request_id(request_id)
        );
    }
    let Some(vm_id) = active.vm_id.clone() else {
        return Err(
            AgentApiError::from_code(AgentApiErrorCode::InvalidRequest).with_request_id(request_id)
        );
    };

    let restore_requested_message = format!(
        "Restore from recovery point requested for Managed OpenClaw on VM {vm_id}. The current state volume will be rolled back to the latest recovery snapshot before the environment is restarted."
    );
    record_managed_environment_event(
        &mut conn,
        owner_npub,
        Some(&active.agent_id),
        Some(&vm_id),
        EVENT_RESTORE_REQUESTED,
        &restore_requested_message,
        request_id,
    )?;
    drop(conn);

    let provider = managed_runtime_provider_for_row(&active, requested).map_err(|err| {
        tracing::error!(
            request_id = %request_id,
            owner_npub = %owner_npub,
            vm_id = %vm_id,
            error = %err,
            "failed to resolve stored restore managed runtime provider"
        );
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    let restored = match provider.restore_vm(&vm_id, Some(request_id)).await {
        Ok(restored) => restored,
        Err(err) => {
            tracing::error!(
                request_id = %request_id,
                owner_npub = %owner_npub,
                agent_id = %active.agent_id,
                vm_id = %vm_id,
                error = %err,
                "failed to restore managed environment from backup"
            );
            if let Ok(mut conn) = state.db_pool.get() {
                let _ = conn.transaction::<AgentInstance, anyhow::Error, _>(|conn| {
                    let _ = mark_agent_errored_preserving_vm(conn, &active)
                        .map_err(|inner| anyhow::anyhow!(inner.error_code()))?;
                    insert_managed_environment_event(
                        conn,
                        owner_npub,
                        Some(&active.agent_id),
                        Some(&vm_id),
                        EVENT_RESTORE_FAILED,
                        "Restore from recovery point failed. The managed environment was left in error for operator review.",
                        Some(request_id),
                    )?;
                    Ok(active.clone())
                });
            }
            return Err(
                AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
            );
        }
    };

    let startup_phase = startup_phase_from_runtime_status(&restored);
    let mut conn = state.db_pool.get().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    let updated = conn
        .transaction::<AgentInstance, anyhow::Error, _>(|conn| {
            let updated = AgentInstance::update_phase(
                conn,
                &active.agent_id,
                phase_from_runtime_status(&restored),
                Some(&restored.id),
            )?;
            let message = format!(
                "Restore from recovery point succeeded. Managed OpenClaw is starting again on VM {} with restored state-volume contents.",
                restored.id
            );
            insert_managed_environment_event(
                conn,
                owner_npub,
                Some(&updated.agent_id),
                Some(&restored.id),
                EVENT_RESTORE_SUCCEEDED,
                &message,
                Some(request_id),
            )?;
            Ok(updated)
        })
        .map_err(|err| {
            tracing::error!(
                request_id = %request_id,
                owner_npub = %owner_npub,
                agent_id = %active.agent_id,
                vm_id = %vm_id,
                error = %err,
                "failed to persist restore-from-backup result"
            );
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
    Ok(ManagedEnvironmentAction {
        row: updated,
        startup_phase,
    })
}

pub async fn recover_my_agent(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    headers: HeaderMap,
    body: Option<Json<AgentProvisionRequest>>,
) -> Result<Json<AgentStateResponse>, AgentApiError> {
    let mut conn = state.db_pool.get().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal)
            .with_request_id(request_context.request_id.clone())
    })?;
    let requester = require_whitelisted_requester(
        &mut conn,
        &headers,
        "POST",
        V1_AGENTS_RECOVER_PATH,
        state.trust_forwarded_host,
    )
    .map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
    drop(conn);
    let requested = body.as_ref().map(|body| body.0.managed_vm_params());
    let recovered = recover_agent_for_owner(
        &state,
        &requester.owner_npub,
        &request_context.request_id,
        requested.as_ref(),
    )
    .await?;
    json_response(
        recovered.row,
        recovered.startup_phase,
        &request_context.request_id,
    )
}

pub async fn agent_api_healthcheck() -> anyhow::Result<()> {
    let configured_incus = default_incus_params_from_env();
    if !incus_params_provided(&configured_incus) {
        tracing::info!("managed-agent incus config not present; skipping agent api healthcheck");
        return Ok(());
    }
    let resolved = resolve_managed_runtime_provider_config(None)
        .context("resolve and validate managed runtime provider")?;
    let provider = managed_runtime_provider_from_resolved(resolved)
        .context("initialize configured managed runtime provider")?;
    let ManagedRuntimeProvider::Incus(incus) = &provider;
    incus.healthcheck().await?;
    provider
        .ensure_customer_openclaw_flow_supported()
        .context("validate managed-agent customer OpenClaw flow")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use serde_json::json;

    fn test_agent_instance(
        agent_id: &str,
        phase: &str,
        vm_id: Option<&str>,
        incus_config: Option<&str>,
    ) -> AgentInstance {
        AgentInstance {
            agent_id: agent_id.to_string(),
            owner_npub: "npub1testowner".to_string(),
            vm_id: vm_id.map(str::to_string),
            incus_config: incus_config.map(str::to_string),
            phase: phase.to_string(),
            created_at: NaiveDate::from_ymd_opt(2026, 3, 6)
                .expect("valid date")
                .and_hms_opt(0, 0, 0)
                .expect("valid timestamp"),
            updated_at: NaiveDate::from_ymd_opt(2026, 3, 6)
                .expect("valid date")
                .and_hms_opt(0, 0, 0)
                .expect("valid timestamp"),
        }
    }

    fn managed_guest_status_snapshot(state: LifecycleState) -> RuntimeStatusSnapshot {
        RuntimeStatusSnapshot {
            schema_version: 1,
            state,
            updated_at: "2026-03-25T20:00:00Z".to_string(),
            message: "managed guest lifecycle".to_string(),
            boot_id: Some("boot-123".to_string()),
            details: Some(json!({
                "agent_kind": "openclaw",
                "backend_mode": "native",
                "service_kind": "openclaw_gateway",
                "service_probe_satisfied": true,
                "probe": "openclaw_gateway_health"
            })),
        }
    }

    fn test_incus_provider() -> IncusManagedRuntimeProvider {
        IncusManagedRuntimeProvider {
            client: reqwest::Client::new(),
            resolved: ResolvedIncusParams {
                endpoint: "https://incus.example.test".to_string(),
                project: "pika-managed-agents".to_string(),
                profile: "pika-agent-dev".to_string(),
                storage_pool: "default".to_string(),
                image_alias: "managed-agent/dev".to_string(),
                insecure_tls: false,
                openclaw_guest_ipv4_cidr: Some("10.77.0.0/24".to_string()),
                openclaw_proxy_host: Some("203.0.113.10".to_string()),
            },
        }
    }

    #[test]
    fn select_visible_agent_row_ignores_legacy_error_row_without_incus_config() {
        let legacy_error = test_agent_instance("agent-legacy", AGENT_PHASE_ERROR, None, None);

        let selected = select_visible_agent_row(None, Some(legacy_error));

        assert!(selected.is_none());
    }

    #[test]
    fn managed_runtime_params_from_row_rejects_missing_incus_config() {
        let legacy_row =
            test_agent_instance("agent-legacy", AGENT_PHASE_READY, Some("vm-legacy"), None);

        let err =
            managed_runtime_params_from_row(&legacy_row).expect_err("legacy row must fail closed");

        assert!(err.to_string().contains("lacks incus_config"));
    }

    #[test]
    fn build_runtime_plan_uses_shared_incus_spec_for_persistent_state() {
        let provider = test_incus_provider();
        let plan = provider
            .build_runtime_plan("pika-agent-123", "pika-agent-123-state")
            .expect("runtime plan");

        assert_eq!(plan.identity.instance_name, "pika-agent-123");
        assert_eq!(plan.incus.project, "pika-managed-agents");
        assert_eq!(plan.incus.profile, "pika-agent-dev");
        assert_eq!(plan.incus.image_alias, "managed-agent/dev");
        assert_eq!(plan.resources.memory_mib, Some(INCUS_DEV_VM_MEMORY_MIB));
        assert_eq!(plan.mounts.len(), 1);
        assert_eq!(plan.mounts[0].kind, MountKind::PersistentVolume);
        assert_eq!(plan.mounts[0].source, "pika-agent-123-state");
        assert_eq!(plan.mounts[0].guest_path, INCUS_PERSISTENT_VOLUME_PATH);
        assert_eq!(plan.mounts[0].device_name, "pk-persis-1c7cfe10");
        assert!(!plan.mounts[0].read_only);
    }

    #[test]
    fn managed_guest_lifecycle_signal_marks_ready_status_as_ready() {
        let signal = managed_guest_lifecycle_signal(
            &managed_guest_status_snapshot(LifecycleState::Ready),
            "boot-123",
        )
        .expect("ready snapshot should validate");

        assert!(signal.startup_probe_satisfied);
        assert!(signal.guest_ready);
        assert!(!signal.failed);
    }

    #[test]
    fn managed_guest_lifecycle_signal_preserves_service_probe_before_ready() {
        let signal = managed_guest_lifecycle_signal(
            &managed_guest_status_snapshot(LifecycleState::Starting),
            "boot-123",
        )
        .expect("starting snapshot should validate");

        assert!(signal.startup_probe_satisfied);
        assert!(!signal.guest_ready);
        assert!(!signal.failed);
    }

    #[test]
    fn managed_guest_lifecycle_signal_surfaces_guest_failure_while_vm_is_running() {
        let mut status = managed_guest_status_snapshot(LifecycleState::Failed);
        status.details = Some(json!({
            "agent_kind": "openclaw",
            "backend_mode": "native",
            "service_kind": "openclaw_gateway",
            "service_probe_satisfied": true,
            "probe": "openclaw_gateway_health",
            "failure_reason": "openclaw_gateway_exited"
        }));

        let signal = managed_guest_lifecycle_signal(&status, "boot-123")
            .expect("failed snapshot should validate");

        assert!(signal.startup_probe_satisfied);
        assert!(!signal.guest_ready);
        assert!(signal.failed);
    }

    #[test]
    fn managed_guest_lifecycle_signal_rejects_boot_id_mismatch() {
        let err = managed_guest_lifecycle_signal(
            &managed_guest_status_snapshot(LifecycleState::Ready),
            "boot-other",
        )
        .expect_err("stale boot id should be rejected");

        assert_eq!(err, "boot_id mismatch");
    }

    #[test]
    fn decode_runtime_status_artifact_rejects_malformed_snapshot() {
        let err = RuntimeArtifacts::decode_status_artifact(
            STATUS_PATH,
            br#"{
                "schema_version": 1,
                "state": "passed",
                "updated_at": "2026-03-25T20:00:00Z",
                "message": "guest declared readiness"
            }"#,
        )
        .expect_err("malformed status should fail");

        assert!(err.to_string().contains("unknown variant"));
    }

    #[test]
    fn managed_guest_lifecycle_signal_rejects_mismatched_guest_details() {
        let mut status = managed_guest_status_snapshot(LifecycleState::Ready);
        status.details = Some(json!({
            "agent_kind": "pikachat",
            "backend_mode": "native",
            "service_kind": "openclaw_gateway",
            "service_probe_satisfied": true,
            "probe": "openclaw_gateway_health"
        }));

        let err =
            managed_guest_lifecycle_signal(&status, "boot-123").expect_err("details must match");

        assert_eq!(err, "mismatched agent_kind");
    }
}
