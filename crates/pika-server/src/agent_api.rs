use std::net::{Ipv4Addr, Ipv6Addr};

use anyhow::Context;
use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::{response::IntoResponse, Json};
use diesel::result::{DatabaseErrorKind, Error as DieselError};
use diesel::PgConnection;
use nostr_sdk::prelude::{Keys, PublicKey};
use nostr_sdk::ToBech32;
use serde::Serialize;
use tracing::error;

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
use crate::State;
use pika_agent_control_plane::MicrovmProvisionParams;
use pika_agent_microvm::{
    build_create_vm_request, resolve_params, spawner_create_error, MicrovmSpawnerClient,
    ResolvedMicrovmParams,
};
use pika_relay_profiles::default_message_relays;

const AGENT_OWNER_ACTIVE_INDEX: &str = "agent_instances_owner_active_idx";
const MICROVM_SPAWNER_URL_ENV: &str = "PIKA_AGENT_MICROVM_SPAWNER_URL";

#[derive(Debug)]
pub struct AgentApiError {
    status: StatusCode,
    code: AgentApiErrorCode,
}

impl AgentApiError {
    fn from_code(code: AgentApiErrorCode) -> Self {
        Self {
            status: code.status_code(),
            code,
        }
    }
}

impl IntoResponse for AgentApiError {
    fn into_response(self) -> axum::response::Response {
        let body = Json(serde_json::json!({
            "error": self.code.as_str(),
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

fn phase_from_vm_status(status: &str) -> &'static str {
    if status.eq_ignore_ascii_case("running") {
        AGENT_PHASE_READY
    } else {
        AGENT_PHASE_CREATING
    }
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

fn resolved_spawner_params() -> anyhow::Result<ResolvedMicrovmParams> {
    let spawner_url = required_microvm_spawner_url_from_env()?;
    let params = MicrovmProvisionParams {
        spawner_url: Some(spawner_url),
    };
    let resolved = resolve_params(&params, true);
    ensure_private_microvm_spawner_url(&resolved.spawner_url)
        .context("validate private microvm spawner URL")?;
    Ok(resolved)
}

async fn provision_vm_for_owner(
    owner_npub: &str,
    bot_identity: &ProvisioningBotIdentity,
) -> anyhow::Result<pika_agent_microvm::VmResponse> {
    let owner_pubkey = PublicKey::parse(owner_npub).context("parse owner npub")?;
    let resolved = resolved_spawner_params()?;
    let create_vm = build_create_vm_request(
        &resolved,
        &owner_pubkey,
        &default_message_relays(),
        &bot_identity.secret_hex,
        &bot_identity.pubkey_hex,
    );
    let spawner = MicrovmSpawnerClient::new(resolved.spawner_url.clone());
    spawner
        .create_vm(&create_vm)
        .await
        .map_err(|err| spawner_create_error(&resolved.spawner_url, err))
}

async fn refresh_agent_phase_from_spawner(
    active: &AgentInstance,
) -> anyhow::Result<Option<(String, Option<String>)>> {
    let Some(vm_id) = active.vm_id.as_deref() else {
        return Ok(None);
    };
    let resolved = resolved_spawner_params()?;
    let spawner = MicrovmSpawnerClient::new(resolved.spawner_url);
    let vm = spawner.get_vm(vm_id).await?;
    let phase = phase_from_vm_status(&vm.status);
    if active.phase == phase && active.vm_id.as_deref() == Some(vm.id.as_str()) {
        return Ok(None);
    }
    Ok(Some((phase.to_string(), Some(vm.id))))
}

pub async fn ensure_agent(
    Extension(state): Extension<State>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<AgentStateResponse>), AgentApiError> {
    let requester = {
        let mut conn = state
            .db_pool
            .get()
            .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;
        let requester = require_whitelisted_requester(
            &mut conn,
            &headers,
            "POST",
            V1_AGENTS_ENSURE_PATH,
            state.trust_forwarded_host,
        )?;
        if AgentInstance::find_active_by_owner(&mut conn, &requester.owner_npub)
            .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?
            .is_some()
        {
            return Err(AgentApiError::from_code(AgentApiErrorCode::AgentExists));
        }
        requester
    };
    let bot_identity = generate_provisioning_bot_identity()
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;

    let created = {
        let mut conn = state
            .db_pool
            .get()
            .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;
        AgentInstance::create(
            &mut conn,
            &requester.owner_npub,
            &bot_identity.pubkey_npub,
            None,
            AGENT_PHASE_CREATING,
        )
        .map_err(|err| {
            if is_owner_active_unique_violation(&err) {
                AgentApiError::from_code(AgentApiErrorCode::AgentExists)
            } else {
                AgentApiError::from_code(AgentApiErrorCode::Internal)
            }
        })?
    };

    let vm = match provision_vm_for_owner(&requester.owner_npub, &bot_identity).await {
        Ok(vm) => vm,
        Err(err) => {
            error!(owner_npub = %requester.owner_npub, error = %err, "failed to provision microvm for agent");
            if let Ok(mut conn) = state.db_pool.get() {
                let _ = AgentInstance::update_phase(
                    &mut conn,
                    &created.agent_id,
                    AGENT_PHASE_ERROR,
                    None,
                );
            }
            return Err(AgentApiError::from_code(AgentApiErrorCode::Internal));
        }
    };

    let mut conn = state
        .db_pool
        .get()
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;
    let updated = AgentInstance::update_phase(
        &mut conn,
        &created.agent_id,
        phase_from_vm_status(&vm.status),
        Some(&vm.id),
    )
    .map_err(|err| {
        error!(agent_id = %created.agent_id, error = %err, "failed to update agent phase after provision");
        AgentApiError::from_code(AgentApiErrorCode::Internal)
    })?;

    Ok((StatusCode::ACCEPTED, Json(map_row_to_response(updated)?)))
}

pub async fn get_my_agent(
    Extension(state): Extension<State>,
    headers: HeaderMap,
) -> Result<Json<AgentStateResponse>, AgentApiError> {
    let mut conn = state
        .db_pool
        .get()
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;
    let requester = require_whitelisted_requester(
        &mut conn,
        &headers,
        "GET",
        V1_AGENTS_ME_PATH,
        state.trust_forwarded_host,
    )?;
    let Some(active) = AgentInstance::find_latest_by_owner(&mut conn, &requester.owner_npub)
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?
    else {
        let Some(latest) = AgentInstance::find_latest_by_owner(&mut conn, &requester.owner_npub)
            .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?
        else {
            return Err(AgentApiError::from_code(AgentApiErrorCode::AgentNotFound));
        };
        if latest.phase == AGENT_PHASE_ERROR {
            return Ok(Json(map_row_to_response(latest)?));
        }
        return Err(AgentApiError::from_code(AgentApiErrorCode::AgentNotFound));
    };
    let normalized = if active.phase == AGENT_PHASE_CREATING && active.vm_id.is_some() {
        match refresh_agent_phase_from_spawner(&active).await {
            Ok(Some((phase, vm_id))) => {
                AgentInstance::update_phase(&mut conn, &active.agent_id, &phase, vm_id.as_deref())
                    .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?
            }
            Ok(None) | Err(_) => active,
        }
    } else {
        active
    };
    Ok(Json(map_row_to_response(normalized)?))
}

pub async fn recover_my_agent(
    Extension(state): Extension<State>,
    headers: HeaderMap,
) -> Result<Json<AgentStateResponse>, AgentApiError> {
    let mut conn = state
        .db_pool
        .get()
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;
    let requester = require_whitelisted_requester(
        &mut conn,
        &headers,
        "POST",
        V1_AGENTS_RECOVER_PATH,
        state.trust_forwarded_host,
    )?;
    let Some(active) = AgentInstance::find_latest_by_owner(&mut conn, &requester.owner_npub)
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?
    else {
        return Err(AgentApiError::from_code(AgentApiErrorCode::AgentNotFound));
    };
    let vm_id = active
        .vm_id
        .clone()
        .ok_or_else(|| AgentApiError::from_code(AgentApiErrorCode::RecoverFailed))?;

    let resolved = resolved_spawner_params()
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::RecoverFailed))?;
    let spawner = MicrovmSpawnerClient::new(resolved.spawner_url);
    let recovered = spawner
        .recover_vm(&vm_id)
        .await
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::RecoverFailed))?;

    let updated = AgentInstance::update_phase(
        &mut conn,
        &active.agent_id,
        phase_from_vm_status(&recovered.status),
        Some(&recovered.id),
    )
    .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;
    Ok(Json(map_row_to_response(updated)?))
}

pub fn agent_api_healthcheck() -> anyhow::Result<()> {
    let _ = resolved_spawner_params().context("resolve and validate microvm spawner params")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;
    use base64::Engine;
    use nostr_sdk::prelude::{EventBuilder, Kind, Tag, TagKind};

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
    fn authenticated_requester_npub_prefers_x_forwarded_host_when_trusted() {
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
    fn authenticated_requester_npub_rejects_untrusted_forwarded_host_override() {
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

        let err = authenticated_requester_npub(&headers, "GET", V1_AGENTS_ME_PATH, false)
            .expect_err("untrusted forwarded host must fail");
        assert_eq!(err.status, StatusCode::UNAUTHORIZED);
        assert_eq!(err.code, AgentApiErrorCode::Unauthorized);
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
    fn phase_from_vm_status_maps_running_to_ready() {
        assert_eq!(phase_from_vm_status("running"), AGENT_PHASE_READY);
        assert_eq!(phase_from_vm_status("starting"), AGENT_PHASE_CREATING);
        assert_eq!(phase_from_vm_status("booting"), AGENT_PHASE_CREATING);
    }
}
