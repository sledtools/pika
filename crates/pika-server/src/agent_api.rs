use std::collections::HashSet;
use std::sync::OnceLock;

use anyhow::Context;
use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::{response::IntoResponse, Json};
use diesel::result::{DatabaseErrorKind, Error as DieselError};
use nostr_sdk::prelude::PublicKey;
use rand::Rng;
use serde::Serialize;

use crate::agent_api_v1_contract::{AgentApiErrorCode, AgentAppState};
use crate::models::agent_instance::{AgentInstance, AGENT_PHASE_CREATING, AGENT_PHASE_READY};
use crate::State;

const OWNER_NPUB_HEADER: &str = "x-pika-owner-npub";
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
    pubkey_hex: String,
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

fn requester_identity(headers: &HeaderMap) -> Result<RequesterIdentity, AgentApiError> {
    let raw = headers
        .get(OWNER_NPUB_HEADER)
        .ok_or_else(|| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))?;
    let owner_npub = raw
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))?;
    if !owner_npub.starts_with("npub1") {
        return Err(AgentApiError::from_code(AgentApiErrorCode::Unauthorized));
    }
    let pubkey_hex = PublicKey::parse(owner_npub)
        .map(|pk| pk.to_hex())
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))?;
    Ok(RequesterIdentity {
        owner_npub: owner_npub.to_string(),
        pubkey_hex,
    })
}

fn require_whitelisted_requester(headers: &HeaderMap) -> Result<RequesterIdentity, AgentApiError> {
    let requester = requester_identity(headers)?;
    if !fixed_allowlist_pubkeys_hex().contains(&requester.pubkey_hex) {
        return Err(AgentApiError::from_code(AgentApiErrorCode::NotWhitelisted));
    }
    Ok(requester)
}

fn phase_to_state(phase: &str) -> Option<AgentAppState> {
    match phase {
        AGENT_PHASE_CREATING => Some(AgentAppState::Creating),
        AGENT_PHASE_READY => Some(AgentAppState::Ready),
        "error" => Some(AgentAppState::Error),
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

fn generate_agent_id() -> String {
    let suffix = rand::thread_rng().r#gen::<u64>();
    format!("agent-{suffix:016x}")
}

pub async fn ensure_agent(
    Extension(state): Extension<State>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<AgentStateResponse>), AgentApiError> {
    let requester = require_whitelisted_requester(&headers)?;
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
        &generate_agent_id(),
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

    Ok((StatusCode::ACCEPTED, Json(map_row_to_response(created)?)))
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
    // v1 step 5 keeps recover behavior simple; dedicated recover flow is implemented in step 9.
    Ok(Json(map_row_to_response(active)?))
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
    fn whitelist_rejects_non_allowed_requester() {
        let random_npub = "npub1y2z0c7un9dwmhk4zrpw8df8p0gh0j2x54qhznwqjnp452ju4078srmwp70";
        let mut headers = HeaderMap::new();
        headers.insert(
            OWNER_NPUB_HEADER,
            random_npub.parse().expect("header value"),
        );

        let err = require_whitelisted_requester(&headers).expect_err("must reject non allowlist");
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        assert_eq!(err.code, AgentApiErrorCode::NotWhitelisted);
    }

    #[test]
    fn whitelist_requires_owner_npub_header() {
        let headers = HeaderMap::new();
        let err = require_whitelisted_requester(&headers).expect_err("missing header must fail");
        assert_eq!(err.status, StatusCode::UNAUTHORIZED);
        assert_eq!(err.code, AgentApiErrorCode::Unauthorized);
    }

    #[test]
    fn whitelist_rejects_non_npub_header_values() {
        let mut headers = HeaderMap::new();
        headers.insert(OWNER_NPUB_HEADER, "abc123".parse().expect("header value"));
        let err = require_whitelisted_requester(&headers).expect_err("invalid header must fail");
        assert_eq!(err.status, StatusCode::UNAUTHORIZED);
        assert_eq!(err.code, AgentApiErrorCode::Unauthorized);
    }
}
