use std::collections::BTreeMap;
use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use anyhow::{anyhow, Context};
use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::{response::IntoResponse, Json};
use base64::Engine;
use diesel::result::{DatabaseErrorKind, Error as DieselError};
use diesel::Connection;
use diesel::PgConnection;
use nostr_sdk::prelude::{Keys, PublicKey};
use nostr_sdk::ToBech32;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::agent_api_v1_contract::{
    AgentApiErrorCode, AgentAppState, V1_AGENTS_ENSURE_PATH, V1_AGENTS_ME_PATH,
    V1_AGENTS_RECOVER_PATH,
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
use pika_agent_control_plane::{
    AgentProvisionRequest, AgentStartupPhase, IncusProvisionParams, ManagedVmProvisionParams,
    MicrovmAgentKind, MicrovmProvisionParams, ProviderKind, SpawnerOpenClawLaunchAuth,
    SpawnerVmBackupStatus, SpawnerVmResponse, VmBackupFreshness, GUEST_READY_MARKER_PATH,
};
use pika_agent_microvm::{
    build_create_vm_request, resolve_params, validate_resolved_params, ManagedVmCreateInput,
    MicrovmManagedVmProvider, ResolvedMicrovmAgentBackend, ResolvedMicrovmAgentKind,
    ResolvedMicrovmParams,
};
use pika_relay_profiles::default_message_relays;

const AGENT_OWNER_ACTIVE_INDEX: &str = "agent_instances_owner_active_idx";
const VM_PROVIDER_ENV: &str = "PIKA_AGENT_VM_PROVIDER";
const MICROVM_SPAWNER_URL_ENV: &str = "PIKA_AGENT_MICROVM_SPAWNER_URL";
const INCUS_ENDPOINT_ENV: &str = "PIKA_AGENT_INCUS_ENDPOINT";
const INCUS_PROJECT_ENV: &str = "PIKA_AGENT_INCUS_PROJECT";
const INCUS_PROFILE_ENV: &str = "PIKA_AGENT_INCUS_PROFILE";
const INCUS_STORAGE_POOL_ENV: &str = "PIKA_AGENT_INCUS_STORAGE_POOL";
const INCUS_IMAGE_ALIAS_ENV: &str = "PIKA_AGENT_INCUS_IMAGE_ALIAS";
const INCUS_INSECURE_TLS_ENV: &str = "PIKA_AGENT_INCUS_INSECURE_TLS";
const INCUS_CLIENT_CERT_PATH_ENV: &str = "PIKA_AGENT_INCUS_CLIENT_CERT_PATH";
const INCUS_CLIENT_KEY_PATH_ENV: &str = "PIKA_AGENT_INCUS_CLIENT_KEY_PATH";
const INCUS_SERVER_CERT_PATH_ENV: &str = "PIKA_AGENT_INCUS_SERVER_CERT_PATH";
const INCUS_VM_KIND: &str = "virtual-machine";
const INCUS_PERSISTENT_VOLUME_TYPE: &str = "custom";
const INCUS_PERSISTENT_VOLUME_CONTENT_TYPE: &str = "filesystem";
const INCUS_PERSISTENT_VOLUME_DEVICE_NAME: &str = "pikastate";
const INCUS_PERSISTENT_VOLUME_PATH: &str = "/mnt/pika-state";
const INCUS_CLOUD_INIT_USER_DATA_KEY: &str = "cloud-init.user-data";
const INCUS_BOOTSTRAP_LAUNCHER_PATH: &str = "/workspace/pika-agent/incus-launcher.sh";
const INCUS_STATE_VOLUME_SETUP_PATH: &str = "/workspace/pika-agent/incus-state-volume-setup.sh";
const INCUS_PERSISTENT_AGENT_STATE_ROOT: &str = "/mnt/pika-state/pika-agent";
const INCUS_PERSISTENT_DAEMON_STATE_DIR: &str = "/mnt/pika-state/pika-agent/state";
const INCUS_PERSISTENT_OPENCLAW_STATE_DIR: &str = "/mnt/pika-state/pika-agent/openclaw";
const INCUS_OPERATION_WAIT_TIMEOUT_SECS: i64 = 60;
const INCUS_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
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
    pub runtime_kind: Option<MicrovmAgentKind>,
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
    pub backup_host: Option<String>,
    pub latest_successful_backup_at: Option<String>,
    pub status_copy: String,
    pub reset_requires_confirmation: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ManagedEnvironmentHandle {
    pub owner_npub: String,
    pub agent_id: String,
    pub vm_id: String,
    pub managed_vm: ManagedVmProvisionParams,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ResolvedIncusParams {
    endpoint: String,
    project: String,
    profile: String,
    storage_pool: String,
    image_alias: String,
    insecure_tls: bool,
    agent_kind: ResolvedMicrovmAgentKind,
    agent_backend: ResolvedMicrovmAgentBackend,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ResolvedIncusTlsConfig {
    client_cert_path: Option<String>,
    client_key_path: Option<String>,
    server_cert_path: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum ResolvedManagedVmProviderConfig {
    Microvm(ResolvedMicrovmParams),
    Incus(ResolvedIncusParams),
}

#[derive(Debug, Clone)]
enum ManagedVmProvider {
    Microvm(MicrovmManagedVmProvider),
    Incus(IncusManagedVmProvider),
}

#[derive(Debug, Clone)]
struct IncusManagedVmProvider {
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

#[derive(Debug, Deserialize)]
struct IncusInstanceState {
    status: String,
}

#[derive(Debug, Deserialize)]
struct IncusGuestReadyMarker {
    ready: bool,
    #[serde(default)]
    agent_kind: Option<String>,
    #[serde(default)]
    probe: Option<String>,
}

fn provider_kind_db_value(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Microvm => "microvm",
        ProviderKind::Incus => "incus",
    }
}

fn provider_kind_from_db_value(value: &str) -> anyhow::Result<ProviderKind> {
    match value.trim() {
        "microvm" => Ok(ProviderKind::Microvm),
        "incus" => Ok(ProviderKind::Incus),
        other => anyhow::bail!("unknown managed VM provider stored on row: {other:?}"),
    }
}

fn materialized_managed_vm_params(
    config: &ResolvedManagedVmProviderConfig,
) -> ManagedVmProvisionParams {
    match config {
        ResolvedManagedVmProviderConfig::Microvm(resolved) => ManagedVmProvisionParams {
            provider: Some(ProviderKind::Microvm),
            microvm: Some(MicrovmProvisionParams {
                spawner_url: Some(resolved.spawner_url.clone()),
                kind: Some(match resolved.kind {
                    pika_agent_microvm::ResolvedMicrovmAgentKind::Pi => MicrovmAgentKind::Pi,
                    pika_agent_microvm::ResolvedMicrovmAgentKind::Openclaw => {
                        MicrovmAgentKind::Openclaw
                    }
                }),
                backend: Some(match &resolved.backend {
                    pika_agent_microvm::ResolvedMicrovmAgentBackend::Native => {
                        pika_agent_control_plane::MicrovmAgentBackend::Native
                    }
                    pika_agent_microvm::ResolvedMicrovmAgentBackend::Acp { exec_command, cwd } => {
                        pika_agent_control_plane::MicrovmAgentBackend::Acp {
                            exec_command: Some(exec_command.clone()),
                            cwd: Some(cwd.clone()),
                        }
                    }
                }),
            }),
            incus: None,
        },
        ResolvedManagedVmProviderConfig::Incus(resolved) => ManagedVmProvisionParams {
            provider: Some(ProviderKind::Incus),
            microvm: Some(MicrovmProvisionParams {
                spawner_url: None,
                kind: Some(match resolved.agent_kind {
                    ResolvedMicrovmAgentKind::Pi => MicrovmAgentKind::Pi,
                    ResolvedMicrovmAgentKind::Openclaw => MicrovmAgentKind::Openclaw,
                }),
                backend: Some(match &resolved.agent_backend {
                    ResolvedMicrovmAgentBackend::Native => {
                        pika_agent_control_plane::MicrovmAgentBackend::Native
                    }
                    ResolvedMicrovmAgentBackend::Acp { exec_command, cwd } => {
                        pika_agent_control_plane::MicrovmAgentBackend::Acp {
                            exec_command: Some(exec_command.clone()),
                            cwd: Some(cwd.clone()),
                        }
                    }
                }),
            }),
            incus: Some(IncusProvisionParams {
                endpoint: Some(resolved.endpoint.clone()),
                project: Some(resolved.project.clone()),
                profile: Some(resolved.profile.clone()),
                storage_pool: Some(resolved.storage_pool.clone()),
                image_alias: Some(resolved.image_alias.clone()),
                insecure_tls: Some(resolved.insecure_tls),
            }),
        },
    }
}

fn serialize_managed_vm_provider_config(
    config: &ResolvedManagedVmProviderConfig,
) -> anyhow::Result<String> {
    serde_json::to_string(&materialized_managed_vm_params(config))
        .context("serialize managed VM provider config")
}

fn managed_vm_params_from_row(row: &AgentInstance) -> anyhow::Result<ManagedVmProvisionParams> {
    let provider = provider_kind_from_db_value(&row.provider)?;
    let mut params = match row.provider_config.as_deref() {
        Some(serialized) => serde_json::from_str::<ManagedVmProvisionParams>(serialized)
            .context("decode managed VM provider config from row")?,
        // Legacy rows predate durable provider-config storage, so they only persist provider kind.
        // Existing-VM request handlers may still merge request-scoped provider knobs on top.
        None => ManagedVmProvisionParams::default(),
    };
    if let Some(configured_provider) = params.provider {
        anyhow::ensure!(
            configured_provider == provider,
            "managed VM row provider/config mismatch: row={:?} config={:?}",
            provider,
            configured_provider
        );
    } else {
        params.provider = Some(provider);
    }
    Ok(params)
}

fn merge_microvm_provision_params(
    base: Option<MicrovmProvisionParams>,
    requested: Option<&MicrovmProvisionParams>,
) -> Option<MicrovmProvisionParams> {
    let mut merged = base.unwrap_or_default();
    let mut changed = false;
    if let Some(requested) = requested {
        if requested
            .spawner_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            merged.spawner_url = requested.spawner_url.clone();
            changed = true;
        }
        if requested.kind.is_some() {
            merged.kind = requested.kind;
            changed = true;
        }
        if requested.backend.is_some() {
            merged.backend = requested.backend.clone();
            changed = true;
        }
    }
    if changed || merged.spawner_url.is_some() || merged.kind.is_some() || merged.backend.is_some()
    {
        Some(merged)
    } else {
        None
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
    }
    if changed
        || merged.endpoint.is_some()
        || merged.project.is_some()
        || merged.profile.is_some()
        || merged.storage_pool.is_some()
        || merged.image_alias.is_some()
        || merged.insecure_tls.is_some()
    {
        Some(merged)
    } else {
        None
    }
}

fn managed_vm_params_for_existing_row(
    row: &AgentInstance,
    requested: Option<&ManagedVmProvisionParams>,
) -> anyhow::Result<ManagedVmProvisionParams> {
    let mut params = managed_vm_params_from_row(row)?;
    if row.provider_config.is_some() || requested.is_none() {
        return Ok(params);
    }

    let requested = requested.expect("checked above");
    let row_provider = params
        .provider
        .ok_or_else(|| anyhow::anyhow!("existing row missing managed VM provider"))?;
    if let Some(requested_provider) = requested.provider {
        anyhow::ensure!(
            requested_provider == row_provider,
            "existing managed VM is bound to provider {:?}, got request for {:?}",
            row_provider,
            requested_provider
        );
    }

    match row_provider {
        ProviderKind::Microvm => {
            if requested.incus.as_ref().is_some_and(incus_params_provided) {
                anyhow::bail!(
                    "existing managed VM is bound to microvm, but request supplied incus params"
                );
            }
            params.microvm =
                merge_microvm_provision_params(params.microvm.take(), requested.microvm.as_ref());
        }
        ProviderKind::Incus => {
            if requested
                .microvm
                .as_ref()
                .is_some_and(microvm_params_provided)
            {
                anyhow::bail!(
                    "existing managed VM is bound to incus, but request supplied microvm params"
                );
            }
            params.incus =
                merge_incus_provision_params(params.incus.take(), requested.incus.as_ref());
        }
    }
    Ok(params)
}

fn managed_vm_provider_for_row(
    row: &AgentInstance,
    requested: Option<&ManagedVmProvisionParams>,
) -> anyhow::Result<ManagedVmProvider> {
    let params = managed_vm_params_for_existing_row(row, requested)?;
    managed_vm_provider(Some(&params))
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

#[cfg(test)]
fn required_microvm_spawner_url(raw: Option<String>) -> anyhow::Result<String> {
    raw.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .with_context(|| format!("missing {MICROVM_SPAWNER_URL_ENV}"))
}

fn microvm_kind_from_env() -> Option<MicrovmAgentKind> {
    match std::env::var("PIKA_AGENT_MICROVM_KIND")
        .ok()
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        Some("openclaw") => Some(MicrovmAgentKind::Openclaw),
        Some("pi") => Some(MicrovmAgentKind::Pi),
        _ => None,
    }
}

fn default_microvm_params_from_env() -> MicrovmProvisionParams {
    MicrovmProvisionParams {
        spawner_url: non_empty_env_var(MICROVM_SPAWNER_URL_ENV),
        kind: microvm_kind_from_env(),
        ..MicrovmProvisionParams::default()
    }
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    let is_cgnat = octets[0] == 100 && (64..=127).contains(&octets[1]);
    ip.is_private() || ip.is_loopback() || ip.is_link_local() || is_cgnat
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback() || ip.is_unicast_link_local() || ip.is_unique_local()
}

fn ensure_private_microvm_spawner_url(spawner_url: &str) -> anyhow::Result<()> {
    let uri: Uri = spawner_url.parse().with_context(|| {
        format!("{MICROVM_SPAWNER_URL_ENV} must be a valid URL or URI host, got: {spawner_url}")
    })?;
    let host = uri.host().with_context(|| {
        format!(
            "microvm spawner URL must include an explicit host: {}",
            spawner_url
        )
    })?;
    let normalized_host = host.trim_matches(|c| c == '[' || c == ']');

    if normalized_host.eq_ignore_ascii_case("localhost") {
        return Ok(());
    }
    if let Ok(ipv4) = normalized_host.parse::<Ipv4Addr>() {
        if is_private_ipv4(ipv4) {
            return Ok(());
        }
        anyhow::bail!(
            "microvm spawner host must be private (RFC1918/CGNAT/loopback), got public IPv4 {}",
            normalized_host
        );
    }
    if let Ok(ipv6) = normalized_host.parse::<Ipv6Addr>() {
        if is_private_ipv6(ipv6) {
            return Ok(());
        }
        anyhow::bail!(
            "microvm spawner host must be private (ULA/link-local/loopback), got public IPv6 {}",
            normalized_host
        );
    }

    let is_private_dns_name = normalized_host.ends_with(".internal")
        || normalized_host.ends_with(".tailnet")
        || normalized_host.ends_with(".tailnet.ts.net");
    if is_private_dns_name {
        return Ok(());
    }
    anyhow::bail!(
        "microvm spawner host must be private DNS (.internal/.tailnet/.tailnet.ts.net) or private IP, got {}",
        normalized_host
    );
}

fn non_empty_env_var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn default_vm_provider_kind_from_env() -> anyhow::Result<ProviderKind> {
    match non_empty_env_var(VM_PROVIDER_ENV).as_deref() {
        None | Some("microvm") => Ok(ProviderKind::Microvm),
        Some("incus") => Ok(ProviderKind::Incus),
        Some(other) => {
            anyhow::bail!("{VM_PROVIDER_ENV} must be one of [microvm, incus], got {other:?}")
        }
    }
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
    }
}

fn should_probe_incus_canary_health() -> bool {
    incus_params_provided(&default_incus_params_from_env())
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

fn microvm_params_provided(params: &MicrovmProvisionParams) -> bool {
    params
        .spawner_url
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || params.kind.is_some()
        || params.backend.is_some()
}

fn microvm_transport_params_provided(params: &MicrovmProvisionParams) -> bool {
    params
        .spawner_url
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

fn microvm_runtime_params_provided(params: &MicrovmProvisionParams) -> bool {
    params.kind.is_some() || params.backend.is_some()
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

impl ManagedVmProvider {
    fn ensure_customer_openclaw_flow_supported(&self) -> anyhow::Result<()> {
        match self {
            Self::Microvm(_) => Ok(()),
            Self::Incus(provider) => {
                anyhow::bail!(
                    "managed VM provider incus passed infrastructure checks but customer-facing OpenClaw launch/proxy is not implemented yet (endpoint={})",
                    provider.resolved.endpoint
                )
            }
        }
    }

    fn openclaw_proxy_base_url(&self) -> anyhow::Result<&str> {
        match self {
            Self::Microvm(provider) => Ok(provider.spawner_base_url()),
            Self::Incus(provider) => {
                anyhow::bail!(
                    "managed VM provider incus does not support OpenClaw proxy base URL yet (endpoint={})",
                    provider.resolved.endpoint
                )
            }
        }
    }

    async fn create_managed_vm(
        &self,
        input: ManagedVmCreateInput<'_>,
        request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerVmResponse> {
        match self {
            Self::Microvm(provider) => provider.create_managed_vm(input, request_id).await,
            Self::Incus(provider) => provider.create_managed_vm(input, request_id).await,
        }
    }

    async fn get_vm_status(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerVmResponse> {
        match self {
            Self::Microvm(provider) => provider.get_vm_status(vm_id, request_id).await,
            Self::Incus(provider) => provider.get_vm_status(vm_id, request_id).await,
        }
    }

    async fn get_vm_backup_status(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerVmBackupStatus> {
        match self {
            Self::Microvm(provider) => provider.get_vm_backup_status(vm_id, request_id).await,
            Self::Incus(provider) => provider.get_vm_backup_status(vm_id, request_id).await,
        }
    }

    async fn recover_vm(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerVmResponse> {
        match self {
            Self::Microvm(provider) => provider.recover_vm(vm_id, request_id).await,
            Self::Incus(provider) => provider.recover_vm(vm_id, request_id).await,
        }
    }

    async fn restore_vm(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerVmResponse> {
        match self {
            Self::Microvm(provider) => provider.restore_vm(vm_id, request_id).await,
            Self::Incus(provider) => provider.restore_vm(vm_id, request_id).await,
        }
    }

    async fn delete_vm(&self, vm_id: &str, request_id: Option<&str>) -> anyhow::Result<()> {
        match self {
            Self::Microvm(provider) => provider.delete_vm(vm_id, request_id).await,
            Self::Incus(provider) => provider.delete_vm(vm_id, request_id).await,
        }
    }

    async fn get_openclaw_launch_auth(
        &self,
        vm_id: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerOpenClawLaunchAuth> {
        match self {
            Self::Microvm(provider) => provider.get_openclaw_launch_auth(vm_id, request_id).await,
            Self::Incus(provider) => {
                anyhow::bail!(
                    "managed VM provider incus does not support OpenClaw launch auth yet (endpoint={})",
                    provider.resolved.endpoint
                )
            }
        }
    }
}

impl IncusManagedVmProvider {
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
        Ok(())
    }

    async fn create_managed_vm(
        &self,
        input: ManagedVmCreateInput<'_>,
        request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerVmResponse> {
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
                Ok(SpawnerVmResponse {
                    id: vm_id,
                    status: "starting".to_string(),
                    agent_kind: Some(agent_kind_from_resolved(self.resolved.agent_kind)),
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
    ) -> anyhow::Result<SpawnerVmResponse> {
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
        let guest_ready = if status == "running" {
            self.guest_ready_signal_satisfied(vm_id, request_id).await
        } else {
            false
        };
        Ok(SpawnerVmResponse {
            id: vm_id.to_string(),
            status: status.to_string(),
            agent_kind: Some(agent_kind_from_resolved(self.resolved.agent_kind)),
            startup_probe_satisfied: guest_ready,
            guest_ready,
        })
    }

    async fn get_vm_backup_status(
        &self,
        vm_id: &str,
        _request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerVmBackupStatus> {
        anyhow::bail!("managed VM provider incus does not support backup status yet for VM {vm_id}")
    }

    async fn recover_vm(
        &self,
        vm_id: &str,
        _request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerVmResponse> {
        anyhow::bail!("managed VM provider incus does not support recover yet for VM {vm_id}")
    }

    async fn restore_vm(
        &self,
        vm_id: &str,
        _request_id: Option<&str>,
    ) -> anyhow::Result<SpawnerVmResponse> {
        anyhow::bail!("managed VM provider incus does not support restore yet for VM {vm_id}")
    }

    async fn delete_vm(&self, vm_id: &str, request_id: Option<&str>) -> anyhow::Result<()> {
        let volume_name = self.persistent_volume_name(vm_id);
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
        input: &ManagedVmCreateInput<'_>,
        request_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut devices = BTreeMap::new();
        devices.insert(
            "root".to_string(),
            serde_json::json!({
                "type": "disk",
                "path": "/",
                "pool": self.resolved.storage_pool.as_str(),
            }),
        );
        devices.insert(
            INCUS_PERSISTENT_VOLUME_DEVICE_NAME.to_string(),
            serde_json::json!({
                "type": "disk",
                "pool": self.resolved.storage_pool.as_str(),
                "source": volume_name,
                "path": INCUS_PERSISTENT_VOLUME_PATH,
            }),
        );

        let cloud_init_user_data = self
            .cloud_init_user_data(input)
            .context("build incus bootstrap user-data")?;
        let body = serde_json::json!({
            "name": vm_id,
            "type": INCUS_VM_KIND,
            "start": true,
            "profiles": [self.resolved.profile.as_str()],
            "source": {
                "type": "image",
                "alias": self.resolved.image_alias.as_str(),
            },
            "devices": devices,
            "config": {
                INCUS_CLOUD_INIT_USER_DATA_KEY: cloud_init_user_data,
                "user.pika.provider": "incus",
                "user.pika.state_volume": volume_name,
                "user.pika.agent_kind": match self.resolved.agent_kind {
                    ResolvedMicrovmAgentKind::Pi => "pi",
                    ResolvedMicrovmAgentKind::Openclaw => "openclaw",
                },
            },
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

    fn instance_name_for_input(&self, input: &ManagedVmCreateInput<'_>) -> String {
        let mut hasher = Sha256::new();
        hasher.update(input.bot_pubkey_hex.as_bytes());
        let digest = hasher.finalize();
        format!("pika-agent-{}", &hex::encode(digest)[..20])
    }

    fn persistent_volume_name(&self, vm_id: &str) -> String {
        format!("{vm_id}-state")
    }

    fn cloud_init_user_data(&self, input: &ManagedVmCreateInput<'_>) -> anyhow::Result<String> {
        let bootstrap_request = build_create_vm_request(
            input.owner_pubkey,
            input.relay_urls,
            input.bot_secret_hex,
            input.bot_pubkey_hex,
            &ResolvedMicrovmParams {
                spawner_url: String::new(),
                kind: self.resolved.agent_kind,
                backend: self.resolved.agent_backend.clone(),
            },
        );
        let guest_autostart = bootstrap_request.guest_autostart;
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
                incus_bootstrap_launcher_script(&guest_autostart.env, &guest_autostart.command),
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
        cloud_init.push_str("  - [systemctl, restart, pika-managed-agent.service]\n");
        Ok(cloud_init)
    }

    async fn guest_ready_signal_satisfied(&self, vm_id: &str, request_id: Option<&str>) -> bool {
        let ready_path = format!("/{}", GUEST_READY_MARKER_PATH);
        let marker_bytes = match self
            .get_instance_file(
                vm_id,
                &ready_path,
                request_id,
                "load incus guest ready marker",
            )
            .await
        {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return false,
            Err(err) => {
                tracing::warn!(
                    vm_id = %vm_id,
                    error = %err,
                    "failed to load incus guest ready marker; reporting guest as not ready"
                );
                return false;
            }
        };
        let marker = match serde_json::from_slice::<IncusGuestReadyMarker>(&marker_bytes) {
            Ok(marker) => marker,
            Err(err) => {
                tracing::warn!(
                    vm_id = %vm_id,
                    error = %err,
                    "incus guest ready marker was malformed; reporting guest as not ready"
                );
                return false;
            }
        };
        if !marker.ready {
            return false;
        }
        let expected_agent_kind = match self.resolved.agent_kind {
            ResolvedMicrovmAgentKind::Pi => "pi",
            ResolvedMicrovmAgentKind::Openclaw => "openclaw",
        };
        if marker.agent_kind.as_deref() != Some(expected_agent_kind) {
            tracing::warn!(
                vm_id = %vm_id,
                expected_agent_kind,
                observed_agent_kind = marker.agent_kind.as_deref().unwrap_or("<missing>"),
                "incus guest ready marker reported a mismatched agent kind; reporting guest as not ready"
            );
            return false;
        }
        if marker
            .probe
            .as_deref()
            .is_none_or(|probe| probe.trim().is_empty())
        {
            tracing::warn!(
                vm_id = %vm_id,
                "incus guest ready marker omitted probe detail; reporting guest as not ready"
            );
            return false;
        }
        true
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
                &["1.0", "instances", vm_id, "file"],
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

fn agent_kind_from_resolved(kind: ResolvedMicrovmAgentKind) -> MicrovmAgentKind {
    match kind {
        ResolvedMicrovmAgentKind::Pi => MicrovmAgentKind::Pi,
        ResolvedMicrovmAgentKind::Openclaw => MicrovmAgentKind::Openclaw,
    }
}

fn bootstrap_file_permissions(path: &str) -> &'static str {
    if path == pika_agent_control_plane::GUEST_AUTOSTART_SCRIPT_PATH {
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
    script.push_str("export PIKA_ENABLE_OPENCLAW_PRIVATE_PROXY=0\n");
    script.push_str(
        "if [[ -z \"${PIKA_VM_IP:-}\" ]]; then\n\
PIKA_VM_IP=\"$(python3 - <<'PY'\n\
import socket\n\
\n\
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)\n\
try:\n\
    sock.connect((\"1.1.1.1\", 80))\n\
    print(sock.getsockname()[0])\n\
except OSError:\n\
    pass\n\
finally:\n\
    sock.close()\n\
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
            "Managed OpenClaw needs recovery. Recover first tries to bring the VM back and preserve the durable home; if that VM is gone, Recover provisions a fresh environment instead."
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

fn managed_environment_backup_status_from_spawner(
    backup: SpawnerVmBackupStatus,
) -> ManagedEnvironmentBackupStatus {
    let freshness = match backup.freshness {
        VmBackupFreshness::Healthy => ManagedEnvironmentBackupFreshness::Healthy,
        VmBackupFreshness::Stale => ManagedEnvironmentBackupFreshness::Stale,
        VmBackupFreshness::Missing => ManagedEnvironmentBackupFreshness::Missing,
        VmBackupFreshness::Unavailable => ManagedEnvironmentBackupFreshness::Unavailable,
    };
    let backup_host = (!backup.backup_host.trim().is_empty()).then_some(backup.backup_host);
    let latest_successful_backup_at = backup
        .latest_successful_backup_at
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let status_copy = match freshness {
        ManagedEnvironmentBackupFreshness::Healthy => {
            "Recent durable-home backup protection is in place for this managed environment."
                .to_string()
        }
        ManagedEnvironmentBackupFreshness::Stale => {
            "Backup protection is stale. The latest successful durable-home backup is older than the healthy window, so destructive reset now requires explicit confirmation.".to_string()
        }
        ManagedEnvironmentBackupFreshness::Missing => {
            "No successful durable-home backup is known yet. Treat destructive reset as unsafe until the first backup completes.".to_string()
        }
        ManagedEnvironmentBackupFreshness::Unavailable => {
            "Backup protection could not be verified from the control plane right now. Destructive reset now requires explicit confirmation.".to_string()
        }
        ManagedEnvironmentBackupFreshness::NotProvisioned => {
            "No managed environment exists yet, so backup protection is not tracked.".to_string()
        }
    };

    ManagedEnvironmentBackupStatus {
        freshness,
        backup_host,
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
        backup_host: None,
        latest_successful_backup_at: None,
        reset_requires_confirmation: true,
        status_copy: status_copy.into(),
    }
}

fn phase_from_spawner_vm(vm: &SpawnerVmResponse) -> &'static str {
    match (vm.status.as_str(), vm.guest_ready) {
        ("failed", _) => AGENT_PHASE_ERROR,
        ("running", true) => AGENT_PHASE_READY,
        _ => AGENT_PHASE_CREATING,
    }
}

fn startup_phase_from_spawner_vm(vm: &SpawnerVmResponse) -> AgentStartupPhase {
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

fn is_inflight_provision_row(row: &AgentInstance) -> bool {
    row.phase == AGENT_PHASE_CREATING && row.vm_id.is_none()
}

fn select_visible_agent_row(
    active: Option<AgentInstance>,
    latest: Option<AgentInstance>,
) -> Option<AgentInstance> {
    active.or_else(|| latest.filter(|row| row.phase == AGENT_PHASE_ERROR))
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
    Ok(select_visible_agent_row(active, latest))
}

#[cfg(test)]
fn visible_agent_response(
    conn: &mut PgConnection,
    owner_npub: &str,
    request_id: &str,
    missing_code: AgentApiErrorCode,
) -> Result<Json<AgentStateResponse>, AgentApiError> {
    let Some(active) = load_visible_agent_row(conn, owner_npub)
        .map_err(|err| err.with_request_id(request_id.to_string()))?
    else {
        return Err(AgentApiError::from_code(missing_code).with_request_id(request_id.to_string()));
    };
    let startup_phase = startup_phase_from_row_phase(&active.phase)
        .ok_or_else(|| AgentApiError::from_code(AgentApiErrorCode::Internal))
        .map_err(|err| err.with_request_id(request_id.to_string()))?;
    json_response(active, startup_phase, request_id)
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

struct RefreshedAgentStatus {
    row: AgentInstance,
    startup_phase: AgentStartupPhase,
    runtime_kind: Option<MicrovmAgentKind>,
}

async fn refresh_agent_from_spawner(
    conn: &mut PgConnection,
    row: AgentInstance,
    request_id: &str,
) -> Result<RefreshedAgentStatus, AgentApiError> {
    let Some(vm_id) = row.vm_id.as_deref() else {
        return Ok(RefreshedAgentStatus {
            startup_phase: startup_phase_from_row_phase(&row.phase)
                .unwrap_or(AgentStartupPhase::Requested),
            runtime_kind: None,
            row,
        });
    };
    let provider = match managed_vm_provider_for_row(&row, None) {
        Ok(provider) => provider,
        Err(err) => {
            tracing::warn!(
                request_id,
                agent_id = %row.agent_id,
                vm_id,
                error = %err,
                "failed to resolve managed VM provider while refreshing agent readiness"
            );
            return Ok(RefreshedAgentStatus {
                startup_phase: startup_phase_from_row_phase(&row.phase)
                    .unwrap_or(AgentStartupPhase::ProvisioningVm),
                runtime_kind: None,
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
                return Ok(RefreshedAgentStatus {
                    row,
                    startup_phase: AgentStartupPhase::Failed,
                    runtime_kind: None,
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
            return Ok(RefreshedAgentStatus {
                row: errored,
                startup_phase: AgentStartupPhase::Failed,
                runtime_kind: None,
            });
        }
        Err(err) => {
            tracing::warn!(
                request_id,
                agent_id = %row.agent_id,
                vm_id,
                error = %err,
                "failed to refresh agent readiness from spawner; keeping existing phase"
            );
            return Ok(RefreshedAgentStatus {
                startup_phase: startup_phase_from_row_phase(&row.phase)
                    .unwrap_or(AgentStartupPhase::ProvisioningVm),
                runtime_kind: None,
                row,
            });
        }
    };

    let next_phase = phase_from_spawner_vm(&vm);
    let startup_phase = startup_phase_from_spawner_vm(&vm);
    if row.phase == next_phase && row.vm_id.as_deref() == Some(vm.id.as_str()) {
        return Ok(RefreshedAgentStatus {
            row,
            startup_phase,
            runtime_kind: vm.agent_kind,
        });
    }

    let updated = AgentInstance::update_phase(conn, &row.agent_id, next_phase, Some(&vm.id))
        .map_err(|_| {
            AgentApiError::from_code(AgentApiErrorCode::Internal)
                .with_request_id(request_id.to_string())
        })?;
    Ok(RefreshedAgentStatus {
        row: updated,
        startup_phase,
        runtime_kind: vm.agent_kind,
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
            runtime_kind: None,
            environment_exists: false,
            status_copy: managed_environment_status_copy(None, None),
        });
    };
    let normalized = normalize_loaded_agent_row(&mut conn, row, request_id)?;
    let refreshed = refresh_agent_from_spawner(&mut conn, normalized, request_id).await?;
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
        runtime_kind: refreshed.runtime_kind,
    })
}

pub(crate) async fn load_managed_environment_backup_status(
    status: &ManagedEnvironmentStatus,
    request_id: &str,
) -> ManagedEnvironmentBackupStatus {
    let Some(row) = status.row.as_ref() else {
        return ManagedEnvironmentBackupStatus {
            freshness: ManagedEnvironmentBackupFreshness::NotProvisioned,
            backup_host: None,
            latest_successful_backup_at: None,
            reset_requires_confirmation: false,
            status_copy: "No managed environment exists yet, so backup protection is not tracked."
                .to_string(),
        };
    };

    let Some(vm_id) = row.vm_id.as_deref() else {
        return unavailable_backup_status(
            "No current VM assignment is available, so backup protection cannot be verified from the control plane.",
        );
    };

    let provider = match managed_vm_provider_for_row(row, None) {
        Ok(provider) => provider,
        Err(err) => {
            tracing::warn!(
                request_id = %request_id,
                agent_id = %row.agent_id,
                vm_id = %vm_id,
                error = %err,
                "failed to resolve managed VM provider while loading backup status"
            );
            return unavailable_backup_status(
                "Backup protection could not be verified because the backup host control plane is unavailable.",
            );
        }
    };
    match provider.get_vm_backup_status(vm_id, Some(request_id)).await {
        Ok(backup) => managed_environment_backup_status_from_spawner(backup),
        Err(err) => {
            tracing::warn!(
                request_id = %request_id,
                agent_id = %row.agent_id,
                vm_id = %vm_id,
                error = %err,
                "failed to load backup status from spawner"
            );
            unavailable_backup_status(
                "Backup protection could not be verified from the control plane right now.",
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
    if status.runtime_kind != Some(MicrovmAgentKind::Openclaw) {
        return Err(
            AgentApiError::from_code(AgentApiErrorCode::InvalidRequest).with_request_id(request_id)
        );
    }
    let managed_vm = managed_vm_params_from_row(&row).map_err(|err| {
        tracing::error!(
            request_id = %request_id,
            agent_id = %row.agent_id,
            error = %err,
            "failed to decode managed VM provider config from row"
        );
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    Ok(ManagedEnvironmentHandle {
        owner_npub: row.owner_npub,
        agent_id: row.agent_id,
        vm_id,
        managed_vm,
    })
}

pub(crate) fn openclaw_proxy_base_url(
    managed_vm: &ManagedVmProvisionParams,
    request_id: &str,
) -> Result<String, AgentApiError> {
    managed_vm_provider(Some(managed_vm))
        .and_then(|provider| provider.openclaw_proxy_base_url().map(str::to_string))
        .map_err(|err| {
            tracing::error!(
                request_id = %request_id,
                error = %err,
                "failed to resolve managed VM provider openclaw proxy base url"
            );
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })
}

pub(crate) async fn load_openclaw_launch_auth(
    managed_vm: &ManagedVmProvisionParams,
    vm_id: &str,
    request_id: &str,
) -> Result<SpawnerOpenClawLaunchAuth, AgentApiError> {
    let provider = managed_vm_provider(Some(managed_vm)).map_err(|err| {
        tracing::error!(
            request_id = %request_id,
            vm_id = %vm_id,
            error = %err,
            "failed to resolve managed VM provider for openclaw launch auth"
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

fn resolve_requested_provider_kind(
    requested: Option<&ManagedVmProvisionParams>,
) -> anyhow::Result<ProviderKind> {
    let env_provider = default_vm_provider_kind_from_env()?;
    let explicit_provider = requested.and_then(|params| params.provider);
    let has_microvm_transport_params = requested
        .and_then(|params| params.microvm.as_ref())
        .is_some_and(microvm_transport_params_provided);
    let has_microvm_runtime_params = requested
        .and_then(|params| params.microvm.as_ref())
        .is_some_and(microvm_runtime_params_provided);
    let has_incus_params = requested
        .and_then(|params| params.incus.as_ref())
        .is_some_and(incus_params_provided);
    if has_microvm_transport_params && has_incus_params {
        anyhow::bail!(
            "managed VM request cannot include both microvm and incus params without a single provider selection"
        );
    }
    Ok(match explicit_provider {
        Some(provider) => provider,
        None if has_microvm_transport_params => ProviderKind::Microvm,
        None if has_incus_params => ProviderKind::Incus,
        None if has_microvm_runtime_params && matches!(env_provider, ProviderKind::Incus) => {
            ProviderKind::Incus
        }
        None if has_microvm_runtime_params => ProviderKind::Microvm,
        None => env_provider,
    })
}

fn resolved_spawner_params(
    requested: Option<&ManagedVmProvisionParams>,
) -> anyhow::Result<ResolvedMicrovmParams> {
    let provider = resolve_requested_provider_kind(requested)?;
    let mut params = default_microvm_params_from_env();
    if let Some(requested) = requested {
        if let Some(incus) = requested
            .incus
            .as_ref()
            .filter(|params| incus_params_provided(params))
        {
            anyhow::bail!(
                "managed VM request selected {:?} but also included incus params: {:?}",
                provider,
                incus
            );
        }
        if let Some(requested) = requested.microvm.as_ref() {
            if requested
                .spawner_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_some()
            {
                params.spawner_url = requested.spawner_url.clone();
            }
            if requested.kind.is_some() {
                params.kind = requested.kind;
            }
            if requested.backend.is_some() {
                params.backend = requested.backend.clone();
            }
        }
    }
    if provider != ProviderKind::Microvm {
        anyhow::bail!(
            "managed VM provider {:?} is not the microvm backend",
            provider
        );
    }
    params.spawner_url = Some(required_non_empty_field(
        params.spawner_url.clone(),
        "microvm.spawner_url",
        MICROVM_SPAWNER_URL_ENV,
    )?);
    let resolved = resolve_params(&params);
    validate_resolved_params(&resolved).context("validate microvm agent selection")?;
    ensure_private_microvm_spawner_url(&resolved.spawner_url)
        .context("validate private microvm spawner URL")?;
    Ok(resolved)
}

fn resolved_incus_params(
    requested: Option<&ManagedVmProvisionParams>,
) -> anyhow::Result<ResolvedIncusParams> {
    let provider = resolve_requested_provider_kind(requested)?;
    let mut params = default_incus_params_from_env();
    let mut guest_selection = MicrovmProvisionParams {
        spawner_url: None,
        kind: microvm_kind_from_env(),
        backend: None,
    };
    if let Some(requested) = requested {
        if let Some(microvm) = requested
            .microvm
            .as_ref()
            .filter(|params| microvm_params_provided(params))
        {
            if microvm_transport_params_provided(microvm) {
                anyhow::bail!(
                    "managed VM request selected {:?} but also included microvm transport params: {:?}",
                    provider,
                    microvm
                );
            }
            if microvm.kind.is_some() {
                guest_selection.kind = microvm.kind;
            }
            if microvm.backend.is_some() {
                guest_selection.backend = microvm.backend.clone();
            }
        }
        if let Some(requested) = requested.incus.as_ref() {
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
        }
    }
    if provider != ProviderKind::Incus {
        anyhow::bail!(
            "managed VM provider {:?} is not the incus backend",
            provider
        );
    }
    let guest_selection = resolve_params(&guest_selection);
    validate_resolved_params(&guest_selection).context("validate incus guest selection")?;
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
        agent_kind: guest_selection.kind,
        agent_backend: guest_selection.backend,
    })
}

fn resolve_managed_vm_provider_config(
    requested: Option<&ManagedVmProvisionParams>,
) -> anyhow::Result<ResolvedManagedVmProviderConfig> {
    match resolve_requested_provider_kind(requested)? {
        ProviderKind::Microvm => Ok(ResolvedManagedVmProviderConfig::Microvm(
            resolved_spawner_params(requested)?,
        )),
        ProviderKind::Incus => Ok(ResolvedManagedVmProviderConfig::Incus(
            resolved_incus_params(requested)?,
        )),
    }
}

fn managed_vm_provider_from_resolved(
    config: ResolvedManagedVmProviderConfig,
) -> anyhow::Result<ManagedVmProvider> {
    match config {
        ResolvedManagedVmProviderConfig::Microvm(resolved) => Ok(ManagedVmProvider::Microvm(
            MicrovmManagedVmProvider::new(resolved),
        )),
        ResolvedManagedVmProviderConfig::Incus(resolved) => Ok(ManagedVmProvider::Incus(
            IncusManagedVmProvider::new(resolved)?,
        )),
    }
}

fn managed_vm_provider(
    requested: Option<&ManagedVmProvisionParams>,
) -> anyhow::Result<ManagedVmProvider> {
    managed_vm_provider_from_resolved(resolve_managed_vm_provider_config(requested)?)
}

async fn provision_vm_for_owner(
    owner_npub: &str,
    bot_identity: &ProvisioningBotIdentity,
    request_id: &str,
    provider: &ManagedVmProvider,
) -> anyhow::Result<pika_agent_control_plane::SpawnerVmResponse> {
    let owner_pubkey = PublicKey::parse(owner_npub).context("parse owner npub")?;
    provider
        .create_managed_vm(
            ManagedVmCreateInput {
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
    requested: Option<&ManagedVmProvisionParams>,
) -> Result<AgentInstance, AgentApiError> {
    let bot_identity = generate_provisioning_bot_identity().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    let resolved_provider = resolve_managed_vm_provider_config(requested).map_err(|err| {
        tracing::error!(
            request_id = %request_id,
            owner_npub = %owner_npub,
            error = %err,
            "failed to resolve managed VM provider for provision"
        );
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    let provider = managed_vm_provider_from_resolved(resolved_provider.clone()).map_err(|err| {
        tracing::error!(
            request_id = %request_id,
            owner_npub = %owner_npub,
            error = %err,
            "failed to initialize managed VM provider for provision"
        );
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    let provider_kind = provider_kind_db_value(match &resolved_provider {
        ResolvedManagedVmProviderConfig::Microvm(_) => ProviderKind::Microvm,
        ResolvedManagedVmProviderConfig::Incus(_) => ProviderKind::Incus,
    });
    let provider_config =
        serialize_managed_vm_provider_config(&resolved_provider).map_err(|err| {
            tracing::error!(
                request_id = %request_id,
                owner_npub = %owner_npub,
                error = %err,
                "failed to serialize managed VM provider config for provision"
            );
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;

    let created = {
        let mut conn = state.db_pool.get().map_err(|_| {
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
        conn.transaction::<AgentInstance, anyhow::Error, _>(|conn| {
            let created = AgentInstance::create_with_provider(
                conn,
                owner_npub,
                &bot_identity.pubkey_npub,
                None,
                provider_kind,
                Some(&provider_config),
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

    let vm = match provision_vm_for_owner(owner_npub, &bot_identity, request_id, &provider).await {
        Ok(vm) => vm,
        Err(err) => {
            tracing::error!(
                request_id,
                owner_npub = %owner_npub,
                error = %err,
                "failed to provision managed VM for agent"
            );
            if let Ok(mut conn) = state.db_pool.get() {
                let _ = mark_agent_errored(&mut conn, &created.agent_id);
            }
            return Err(
                AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
            );
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
                phase_from_spawner_vm(&vm),
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
        "provisioned agent managed VM"
    );
    Ok(updated)
}

async fn provision_or_existing_managed_environment(
    state: &State,
    owner_npub: &str,
    request_id: &str,
    requested: Option<&ManagedVmProvisionParams>,
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
    requested: Option<&ManagedVmProvisionParams>,
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
        refresh_agent_from_spawner(&mut conn, normalized, &request_context.request_id).await?;
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
    requested: Option<&ManagedVmProvisionParams>,
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
    if is_inflight_provision_row(&active) {
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
            "Recover could not preserve the previous environment because no recoverable VM was available. Provisioning a fresh Managed OpenClaw environment.",
            request_id,
        )?;
        drop(conn);
        return provision_or_existing_managed_environment(state, owner_npub, request_id, requested)
            .await;
    }
    let vm_id = active.vm_id.clone().ok_or_else(|| {
        AgentApiError::from_code(AgentApiErrorCode::RecoverFailed).with_request_id(request_id)
    })?;

    let provider = managed_vm_provider_for_row(&active, requested).map_err(|_| {
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
                "Recover could not preserve the previous environment because VM {vm_id} was missing. Provisioning a fresh Managed OpenClaw environment."
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
                "failed to recover agent managed VM"
            );
            return Err(AgentApiError::from_code(AgentApiErrorCode::RecoverFailed)
                .with_request_id(request_id));
        }
    };

    let startup_phase = startup_phase_from_spawner_vm(&recovered);
    let updated = conn
        .transaction::<AgentInstance, anyhow::Error, _>(|conn| {
            let updated = AgentInstance::update_phase(
                conn,
                &active.agent_id,
                phase_from_spawner_vm(&recovered),
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
    requested: Option<&ManagedVmProvisionParams>,
) -> Result<ManagedEnvironmentAction, AgentApiError> {
    let existing = {
        let mut conn = state.db_pool.get().map_err(|_| {
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
        let existing = load_visible_agent_row(&mut conn, owner_npub)
            .map_err(|err| err.with_request_id(request_id.to_string()))?;
        if let Some(existing) = existing
            .as_ref()
            .filter(|row| is_inflight_provision_row(row))
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
        let provider =
            managed_vm_provider_for_row(existing.as_ref().expect("existing row"), requested)
                .map_err(|err| {
                    tracing::error!(
                        request_id = %request_id,
                        owner_npub = %owner_npub,
                        error = %err,
                        "failed to resolve stored reset managed VM provider"
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
    requested: Option<&ManagedVmProvisionParams>,
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
    if is_inflight_provision_row(&active) {
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
        "Restore from backup requested for Managed OpenClaw on VM {vm_id}. The durable home will be replaced from the latest backup before the environment is recreated."
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

    let provider = managed_vm_provider_for_row(&active, requested).map_err(|err| {
        tracing::error!(
            request_id = %request_id,
            owner_npub = %owner_npub,
            vm_id = %vm_id,
            error = %err,
            "failed to resolve stored restore managed VM provider"
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
                        "Restore from backup failed. The managed environment was left in error for operator review.",
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

    let startup_phase = startup_phase_from_spawner_vm(&restored);
    let mut conn = state.db_pool.get().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;
    let updated = conn
        .transaction::<AgentInstance, anyhow::Error, _>(|conn| {
            let updated = AgentInstance::update_phase(
                conn,
                &active.agent_id,
                phase_from_spawner_vm(&restored),
                Some(&restored.id),
            )?;
            let message = format!(
                "Restore from backup succeeded. Managed OpenClaw is starting again on VM {} with restored durable-home contents.",
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
    let resolved = resolve_managed_vm_provider_config(None)
        .context("resolve and validate managed VM provider")?;
    let provider = managed_vm_provider_from_resolved(resolved)
        .context("initialize configured managed VM provider")?;
    if let ManagedVmProvider::Incus(incus) = &provider {
        incus.healthcheck().await?;
    }
    if should_probe_incus_canary_health() && !matches!(provider, ManagedVmProvider::Incus(_)) {
        let requested = ManagedVmProvisionParams {
            provider: Some(ProviderKind::Incus),
            microvm: None,
            incus: None,
        };
        let incus = managed_vm_provider(Some(&requested))
            .context("initialize configured Incus canary provider")?;
        match incus {
            ManagedVmProvider::Incus(incus) => incus
                .healthcheck()
                .await
                .context("validate configured Incus canary backend")?,
            ManagedVmProvider::Microvm(_) => {
                unreachable!("explicit incus provider must resolve to Incus")
            }
        }
    }
    provider
        .ensure_customer_openclaw_flow_supported()
        .context("validate managed-agent customer OpenClaw flow")?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::admin::AdminConfig;
    use crate::browser_auth::BrowserAuthConfig;
    use crate::models::group_subscription::GroupFilterInfo;
    use crate::test_support::serial_test_guard;
    use axum::body::to_bytes;
    use axum::http::header;
    use base64::Engine;
    use chrono::NaiveDate;
    use diesel::prelude::*;
    use diesel::r2d2::{ConnectionManager, Pool};
    use diesel_migrations::MigrationHarness;
    use nostr_sdk::prelude::{EventBuilder, Kind, Tag, TagKind};
    use pika_agent_microvm::build_create_vm_request;
    use pika_test_utils::{spawn_one_shot_server, spawn_response_sequence_server};
    use std::collections::HashSet;
    use std::future::Future;

    fn test_agent_instance(agent_id: &str, phase: &str, vm_id: Option<&str>) -> AgentInstance {
        AgentInstance {
            agent_id: agent_id.to_string(),
            owner_npub: "npub1testowner".to_string(),
            vm_id: vm_id.map(str::to_string),
            provider: "microvm".to_string(),
            provider_config: None,
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

    fn init_test_db_connection() -> Option<PgConnection> {
        dotenv::dotenv().ok();
        let Some(url) = std::env::var("DATABASE_URL").ok() else {
            eprintln!("SKIP: DATABASE_URL must be set for agent_api db test");
            return None;
        };
        let mut conn = match PgConnection::establish(&url) {
            Ok(conn) => conn,
            Err(err) => {
                eprintln!("SKIP: postgres unavailable for agent_api db test: {err}");
                return None;
            }
        };
        conn.run_pending_migrations(crate::models::MIGRATIONS)
            .expect("run migrations");
        Some(conn)
    }

    fn init_test_db_pool() -> Option<Pool<ConnectionManager<PgConnection>>> {
        dotenv::dotenv().ok();
        let Some(url) = std::env::var("DATABASE_URL").ok() else {
            eprintln!("SKIP: DATABASE_URL must be set for agent_api db test pool");
            return None;
        };
        if let Err(err) = PgConnection::establish(&url) {
            eprintln!("SKIP: postgres unavailable for agent_api db test pool: {err}");
            return None;
        }
        let manager = ConnectionManager::<PgConnection>::new(url);
        let db_pool = Pool::builder()
            .max_size(4)
            .build(manager)
            .expect("build test db pool");
        let mut conn = db_pool.get().expect("get migration connection");
        conn.run_pending_migrations(crate::models::MIGRATIONS)
            .expect("run migrations");
        Some(db_pool)
    }

    fn clear_test_database(conn: &mut PgConnection) {
        diesel::sql_query(
            "TRUNCATE TABLE managed_environment_events, agent_instances, agent_allowlist_audit, agent_allowlist, group_subscriptions, subscription_info RESTART IDENTITY CASCADE",
        )
        .execute(conn)
        .expect("truncate test tables");
    }

    fn test_state(db_pool: Pool<ConnectionManager<PgConnection>>) -> State {
        let (sender, _receiver) = tokio::sync::watch::channel(GroupFilterInfo::default());
        State {
            db_pool,
            apns_client: None,
            fcm_client: None,
            apns_topic: String::new(),
            channel: std::sync::Arc::new(tokio::sync::Mutex::new(sender)),
            admin_config: std::sync::Arc::new(AdminConfig {
                bootstrap_admins: HashSet::new(),
                browser_auth: BrowserAuthConfig::new(
                    b"0123456789abcdef0123456789abcdef".to_vec(),
                    true,
                    false,
                    None,
                )
                .expect("browser auth config"),
            }),
            min_app_version: "0.0.0".to_string(),
            trust_forwarded_host: false,
        }
    }

    fn with_spawner_env<T>(value: &str, f: impl FnOnce() -> T) -> T {
        let _guard = serial_test_guard();
        let prior = std::env::var(MICROVM_SPAWNER_URL_ENV).ok();
        unsafe {
            std::env::set_var(MICROVM_SPAWNER_URL_ENV, value);
        }
        let result = f();
        match prior {
            Some(prior) => unsafe {
                std::env::set_var(MICROVM_SPAWNER_URL_ENV, prior);
            },
            None => unsafe {
                std::env::remove_var(MICROVM_SPAWNER_URL_ENV);
            },
        }
        result
    }

    fn with_env_overrides<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
        let _guard = serial_test_guard();
        let prior = vars
            .iter()
            .map(|(name, _)| ((*name).to_string(), std::env::var(name).ok()))
            .collect::<Vec<_>>();
        for (name, value) in vars {
            match value {
                Some(value) => unsafe {
                    std::env::set_var(name, value);
                },
                None => unsafe {
                    std::env::remove_var(name);
                },
            }
        }
        let result = f();
        for (name, value) in prior {
            match value {
                Some(value) => unsafe {
                    std::env::set_var(name, value);
                },
                None => unsafe {
                    std::env::remove_var(name);
                },
            }
        }
        result
    }

    fn requested_microvm_params(params: MicrovmProvisionParams) -> ManagedVmProvisionParams {
        ManagedVmProvisionParams {
            provider: Some(ProviderKind::Microvm),
            microvm: Some(params),
            incus: None,
        }
    }

    fn requested_incus_params(params: IncusProvisionParams) -> ManagedVmProvisionParams {
        ManagedVmProvisionParams {
            provider: Some(ProviderKind::Incus),
            microvm: None,
            incus: Some(params),
        }
    }

    const TEST_INCUS_CLIENT_CERT_PEM: &str = r#"-----BEGIN CERTIFICATE-----
MIIDGTCCAgGgAwIBAgIUSLL0u6Or6OhJyD/VqMAFn2AfwxIwDQYJKoZIhvcNAQEL
BQAwHDEaMBgGA1UEAwwRdGVzdC1pbmN1cy1jbGllbnQwHhcNMjYwMzE3MjE1ODA4
WhcNMjYwMzE4MjE1ODA4WjAcMRowGAYDVQQDDBF0ZXN0LWluY3VzLWNsaWVudDCC
ASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBAIcmwlVgzsMaDL7OGIQkJ2Jh
uPeooE/8TWzlXGygsZ6p7Hr0ldWR6FwhkWqMvxP3DLYtrDulNAlDQvdqXiUvLqNB
O3jG3QTG+tra98xD2rC6kPX1Br9K4IY/dIlIDt0wRprzVdmTTn58XyoBj5jHiJ6w
b1uAtVI3sJHEjJSSkZtcFbwe7YveWjLRIugnGLKKXPvRp+lxnSIAygBMroUHwOeP
RwQ42ay4Uea96oWq/Sj9YGT3GUJkFj5rhHh+Tg7svnTv9sKWE2O3mLTSaCVJCujk
z62PIVJqmc4DG/7Paju6uBCfc+TbSGaCTawTdk0QnglZXLHYfqBfP91XZosG6LkC
AwEAAaNTMFEwHQYDVR0OBBYEFIHOohIqdDLl51aC+ORm/SlNmBupMB8GA1UdIwQY
MBaAFIHOohIqdDLl51aC+ORm/SlNmBupMA8GA1UdEwEB/wQFMAMBAf8wDQYJKoZI
hvcNAQELBQADggEBAHGLFSolSFhibpzXeH5ykncCnu9iUs1awhYKGDtWrclSeOKB
Z23bvWdkHKJVJvrE3nN+VGlLTVNA14MnvK2rmXFBhCx9QBdXqfzbxD6NRNFTxzAS
BqZ+h1+rHqc0hQN9an2tPXWuMQsE+Zh2gFDAtuOjYybTr+PRqKv2W4sMtMDH7N7k
xjQ7sRljlkRmzU9pPwgtApJ83/x9+2SO7+tge2ia8oLs3+XvHAf8pEhX+OvQXXPp
+nkb/19iwR7/hNf1gJPKvIF2//tY26XYesM1ORmk0rxiz8bsL/LBmJ0wkv+yy41V
atWQmMQ8cvpIyjH1YV5cDViWH2OobPHNgA1XOMk=
-----END CERTIFICATE-----
"#;

    const TEST_INCUS_CLIENT_KEY_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCHJsJVYM7DGgy+
zhiEJCdiYbj3qKBP/E1s5VxsoLGeqex69JXVkehcIZFqjL8T9wy2Law7pTQJQ0L3
al4lLy6jQTt4xt0Exvra2vfMQ9qwupD19Qa/SuCGP3SJSA7dMEaa81XZk05+fF8q
AY+Yx4iesG9bgLVSN7CRxIyUkpGbXBW8Hu2L3loy0SLoJxiyilz70afpcZ0iAMoA
TK6FB8Dnj0cEONmsuFHmveqFqv0o/WBk9xlCZBY+a4R4fk4O7L507/bClhNjt5i0
0mglSQro5M+tjyFSapnOAxv+z2o7urgQn3Pk20hmgk2sE3ZNEJ4JWVyx2H6gXz/d
V2aLBui5AgMBAAECggEAOk2OKCbLC3+BYA6opNiz5M0jbjNgdSDyhbesV3A7L6c+
TQyWVrvK8XPJt51gEMzSvwSU+GYcPKK3kORiGMhx5huN/FxNnHH6Zc9wdr4O6Y6S
WoiJkJxMn51gOJjNUL4yt0WiE2powkgFBaoGuHHbjhmu8Fpl3kIH+dpAixdvmQVQ
Hg5BTjsu3Hw2+DUgE8JxNrIc67fHWIgsUzOIulYq0LPLTnM4oFSeAA7s6tSQWnC2
Kc5sevg3bA1IszoslIwdTYF5g9xTRKfWtuWwUPSYE4++/OssEoB0epqNozn6gf7W
fXNmEHOhDB6eBwiXZCG4HLxC8r6B2kzsZ/nGfjf6wQKBgQC7AMKAoUzJRV2sRZTp
ap0C/DdzyY54IvgaN7A/nnxxsU2uq1dee1DpGF4aHgoNdo1546P9PY4LUnifBglj
Et369RIWFs8wTJ+uJM5wwIlT6UJCsehI6iosS80XgnsjrIvtDRGSTZNbOjb6m3g/
HIrrt4SztWNj3cWDPqTAe9X0RwKBgQC5BGT6o5wrJ01UQkMmpxbi+Hkje7KVIRWu
hYifKhFGdQBKhvcmHgPkooEEwy2oItphaWDQ4wlz63i4h7pQ/ZKWEMGQEgIT58M0
USu+G0BI9kq7OroIYg2oOqZeVJBGmIPnlqk7PFq/P7YCBtbrcqYu7dM21L9ir1fB
pXN+3qu6/wKBgEUCZcTEQarw7z2Yu/hbgK/OVcRj+DB7byV1sZP4r6HhNXKlBmv2
hAhRFsD6nukS++ikSis1IQsqlxrQRnyKROLMt6zxI+qGDFNef9R6KPOPXAVy0+68
g22vV3M6kqi6jzSeowJjoGKFHC7lWr2nkdik89LBuHjtKWtinbfuuykXAoGAA94d
pkepShWmPi6sbLBtgA0lqyI413k7lMxh0MH2Xnyvpt8vZ3KVLkBfZhQWbj9cRVEI
nxU/61ZuzZy4vlyupchv420c8gGUSRGxUmYLb/sGEOfnX6l9E5k2RR6LbY5eo4a4
vu5CD2FrkptF/uIEq1J5adoErjFwKjIlOe+5s00CgYBPiSt15PUz83TcNXcn6BHL
Fm+QL4t+94HlkGR3BXyrNJ0kdKxM0kgDodXXDhzWdcsape1TUcubzHC90FbXC5NY
eaWH/THQo6Z7ayz1/fqyCldZbdtdEt+JM5lGrRqSSz8MM1+iAAu3w1RON6DDQ/ZL
GFs2pW5hEhS7cCO0qXaa5g==
-----END PRIVATE KEY-----
"#;

    fn write_temp_test_file(prefix: &str, contents: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        path.push(format!("{prefix}-{suffix}-{}.pem", std::process::id()));
        std::fs::write(&path, contents).expect("write temp test file");
        path
    }

    fn cloud_init_write_file_content(user_data: &str, path: &str) -> Option<String> {
        let mut saw_path = false;
        for line in user_data.lines() {
            if line.trim_start() == format!("- path: {path}") {
                saw_path = true;
                continue;
            }
            if saw_path {
                if let Some(encoded) = line.trim_start().strip_prefix("content: ") {
                    let decoded = base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .ok()?;
                    return String::from_utf8(decoded).ok();
                }
                if line.trim_start().starts_with("- path: ") {
                    return None;
                }
            }
        }
        None
    }

    fn with_server_microvm_env<T>(
        spawner_url: &str,
        kind: Option<&str>,
        f: impl FnOnce() -> T,
    ) -> T {
        let _guard = serial_test_guard();
        let _env = ServerMicrovmEnvGuard::set(spawner_url, kind);
        f()
    }

    async fn with_server_microvm_env_async<T, F, Fut>(
        spawner_url: &str,
        kind: Option<&str>,
        f: F,
    ) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let _env = ServerMicrovmEnvGuard::set(spawner_url, kind);
        f().await
    }

    struct ServerMicrovmEnvGuard {
        prior_spawner: Option<String>,
        prior_kind: Option<String>,
        prior_provider: Option<String>,
        prior_incus_endpoint: Option<String>,
        prior_incus_project: Option<String>,
        prior_incus_profile: Option<String>,
        prior_incus_storage_pool: Option<String>,
        prior_incus_image_alias: Option<String>,
        prior_incus_insecure_tls: Option<String>,
        prior_incus_client_cert_path: Option<String>,
        prior_incus_client_key_path: Option<String>,
        prior_incus_server_cert_path: Option<String>,
    }

    impl ServerMicrovmEnvGuard {
        fn set(spawner_url: &str, kind: Option<&str>) -> Self {
            let prior_spawner = std::env::var(MICROVM_SPAWNER_URL_ENV).ok();
            let prior_kind = std::env::var("PIKA_AGENT_MICROVM_KIND").ok();
            let prior_provider = std::env::var(VM_PROVIDER_ENV).ok();
            let prior_incus_endpoint = std::env::var(INCUS_ENDPOINT_ENV).ok();
            let prior_incus_project = std::env::var(INCUS_PROJECT_ENV).ok();
            let prior_incus_profile = std::env::var(INCUS_PROFILE_ENV).ok();
            let prior_incus_storage_pool = std::env::var(INCUS_STORAGE_POOL_ENV).ok();
            let prior_incus_image_alias = std::env::var(INCUS_IMAGE_ALIAS_ENV).ok();
            let prior_incus_insecure_tls = std::env::var(INCUS_INSECURE_TLS_ENV).ok();
            let prior_incus_client_cert_path = std::env::var(INCUS_CLIENT_CERT_PATH_ENV).ok();
            let prior_incus_client_key_path = std::env::var(INCUS_CLIENT_KEY_PATH_ENV).ok();
            let prior_incus_server_cert_path = std::env::var(INCUS_SERVER_CERT_PATH_ENV).ok();
            unsafe {
                std::env::set_var(MICROVM_SPAWNER_URL_ENV, spawner_url);
                std::env::set_var(VM_PROVIDER_ENV, "microvm");
                std::env::remove_var(INCUS_ENDPOINT_ENV);
                std::env::remove_var(INCUS_PROJECT_ENV);
                std::env::remove_var(INCUS_PROFILE_ENV);
                std::env::remove_var(INCUS_STORAGE_POOL_ENV);
                std::env::remove_var(INCUS_IMAGE_ALIAS_ENV);
                std::env::remove_var(INCUS_INSECURE_TLS_ENV);
                std::env::remove_var(INCUS_CLIENT_CERT_PATH_ENV);
                std::env::remove_var(INCUS_CLIENT_KEY_PATH_ENV);
                std::env::remove_var(INCUS_SERVER_CERT_PATH_ENV);
            }
            match kind {
                Some(kind) => unsafe {
                    std::env::set_var("PIKA_AGENT_MICROVM_KIND", kind);
                },
                None => unsafe {
                    std::env::remove_var("PIKA_AGENT_MICROVM_KIND");
                },
            }
            Self {
                prior_spawner,
                prior_kind,
                prior_provider,
                prior_incus_endpoint,
                prior_incus_project,
                prior_incus_profile,
                prior_incus_storage_pool,
                prior_incus_image_alias,
                prior_incus_insecure_tls,
                prior_incus_client_cert_path,
                prior_incus_client_key_path,
                prior_incus_server_cert_path,
            }
        }
    }

    impl Drop for ServerMicrovmEnvGuard {
        fn drop(&mut self) {
            match self.prior_spawner.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var(MICROVM_SPAWNER_URL_ENV, prior);
                },
                None => unsafe {
                    std::env::remove_var(MICROVM_SPAWNER_URL_ENV);
                },
            }
            match self.prior_kind.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var("PIKA_AGENT_MICROVM_KIND", prior);
                },
                None => unsafe {
                    std::env::remove_var("PIKA_AGENT_MICROVM_KIND");
                },
            }
            match self.prior_provider.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var(VM_PROVIDER_ENV, prior);
                },
                None => unsafe {
                    std::env::remove_var(VM_PROVIDER_ENV);
                },
            }
            match self.prior_incus_endpoint.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var(INCUS_ENDPOINT_ENV, prior);
                },
                None => unsafe {
                    std::env::remove_var(INCUS_ENDPOINT_ENV);
                },
            }
            match self.prior_incus_project.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var(INCUS_PROJECT_ENV, prior);
                },
                None => unsafe {
                    std::env::remove_var(INCUS_PROJECT_ENV);
                },
            }
            match self.prior_incus_profile.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var(INCUS_PROFILE_ENV, prior);
                },
                None => unsafe {
                    std::env::remove_var(INCUS_PROFILE_ENV);
                },
            }
            match self.prior_incus_storage_pool.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var(INCUS_STORAGE_POOL_ENV, prior);
                },
                None => unsafe {
                    std::env::remove_var(INCUS_STORAGE_POOL_ENV);
                },
            }
            match self.prior_incus_image_alias.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var(INCUS_IMAGE_ALIAS_ENV, prior);
                },
                None => unsafe {
                    std::env::remove_var(INCUS_IMAGE_ALIAS_ENV);
                },
            }
            match self.prior_incus_insecure_tls.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var(INCUS_INSECURE_TLS_ENV, prior);
                },
                None => unsafe {
                    std::env::remove_var(INCUS_INSECURE_TLS_ENV);
                },
            }
            match self.prior_incus_client_cert_path.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var(INCUS_CLIENT_CERT_PATH_ENV, prior);
                },
                None => unsafe {
                    std::env::remove_var(INCUS_CLIENT_CERT_PATH_ENV);
                },
            }
            match self.prior_incus_client_key_path.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var(INCUS_CLIENT_KEY_PATH_ENV, prior);
                },
                None => unsafe {
                    std::env::remove_var(INCUS_CLIENT_KEY_PATH_ENV);
                },
            }
            match self.prior_incus_server_cert_path.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var(INCUS_SERVER_CERT_PATH_ENV, prior);
                },
                None => unsafe {
                    std::env::remove_var(INCUS_SERVER_CERT_PATH_ENV);
                },
            }
        }
    }

    #[test]
    fn authenticated_requester_npub_requires_nostr_authorization_header() {
        let headers = HeaderMap::new();
        let err = authenticated_requester_npub(&headers, "GET", V1_AGENTS_ME_PATH, false)
            .expect_err("missing header must fail");
        assert_eq!(err.status, StatusCode::UNAUTHORIZED);
        assert_eq!(err.code, AgentApiErrorCode::Unauthorized);
    }

    #[test]
    fn authenticated_requester_npub_accepts_valid_nip98_header() {
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::Custom(27235), "")
            .tags([
                Tag::custom(TagKind::custom("u"), ["https://example.com/v1/agents/me"]),
                Tag::custom(TagKind::custom("method"), ["GET"]),
            ])
            .sign_with_keys(&keys)
            .expect("sign nip98 event");
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&event).expect("serialize event"));

        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Nostr {encoded}").parse().expect("auth value"),
        );
        headers.insert(header::HOST, "example.com".parse().expect("host value"));

        let npub = authenticated_requester_npub(&headers, "GET", V1_AGENTS_ME_PATH, false)
            .expect("extract authenticated npub");
        assert_eq!(
            npub,
            keys.public_key()
                .to_bech32()
                .expect("encode npub")
                .to_lowercase()
        );
    }

    #[test]
    fn authenticated_requester_npub_rejects_mismatched_u_tag_authority() {
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::Custom(27235), "")
            .tags([
                Tag::custom(TagKind::custom("u"), ["https://wrong.example/v1/agents/me"]),
                Tag::custom(TagKind::custom("method"), ["GET"]),
            ])
            .sign_with_keys(&keys)
            .expect("sign nip98 event");
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&event).expect("serialize event"));

        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Nostr {encoded}").parse().expect("auth value"),
        );
        headers.insert(header::HOST, "example.com".parse().expect("host value"));

        let err = authenticated_requester_npub(&headers, "GET", V1_AGENTS_ME_PATH, false)
            .expect_err("mismatched authority must fail");
        assert_eq!(err.status, StatusCode::UNAUTHORIZED);
        assert_eq!(err.code, AgentApiErrorCode::Unauthorized);
    }

    #[test]
    fn authenticated_requester_npub_prefers_x_forwarded_host_over_host() {
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::Custom(27235), "")
            .tags([
                Tag::custom(
                    TagKind::custom("u"),
                    ["https://public.example.com/v1/agents/me"],
                ),
                Tag::custom(TagKind::custom("method"), ["GET"]),
            ])
            .sign_with_keys(&keys)
            .expect("sign nip98 event");
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&event).expect("serialize event"));

        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Nostr {encoded}").parse().expect("auth value"),
        );
        headers.insert(
            header::HOST,
            "127.0.0.1:8080".parse().expect("internal host value"),
        );
        headers.insert(
            "x-forwarded-host",
            "public.example.com".parse().expect("forwarded host value"),
        );

        let npub = authenticated_requester_npub(&headers, "GET", V1_AGENTS_ME_PATH, true)
            .expect("extract authenticated npub");
        assert_eq!(
            npub,
            keys.public_key()
                .to_bech32()
                .expect("encode npub")
                .to_lowercase()
        );
    }

    #[test]
    fn generated_bot_identity_round_trips_npub_and_hex() {
        let identity = generate_provisioning_bot_identity().expect("generate identity");
        let parsed = PublicKey::parse(&identity.pubkey_npub).expect("parse npub");
        assert_eq!(parsed.to_hex(), identity.pubkey_hex);
        assert!(!identity.secret_hex.is_empty());
    }

    #[test]
    fn required_microvm_spawner_url_rejects_missing_value() {
        let err = required_microvm_spawner_url(None).expect_err("missing env must fail");
        assert!(err.to_string().contains(MICROVM_SPAWNER_URL_ENV));
    }

    #[test]
    fn required_microvm_spawner_url_rejects_blank_value() {
        let err =
            required_microvm_spawner_url(Some("   ".to_string())).expect_err("blank env must fail");
        assert!(err.to_string().contains(MICROVM_SPAWNER_URL_ENV));
    }

    #[test]
    fn required_microvm_spawner_url_accepts_non_empty_value() {
        let value = required_microvm_spawner_url(Some("http://127.0.0.1:8080".to_string()))
            .expect("parse spawner url env value");
        assert_eq!(value, "http://127.0.0.1:8080");
    }

    #[test]
    fn private_spawner_url_validation_accepts_localhost() {
        ensure_private_microvm_spawner_url("http://127.0.0.1:8080").expect("localhost url");
    }

    #[test]
    fn private_spawner_url_validation_rejects_public_host() {
        let err = ensure_private_microvm_spawner_url("https://example.com")
            .expect_err("public host must be rejected");
        assert!(err.to_string().contains("private DNS"));
    }

    #[test]
    fn provider_selection_defaults_to_microvm_when_unset() {
        with_env_overrides(&[(VM_PROVIDER_ENV, None)], || {
            let provider = resolve_requested_provider_kind(None).expect("default provider");
            assert_eq!(provider, ProviderKind::Microvm);
        });
    }

    #[test]
    fn provider_selection_infers_microvm_from_request_params() {
        with_env_overrides(
            &[
                (VM_PROVIDER_ENV, Some("incus")),
                (MICROVM_SPAWNER_URL_ENV, Some("http://127.0.0.1:8080")),
            ],
            || {
                let requested = requested_microvm_params(MicrovmProvisionParams {
                    spawner_url: None,
                    kind: Some(MicrovmAgentKind::Openclaw),
                    backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Native),
                });
                let provider =
                    resolve_requested_provider_kind(Some(&requested)).expect("selected provider");
                assert_eq!(provider, ProviderKind::Microvm);
            },
        );
    }

    #[test]
    fn provider_selection_rejects_mixed_provider_specific_params() {
        let requested = ManagedVmProvisionParams {
            provider: None,
            microvm: Some(MicrovmProvisionParams {
                spawner_url: Some("http://127.0.0.1:8080".to_string()),
                kind: None,
                backend: None,
            }),
            incus: Some(IncusProvisionParams {
                endpoint: Some("https://incus.internal:8443".to_string()),
                project: None,
                profile: None,
                storage_pool: None,
                image_alias: None,
                insecure_tls: None,
            }),
        };

        let err = resolve_requested_provider_kind(Some(&requested))
            .expect_err("mixed provider-specific params must fail");
        assert!(err.to_string().contains("both microvm and incus params"));
    }

    #[test]
    fn resolved_incus_params_require_all_scaffolding_fields() {
        let _guard = serial_test_guard();
        let requested = ManagedVmProvisionParams {
            provider: Some(ProviderKind::Incus),
            microvm: None,
            incus: Some(IncusProvisionParams {
                endpoint: Some("https://incus.internal:8443".to_string()),
                project: Some("managed-agents".to_string()),
                profile: Some("pika-agent".to_string()),
                storage_pool: None,
                image_alias: Some("pika-agent/dev".to_string()),
                insecure_tls: None,
            }),
        };

        let err =
            resolved_incus_params(Some(&requested)).expect_err("missing storage pool must fail");
        assert!(err.to_string().contains("incus.storage_pool"));
    }

    #[test]
    fn resolved_managed_vm_provider_config_accepts_incus_request_params() {
        let requested = ManagedVmProvisionParams {
            provider: Some(ProviderKind::Incus),
            microvm: Some(MicrovmProvisionParams {
                spawner_url: None,
                kind: Some(MicrovmAgentKind::Openclaw),
                backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Native),
            }),
            incus: Some(IncusProvisionParams {
                endpoint: Some("https://incus.internal:8443".to_string()),
                project: Some("managed-agents".to_string()),
                profile: Some("pika-agent".to_string()),
                storage_pool: Some("managed-agents-zfs".to_string()),
                image_alias: Some("pika-agent/dev".to_string()),
                insecure_tls: Some(true),
            }),
        };

        let resolved =
            resolve_managed_vm_provider_config(Some(&requested)).expect("resolve incus config");
        assert_eq!(
            resolved,
            ResolvedManagedVmProviderConfig::Incus(ResolvedIncusParams {
                endpoint: "https://incus.internal:8443".to_string(),
                project: "managed-agents".to_string(),
                profile: "pika-agent".to_string(),
                storage_pool: "managed-agents-zfs".to_string(),
                image_alias: "pika-agent/dev".to_string(),
                insecure_tls: true,
                agent_kind: ResolvedMicrovmAgentKind::Openclaw,
                agent_backend: ResolvedMicrovmAgentBackend::Native,
            })
        );
    }

    #[test]
    fn resolved_incus_tls_config_requires_cert_and_key_together() {
        let resolved = ResolvedIncusParams {
            endpoint: "http://127.0.0.1:8443".to_string(),
            project: "managed-agents".to_string(),
            profile: "pika-agent".to_string(),
            storage_pool: "managed-agents-zfs".to_string(),
            image_alias: "pika-agent/dev".to_string(),
            insecure_tls: false,
            agent_kind: ResolvedMicrovmAgentKind::Openclaw,
            agent_backend: ResolvedMicrovmAgentBackend::Native,
        };

        with_env_overrides(
            &[
                (INCUS_CLIENT_CERT_PATH_ENV, Some("/tmp/incus-client.crt")),
                (INCUS_CLIENT_KEY_PATH_ENV, None),
                (INCUS_SERVER_CERT_PATH_ENV, None),
            ],
            || {
                let err = resolved_incus_tls_config(&resolved)
                    .expect_err("missing key should fail validation");
                assert!(err.to_string().contains(INCUS_CLIENT_KEY_PATH_ENV));
            },
        );
    }

    #[test]
    fn resolved_incus_tls_config_requires_client_identity_for_https() {
        let resolved = ResolvedIncusParams {
            endpoint: "https://incus.internal:8443".to_string(),
            project: "managed-agents".to_string(),
            profile: "pika-agent".to_string(),
            storage_pool: "managed-agents-zfs".to_string(),
            image_alias: "pika-agent/dev".to_string(),
            insecure_tls: true,
            agent_kind: ResolvedMicrovmAgentKind::Openclaw,
            agent_backend: ResolvedMicrovmAgentBackend::Native,
        };

        with_env_overrides(
            &[
                (INCUS_CLIENT_CERT_PATH_ENV, None),
                (INCUS_CLIENT_KEY_PATH_ENV, None),
                (INCUS_SERVER_CERT_PATH_ENV, None),
            ],
            || {
                let err = resolved_incus_tls_config(&resolved)
                    .expect_err("https endpoint should require client identity");
                assert!(err.to_string().contains(INCUS_CLIENT_CERT_PATH_ENV));
            },
        );
    }

    #[test]
    fn build_incus_http_client_accepts_valid_client_identity_paths() {
        let cert_path = write_temp_test_file("incus-client-cert", TEST_INCUS_CLIENT_CERT_PEM);
        let key_path = write_temp_test_file("incus-client-key", TEST_INCUS_CLIENT_KEY_PEM);
        let server_cert_path =
            write_temp_test_file("incus-server-cert", TEST_INCUS_CLIENT_CERT_PEM);
        let resolved = ResolvedIncusParams {
            endpoint: "https://incus.internal:8443".to_string(),
            project: "managed-agents".to_string(),
            profile: "pika-agent".to_string(),
            storage_pool: "managed-agents-zfs".to_string(),
            image_alias: "pika-agent/dev".to_string(),
            insecure_tls: true,
            agent_kind: ResolvedMicrovmAgentKind::Openclaw,
            agent_backend: ResolvedMicrovmAgentBackend::Native,
        };

        with_env_overrides(
            &[
                (
                    INCUS_CLIENT_CERT_PATH_ENV,
                    Some(cert_path.to_str().expect("cert path utf8")),
                ),
                (
                    INCUS_CLIENT_KEY_PATH_ENV,
                    Some(key_path.to_str().expect("key path utf8")),
                ),
                (
                    INCUS_SERVER_CERT_PATH_ENV,
                    Some(server_cert_path.to_str().expect("server cert path utf8")),
                ),
            ],
            || {
                build_incus_http_client(&resolved).expect("valid client identity should build");
            },
        );

        std::fs::remove_file(cert_path).ok();
        std::fs::remove_file(key_path).ok();
        std::fs::remove_file(server_cert_path).ok();
    }

    #[test]
    fn resolved_incus_params_require_image_alias() {
        let _guard = serial_test_guard();
        let requested = requested_incus_params(IncusProvisionParams {
            endpoint: Some("https://incus.internal:8443".to_string()),
            project: Some("managed-agents".to_string()),
            profile: Some("pika-agent".to_string()),
            storage_pool: Some("managed-agents-zfs".to_string()),
            image_alias: None,
            insecure_tls: None,
        });

        let err = resolved_incus_params(Some(&requested)).expect_err("missing image alias");
        assert!(err.to_string().contains("incus.image_alias"));
    }

    #[test]
    fn resolved_spawner_params_overlays_requested_acp_backend() {
        with_spawner_env("http://127.0.0.1:8080", || {
            let requested = MicrovmProvisionParams {
                spawner_url: None,
                kind: Some(pika_agent_control_plane::MicrovmAgentKind::Pi),
                backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Acp {
                    exec_command: Some("npx -y pi-acp".to_string()),
                    cwd: Some("/root/pika-agent/acp".to_string()),
                }),
            };

            let requested = requested_microvm_params(requested);
            let resolved = resolved_spawner_params(Some(&requested)).expect("resolve params");
            assert_eq!(resolved.spawner_url, "http://127.0.0.1:8080");
            assert_eq!(
                resolved.kind,
                pika_agent_microvm::ResolvedMicrovmAgentKind::Pi
            );
            assert_eq!(
                resolved.backend,
                pika_agent_microvm::ResolvedMicrovmAgentBackend::Acp {
                    exec_command: "npx -y pi-acp".to_string(),
                    cwd: "/root/pika-agent/acp".to_string(),
                }
            );
        });
    }

    #[test]
    fn resolved_spawner_params_overlays_requested_openclaw_kind() {
        with_spawner_env("http://127.0.0.1:8080", || {
            let requested = MicrovmProvisionParams {
                spawner_url: None,
                kind: Some(pika_agent_control_plane::MicrovmAgentKind::Openclaw),
                backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Native),
            };

            let requested = requested_microvm_params(requested);
            let resolved = resolved_spawner_params(Some(&requested)).expect("resolve params");
            assert_eq!(resolved.spawner_url, "http://127.0.0.1:8080");
            assert_eq!(
                resolved.kind,
                pika_agent_microvm::ResolvedMicrovmAgentKind::Openclaw
            );
            assert_eq!(
                resolved.backend,
                pika_agent_microvm::ResolvedMicrovmAgentBackend::Native
            );
        });
    }

    #[test]
    fn resolved_spawner_params_defaults_pi_to_acp_backend() {
        with_spawner_env("http://127.0.0.1:8080", || {
            let requested = MicrovmProvisionParams {
                spawner_url: None,
                kind: Some(pika_agent_control_plane::MicrovmAgentKind::Pi),
                backend: None,
            };

            let requested = requested_microvm_params(requested);
            let resolved = resolved_spawner_params(Some(&requested)).expect("resolve params");
            assert_eq!(
                resolved.backend,
                pika_agent_microvm::ResolvedMicrovmAgentBackend::Acp {
                    exec_command: pika_agent_microvm::DEFAULT_ACP_EXEC_COMMAND.to_string(),
                    cwd: pika_agent_microvm::DEFAULT_ACP_CWD.to_string(),
                }
            );
        });
    }

    #[test]
    fn resolved_spawner_params_rejects_pi_native_mode() {
        with_spawner_env("http://127.0.0.1:8080", || {
            let requested = MicrovmProvisionParams {
                spawner_url: None,
                kind: Some(pika_agent_control_plane::MicrovmAgentKind::Pi),
                backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Native),
            };

            let requested = requested_microvm_params(requested);
            let err = resolved_spawner_params(Some(&requested)).expect_err("pi native should fail");
            let msg = err.to_string();
            assert!(msg.contains("validate microvm agent selection"));
        });
    }

    #[test]
    fn server_env_selected_openclaw_builds_typed_startup_plan_request() {
        with_server_microvm_env("http://127.0.0.1:8080", Some("openclaw"), || {
            let resolved = resolved_spawner_params(None).expect("resolve server env defaults");
            assert_eq!(
                resolved.kind,
                pika_agent_microvm::ResolvedMicrovmAgentKind::Openclaw
            );
            assert_eq!(
                resolved.backend,
                pika_agent_microvm::ResolvedMicrovmAgentBackend::Native
            );

            let owner_keys = Keys::generate();
            let bot_keys = Keys::generate();
            let request = build_create_vm_request(
                &owner_keys.public_key(),
                &default_message_relays(),
                &bot_keys.secret_key().to_secret_hex(),
                &bot_keys.public_key().to_hex(),
                &resolved,
            );

            let startup_plan = request.guest_autostart.startup_plan.clone();
            assert_eq!(
                startup_plan.agent_kind,
                pika_agent_control_plane::MicrovmAgentKind::Openclaw
            );
            assert_eq!(
                startup_plan.service_kind,
                pika_agent_control_plane::GuestServiceKind::OpenclawGateway
            );
            assert_eq!(
                startup_plan.backend_mode,
                pika_agent_control_plane::GuestServiceBackendMode::Native
            );

            let startup_plan_file = request
                .guest_autostart
                .files
                .get(pika_agent_control_plane::GUEST_STARTUP_PLAN_PATH)
                .expect("startup plan file");
            let serialized_plan: pika_agent_control_plane::GuestStartupPlan =
                serde_json::from_str(startup_plan_file).expect("parse startup plan file");
            assert_eq!(serialized_plan, startup_plan);
        });
    }

    #[tokio::test]
    async fn managed_vm_provider_create_defaults_to_microvm_backend() {
        let (base_url, rx) = spawn_one_shot_server(
            "200 OK",
            r#"{"id":"vm-provider-create","status":"starting","agent_kind":"openclaw"}"#,
        );
        let requested = requested_microvm_params(MicrovmProvisionParams {
            spawner_url: Some(base_url.clone()),
            kind: Some(MicrovmAgentKind::Openclaw),
            backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Native),
        });
        let provider = managed_vm_provider(Some(&requested)).expect("resolve provider");
        let owner_keys = Keys::generate();
        let bot_keys = Keys::generate();
        let created = provider
            .create_managed_vm(
                ManagedVmCreateInput {
                    owner_pubkey: &owner_keys.public_key(),
                    relay_urls: &default_message_relays(),
                    bot_secret_hex: &bot_keys.secret_key().to_secret_hex(),
                    bot_pubkey_hex: &bot_keys.public_key().to_hex(),
                },
                Some("req-provider-create"),
            )
            .await
            .expect("create should succeed");
        assert_eq!(created.id, "vm-provider-create");
        assert_eq!(created.status, "starting");
        assert_eq!(created.agent_kind, Some(MicrovmAgentKind::Openclaw));

        let request = rx.recv().expect("captured create request");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/vms");
        assert_eq!(
            request.headers.get("x-request-id").map(String::as_str),
            Some("req-provider-create")
        );
        assert!(request.body.contains("\"startup_plan\""));
    }

    #[tokio::test]
    async fn managed_vm_provider_recover_uses_microvm_backend() {
        let (base_url, rx) = spawn_one_shot_server(
            "200 OK",
            r#"{"id":"vm-provider-recover","status":"starting","agent_kind":"openclaw"}"#,
        );
        let requested = requested_microvm_params(MicrovmProvisionParams {
            spawner_url: Some(base_url.clone()),
            kind: Some(MicrovmAgentKind::Openclaw),
            backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Native),
        });
        let provider = managed_vm_provider(Some(&requested)).expect("resolve provider");
        let recovered = provider
            .recover_vm("vm-provider-recover", Some("req-provider-recover"))
            .await
            .expect("recover should succeed");
        assert_eq!(recovered.id, "vm-provider-recover");
        assert_eq!(recovered.status, "starting");

        let request = rx.recv().expect("captured recover request");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/vms/vm-provider-recover/recover");
        assert_eq!(
            request.headers.get("x-request-id").map(String::as_str),
            Some("req-provider-recover")
        );
    }

    #[tokio::test]
    async fn managed_vm_provider_delete_uses_microvm_backend() {
        let (base_url, rx) = spawn_one_shot_server("204 No Content", "");
        let requested = requested_microvm_params(MicrovmProvisionParams {
            spawner_url: Some(base_url.clone()),
            kind: Some(MicrovmAgentKind::Openclaw),
            backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Native),
        });
        let provider = managed_vm_provider(Some(&requested)).expect("resolve provider");
        provider
            .delete_vm("vm-provider-delete", Some("req-provider-delete"))
            .await
            .expect("delete should succeed");

        let request = rx.recv().expect("captured delete request");
        assert_eq!(request.method, "DELETE");
        assert_eq!(request.path, "/vms/vm-provider-delete");
        assert_eq!(
            request.headers.get("x-request-id").map(String::as_str),
            Some("req-provider-delete")
        );
    }

    #[tokio::test]
    async fn managed_vm_provider_create_uses_incus_backend() {
        let (base_url, rx) = spawn_response_sequence_server(vec![
            ("200 OK", r#"{"type":"sync","metadata":{}}"#),
            (
                "202 Accepted",
                r#"{"type":"async","operation":"/1.0/operations/op-create","metadata":{"err":""}}"#,
            ),
            ("200 OK", r#"{"type":"sync","metadata":{"err":""}}"#),
            (
                "200 OK",
                r#"{"type":"sync","metadata":{"status":"Running"}}"#,
            ),
            ("404 Not Found", ""),
        ]);
        let requested = ManagedVmProvisionParams {
            provider: Some(ProviderKind::Incus),
            microvm: Some(MicrovmProvisionParams {
                spawner_url: None,
                kind: Some(MicrovmAgentKind::Openclaw),
                backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Native),
            }),
            incus: Some(IncusProvisionParams {
                endpoint: Some(base_url.clone()),
                project: Some("managed-agents".to_string()),
                profile: Some("pika-agent".to_string()),
                storage_pool: Some("managed-agents-zfs".to_string()),
                image_alias: Some("pika-agent/dev".to_string()),
                insecure_tls: Some(true),
            }),
        };
        let provider = managed_vm_provider(Some(&requested)).expect("resolve incus provider");
        let owner_keys = Keys::generate();
        let bot_keys = Keys::generate();
        let created = provider
            .create_managed_vm(
                ManagedVmCreateInput {
                    owner_pubkey: &owner_keys.public_key(),
                    relay_urls: &default_message_relays(),
                    bot_secret_hex: &bot_keys.secret_key().to_secret_hex(),
                    bot_pubkey_hex: &bot_keys.public_key().to_hex(),
                },
                Some("req-incus-create"),
            )
            .await
            .expect("incus create should succeed");
        assert_eq!(created.status, "running");
        assert_eq!(created.agent_kind, Some(MicrovmAgentKind::Openclaw));
        assert!(!created.guest_ready);

        let volume_request = rx.recv().expect("captured volume create request");
        assert_eq!(volume_request.method, "POST");
        assert_eq!(
            volume_request.path,
            "/1.0/storage-pools/managed-agents-zfs/volumes/custom?project=managed-agents"
        );
        let volume_body: serde_json::Value =
            serde_json::from_str(&volume_request.body).expect("parse volume create body");
        let volume_name = volume_body
            .get("name")
            .and_then(serde_json::Value::as_str)
            .expect("volume name");

        let instance_request = rx.recv().expect("captured instance create request");
        assert_eq!(instance_request.method, "POST");
        assert_eq!(
            instance_request.path,
            "/1.0/instances?project=managed-agents"
        );
        assert_eq!(
            instance_request
                .headers
                .get("x-request-id")
                .map(String::as_str),
            Some("req-incus-create")
        );
        let instance_body: serde_json::Value =
            serde_json::from_str(&instance_request.body).expect("parse instance create body");
        let instance_name = instance_body
            .get("name")
            .and_then(serde_json::Value::as_str)
            .expect("instance name");
        assert_eq!(created.id, instance_name);
        assert_eq!(volume_name, format!("{instance_name}-state"));
        assert_eq!(instance_body["type"], "virtual-machine");
        assert_eq!(instance_body["source"]["alias"], "pika-agent/dev");
        assert_eq!(
            instance_body["devices"][INCUS_PERSISTENT_VOLUME_DEVICE_NAME]["path"],
            INCUS_PERSISTENT_VOLUME_PATH
        );
        assert_eq!(
            instance_body["devices"][INCUS_PERSISTENT_VOLUME_DEVICE_NAME]["source"],
            volume_name
        );
        let user_data = instance_body["config"][INCUS_CLOUD_INIT_USER_DATA_KEY]
            .as_str()
            .expect("cloud-init user-data");
        let launcher = cloud_init_write_file_content(user_data, INCUS_BOOTSTRAP_LAUNCHER_PATH)
            .expect("launcher script in cloud-init");
        assert!(launcher.contains("export PIKA_OWNER_PUBKEY="));
        assert!(launcher.contains("export PIKA_RELAY_URLS="));
        assert!(launcher.contains("export PIKA_BOT_PUBKEY="));
        assert!(launcher.contains("export PIKA_ENABLE_OPENCLAW_PRIVATE_PROXY=0"));
        assert!(launcher.contains("sock.connect((\"1.1.1.1\", 80))"));
        assert!(launcher.contains("exec bash /workspace/pika-agent/start-agent.sh"));
        assert!(user_data.contains("runcmd:"));
        assert!(user_data.contains("systemctl, restart, pika-managed-agent.service"));
        let state_setup = cloud_init_write_file_content(user_data, INCUS_STATE_VOLUME_SETUP_PATH)
            .expect("state-volume setup script in cloud-init");
        assert!(state_setup.contains(INCUS_PERSISTENT_DAEMON_STATE_DIR));
        assert!(state_setup.contains(INCUS_PERSISTENT_OPENCLAW_STATE_DIR));
        assert!(state_setup.contains("link_state_dir \"$agent_root/state\""));
        assert!(state_setup.contains("link_state_dir \"$agent_root/openclaw\""));
        let startup_plan = cloud_init_write_file_content(
            user_data,
            &format!("/{}", pika_agent_control_plane::GUEST_STARTUP_PLAN_PATH),
        )
        .expect("startup plan in cloud-init");
        assert!(startup_plan.contains("\"agent_kind\": \"openclaw\""));
        assert!(
            !user_data.contains("/etc/systemd/system/pika-managed-agent.service"),
            "service unit should be baked into the Incus guest image, not written by cloud-init"
        );
        assert_eq!(instance_body["config"]["user.pika.agent_kind"], "openclaw");

        let wait_request = rx.recv().expect("captured operation wait request");
        assert_eq!(wait_request.method, "GET");
        assert_eq!(
            wait_request.path,
            "/1.0/operations/op-create/wait?timeout=60"
        );

        let status_request = rx.recv().expect("captured status request");
        assert_eq!(status_request.method, "GET");
        assert_eq!(
            status_request.path,
            format!("/1.0/instances/{instance_name}/state?project=managed-agents")
        );
        let ready_request = rx.recv().expect("captured ready-marker request");
        assert_eq!(ready_request.method, "GET");
        assert_eq!(
            ready_request.path,
            format!(
                "/1.0/instances/{instance_name}/file?project=managed-agents&path=%2Fworkspace%2Fpika-agent%2Fservice-ready.json"
            )
        );
    }

    #[tokio::test]
    async fn managed_vm_provider_status_marks_guest_ready_from_in_guest_signal() {
        let vm_id = "pika-agent-ready";
        let ready_marker = serde_json::json!({
            "ready": true,
            "agent_kind": "openclaw",
            "probe": "openclaw_gateway_health",
        });
        let (base_url, rx) = spawn_response_sequence_server(vec![
            (
                "200 OK",
                r#"{"type":"sync","metadata":{"status":"Running"}}"#,
            ),
            ("200 OK", &ready_marker.to_string()),
        ]);
        let requested = ManagedVmProvisionParams {
            provider: Some(ProviderKind::Incus),
            microvm: Some(MicrovmProvisionParams {
                spawner_url: None,
                kind: Some(MicrovmAgentKind::Openclaw),
                backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Native),
            }),
            incus: Some(IncusProvisionParams {
                endpoint: Some(base_url.clone()),
                project: Some("managed-agents".to_string()),
                profile: Some("pika-agent".to_string()),
                storage_pool: Some("managed-agents-zfs".to_string()),
                image_alias: Some("pika-agent/dev".to_string()),
                insecure_tls: None,
            }),
        };
        let provider = managed_vm_provider(Some(&requested)).expect("resolve incus provider");
        let status = provider
            .get_vm_status(vm_id, Some("req-incus-ready"))
            .await
            .expect("load incus status");
        assert_eq!(status.status, "running");
        assert!(status.startup_probe_satisfied);
        assert!(status.guest_ready);

        let state_request = rx.recv().expect("captured state request");
        assert_eq!(
            state_request.path,
            format!("/1.0/instances/{vm_id}/state?project=managed-agents")
        );
        let ready_request = rx.recv().expect("captured ready-marker request");
        assert_eq!(
            ready_request.path,
            format!(
                "/1.0/instances/{vm_id}/file?project=managed-agents&path=%2Fworkspace%2Fpika-agent%2Fservice-ready.json"
            )
        );
    }

    #[tokio::test]
    async fn managed_vm_provider_status_keeps_guest_unready_when_ready_signal_is_malformed() {
        let vm_id = "pika-agent-malformed";
        let (base_url, _rx) = spawn_response_sequence_server(vec![
            (
                "200 OK",
                r#"{"type":"sync","metadata":{"status":"Running"}}"#,
            ),
            ("200 OK", "not-json"),
        ]);
        let requested = ManagedVmProvisionParams {
            provider: Some(ProviderKind::Incus),
            microvm: Some(MicrovmProvisionParams {
                spawner_url: None,
                kind: Some(MicrovmAgentKind::Openclaw),
                backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Native),
            }),
            incus: Some(IncusProvisionParams {
                endpoint: Some(base_url.clone()),
                project: Some("managed-agents".to_string()),
                profile: Some("pika-agent".to_string()),
                storage_pool: Some("managed-agents-zfs".to_string()),
                image_alias: Some("pika-agent/dev".to_string()),
                insecure_tls: None,
            }),
        };
        let provider = managed_vm_provider(Some(&requested)).expect("resolve incus provider");
        let status = provider
            .get_vm_status(vm_id, Some("req-incus-malformed"))
            .await
            .expect("load incus status");
        assert_eq!(status.status, "running");
        assert!(!status.startup_probe_satisfied);
        assert!(!status.guest_ready);
    }

    #[tokio::test]
    async fn managed_vm_provider_create_cleans_up_instance_and_volume_after_failed_start() {
        let (base_url, rx) = spawn_response_sequence_server(vec![
            ("200 OK", r#"{"type":"sync","metadata":{}}"#),
            (
                "202 Accepted",
                r#"{"type":"async","operation":"/1.0/operations/op-create","metadata":{"err":""}}"#,
            ),
            (
                "200 OK",
                r#"{"type":"sync","metadata":{"err":"instance failed to start"}}"#,
            ),
            (
                "202 Accepted",
                r#"{"type":"async","operation":"/1.0/operations/op-cleanup","metadata":{"err":""}}"#,
            ),
            ("200 OK", r#"{"type":"sync","metadata":{"err":""}}"#),
            ("200 OK", r#"{"type":"sync","metadata":{}}"#),
        ]);
        let requested = ManagedVmProvisionParams {
            provider: Some(ProviderKind::Incus),
            microvm: Some(MicrovmProvisionParams {
                spawner_url: None,
                kind: Some(MicrovmAgentKind::Pi),
                backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Acp {
                    exec_command: Some("npx -y pi-acp".to_string()),
                    cwd: Some("/root/pika-agent/acp".to_string()),
                }),
            }),
            incus: Some(IncusProvisionParams {
                endpoint: Some(base_url.clone()),
                project: Some("managed-agents".to_string()),
                profile: Some("pika-agent".to_string()),
                storage_pool: Some("managed-agents-zfs".to_string()),
                image_alias: Some("pika-agent/dev".to_string()),
                insecure_tls: None,
            }),
        };
        let provider = managed_vm_provider(Some(&requested)).expect("resolve incus provider");
        let owner_keys = Keys::generate();
        let bot_keys = Keys::generate();
        let err = provider
            .create_managed_vm(
                ManagedVmCreateInput {
                    owner_pubkey: &owner_keys.public_key(),
                    relay_urls: &default_message_relays(),
                    bot_secret_hex: &bot_keys.secret_key().to_secret_hex(),
                    bot_pubkey_hex: &bot_keys.public_key().to_hex(),
                },
                Some("req-incus-create-cleanup"),
            )
            .await
            .expect_err("incus create should fail after operation error");
        assert!(err.to_string().contains("instance failed to start"));

        let volume_create = rx.recv().expect("captured volume create");
        let volume_body: serde_json::Value =
            serde_json::from_str(&volume_create.body).expect("parse volume body");
        assert_eq!(
            volume_create.path,
            "/1.0/storage-pools/managed-agents-zfs/volumes/custom?project=managed-agents"
        );
        assert_eq!(volume_body["content_type"], "filesystem");
        assert!(volume_body.get("type").is_none());
        let instance_create = rx.recv().expect("captured instance create");
        let instance_body: serde_json::Value =
            serde_json::from_str(&instance_create.body).expect("parse instance body");
        let instance_name = instance_body["name"].as_str().expect("instance name");
        assert_eq!(volume_body["name"], format!("{instance_name}-state"));
        assert_eq!(
            volume_body["description"],
            format!("Persistent managed-agent state volume for {instance_name}-state")
        );
        let _create_wait = rx.recv().expect("captured create wait");
        let cleanup_delete = rx.recv().expect("captured cleanup delete");
        assert_eq!(
            cleanup_delete.path,
            format!("/1.0/instances/{instance_name}?project=managed-agents")
        );
        let _cleanup_wait = rx.recv().expect("captured cleanup wait");
        let volume_delete = rx.recv().expect("captured volume delete");
        assert_eq!(
            volume_delete.path,
            format!(
                "/1.0/storage-pools/managed-agents-zfs/volumes/custom/{instance_name}-state?project=managed-agents"
            )
        );
    }

    #[tokio::test]
    async fn managed_vm_provider_delete_uses_incus_backend_and_deletes_volume() {
        let vm_id = "pika-agent-testdelete";
        let (base_url, rx) = spawn_response_sequence_server(vec![
            (
                "202 Accepted",
                r#"{"type":"async","operation":"/1.0/operations/op-delete","metadata":{"err":""}}"#,
            ),
            ("200 OK", r#"{"type":"sync","metadata":{"err":""}}"#),
            ("200 OK", r#"{"type":"sync","metadata":{}}"#),
        ]);
        let requested = requested_incus_params(IncusProvisionParams {
            endpoint: Some(base_url.clone()),
            project: Some("managed-agents".to_string()),
            profile: Some("pika-agent".to_string()),
            storage_pool: Some("managed-agents-zfs".to_string()),
            image_alias: Some("pika-agent/dev".to_string()),
            insecure_tls: None,
        });
        let provider = managed_vm_provider(Some(&requested)).expect("resolve incus provider");
        provider
            .delete_vm(vm_id, Some("req-incus-delete"))
            .await
            .expect("incus delete should succeed");

        let instance_delete = rx.recv().expect("captured instance delete request");
        assert_eq!(instance_delete.method, "DELETE");
        assert_eq!(
            instance_delete.path,
            format!("/1.0/instances/{vm_id}?project=managed-agents")
        );
        assert_eq!(
            instance_delete
                .headers
                .get("x-request-id")
                .map(String::as_str),
            Some("req-incus-delete")
        );

        let wait_request = rx.recv().expect("captured delete wait request");
        assert_eq!(
            wait_request.path,
            "/1.0/operations/op-delete/wait?timeout=60"
        );

        let volume_delete = rx.recv().expect("captured volume delete request");
        assert_eq!(volume_delete.method, "DELETE");
        assert_eq!(
            volume_delete.path,
            format!(
                "/1.0/storage-pools/managed-agents-zfs/volumes/custom/{vm_id}-state?project=managed-agents"
            )
        );
    }

    #[test]
    fn incus_row_provider_config_routes_status_requests_through_stored_endpoint() {
        let (base_url, rx) = spawn_response_sequence_server(vec![
            (
                "200 OK",
                r#"{"type":"sync","metadata":{"status":"Running"}}"#,
            ),
            ("404 Not Found", ""),
        ]);
        let provider_config = serialize_managed_vm_provider_config(
            &ResolvedManagedVmProviderConfig::Incus(ResolvedIncusParams {
                endpoint: base_url.clone(),
                project: "managed-agents".to_string(),
                profile: "pika-agent".to_string(),
                storage_pool: "managed-agents-zfs".to_string(),
                image_alias: "pika-agent/dev".to_string(),
                insecure_tls: true,
                agent_kind: ResolvedMicrovmAgentKind::Openclaw,
                agent_backend: ResolvedMicrovmAgentBackend::Native,
            }),
        )
        .expect("serialize incus row provider config");
        let row = AgentInstance {
            provider: "incus".to_string(),
            provider_config: Some(provider_config),
            ..test_agent_instance(
                "agent-incus-row",
                AGENT_PHASE_CREATING,
                Some("vm-incus-row"),
            )
        };

        with_env_overrides(
            &[
                (VM_PROVIDER_ENV, Some("microvm")),
                (MICROVM_SPAWNER_URL_ENV, Some("http://127.0.0.1:9999")),
            ],
            || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build runtime");
                runtime.block_on(async {
                    let provider =
                        managed_vm_provider_for_row(&row, None).expect("resolve row provider");
                    let status = provider
                        .get_vm_status("vm-incus-row", Some("req-incus-row"))
                        .await
                        .expect("load incus status");
                    assert_eq!(status.status, "running");
                    assert_eq!(status.agent_kind, Some(MicrovmAgentKind::Openclaw));
                    assert!(!status.guest_ready);
                });
            },
        );

        let request = rx.recv().expect("captured row status request");
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.path,
            "/1.0/instances/vm-incus-row/state?project=managed-agents"
        );
        let ready_request = rx.recv().expect("captured row ready-marker request");
        assert_eq!(
            ready_request.path,
            "/1.0/instances/vm-incus-row/file?project=managed-agents&path=%2Fworkspace%2Fpika-agent%2Fservice-ready.json"
        );
    }

    #[test]
    fn agent_api_healthcheck_rejects_incus_when_customer_openclaw_flow_is_unimplemented() {
        let (base_url, rx) = spawn_response_sequence_server(vec![
            ("200 OK", r#"{"type":"sync","metadata":{}}"#),
            (
                "200 OK",
                r#"{"type":"sync","metadata":{"devices":{"eth0":{"type":"nic","network":"incusbr0"}}}}"#,
            ),
            ("200 OK", r#"{"type":"sync","metadata":{}}"#),
            ("200 OK", r#"{"type":"sync","metadata":{}}"#),
        ]);
        with_env_overrides(
            &[
                (VM_PROVIDER_ENV, Some("incus")),
                (INCUS_ENDPOINT_ENV, Some(base_url.as_str())),
                (INCUS_PROJECT_ENV, Some("managed-agents")),
                (INCUS_PROFILE_ENV, Some("pika-agent")),
                (INCUS_STORAGE_POOL_ENV, Some("managed-agents-zfs")),
                (INCUS_IMAGE_ALIAS_ENV, Some("pika-agent/dev")),
                (INCUS_INSECURE_TLS_ENV, Some("true")),
            ],
            || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build runtime");
                let err = runtime
                    .block_on(agent_api_healthcheck())
                    .expect_err("incus healthcheck should reject unsupported customer flow");
                assert!(err
                    .to_string()
                    .contains("validate managed-agent customer OpenClaw flow"));
            },
        );

        let project_request = rx.recv().expect("project probe");
        assert_eq!(project_request.path, "/1.0/projects/managed-agents");
        let profile_request = rx.recv().expect("profile probe");
        assert_eq!(
            profile_request.path,
            "/1.0/profiles/pika-agent?project=managed-agents"
        );
        let pool_request = rx.recv().expect("storage pool probe");
        assert_eq!(
            pool_request.path,
            "/1.0/storage-pools/managed-agents-zfs?project=managed-agents"
        );
        let image_request = rx.recv().expect("image alias probe");
        assert_eq!(
            image_request.path,
            "/1.0/images/aliases/pika-agent%2Fdev?project=managed-agents"
        );
    }

    #[test]
    fn agent_api_healthcheck_rejects_incus_profile_without_nic() {
        let (base_url, rx) = spawn_response_sequence_server(vec![
            ("200 OK", r#"{"type":"sync","metadata":{}}"#),
            ("200 OK", r#"{"type":"sync","metadata":{"devices":{}}}"#),
        ]);
        with_env_overrides(
            &[
                (VM_PROVIDER_ENV, Some("incus")),
                (INCUS_ENDPOINT_ENV, Some(base_url.as_str())),
                (INCUS_PROJECT_ENV, Some("managed-agents")),
                (INCUS_PROFILE_ENV, Some("pika-agent")),
                (INCUS_STORAGE_POOL_ENV, Some("managed-agents-zfs")),
                (INCUS_IMAGE_ALIAS_ENV, Some("pika-agent/dev")),
                (INCUS_INSECURE_TLS_ENV, Some("true")),
            ],
            || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build runtime");
                let err = runtime
                    .block_on(agent_api_healthcheck())
                    .expect_err("incus healthcheck should reject profile without nic");
                assert!(err
                    .to_string()
                    .contains("must include at least one nic device"));
            },
        );

        let project_request = rx.recv().expect("project probe");
        assert_eq!(project_request.path, "/1.0/projects/managed-agents");
        let profile_request = rx.recv().expect("profile probe");
        assert_eq!(
            profile_request.path,
            "/1.0/profiles/pika-agent?project=managed-agents"
        );
    }

    #[test]
    fn agent_api_healthcheck_probes_configured_incus_canary_backend_when_microvm_is_default() {
        let (base_url, rx) = spawn_response_sequence_server(vec![
            ("200 OK", r#"{"type":"sync","metadata":{}}"#),
            (
                "200 OK",
                r#"{"type":"sync","metadata":{"devices":{"eth0":{"type":"nic","network":"incusbr0"}}}}"#,
            ),
            ("200 OK", r#"{"type":"sync","metadata":{}}"#),
            ("200 OK", r#"{"type":"sync","metadata":{}}"#),
        ]);
        with_env_overrides(
            &[
                (VM_PROVIDER_ENV, Some("microvm")),
                (MICROVM_SPAWNER_URL_ENV, Some("http://127.0.0.1:8080")),
                (INCUS_ENDPOINT_ENV, Some(base_url.as_str())),
                (INCUS_PROJECT_ENV, Some("managed-agents")),
                (INCUS_PROFILE_ENV, Some("pika-agent")),
                (INCUS_STORAGE_POOL_ENV, Some("managed-agents-zfs")),
                (INCUS_IMAGE_ALIAS_ENV, Some("pika-agent/dev")),
                (INCUS_INSECURE_TLS_ENV, None),
                (INCUS_CLIENT_CERT_PATH_ENV, None),
                (INCUS_CLIENT_KEY_PATH_ENV, None),
                (INCUS_SERVER_CERT_PATH_ENV, None),
            ],
            || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build runtime");
                runtime
                    .block_on(agent_api_healthcheck())
                    .expect("microvm-default healthcheck should probe configured incus canary");
            },
        );

        let project_request = rx.recv().expect("project probe");
        assert_eq!(project_request.path, "/1.0/projects/managed-agents");
        let profile_request = rx.recv().expect("profile probe");
        assert_eq!(
            profile_request.path,
            "/1.0/profiles/pika-agent?project=managed-agents"
        );
        let pool_request = rx.recv().expect("storage pool probe");
        assert_eq!(
            pool_request.path,
            "/1.0/storage-pools/managed-agents-zfs?project=managed-agents"
        );
        let image_request = rx.recv().expect("image alias probe");
        assert_eq!(
            image_request.path,
            "/1.0/images/aliases/pika-agent%2Fdev?project=managed-agents"
        );
    }

    #[test]
    fn refresh_agent_from_spawner_uses_row_provider_config_over_current_env() {
        let _guard = serial_test_guard();
        let Some(mut conn) = init_test_db_connection() else {
            return;
        };
        clear_test_database(&mut conn);

        let (base_url, rx) = spawn_one_shot_server(
            "200 OK",
            r#"{"id":"vm-row-provider","status":"running","agent_kind":"openclaw","guest_ready":true}"#,
        );
        let provider_config = serialize_managed_vm_provider_config(
            &ResolvedManagedVmProviderConfig::Microvm(ResolvedMicrovmParams {
                spawner_url: base_url.clone(),
                kind: pika_agent_microvm::ResolvedMicrovmAgentKind::Openclaw,
                backend: pika_agent_microvm::ResolvedMicrovmAgentBackend::Native,
            }),
        )
        .expect("serialize row provider config");
        let row = AgentInstance::create_with_provider(
            &mut conn,
            "npub1rowproviderconfig",
            "agent-row-provider",
            Some("vm-row-provider"),
            "microvm",
            Some(&provider_config),
            AGENT_PHASE_CREATING,
        )
        .expect("insert managed environment row");

        with_env_overrides(&[(MICROVM_SPAWNER_URL_ENV, None)], || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build runtime");
            runtime.block_on(async {
                let refreshed = refresh_agent_from_spawner(&mut conn, row, "req-row-provider")
                    .await
                    .expect("refresh should use row provider config");
                assert_eq!(refreshed.row.vm_id.as_deref(), Some("vm-row-provider"));
                assert_eq!(refreshed.startup_phase, AgentStartupPhase::Ready);
                assert_eq!(refreshed.runtime_kind, Some(MicrovmAgentKind::Openclaw));
            });
        });

        let request = rx.recv().expect("captured refresh request");
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/vms/vm-row-provider");

        clear_test_database(&mut conn);
    }

    #[tokio::test]
    async fn with_server_microvm_env_async_keeps_env_set_until_future_completes() {
        let _guard = serial_test_guard();
        let prior_spawner = std::env::var(MICROVM_SPAWNER_URL_ENV).ok();
        let prior_kind = std::env::var("PIKA_AGENT_MICROVM_KIND").ok();
        unsafe {
            std::env::set_var(MICROVM_SPAWNER_URL_ENV, "http://prior-spawner:1234");
            std::env::set_var("PIKA_AGENT_MICROVM_KIND", "pi");
        }

        with_server_microvm_env_async("http://test-spawner:8080", Some("openclaw"), || async {
            tokio::task::yield_now().await;
            assert_eq!(
                std::env::var(MICROVM_SPAWNER_URL_ENV).ok().as_deref(),
                Some("http://test-spawner:8080")
            );
            assert_eq!(
                std::env::var("PIKA_AGENT_MICROVM_KIND").ok().as_deref(),
                Some("openclaw")
            );
        })
        .await;

        assert_eq!(
            std::env::var(MICROVM_SPAWNER_URL_ENV).ok().as_deref(),
            Some("http://prior-spawner:1234")
        );
        assert_eq!(
            std::env::var("PIKA_AGENT_MICROVM_KIND").ok().as_deref(),
            Some("pi")
        );

        match prior_spawner {
            Some(prior) => unsafe {
                std::env::set_var(MICROVM_SPAWNER_URL_ENV, prior);
            },
            None => unsafe {
                std::env::remove_var(MICROVM_SPAWNER_URL_ENV);
            },
        }
        match prior_kind {
            Some(prior) => unsafe {
                std::env::set_var("PIKA_AGENT_MICROVM_KIND", prior);
            },
            None => unsafe {
                std::env::remove_var("PIKA_AGENT_MICROVM_KIND");
            },
        }
    }

    #[test]
    fn select_visible_agent_row_prefers_active_row_over_newer_error_row() {
        let active = test_agent_instance("agent-active", AGENT_PHASE_READY, Some("vm-active"));
        let latest_error = test_agent_instance("agent-error", AGENT_PHASE_ERROR, None);

        let selected = select_visible_agent_row(Some(active.clone()), Some(latest_error))
            .expect("active row should win");

        assert_eq!(selected.agent_id, active.agent_id);
        assert_eq!(selected.phase, AGENT_PHASE_READY);
    }

    #[test]
    fn select_visible_agent_row_falls_back_to_latest_error_row() {
        let latest_error = test_agent_instance("agent-error", AGENT_PHASE_ERROR, None);

        let selected = select_visible_agent_row(None, Some(latest_error.clone()))
            .expect("error row should be visible when no active row exists");

        assert_eq!(selected.agent_id, latest_error.agent_id);
        assert_eq!(selected.phase, AGENT_PHASE_ERROR);
    }

    #[test]
    fn select_visible_agent_row_ignores_non_error_latest_without_active_row() {
        let latest_ready = test_agent_instance("agent-ready", AGENT_PHASE_READY, Some("vm-ready"));

        let selected = select_visible_agent_row(None, Some(latest_ready));

        assert!(
            selected.is_none(),
            "non-error latest row must not replace active lookup"
        );
    }

    #[test]
    fn vm_not_found_detection_matches_spawner_404_recover_errors() {
        let err = anyhow::anyhow!(
            "failed to recover vm vm-123: 404 Not Found {{\"error\":\"vm not found: vm-123\"}}"
        );
        assert!(is_vm_not_found_error(&err));
    }

    #[test]
    fn vm_not_found_detection_matches_plain_vm_not_found_text() {
        let err = anyhow::anyhow!("vm not found: vm-123");
        assert!(is_vm_not_found_error(&err));
    }

    #[test]
    fn vm_not_found_detection_rejects_other_recover_errors() {
        let err = anyhow::anyhow!("failed to recover vm vm-123: 500 Internal Server Error");
        assert!(!is_vm_not_found_error(&err));
    }

    #[test]
    fn phase_from_spawner_vm_requires_guest_ready_before_ready_phase() {
        assert_eq!(
            phase_from_spawner_vm(&SpawnerVmResponse {
                id: "vm-1".to_string(),
                status: "running".to_string(),
                agent_kind: None,
                startup_probe_satisfied: true,
                guest_ready: true,
            }),
            AGENT_PHASE_READY
        );
        assert_eq!(
            phase_from_spawner_vm(&SpawnerVmResponse {
                id: "vm-1".to_string(),
                status: "running".to_string(),
                agent_kind: None,
                startup_probe_satisfied: true,
                guest_ready: false,
            }),
            AGENT_PHASE_CREATING
        );
        assert_eq!(
            phase_from_spawner_vm(&SpawnerVmResponse {
                id: "vm-1".to_string(),
                status: "failed".to_string(),
                agent_kind: None,
                startup_probe_satisfied: false,
                guest_ready: false,
            }),
            AGENT_PHASE_ERROR
        );
    }

    #[test]
    fn startup_phase_from_spawner_vm_surfaces_boot_and_waiting_detail() {
        assert_eq!(
            startup_phase_from_spawner_vm(&SpawnerVmResponse {
                id: "vm-1".to_string(),
                status: "starting".to_string(),
                agent_kind: None,
                startup_probe_satisfied: false,
                guest_ready: false,
            }),
            AgentStartupPhase::BootingGuest
        );
        assert_eq!(
            startup_phase_from_spawner_vm(&SpawnerVmResponse {
                id: "vm-1".to_string(),
                status: "running".to_string(),
                agent_kind: None,
                startup_probe_satisfied: false,
                guest_ready: false,
            }),
            AgentStartupPhase::WaitingForServiceReady
        );
        assert_eq!(
            startup_phase_from_spawner_vm(&SpawnerVmResponse {
                id: "vm-1".to_string(),
                status: "running".to_string(),
                agent_kind: None,
                startup_probe_satisfied: true,
                guest_ready: false,
            }),
            AgentStartupPhase::WaitingForKeypackagePublish
        );
    }

    #[test]
    fn managed_environment_status_copy_failed_with_vm_explains_fallback_to_fresh() {
        let row = test_agent_instance("agent-1", AGENT_PHASE_ERROR, Some("vm-1"));

        let copy = managed_environment_status_copy(Some(&row), Some(AgentStartupPhase::Failed));

        assert!(copy.contains("preserve the durable home"));
        assert!(copy.contains("provisions a fresh environment instead"));
    }

    #[test]
    fn managed_environment_status_copy_failed_without_vm_explains_fresh_reprovision() {
        let row = test_agent_instance("agent-1", AGENT_PHASE_ERROR, None);

        let copy = managed_environment_status_copy(Some(&row), Some(AgentStartupPhase::Failed));

        assert!(copy.contains("No recoverable VM is available"));
        assert!(copy.contains("Recover provisions a fresh environment"));
    }

    #[test]
    fn prepare_agent_for_reprovision_clears_active_constraint_for_missing_vm_id_row() {
        let _guard = serial_test_guard();
        let Some(mut conn) = init_test_db_connection() else {
            return;
        };
        clear_test_database(&mut conn);

        let owner_npub = "npub1recovermissingvmtest";
        let active = AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-stale",
            None,
            AGENT_PHASE_CREATING,
        )
        .expect("insert stale active row");

        prepare_agent_for_reprovision(&mut conn, &active)
            .expect("mark stale row errored before reprovision");

        let latest = AgentInstance::find_latest_by_owner(&mut conn, owner_npub)
            .expect("query latest row")
            .expect("latest row should exist");
        assert_eq!(latest.agent_id, "agent-stale");
        assert_eq!(latest.phase, AGENT_PHASE_ERROR);
        assert_eq!(latest.vm_id, None);

        let reprovisioned = AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-fresh",
            Some("vm-fresh"),
            AGENT_PHASE_CREATING,
        )
        .expect("erroring stale row should clear active-owner constraint");
        assert_eq!(reprovisioned.agent_id, "agent-fresh");

        clear_test_database(&mut conn);
    }

    #[test]
    fn visible_agent_response_returns_active_row_for_recover_retry() {
        let _guard = serial_test_guard();
        let Some(mut conn) = init_test_db_connection() else {
            return;
        };
        clear_test_database(&mut conn);

        let owner_npub = "npub1recovertestvisible";
        AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-ready",
            Some("vm-ready"),
            AGENT_PHASE_READY,
        )
        .expect("insert active row");

        let response = visible_agent_response(
            &mut conn,
            owner_npub,
            "req-visible",
            AgentApiErrorCode::RecoverFailed,
        )
        .expect("visible active row should be returned");

        assert_eq!(response.0.agent_id, "agent-ready");
        assert_eq!(response.0.vm_id.as_deref(), Some("vm-ready"));
        assert_eq!(response.0.state, AgentAppState::Ready);
        assert_eq!(response.0.startup_phase, AgentStartupPhase::Ready);

        clear_test_database(&mut conn);
    }

    #[tokio::test]
    async fn provision_or_existing_managed_environment_returns_active_row_after_agent_exists() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        let state = test_state(db_pool.clone());
        let owner_npub = "npub1raceconvergencetest";
        let mut conn = db_pool.get().expect("get test connection");
        clear_test_database(&mut conn);
        let existing = AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-existing",
            Some("vm-existing"),
            AGENT_PHASE_READY,
        )
        .expect("seed active row");
        drop(conn);

        let action =
            provision_or_existing_managed_environment(&state, owner_npub, "req-race", None)
                .await
                .expect("should converge on existing active row");
        assert_eq!(action.row.agent_id, existing.agent_id);
        assert_eq!(action.row.vm_id.as_deref(), Some("vm-existing"));
        assert_eq!(action.startup_phase, AgentStartupPhase::Ready);

        let mut conn = db_pool.get().expect("get clear connection");
        clear_test_database(&mut conn);
    }

    #[tokio::test]
    async fn provision_or_existing_managed_environment_keeps_inflight_creating_row_active() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        let state = test_state(db_pool.clone());
        let owner_npub = "npub1inflightconvergencetest";
        let mut conn = db_pool.get().expect("get test connection");
        clear_test_database(&mut conn);
        let existing = AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-inflight",
            None,
            AGENT_PHASE_CREATING,
        )
        .expect("seed inflight row");
        drop(conn);

        let action =
            provision_or_existing_managed_environment(&state, owner_npub, "req-inflight", None)
                .await
                .expect("should converge on existing inflight row");
        assert_eq!(action.row.agent_id, existing.agent_id);
        assert_eq!(action.row.phase, AGENT_PHASE_CREATING);
        assert_eq!(action.row.vm_id, None);
        assert_eq!(action.startup_phase, AgentStartupPhase::ProvisioningVm);

        let mut conn = db_pool.get().expect("get verify connection");
        let latest = AgentInstance::find_latest_by_owner(&mut conn, owner_npub)
            .expect("query latest row")
            .expect("latest row");
        assert_eq!(latest.agent_id, existing.agent_id);
        assert_eq!(latest.phase, AGENT_PHASE_CREATING);
        assert_eq!(latest.vm_id, None);

        clear_test_database(&mut conn);
    }

    #[tokio::test]
    async fn recover_agent_for_owner_keeps_inflight_creating_row_active() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        let state = test_state(db_pool.clone());
        let owner_npub = "npub1recoverinflightguard";
        let mut conn = db_pool.get().expect("get test connection");
        clear_test_database(&mut conn);
        let existing = AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-inflight-recover",
            None,
            AGENT_PHASE_CREATING,
        )
        .expect("seed inflight row");
        drop(conn);

        let action = recover_agent_for_owner(&state, owner_npub, "req-inflight-recover", None)
            .await
            .expect("recover should converge on inflight row");
        assert_eq!(action.row.agent_id, existing.agent_id);
        assert_eq!(action.row.phase, AGENT_PHASE_CREATING);
        assert_eq!(action.row.vm_id, None);
        assert_eq!(action.startup_phase, AgentStartupPhase::ProvisioningVm);

        let mut conn = db_pool.get().expect("get verify connection");
        let latest = AgentInstance::find_latest_by_owner(&mut conn, owner_npub)
            .expect("query latest row")
            .expect("latest row");
        assert_eq!(latest.agent_id, existing.agent_id);
        assert_eq!(latest.phase, AGENT_PHASE_CREATING);
        assert_eq!(latest.vm_id, None);

        clear_test_database(&mut conn);
    }

    #[tokio::test]
    async fn reset_agent_for_owner_keeps_inflight_creating_row_active() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        let state = test_state(db_pool.clone());
        let owner_npub = "npub1resetinflightguard";
        let mut conn = db_pool.get().expect("get test connection");
        clear_test_database(&mut conn);
        let existing = AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-inflight-reset",
            None,
            AGENT_PHASE_CREATING,
        )
        .expect("seed inflight row");
        drop(conn);

        let action = reset_agent_for_owner(&state, owner_npub, "req-inflight-reset", None)
            .await
            .expect("reset should converge on inflight row");
        assert_eq!(action.row.agent_id, existing.agent_id);
        assert_eq!(action.row.phase, AGENT_PHASE_CREATING);
        assert_eq!(action.row.vm_id, None);
        assert_eq!(action.startup_phase, AgentStartupPhase::ProvisioningVm);

        let mut conn = db_pool.get().expect("get verify connection");
        let latest = AgentInstance::find_latest_by_owner(&mut conn, owner_npub)
            .expect("query latest row")
            .expect("latest row");
        assert_eq!(latest.agent_id, existing.agent_id);
        assert_eq!(latest.phase, AGENT_PHASE_CREATING);
        assert_eq!(latest.vm_id, None);

        clear_test_database(&mut conn);
    }

    #[tokio::test]
    async fn restore_managed_environment_from_backup_records_success_events() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        let state = test_state(db_pool.clone());
        let owner_npub = "npub1restoreeventssuccess";
        let mut conn = db_pool.get().expect("get test connection");
        clear_test_database(&mut conn);
        AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-restore-success",
            Some("vm-restore-success"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready row");
        drop(conn);

        let (base_url, rx) = spawn_one_shot_server(
            "200 OK",
            r#"{"id":"vm-restore-success","status":"starting"}"#,
        );
        with_server_microvm_env_async(&base_url, Some("openclaw"), || async {
            let action = restore_managed_environment_from_backup(
                &state,
                owner_npub,
                "req-restore-success",
                None,
            )
            .await
            .expect("restore should succeed");
            assert_eq!(action.row.agent_id, "agent-restore-success");
            assert_eq!(action.row.phase, AGENT_PHASE_CREATING);
            assert_eq!(action.row.vm_id.as_deref(), Some("vm-restore-success"));
            assert_eq!(action.startup_phase, AgentStartupPhase::BootingGuest);
        })
        .await;

        let request = rx.recv().expect("captured restore request");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/vms/vm-restore-success/restore");

        let mut conn = db_pool.get().expect("get verify connection");
        let events = ManagedEnvironmentEvent::list_recent_by_owner(&mut conn, owner_npub, 10)
            .expect("list restore events");
        assert_eq!(events[0].event_kind, EVENT_RESTORE_SUCCEEDED);
        assert_eq!(events[1].event_kind, EVENT_RESTORE_REQUESTED);
        let latest = AgentInstance::find_latest_by_owner(&mut conn, owner_npub)
            .expect("query latest row")
            .expect("latest row");
        assert_eq!(latest.phase, AGENT_PHASE_CREATING);
        assert_eq!(latest.vm_id.as_deref(), Some("vm-restore-success"));

        clear_test_database(&mut conn);
    }

    #[tokio::test]
    async fn recover_agent_for_owner_legacy_row_accepts_requested_spawner_override() {
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        let state = test_state(db_pool.clone());
        let owner_npub = "npub1recoverlegacyoverride";
        let mut conn = db_pool.get().expect("get test connection");
        clear_test_database(&mut conn);
        AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-recover-legacy",
            Some("vm-recover-legacy"),
            AGENT_PHASE_READY,
        )
        .expect("seed legacy row without provider config");
        drop(conn);

        let (base_url, rx) = spawn_one_shot_server(
            "200 OK",
            r#"{"id":"vm-recover-legacy","status":"starting","agent_kind":"openclaw"}"#,
        );
        let requested = requested_microvm_params(MicrovmProvisionParams {
            spawner_url: Some(base_url.clone()),
            kind: Some(MicrovmAgentKind::Openclaw),
            backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Native),
        });
        with_env_overrides(&[(MICROVM_SPAWNER_URL_ENV, None)], || async {
            let action =
                recover_agent_for_owner(&state, owner_npub, "req-recover-legacy", Some(&requested))
                    .await
                    .expect("recover should use requested override for legacy row");
            assert_eq!(action.row.agent_id, "agent-recover-legacy");
            assert_eq!(action.row.vm_id.as_deref(), Some("vm-recover-legacy"));
            assert_eq!(action.startup_phase, AgentStartupPhase::BootingGuest);
        })
        .await;

        let request = rx.recv().expect("captured recover request");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/vms/vm-recover-legacy/recover");
        assert_eq!(
            request.headers.get("x-request-id").map(String::as_str),
            Some("req-recover-legacy")
        );

        let mut conn = db_pool.get().expect("get verify connection");
        clear_test_database(&mut conn);
    }

    #[tokio::test]
    async fn restore_legacy_row_accepts_requested_spawner_override() {
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        let state = test_state(db_pool.clone());
        let owner_npub = "npub1restorelegacyoverride";
        let mut conn = db_pool.get().expect("get test connection");
        clear_test_database(&mut conn);
        AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-restore-legacy",
            Some("vm-restore-legacy"),
            AGENT_PHASE_READY,
        )
        .expect("seed legacy row without provider config");
        drop(conn);

        let (base_url, rx) = spawn_one_shot_server(
            "200 OK",
            r#"{"id":"vm-restore-legacy","status":"starting","agent_kind":"openclaw"}"#,
        );
        let requested = requested_microvm_params(MicrovmProvisionParams {
            spawner_url: Some(base_url.clone()),
            kind: Some(MicrovmAgentKind::Openclaw),
            backend: Some(pika_agent_control_plane::MicrovmAgentBackend::Native),
        });
        with_env_overrides(&[(MICROVM_SPAWNER_URL_ENV, None)], || async {
            let action = restore_managed_environment_from_backup(
                &state,
                owner_npub,
                "req-restore-legacy",
                Some(&requested),
            )
            .await
            .expect("restore should use requested override for legacy row");
            assert_eq!(action.row.agent_id, "agent-restore-legacy");
            assert_eq!(action.row.vm_id.as_deref(), Some("vm-restore-legacy"));
            assert_eq!(action.startup_phase, AgentStartupPhase::BootingGuest);
        })
        .await;

        let request = rx.recv().expect("captured restore request");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/vms/vm-restore-legacy/restore");
        assert_eq!(
            request.headers.get("x-request-id").map(String::as_str),
            Some("req-restore-legacy")
        );

        let mut conn = db_pool.get().expect("get verify connection");
        clear_test_database(&mut conn);
    }

    #[tokio::test]
    async fn restore_managed_environment_from_backup_records_failed_event_and_marks_row_error() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        let state = test_state(db_pool.clone());
        let owner_npub = "npub1restoreeventsfail";
        let mut conn = db_pool.get().expect("get test connection");
        clear_test_database(&mut conn);
        AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-restore-failed",
            Some("vm-restore-failed"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready row");
        drop(conn);

        let (base_url, rx) =
            spawn_one_shot_server("500 Internal Server Error", r#"{"error":"restore failed"}"#);
        with_server_microvm_env_async(&base_url, Some("openclaw"), || async {
            let err = restore_managed_environment_from_backup(
                &state,
                owner_npub,
                "req-restore-failed",
                None,
            )
            .await
            .expect_err("restore should fail");
            assert_eq!(err.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
        })
        .await;

        let request = rx.recv().expect("captured restore request");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/vms/vm-restore-failed/restore");

        let mut conn = db_pool.get().expect("get verify connection");
        let events = ManagedEnvironmentEvent::list_recent_by_owner(&mut conn, owner_npub, 10)
            .expect("list restore events");
        assert_eq!(events[0].event_kind, EVENT_RESTORE_FAILED);
        assert_eq!(events[1].event_kind, EVENT_RESTORE_REQUESTED);
        let latest = AgentInstance::find_latest_by_owner(&mut conn, owner_npub)
            .expect("query latest row")
            .expect("latest row");
        assert_eq!(latest.phase, AGENT_PHASE_ERROR);
        assert_eq!(latest.vm_id.as_deref(), Some("vm-restore-failed"));

        clear_test_database(&mut conn);
    }

    #[tokio::test]
    async fn agent_api_error_response_includes_request_id() {
        let response = AgentApiError::from_code(AgentApiErrorCode::RecoverFailed)
            .with_request_id("req-123")
            .into_response();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("parse error body");
        assert_eq!(json["error"], AgentApiErrorCode::RecoverFailed.as_str());
        assert_eq!(json["request_id"], "req-123");
    }
}
