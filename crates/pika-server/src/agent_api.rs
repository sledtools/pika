use std::net::{Ipv4Addr, Ipv6Addr};

use anyhow::Context;
use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::{response::IntoResponse, Json};
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
use crate::nostr_auth::{
    event_from_authorization_header, expected_host_from_headers, verify_nip98_event,
};
use crate::{RequestContext, State};
use pika_agent_control_plane::MicrovmProvisionParams;
use pika_agent_microvm::{
    build_create_vm_request, resolve_params, spawner_create_error, MicrovmSpawnerClient,
    ResolvedMicrovmParams,
};
use pika_relay_profiles::default_message_relays;

const MICROVM_SPAWNER_URL_ENV: &str = "PIKA_AGENT_MICROVM_SPAWNER_URL";

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
}

fn required_microvm_spawner_url(raw: Option<String>) -> anyhow::Result<String> {
    raw.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .with_context(|| format!("missing {MICROVM_SPAWNER_URL_ENV}"))
}

fn required_microvm_spawner_url_from_env() -> anyhow::Result<String> {
    required_microvm_spawner_url(std::env::var(MICROVM_SPAWNER_URL_ENV).ok())
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

fn map_row_to_response(row: AgentInstance) -> Result<AgentStateResponse, AgentApiError> {
    let Some(state) = phase_to_state(&row.phase) else {
        return Err(AgentApiError::from_code(AgentApiErrorCode::Internal));
    };
    Ok(AgentStateResponse {
        agent_id: row.agent_id,
        vm_id: row.vm_id,
        state,
    })
}

fn phase_from_spawner_status(status: &str) -> &'static str {
    match status {
        "running" => AGENT_PHASE_READY,
        _ => AGENT_PHASE_CREATING,
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

fn is_vm_not_found_error(err: &anyhow::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    (message.contains("vm not found") || (message.contains("404") && message.contains("not found")))
        && message.contains("vm")
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

fn resolved_spawner_params() -> anyhow::Result<ResolvedMicrovmParams> {
    let spawner_url = required_microvm_spawner_url_from_env()?;
    let params = MicrovmProvisionParams {
        spawner_url: Some(spawner_url),
    };
    let resolved = resolve_params(&params);
    ensure_private_microvm_spawner_url(&resolved.spawner_url)
        .context("validate private microvm spawner URL")?;
    Ok(resolved)
}

async fn provision_vm_for_owner(
    owner_npub: &str,
    bot_identity: &ProvisioningBotIdentity,
    request_id: &str,
) -> anyhow::Result<pika_agent_control_plane::SpawnerVmResponse> {
    let owner_pubkey = PublicKey::parse(owner_npub).context("parse owner npub")?;
    let resolved = resolved_spawner_params()?;
    let create_vm = build_create_vm_request(
        &owner_pubkey,
        &default_message_relays(),
        &bot_identity.secret_hex,
        &bot_identity.pubkey_hex,
    );
    let spawner = MicrovmSpawnerClient::new(resolved.spawner_url.clone());
    spawner
        .create_vm_with_request_id(&create_vm, Some(request_id))
        .await
        .map_err(|err| spawner_create_error(&resolved.spawner_url, err))
}

/// Provision a new agent for the given owner. When `max_active` is `Some`, the
/// insert is performed atomically with a count guard so concurrent requests
/// cannot exceed the limit. When `None` (unlimited), a plain insert is used.
async fn provision_agent_for_owner(
    state: &State,
    owner_npub: &str,
    request_id: &str,
    max_active: Option<i64>,
) -> Result<AgentInstance, AgentApiError> {
    let bot_identity = generate_provisioning_bot_identity().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
    })?;

    let created = {
        let mut conn = state.db_pool.get().map_err(|_| {
            AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
        })?;
        if let Some(limit) = max_active {
            AgentInstance::create_if_under_limit(
                &mut conn,
                owner_npub,
                &bot_identity.pubkey_npub,
                None,
                AGENT_PHASE_CREATING,
                limit,
            )
            .map_err(|err| {
                tracing::error!(
                    request_id,
                    owner_npub = %owner_npub,
                    error = %err,
                    "failed to create agent instance row (limit-guarded)"
                );
                AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
            })?
            .ok_or_else(|| {
                AgentApiError::from_code(AgentApiErrorCode::AgentExists).with_request_id(request_id)
            })?
        } else {
            AgentInstance::create(
                &mut conn,
                owner_npub,
                &bot_identity.pubkey_npub,
                None,
                AGENT_PHASE_CREATING,
            )
            .map_err(|err| {
                tracing::error!(
                    request_id,
                    owner_npub = %owner_npub,
                    error = %err,
                    "failed to create agent instance row"
                );
                AgentApiError::from_code(AgentApiErrorCode::Internal).with_request_id(request_id)
            })?
        }
    };

    let vm = match provision_vm_for_owner(owner_npub, &bot_identity, request_id).await {
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
    let updated = AgentInstance::update_phase(
        &mut conn,
        &created.agent_id,
        phase_from_spawner_status(&vm.status),
        Some(&vm.id),
    )
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

pub async fn ensure_agent(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<AgentStateResponse>), AgentApiError> {
    let (requester, max_active) = {
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

        // Resolve per-user agent limit (default 1, NULL = unlimited).
        // The actual enforcement happens atomically inside provision_agent_for_owner
        // via INSERT … SELECT WHERE count < limit.
        let max_agents = AgentAllowlistEntry::get(&mut conn, &requester.owner_npub)
            .map_err(|_| {
                AgentApiError::from_code(AgentApiErrorCode::Internal)
                    .with_request_id(request_context.request_id.clone())
            })?
            .and_then(|entry| entry.max_agents);

        let max_active = max_agents.map(|limit| limit as i64);
        (requester, max_active)
    };

    let updated = provision_agent_for_owner(
        &state,
        &requester.owner_npub,
        &request_context.request_id,
        max_active,
    )
    .await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(
            map_row_to_response(updated)
                .map_err(|err| err.with_request_id(request_context.request_id.clone()))?,
        ),
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
    let normalized = match active.vm_id.as_deref() {
        Some(_) => active,
        None if active.phase != AGENT_PHASE_ERROR => {
            mark_agent_errored(&mut conn, &active.agent_id)
                .map_err(|err| err.with_request_id(request_context.request_id.clone()))?
        }
        None => active,
    };
    Ok(Json(map_row_to_response(normalized).map_err(|err| {
        err.with_request_id(request_context.request_id.clone())
    })?))
}

pub async fn recover_my_agent(
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
        "POST",
        V1_AGENTS_RECOVER_PATH,
        state.trust_forwarded_host,
    )
    .map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
    let Some(active) = load_visible_agent_row(&mut conn, &requester.owner_npub)
        .map_err(|err| err.with_request_id(request_context.request_id.clone()))?
    else {
        return Err(AgentApiError::from_code(AgentApiErrorCode::AgentNotFound)
            .with_request_id(request_context.request_id.clone()));
    };
    if active.phase == AGENT_PHASE_ERROR || active.vm_id.is_none() {
        prepare_agent_for_reprovision(&mut conn, &active)
            .map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
        drop(conn);
        let reprovisioned = provision_agent_for_owner(
            &state,
            &requester.owner_npub,
            &request_context.request_id,
            None,
        )
        .await
        .map_err(|err| match err.code {
            AgentApiErrorCode::AgentExists => {
                AgentApiError::from_code(AgentApiErrorCode::RecoverFailed)
                    .with_request_id(request_context.request_id.clone())
            }
            _ => err,
        })?;
        return Ok(Json(map_row_to_response(reprovisioned).map_err(|err| {
            err.with_request_id(request_context.request_id.clone())
        })?));
    }
    let vm_id = active.vm_id.clone().ok_or_else(|| {
        AgentApiError::from_code(AgentApiErrorCode::RecoverFailed)
            .with_request_id(request_context.request_id.clone())
    })?;

    let resolved = resolved_spawner_params().map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::RecoverFailed)
            .with_request_id(request_context.request_id.clone())
    })?;
    let spawner = MicrovmSpawnerClient::new(resolved.spawner_url);
    let recovered = match spawner
        .recover_vm_with_request_id(&vm_id, Some(&request_context.request_id))
        .await
    {
        Ok(recovered) => recovered,
        Err(err) if is_vm_not_found_error(&err) => {
            tracing::error!(
                request_id = %request_context.request_id,
                owner_npub = %requester.owner_npub,
                agent_id = %active.agent_id,
                vm_id = %vm_id,
                error = %err,
                "recover requested for missing vm; marking stale agent errored and reprovisioning"
            );
            prepare_agent_for_reprovision(&mut conn, &active)
                .map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
            drop(conn);
            let reprovisioned = provision_agent_for_owner(
                &state,
                &requester.owner_npub,
                &request_context.request_id,
                None,
            )
            .await
            .map_err(|err| match err.code {
                AgentApiErrorCode::AgentExists => {
                    AgentApiError::from_code(AgentApiErrorCode::RecoverFailed)
                        .with_request_id(request_context.request_id.clone())
                }
                _ => err,
            })?;
            return Ok(Json(map_row_to_response(reprovisioned).map_err(|err| {
                err.with_request_id(request_context.request_id.clone())
            })?));
        }
        Err(err) => {
            tracing::error!(
                request_id = %request_context.request_id,
                agent_id = %active.agent_id,
                vm_id = %vm_id,
                owner_npub = %requester.owner_npub,
                error = %err,
                "failed to recover agent microvm"
            );
            return Err(AgentApiError::from_code(AgentApiErrorCode::RecoverFailed)
                .with_request_id(request_context.request_id.clone()));
        }
    };

    let updated = AgentInstance::update_phase(
        &mut conn,
        &active.agent_id,
        phase_from_spawner_status(&recovered.status),
        Some(&recovered.id),
    )
    .map_err(|_| {
        AgentApiError::from_code(AgentApiErrorCode::Internal)
            .with_request_id(request_context.request_id.clone())
    })?;
    Ok(Json(map_row_to_response(updated).map_err(|err| {
        err.with_request_id(request_context.request_id.clone())
    })?))
}

pub fn agent_api_healthcheck() -> anyhow::Result<()> {
    let _ = resolved_spawner_params().context("resolve and validate microvm spawner params")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::HttpBody;
    use axum::http::header;
    use base64::Engine;
    use chrono::NaiveDate;
    use diesel::prelude::*;
    use diesel_migrations::MigrationHarness;
    use nostr_sdk::prelude::{EventBuilder, Kind, Tag, TagKind};
    use std::sync::{Mutex, OnceLock};

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

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        match GUARD.get_or_init(|| Mutex::new(())).lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn init_test_db_connection() -> Option<PgConnection> {
        dotenv::dotenv().ok();
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
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

    fn clear_test_database(conn: &mut PgConnection) {
        diesel::sql_query(
            "TRUNCATE TABLE agent_instances, agent_allowlist_audit, agent_allowlist, group_subscriptions, subscription_info RESTART IDENTITY CASCADE",
        )
        .execute(conn)
        .expect("truncate test tables");
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
    fn phase_from_spawner_status_only_marks_running_ready() {
        assert_eq!(phase_from_spawner_status("running"), AGENT_PHASE_READY);
        assert_eq!(phase_from_spawner_status("starting"), AGENT_PHASE_CREATING);
        assert_eq!(
            phase_from_spawner_status("anything-else"),
            AGENT_PHASE_CREATING
        );
    }

    #[test]
    fn prepare_agent_for_reprovision_clears_active_constraint_for_missing_vm_id_row() {
        let _guard = test_guard();
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
