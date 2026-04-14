use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use nostr_sdk::prelude::Timestamp;
use serde::Serialize;

use crate::nostr_auth::{
    event_from_authorization_header, expected_host_from_headers, verify_nip98_event,
};
use crate::protocol::{
    AppendRoomEventRequest, AppendRoomEventResponse, CreateRoomRequest, CreateRoomResponse,
    RegisterDeviceRequest, RegisterDeviceResponse, SyncRoomEventsQuery, SyncRoomEventsResponse,
};
use crate::session::{SessionClaims, SessionManager, SessionTokenResponse};
use crate::store::{StoreError, StoreHandle, StoreHandleError};

#[derive(Clone)]
pub struct AppState {
    pub sessions: SessionManager,
    pub trust_forwarded_host: bool,
    pub store: StoreHandle,
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
        .route("/v1/devices/register", post(register_device))
        .route("/v1/rooms", post(create_room))
        .route(
            "/v1/rooms/:room_id/events",
            post(append_room_event).get(sync_room_events),
        )
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
    let claims = session_claims(&state, &headers)?;
    Ok(Json(session_info(claims)))
}

async fn register_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RegisterDeviceRequest>,
) -> Result<Json<RegisterDeviceResponse>, (StatusCode, String)> {
    let claims = session_claims(&state, &headers)?;
    let now = Timestamp::now().as_secs();
    let device = state
        .store
        .register_device(&claims.npub, request, now)
        .await;
    Ok(Json(RegisterDeviceResponse {
        device: device.map_err(store_handle_error)?,
    }))
}

async fn create_room(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateRoomRequest>,
) -> Result<Json<CreateRoomResponse>, (StatusCode, String)> {
    let claims = session_claims(&state, &headers)?;
    let now = Timestamp::now().as_secs();
    let room = state.store.create_room(&claims.npub, request, now).await;
    Ok(Json(CreateRoomResponse {
        room: room.map_err(store_handle_error)?,
    }))
}

async fn append_room_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(room_id): Path<String>,
    Json(request): Json<AppendRoomEventRequest>,
) -> Result<Json<AppendRoomEventResponse>, (StatusCode, String)> {
    let claims = session_claims(&state, &headers)?;
    let now = Timestamp::now().as_secs();
    let event = state
        .store
        .append_room_event(&claims.npub, &room_id, request, now)
        .await
        .map_err(store_handle_error)?;
    Ok(Json(AppendRoomEventResponse { event }))
}

async fn sync_room_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(room_id): Path<String>,
    Query(query): Query<SyncRoomEventsQuery>,
) -> Result<Json<SyncRoomEventsResponse>, (StatusCode, String)> {
    let claims = session_claims(&state, &headers)?;
    let after_seq = query.after_seq.unwrap_or(0);
    let limit = query.limit.unwrap_or(100);
    let room = state
        .store
        .room_summary_for_member(&claims.npub, &room_id)
        .await
        .map_err(store_error)?;
    let events = state
        .store
        .sync_room_events(&claims.npub, &room_id, after_seq, limit)
        .await
        .map_err(store_error)?;
    Ok(Json(SyncRoomEventsResponse { room, events }))
}

fn session_info(claims: SessionClaims) -> SessionInfoResponse {
    SessionInfoResponse {
        npub: claims.npub,
        expires_at: claims.expires_at,
    }
}

fn session_claims(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<SessionClaims, (StatusCode, String)> {
    state
        .sessions
        .claims_from_bearer(headers, Timestamp::now().as_secs())
        .map_err(unauthorized)
}

fn unauthorized(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::UNAUTHORIZED, err.to_string())
}

fn internal(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

fn store_error(err: StoreError) -> (StatusCode, String) {
    let status = match err {
        StoreError::RoomNotFound | StoreError::DeviceNotFound => StatusCode::NOT_FOUND,
        StoreError::NotRoomMember | StoreError::DeviceOwnerMismatch => StatusCode::FORBIDDEN,
        StoreError::EmptyEventContent => StatusCode::BAD_REQUEST,
    };
    (status, err.to_string())
}

fn store_handle_error(err: StoreHandleError) -> (StatusCode, String) {
    match err {
        StoreHandleError::Store(err) => store_error(err),
        StoreHandleError::Persist(err) => internal(err),
    }
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
            store: StoreHandle::in_memory(),
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

    async fn login_token(app: Router, host: &str) -> (Router, String, String) {
        let (headers, npub) = signed_login_headers(host);
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
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read login body");
        let login_response: SessionTokenResponse =
            serde_json::from_slice(&body).expect("decode login response");
        (app, login_response.access_token, npub)
    }

    #[tokio::test]
    async fn member_can_create_room_append_and_sync_events() {
        let (app, alice_token, alice_npub) = login_token(router(test_state()), "chat.test").await;

        let create_room_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/rooms")
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&CreateRoomRequest {
                            member_npubs: vec!["npub1bob".to_string()],
                        })
                        .expect("serialize create request"),
                    ))
                    .expect("build create room request"),
            )
            .await
            .expect("create room response");
        assert_eq!(create_room_response.status(), StatusCode::OK);
        let create_room_body = to_bytes(create_room_response.into_body(), usize::MAX)
            .await
            .expect("read create room body");
        let create_room: CreateRoomResponse =
            serde_json::from_slice(&create_room_body).expect("decode create room body");
        assert!(
            create_room
                .room
                .members
                .iter()
                .any(|member| member == &alice_npub),
            "creator should be present in room membership"
        );

        let register_device_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/devices/register")
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&RegisterDeviceRequest {
                            platform: Some("ios".to_string()),
                            push_token: Some("push-token".to_string()),
                        })
                        .expect("serialize register device request"),
                    ))
                    .expect("build register device request"),
            )
            .await
            .expect("register device response");
        assert_eq!(register_device_response.status(), StatusCode::OK);
        let register_device_body = to_bytes(register_device_response.into_body(), usize::MAX)
            .await
            .expect("read register device body");
        let register_device: RegisterDeviceResponse =
            serde_json::from_slice(&register_device_body).expect("decode register device body");

        let append_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/rooms/{}/events", create_room.room.room_id))
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&AppendRoomEventRequest {
                            event_type: crate::protocol::RoomEventType::ApplicationMessage,
                            epoch: 1,
                            sender_device_id: Some(register_device.device.device_id.clone()),
                            content: "ciphertext-1".to_string(),
                        })
                        .expect("serialize append request"),
                    ))
                    .expect("build append request"),
            )
            .await
            .expect("append response");
        assert_eq!(append_response.status(), StatusCode::OK);
        let append_body = to_bytes(append_response.into_body(), usize::MAX)
            .await
            .expect("read append body");
        let appended: AppendRoomEventResponse =
            serde_json::from_slice(&append_body).expect("decode append body");
        assert_eq!(appended.event.seq, 1);

        let sync_response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/rooms/{}/events?after_seq=0&limit=10",
                        create_room.room.room_id
                    ))
                    .method("GET")
                    .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                    .body(Body::empty())
                    .expect("build sync request"),
            )
            .await
            .expect("sync response");
        assert_eq!(sync_response.status(), StatusCode::OK);
        let sync_body = to_bytes(sync_response.into_body(), usize::MAX)
            .await
            .expect("read sync body");
        let sync: SyncRoomEventsResponse =
            serde_json::from_slice(&sync_body).expect("decode sync body");
        assert_eq!(sync.room.last_seq, 1);
        assert_eq!(sync.events.len(), 1);
        assert_eq!(sync.events[0].content, "ciphertext-1");
    }

    #[tokio::test]
    async fn outsider_cannot_append_to_room() {
        let app = router(test_state());
        let (app, alice_token, _) = login_token(app, "chat.test").await;
        let (app, mallory_token, _) = login_token(app, "chat.test").await;

        let create_room_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/rooms")
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&CreateRoomRequest {
                            member_npubs: vec!["npub1bob".to_string()],
                        })
                        .expect("serialize create request"),
                    ))
                    .expect("build create room request"),
            )
            .await
            .expect("create room response");
        let create_room_body = to_bytes(create_room_response.into_body(), usize::MAX)
            .await
            .expect("read create room body");
        let create_room: CreateRoomResponse =
            serde_json::from_slice(&create_room_body).expect("decode create room body");

        let append_response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/rooms/{}/events", create_room.room.room_id))
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {mallory_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&AppendRoomEventRequest {
                            event_type: crate::protocol::RoomEventType::ApplicationMessage,
                            epoch: 1,
                            sender_device_id: None,
                            content: "ciphertext-1".to_string(),
                        })
                        .expect("serialize append request"),
                    ))
                    .expect("build append request"),
            )
            .await
            .expect("append response");
        assert_eq!(append_response.status(), StatusCode::FORBIDDEN);
    }
}
