use std::collections::HashSet;
use std::sync::OnceLock;

use anyhow::Context;
use axum::http::{HeaderMap, StatusCode};
use axum::{response::IntoResponse, Json};
use nostr_sdk::prelude::PublicKey;
use serde::Serialize;

use crate::agent_api_v1_contract::{AgentApiErrorCode, AgentAppState};

const OWNER_NPUB_HEADER: &str = "x-pika-owner-npub";
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

fn requester_pubkey_hex(headers: &HeaderMap) -> Result<String, AgentApiError> {
    let raw = headers
        .get(OWNER_NPUB_HEADER)
        .ok_or_else(|| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))?;
    let npub = raw
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))?;
    PublicKey::parse(npub)
        .map(|pk| pk.to_hex())
        .map_err(|_| AgentApiError::from_code(AgentApiErrorCode::Unauthorized))
}

fn require_whitelisted_requester(headers: &HeaderMap) -> Result<String, AgentApiError> {
    let requester_hex = requester_pubkey_hex(headers)?;
    if !fixed_allowlist_pubkeys_hex().contains(&requester_hex) {
        return Err(AgentApiError::from_code(AgentApiErrorCode::NotWhitelisted));
    }
    Ok(requester_hex)
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentStateResponse {
    state: AgentAppState,
}

pub async fn ensure_agent(
    headers: HeaderMap,
) -> Result<(StatusCode, Json<AgentStateResponse>), AgentApiError> {
    let _owner = require_whitelisted_requester(&headers)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(AgentStateResponse {
            state: AgentAppState::Creating,
        }),
    ))
}

pub async fn get_my_agent(headers: HeaderMap) -> Result<Json<AgentStateResponse>, AgentApiError> {
    let _owner = require_whitelisted_requester(&headers)?;
    Err(AgentApiError::from_code(AgentApiErrorCode::AgentNotFound))
}

pub async fn recover_my_agent(
    headers: HeaderMap,
) -> Result<Json<AgentStateResponse>, AgentApiError> {
    let _owner = require_whitelisted_requester(&headers)?;
    Err(AgentApiError::from_code(AgentApiErrorCode::AgentNotFound))
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
    use nostr_sdk::prelude::Keys;

    #[test]
    fn whitelist_healthcheck_validates_fixed_three_entries() {
        whitelist_healthcheck().expect("fixed allowlist should be valid");
        assert_eq!(fixed_allowlist_pubkeys_hex().len(), 3);
    }

    #[test]
    fn whitelist_rejects_non_allowed_requester() {
        let non_whitelisted = Keys::generate().public_key().to_hex();
        let mut headers = HeaderMap::new();
        headers.insert(
            OWNER_NPUB_HEADER,
            non_whitelisted.parse().expect("header value"),
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
}
