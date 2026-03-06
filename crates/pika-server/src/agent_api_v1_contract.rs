use std::collections::HashSet;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

pub const V1_AGENTS_ENSURE_PATH: &str = "/v1/agents/ensure";
pub const V1_AGENTS_ME_PATH: &str = "/v1/agents/me";
pub const V1_AGENTS_RECOVER_PATH: &str = "/v1/agents/me/recover";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAppState {
    Creating,
    Ready,
    Error,
}

pub const fn app_visible_states() -> [AgentAppState; 3] {
    [
        AgentAppState::Creating,
        AgentAppState::Ready,
        AgentAppState::Error,
    ]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentApiErrorCode {
    Unauthorized,
    NotWhitelisted,
    InvalidRequest,
    AgentExists,
    AgentNotFound,
    RecoverFailed,
    Internal,
}

pub const AGENT_API_V1_ERROR_CODES: [AgentApiErrorCode; 7] = [
    AgentApiErrorCode::Unauthorized,
    AgentApiErrorCode::NotWhitelisted,
    AgentApiErrorCode::InvalidRequest,
    AgentApiErrorCode::AgentExists,
    AgentApiErrorCode::AgentNotFound,
    AgentApiErrorCode::RecoverFailed,
    AgentApiErrorCode::Internal,
];

impl AgentApiErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::NotWhitelisted => "not_whitelisted",
            Self::InvalidRequest => "invalid_request",
            Self::AgentExists => "agent_exists",
            Self::AgentNotFound => "agent_not_found",
            Self::RecoverFailed => "recover_failed",
            Self::Internal => "internal",
        }
    }

    pub fn status_code(self) -> StatusCode {
        match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::NotWhitelisted => StatusCode::FORBIDDEN,
            Self::InvalidRequest => StatusCode::BAD_REQUEST,
            Self::AgentExists => StatusCode::CONFLICT,
            Self::AgentNotFound => StatusCode::NOT_FOUND,
            Self::RecoverFailed => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

pub fn contract_healthcheck() -> anyhow::Result<()> {
    anyhow::ensure!(V1_AGENTS_ENSURE_PATH == "/v1/agents/ensure");
    anyhow::ensure!(V1_AGENTS_ME_PATH == "/v1/agents/me");
    anyhow::ensure!(V1_AGENTS_RECOVER_PATH == "/v1/agents/me/recover");
    anyhow::ensure!(
        app_visible_states()
            == [
                AgentAppState::Creating,
                AgentAppState::Ready,
                AgentAppState::Error
            ]
    );

    let mut seen = HashSet::new();
    for code in AGENT_API_V1_ERROR_CODES {
        anyhow::ensure!(
            seen.insert(code.as_str()),
            "duplicate error code in v1 contract"
        );
        let status = code.status_code();
        anyhow::ensure!(
            status.is_client_error() || status.is_server_error(),
            "unexpected status mapping for {}: {}",
            code.as_str(),
            status
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_fixed() {
        assert_eq!(V1_AGENTS_ENSURE_PATH, "/v1/agents/ensure");
        assert_eq!(V1_AGENTS_ME_PATH, "/v1/agents/me");
        assert_eq!(V1_AGENTS_RECOVER_PATH, "/v1/agents/me/recover");
    }

    #[test]
    fn app_visible_states_are_exactly_creating_ready_error() {
        let states = app_visible_states();
        assert_eq!(states.len(), 3);

        let encoded: Vec<String> = states
            .iter()
            .map(|state| serde_json::to_string(state).expect("encode app state"))
            .collect();
        assert_eq!(encoded, vec!["\"creating\"", "\"ready\"", "\"error\""]);
    }

    #[test]
    fn error_codes_and_statuses_are_fixed() {
        let observed: Vec<(&'static str, StatusCode)> = AGENT_API_V1_ERROR_CODES
            .iter()
            .map(|code| (code.as_str(), code.status_code()))
            .collect();
        assert_eq!(
            observed,
            vec![
                ("unauthorized", StatusCode::UNAUTHORIZED),
                ("not_whitelisted", StatusCode::FORBIDDEN),
                ("invalid_request", StatusCode::BAD_REQUEST),
                ("agent_exists", StatusCode::CONFLICT),
                ("agent_not_found", StatusCode::NOT_FOUND),
                ("recover_failed", StatusCode::SERVICE_UNAVAILABLE),
                ("internal", StatusCode::INTERNAL_SERVER_ERROR),
            ]
        );
    }
}
