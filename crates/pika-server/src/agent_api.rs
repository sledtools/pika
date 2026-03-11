use std::net::{Ipv4Addr, Ipv6Addr};

use anyhow::Context;
use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::{response::IntoResponse, Json};
use diesel::result::{DatabaseErrorKind, Error as DieselError};
use diesel::Connection;
use diesel::PgConnection;
use nostr_sdk::prelude::{Keys, PublicKey};
use nostr_sdk::ToBech32;
use serde::Serialize;

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
    AgentProvisionRequest, AgentStartupPhase, MicrovmAgentKind, MicrovmProvisionParams,
    SpawnerVmResponse,
};
use pika_agent_microvm::{
    build_create_vm_request, resolve_params, spawner_create_error, validate_resolved_params,
    MicrovmSpawnerClient, ResolvedMicrovmParams,
};
use pika_relay_profiles::default_message_relays;

const AGENT_OWNER_ACTIVE_INDEX: &str = "agent_instances_owner_active_idx";
const MICROVM_SPAWNER_URL_ENV: &str = "PIKA_AGENT_MICROVM_SPAWNER_URL";
const EVENT_PROVISION_REQUESTED: &str = "provision_requested";
const EVENT_PROVISION_ACCEPTED: &str = "provision_accepted";
const EVENT_RECOVER_REQUESTED: &str = "recover_requested";
const EVENT_RECOVER_SUCCEEDED: &str = "recover_succeeded";
const EVENT_RECOVER_FELL_BACK_TO_FRESH: &str = "recover_fell_back_to_fresh";
const EVENT_RESET_REQUESTED: &str = "reset_requested";
const EVENT_RESET_DESTROYED_OLD_VM: &str = "reset_destroyed_old_vm";
const EVENT_RESET_CONTINUED_MISSING_VM: &str = "reset_continued_missing_vm";
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

fn required_microvm_spawner_url(raw: Option<String>) -> anyhow::Result<String> {
    raw.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .with_context(|| format!("missing {MICROVM_SPAWNER_URL_ENV}"))
}

fn required_microvm_spawner_url_from_env() -> anyhow::Result<String> {
    required_microvm_spawner_url(std::env::var(MICROVM_SPAWNER_URL_ENV).ok())
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

fn default_microvm_params_from_env() -> anyhow::Result<MicrovmProvisionParams> {
    Ok(MicrovmProvisionParams {
        spawner_url: Some(required_microvm_spawner_url_from_env()?),
        kind: microvm_kind_from_env(),
        ..MicrovmProvisionParams::default()
    })
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

fn phase_from_spawner_vm(vm: &SpawnerVmResponse) -> &'static str {
    match (vm.status.as_str(), vm.guest_ready) {
        ("failed", _) => AGENT_PHASE_ERROR,
        ("running", true) => AGENT_PHASE_READY,
        _ => AGENT_PHASE_CREATING,
    }
}

fn startup_phase_from_spawner_vm(vm: &SpawnerVmResponse) -> AgentStartupPhase {
    match (vm.status.as_str(), vm.guest_ready) {
        ("failed", _) => AgentStartupPhase::Failed,
        ("running", true) => AgentStartupPhase::Ready,
        ("running", false) => AgentStartupPhase::WaitingForServiceReady,
        ("starting", _) => AgentStartupPhase::BootingGuest,
        _ => AgentStartupPhase::ProvisioningVm,
    }
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
            row,
        });
    };
    let resolved = match resolved_spawner_params(None) {
        Ok(resolved) => resolved,
        Err(err) => {
            tracing::warn!(
                request_id,
                agent_id = %row.agent_id,
                vm_id,
                error = %err,
                "failed to resolve spawner params while refreshing agent readiness"
            );
            return Ok(RefreshedAgentStatus {
                startup_phase: startup_phase_from_row_phase(&row.phase)
                    .unwrap_or(AgentStartupPhase::ProvisioningVm),
                row,
            });
        }
    };
    let spawner = MicrovmSpawnerClient::new(resolved.spawner_url);
    let vm = match spawner
        .get_vm_with_request_id(vm_id, Some(request_id))
        .await
    {
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
                row,
            });
        }
    };

    let next_phase = phase_from_spawner_vm(&vm);
    let startup_phase = startup_phase_from_spawner_vm(&vm);
    if row.phase == next_phase && row.vm_id.as_deref() == Some(vm.id.as_str()) {
        return Ok(RefreshedAgentStatus { row, startup_phase });
    }

    let updated = AgentInstance::update_phase(conn, &row.agent_id, next_phase, Some(&vm.id))
        .map_err(|_| {
            AgentApiError::from_code(AgentApiErrorCode::Internal)
                .with_request_id(request_id.to_string())
        })?;
    Ok(RefreshedAgentStatus {
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

fn prepare_agent_for_reprovision(
    conn: &mut PgConnection,
    active: &AgentInstance,
) -> Result<(), AgentApiError> {
    if active.phase != AGENT_PHASE_ERROR {
        mark_agent_errored(conn, &active.agent_id)?;
    }
    Ok(())
}

fn resolved_spawner_params(
    requested: Option<&MicrovmProvisionParams>,
) -> anyhow::Result<ResolvedMicrovmParams> {
    let mut params = default_microvm_params_from_env()?;
    if let Some(requested) = requested {
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
    let resolved = resolve_params(&params);
    validate_resolved_params(&resolved).context("validate microvm agent selection")?;
    ensure_private_microvm_spawner_url(&resolved.spawner_url)
        .context("validate private microvm spawner URL")?;
    Ok(resolved)
}

async fn provision_vm_for_owner(
    owner_npub: &str,
    bot_identity: &ProvisioningBotIdentity,
    request_id: &str,
    requested: Option<&MicrovmProvisionParams>,
) -> anyhow::Result<pika_agent_control_plane::SpawnerVmResponse> {
    let owner_pubkey = PublicKey::parse(owner_npub).context("parse owner npub")?;
    let resolved = resolved_spawner_params(requested)?;
    let create_vm = build_create_vm_request(
        &owner_pubkey,
        &default_message_relays(),
        &bot_identity.secret_hex,
        &bot_identity.pubkey_hex,
        &resolved,
    );
    let spawner = MicrovmSpawnerClient::new(resolved.spawner_url.clone());
    spawner
        .create_vm_with_request_id(&create_vm, Some(request_id))
        .await
        .map_err(|err| spawner_create_error(&resolved.spawner_url, err))
}

async fn provision_agent_for_owner(
    state: &State,
    owner_npub: &str,
    request_id: &str,
    requested: Option<&MicrovmProvisionParams>,
) -> Result<AgentInstance, AgentApiError> {
    let bot_identity = generate_provisioning_bot_identity().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;

    let created = {
        let mut conn = state.db_pool.get().map_err(|_| {
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
        conn.transaction::<AgentInstance, anyhow::Error, _>(|conn| {
            let created = AgentInstance::create(
                conn,
                owner_npub,
                &bot_identity.pubkey_npub,
                None,
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

    let vm = match provision_vm_for_owner(owner_npub, &bot_identity, request_id, requested).await {
        Ok(vm) => vm,
        Err(err) => {
            tracing::error!(
                request_id,
                owner_npub = %owner_npub,
                error = %err,
                "failed to provision microvm for agent"
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
        "provisioned agent microvm"
    );
    Ok(updated)
}

async fn provision_or_existing_managed_environment(
    state: &State,
    owner_npub: &str,
    request_id: &str,
    requested: Option<&MicrovmProvisionParams>,
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
    requested: Option<&MicrovmProvisionParams>,
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

    let requested = body.as_ref().and_then(|body| body.microvm.as_ref());
    let updated = provision_agent_for_owner(
        &state,
        &requester.owner_npub,
        &request_context.request_id,
        requested,
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
    requested: Option<&MicrovmProvisionParams>,
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

    let resolved = resolved_spawner_params(requested).map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::RecoverFailed).with_request_id(request_id)
    })?;
    let spawner = MicrovmSpawnerClient::new(resolved.spawner_url);
    let recovered = match spawner
        .recover_vm_with_request_id(&vm_id, Some(request_id))
        .await
    {
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
                "failed to recover agent microvm"
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
    requested: Option<&MicrovmProvisionParams>,
) -> Result<ManagedEnvironmentAction, AgentApiError> {
    let existing = {
        let mut conn = state.db_pool.get().map_err(|_| {
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
        let existing = load_visible_agent_row(&mut conn, owner_npub)
            .map_err(|err| err.with_request_id(request_id.to_string()))?;
        let reset_requested_message = match existing.as_ref() {
            Some(_) => "Destructive reset requested. The current managed environment will be replaced."
                .to_string(),
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
        let resolved = resolved_spawner_params(requested).map_err(|err| {
            tracing::error!(
                request_id = %request_id,
                owner_npub = %owner_npub,
                error = %err,
                "failed to resolve reset spawner params"
            );
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
        let spawner = MicrovmSpawnerClient::new(resolved.spawner_url);
        match spawner
            .delete_vm_with_request_id(vm_id, Some(request_id))
            .await
        {
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
    let requested = body.as_ref().and_then(|body| body.microvm.as_ref());
    let recovered = recover_agent_for_owner(
        &state,
        &requester.owner_npub,
        &request_context.request_id,
        requested,
    )
    .await?;
    json_response(
        recovered.row,
        recovered.startup_phase,
        &request_context.request_id,
    )
}

pub fn agent_api_healthcheck() -> anyhow::Result<()> {
    let _ = resolved_spawner_params(None).context("resolve and validate microvm spawner params")?;
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
    use axum::body::HttpBody;
    use axum::http::header;
    use base64::Engine;
    use chrono::NaiveDate;
    use diesel::prelude::*;
    use diesel::r2d2::{ConnectionManager, Pool};
    use diesel_migrations::MigrationHarness;
    use nostr_sdk::prelude::{EventBuilder, Kind, Tag, TagKind};
    use std::collections::HashSet;

    fn test_agent_instance(agent_id: &str, phase: &str, vm_id: Option<&str>) -> AgentInstance {
        AgentInstance {
            agent_id: agent_id.to_string(),
            owner_npub: "npub1testowner".to_string(),
            vm_id: vm_id.map(str::to_string),
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

    fn with_server_microvm_env<T>(
        spawner_url: &str,
        kind: Option<&str>,
        f: impl FnOnce() -> T,
    ) -> T {
        let _guard = serial_test_guard();
        let prior_spawner = std::env::var(MICROVM_SPAWNER_URL_ENV).ok();
        let prior_kind = std::env::var("PIKA_AGENT_MICROVM_KIND").ok();
        unsafe {
            std::env::set_var(MICROVM_SPAWNER_URL_ENV, spawner_url);
        }
        match kind {
            Some(kind) => unsafe {
                std::env::set_var("PIKA_AGENT_MICROVM_KIND", kind);
            },
            None => unsafe {
                std::env::remove_var("PIKA_AGENT_MICROVM_KIND");
            },
        }

        let result = f();

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
        result
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

            let startup_plan = request
                .guest_autostart
                .startup_plan
                .clone()
                .expect("startup plan");
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
                guest_ready: true,
            }),
            AGENT_PHASE_READY
        );
        assert_eq!(
            phase_from_spawner_vm(&SpawnerVmResponse {
                id: "vm-1".to_string(),
                status: "running".to_string(),
                guest_ready: false,
            }),
            AGENT_PHASE_CREATING
        );
        assert_eq!(
            phase_from_spawner_vm(&SpawnerVmResponse {
                id: "vm-1".to_string(),
                status: "failed".to_string(),
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
                guest_ready: false,
            }),
            AgentStartupPhase::BootingGuest
        );
        assert_eq!(
            startup_phase_from_spawner_vm(&SpawnerVmResponse {
                id: "vm-1".to_string(),
                status: "running".to_string(),
                guest_ready: false,
            }),
            AgentStartupPhase::WaitingForServiceReady
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
    async fn agent_api_error_response_includes_request_id() {
        let response = AgentApiError::from_code(AgentApiErrorCode::RecoverFailed)
            .with_request_id("req-123")
            .into_response();
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(chunk) = body.data().await {
            bytes.extend_from_slice(&chunk.expect("read response chunk"));
        }
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("parse error body");
        assert_eq!(json["error"], AgentApiErrorCode::RecoverFailed.as_str());
        assert_eq!(json["request_id"], "req-123");
    }
}
