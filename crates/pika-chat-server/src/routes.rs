use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use nostr_sdk::prelude::Timestamp;
use serde::Serialize;

use crate::nostr_auth::{
    event_from_authorization_header, expected_host_from_headers, verify_nip98_event,
};
use crate::session::{SessionClaims, SessionManager, SessionTokenResponse};

#[derive(Clone)]
pub struct AppState {
    pub sessions: SessionManager,
    pub trust_forwarded_host: bool,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
}

#[derive(Debug, Serialize)]
struct SessionInfoResponse {
    npub: String,
    expires_at: u64,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health-check", get(health_check))
        .route("/v1/session/login", post(login))
        .route("/v1/session/me", get(me))
        .with_state(state)
}

async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SessionTokenResponse>, (StatusCode, String)> {
    let event = event_from_authorization_header(&headers).map_err(unauthorized)?;
    let expected_host = expected_host_from_headers(&headers, state.trust_forwarded_host);
    let npub = verify_nip98_event(
        &event,
        "POST",
        "/v1/session/login",
        expected_host.as_deref(),
        None,
    )
    .map_err(unauthorized)?;

    let response = state
        .sessions
        .issue_token(&npub, Timestamp::now().as_secs())
        .map_err(internal)?;
    Ok(Json(response))
}

async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SessionInfoResponse>, (StatusCode, String)> {
    let claims = state
        .sessions
        .claims_from_bearer(&headers, Timestamp::now().as_secs())
        .map_err(unauthorized)?;
    Ok(Json(session_info(claims)))
}

fn session_info(claims: SessionClaims) -> SessionInfoResponse {
    SessionInfoResponse {
        npub: claims.npub,
        expires_at: claims.expires_at,
    }
}

fn unauthorized(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::UNAUTHORIZED, err.to_string())
}

fn internal(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request};
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use nostr_sdk::prelude::{EventBuilder, Keys, Kind, Tag, TagKind, ToBech32};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        AppState {
            sessions: SessionManager::new([9u8; 32], 600),
            trust_forwarded_host: false,
        }
    }

    fn signed_login_headers(host: &str) -> (HeaderMap, String) {
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::Custom(27235), "")
            .tags([
                Tag::custom(
                    TagKind::custom("u"),
                    [format!("https://{host}/v1/session/login")],
                ),
                Tag::custom(TagKind::custom("method"), ["POST"]),
            ])
            .sign_with_keys(&keys)
            .expect("sign login event");
        let payload = serde_json::to_vec(&event).expect("serialize event");

        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Nostr {}", STANDARD.encode(payload))
                .parse()
                .expect("authorization header"),
        );
        headers.insert(header::HOST, host.parse().expect("host header"));

        (
            headers,
            keys.public_key()
                .to_bech32()
                .expect("encode npub")
                .to_lowercase(),
        )
    }

    #[tokio::test]
    async fn login_issues_bearer_session_and_me_reads_it() {
        let app = router(test_state());
        let (headers, expected_npub) = signed_login_headers("chat.test");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/session/login")
                    .method("POST")
                    .header(
                        header::AUTHORIZATION,
                        headers
                            .get(header::AUTHORIZATION)
                            .expect("authorization header")
                            .clone(),
                    )
                    .header(
                        header::HOST,
                        headers.get(header::HOST).expect("host header").clone(),
                    )
                    .body(Body::empty())
                    .expect("build login request"),
            )
            .await
            .expect("login response");
        assert_eq!(response.status(), StatusCode::OK);
        let login_body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read login body");
        let login_response: SessionTokenResponse =
            serde_json::from_slice(&login_body).expect("decode login response");
        assert_eq!(login_response.npub, expected_npub);

        let me_response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/session/me")
                    .method("GET")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", login_response.access_token),
                    )
                    .body(Body::empty())
                    .expect("build me request"),
            )
            .await
            .expect("me response");
        assert_eq!(me_response.status(), StatusCode::OK);
        let me_body = to_bytes(me_response.into_body(), usize::MAX)
            .await
            .expect("read me body");
        let me: serde_json::Value = serde_json::from_slice(&me_body).expect("decode me body");
        assert_eq!(me["npub"], expected_npub);
        assert!(
            me["expires_at"].as_u64().is_some(),
            "me response should include expiry"
        );
    }
}
