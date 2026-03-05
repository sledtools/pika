use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use anyhow::Context;
use axum::extract::Extension;
use axum::http::{header, HeaderMap, StatusCode};
use axum::{response::IntoResponse, Json};
use diesel::result::{DatabaseErrorKind, Error as DieselError};
use nostr_sdk::prelude::{Keys, PublicKey};
use nostr_sdk::ToBech32;
use serde::Serialize;

use crate::agent_api_v1_contract::{AgentApiErrorCode, AgentAppState};
use crate::agent_control::{
    default_microvm_spawner_url_from_env, ensure_private_microvm_spawner_url,
};
use crate::models::agent_instance::{
    AgentInstance, AGENT_PHASE_CREATING, AGENT_PHASE_ERROR, AGENT_PHASE_READY,
};
use crate::State;
use pika_agent_control_plane::MicrovmProvisionParams;
use pika_agent_microvm::{
    build_create_vm_request, resolve_params, spawner_create_error, MicrovmSpawnerClient,
    ResolvedMicrovmParams,
};
use pika_relay_profiles::default_message_relays;

const OWNER_TOKEN_MAP_ENV: &str = "PIKA_AGENT_OWNER_TOKEN_MAP";
const AGENT_OWNER_ACTIVE_INDEX: &str = "agent_instances_owner_active_idx";
const FIXED_ALLOWLIST_NPUBS: [&str; 3] = [
    // justin
    "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y",
    // benthecarman (Ben)
    "npub1rtrxx9eyvag0ap3v73c4dvsqq5d2yxwe5d72qxrfpwe5svr96wuqed4p38",
    // Paul
    "npub1p4kg8zxukpym3h20erfa3samj00rm2gt4q5wfuyu3tg0x3jg3gesvncxf8",
];

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

fn fixed_allowlist_pubkeys_hex() -> &'static HashSet<String> {
    static ALLOWLIST_HEX: OnceLock<HashSet<String>> = OnceLock::new();
    ALLOWLIST_HEX.get_or_init(|| {
        FIXED_ALLOWLIST_NPUBS
            .iter()
            .map(|npub| {
                PublicKey::parse(npub)
                    .expect("fixed allowlist npub must parse")
                    .to_hex()
            })
            .collect::<HashSet<_>>()
    })
}

fn parse_owner_token_map(raw: &str) -> anyhow::Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for entry in raw.split(',') {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (token, owner_npub) = trimmed.split_once('=').with_context(|| {
            format!("invalid token map entry `{trimmed}` (expected token=npub)")
        })?;
        let token = token.trim();
        let owner_npub = owner_npub.trim();
        anyhow::ensure!(!token.is_empty(), "token cannot be empty");
        anyhow::ensure!(!owner_npub.is_empty(), "owner npub cannot be empty");
        anyhow::ensure!(
            owner_npub.starts_with("npub1"),
            "owner must be provided as npub in token map: {owner_npub}"
        );
        PublicKey::parse(owner_npub)
            .with_context(|| format!("invalid owner npub in token map: {owner_npub}"))?;
        map.insert(token.to_string(), owner_npub.to_string());
    }
    Ok(map)
}

fn configured_owner_token_map() -> anyhow::Result<HashMap<String, String>> {
    let raw = std::env::var(OWNER_TOKEN_MAP_ENV).unwrap_or_default();
    parse_owner_token_map(&raw)
}

fn extract_bearer_token(headers: &HeaderMap) -> Result<String, AgentApiError> {
    let auth = headers
        .get(header::AUTHORIZATION)
        .ok_or_else(|| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))?;
    let auth = auth
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))?;
    let token = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))?;
    Ok(token.to_string())
}

fn require_whitelisted_requester(headers: &HeaderMap) -> Result<RequesterIdentity, AgentApiError> {
    let token = extract_bearer_token(headers)?;
    let owner_npub = configured_owner_token_map()
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))?
        .get(&token)
        .cloned()
        .ok_or_else(|| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))?;

    let pubkey_hex = PublicKey::parse(&owner_npub)
        .map(|pk| pk.to_hex())
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))?;
    if !fixed_allowlist_pubkeys_hex().contains(&pubkey_hex) {
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
    let params = MicrovmProvisionParams {
        spawner_url: default_microvm_spawner_url_from_env(),
        ..MicrovmProvisionParams::default()
    };
    let resolved = resolve_params(&params, false);
    ensure_private_microvm_spawner_url(&resolved.spawner_url)
        .context("validate private microvm spawner URL")?;
    Ok(resolved)
}

async fn provision_vm_for_owner(
    owner_npub: &str,
    bot_identity: &ProvisioningBotIdentity,
) -> anyhow::Result<String> {
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
    let vm = spawner
        .create_vm(&create_vm)
        .await
        .map_err(|err| spawner_create_error(&resolved.spawner_url, err))?;
    Ok(vm.id)
}

pub async fn ensure_agent(
    Extension(state): Extension<State>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<AgentStateResponse>), AgentApiError> {
    let requester = require_whitelisted_requester(&headers)?;
    let bot_identity = generate_provisioning_bot_identity()
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;
    let mut conn = state
        .db_pool
        .get()
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;

    if AgentInstance::find_active_by_owner(&mut conn, &requester.owner_npub)
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?
        .is_some()
    {
        return Err(AgentApiError::from_code(AgentApiErrorCode::AgentExists));
    }

    let created = AgentInstance::create(
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
    })?;

    let vm_id = match provision_vm_for_owner(&requester.owner_npub, &bot_identity).await {
        Ok(vm_id) => vm_id,
        Err(_) => {
            let _ =
                AgentInstance::update_phase(&mut conn, &created.agent_id, AGENT_PHASE_ERROR, None);
            return Err(AgentApiError::from_code(AgentApiErrorCode::Internal));
        }
    };

    let updated = AgentInstance::update_phase(
        &mut conn,
        &created.agent_id,
        AGENT_PHASE_CREATING,
        Some(&vm_id),
    )
    .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;

    Ok((StatusCode::ACCEPTED, Json(map_row_to_response(updated)?)))
}

pub async fn get_my_agent(
    Extension(state): Extension<State>,
    headers: HeaderMap,
) -> Result<Json<AgentStateResponse>, AgentApiError> {
    let requester = require_whitelisted_requester(&headers)?;
    let mut conn = state
        .db_pool
        .get()
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;
    let Some(active) = AgentInstance::find_active_by_owner(&mut conn, &requester.owner_npub)
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?
    else {
        return Err(AgentApiError::from_code(AgentApiErrorCode::AgentNotFound));
    };
    Ok(Json(map_row_to_response(active)?))
}

pub async fn recover_my_agent(
    Extension(state): Extension<State>,
    headers: HeaderMap,
) -> Result<Json<AgentStateResponse>, AgentApiError> {
    let requester = require_whitelisted_requester(&headers)?;
    let mut conn = state
        .db_pool
        .get()
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;
    let Some(active) = AgentInstance::find_active_by_owner(&mut conn, &requester.owner_npub)
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
        AGENT_PHASE_CREATING,
        Some(&recovered.id),
    )
    .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Internal))?;
    Ok(Json(map_row_to_response(updated)?))
}

pub fn whitelist_healthcheck() -> anyhow::Result<()> {
    anyhow::ensure!(
        FIXED_ALLOWLIST_NPUBS.len() == 3,
        "fixed microvm allowlist must include exactly three npubs"
    );

    let mut dedupe = HashSet::new();
    for npub in FIXED_ALLOWLIST_NPUBS {
        let normalized = PublicKey::parse(npub)
            .with_context(|| format!("invalid fixed allowlist npub: {npub}"))?
            .to_hex();
        anyhow::ensure!(
            dedupe.insert(normalized),
            "duplicate npub in fixed allowlist"
        );
    }

    let raw_map = std::env::var(OWNER_TOKEN_MAP_ENV).unwrap_or_default();
    if !raw_map.trim().is_empty() {
        let map = parse_owner_token_map(&raw_map)?;
        for owner_npub in map.values() {
            let owner_hex = PublicKey::parse(owner_npub)?.to_hex();
            anyhow::ensure!(
                fixed_allowlist_pubkeys_hex().contains(&owner_hex),
                "owner in {} is not part of fixed 3-npub allowlist: {}",
                OWNER_TOKEN_MAP_ENV,
                owner_npub
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_healthcheck_validates_fixed_three_entries() {
        whitelist_healthcheck().expect("fixed allowlist should be valid");
        assert_eq!(fixed_allowlist_pubkeys_hex().len(), 3);
    }

    #[test]
    fn parse_owner_token_map_accepts_valid_entries() {
        let raw = "token-a=npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y";
        let map = parse_owner_token_map(raw).expect("parse token map");
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("token-a"));
    }

    #[test]
    fn parse_owner_token_map_rejects_invalid_entries() {
        let raw = "missing_equals";
        let err = parse_owner_token_map(raw).expect_err("invalid format must fail");
        assert!(err.to_string().contains("expected token=npub"));
    }

    #[test]
    fn extract_bearer_token_requires_authorization_header() {
        let headers = HeaderMap::new();
        let err = extract_bearer_token(&headers).expect_err("missing header must fail");
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
}
