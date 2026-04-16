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
    AppendRoomEventRequest, AppendRoomEventResponse, ClaimKeyPackageRequest,
    ClaimKeyPackageResponse, ClaimWelcomesResponse, CreateRoomRequest, CreateRoomResponse,
    SubmitMembershipCommitRequest, SubmitMembershipCommitResponse, SyncRoomEventsQuery,
    SyncRoomEventsResponse, UploadKeyPackageRequest, UploadKeyPackageResponse,
    UploadWelcomeRequest, UploadWelcomeResponse,
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
        .route("/v1/key-packages", post(upload_key_package))
        .route("/v1/key-packages/claim", post(claim_key_package))
        .route("/v1/welcomes", post(upload_welcome))
        .route("/v1/welcomes/claim", post(claim_welcomes))
        .route("/v1/rooms", post(create_room))
        .route(
            "/v1/rooms/:room_id/events",
            post(append_room_event).get(sync_room_events),
        )
        .route(
            "/v1/rooms/:room_id/membership-commits",
            post(submit_membership_commit),
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

async fn upload_key_package(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UploadKeyPackageRequest>,
) -> Result<Json<UploadKeyPackageResponse>, (StatusCode, String)> {
    let claims = session_claims(&state, &headers)?;
    let now = Timestamp::now().as_secs();
    let key_package = state
        .store
        .upload_key_package(&claims.npub, request, now)
        .await;
    Ok(Json(UploadKeyPackageResponse {
        key_package: key_package.map_err(store_handle_error)?,
    }))
}

async fn claim_key_package(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ClaimKeyPackageRequest>,
) -> Result<Json<ClaimKeyPackageResponse>, (StatusCode, String)> {
    let claims = session_claims(&state, &headers)?;
    let now = Timestamp::now().as_secs();
    let key_package = state
        .store
        .claim_key_package(&claims.npub, request, now)
        .await;
    Ok(Json(ClaimKeyPackageResponse {
        key_package: key_package.map_err(store_handle_error)?,
    }))
}

async fn upload_welcome(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UploadWelcomeRequest>,
) -> Result<Json<UploadWelcomeResponse>, (StatusCode, String)> {
    let claims = session_claims(&state, &headers)?;
    let now = Timestamp::now().as_secs();
    let welcome = state
        .store
        .enqueue_welcome(&claims.npub, request, now)
        .await
        .map_err(store_handle_error)?;
    Ok(Json(UploadWelcomeResponse { welcome }))
}

async fn claim_welcomes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ClaimWelcomesResponse>, (StatusCode, String)> {
    let claims = session_claims(&state, &headers)?;
    let welcomes = state
        .store
        .claim_welcomes(&claims.npub)
        .await
        .map_err(store_handle_error)?;
    Ok(Json(ClaimWelcomesResponse { welcomes }))
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

async fn submit_membership_commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(room_id): Path<String>,
    Json(request): Json<SubmitMembershipCommitRequest>,
) -> Result<Json<SubmitMembershipCommitResponse>, (StatusCode, String)> {
    let claims = session_claims(&state, &headers)?;
    let now = Timestamp::now().as_secs();
    let (room, event) = state
        .store
        .submit_membership_commit(&claims.npub, &room_id, request, now)
        .await
        .map_err(store_handle_error)?;
    Ok(Json(SubmitMembershipCommitResponse { room, event }))
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
        StoreError::RoomNotFound | StoreError::KeyPackageNotFound => StatusCode::NOT_FOUND,
        StoreError::NotRoomMember => StatusCode::FORBIDDEN,
        StoreError::RoomEpochMismatch { .. } => StatusCode::CONFLICT,
        StoreError::EmptyEventContent | StoreError::EmptyKeyPackagePayload => {
            StatusCode::BAD_REQUEST
        }
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
        assert_eq!(create_room.room.epoch, 0);
        assert!(
            create_room
                .room
                .members
                .iter()
                .any(|member| member == &alice_npub),
            "creator should be present in room membership"
        );

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
        assert_eq!(sync.room.epoch, 0);
        assert_eq!(sync.room.last_seq, 1);
        assert_eq!(sync.events.len(), 1);
        assert_eq!(sync.events[0].content, "ciphertext-1");
    }

    #[tokio::test]
    async fn member_can_submit_authoritative_membership_commit() {
        let app = router(test_state());
        let (app, alice_token, alice_npub) = login_token(app, "chat.test").await;
        let (app, _bob_token, bob_npub) = login_token(app, "chat.test").await;
        let (app, carol_token, carol_npub) = login_token(app, "chat.test").await;

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
                            member_npubs: vec![bob_npub.clone()],
                        })
                        .expect("serialize create room request"),
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

        let commit_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/rooms/{}/membership-commits",
                        create_room.room.room_id
                    ))
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&crate::protocol::SubmitMembershipCommitRequest {
                            expected_epoch: 0,
                            member_npubs: vec![
                                alice_npub.clone(),
                                bob_npub.clone(),
                                carol_npub.clone(),
                            ],
                            wrapper_event_json: "{\"kind\":1059,\"content\":\"membership-commit\"}"
                                .to_string(),
                            welcomes: vec![crate::protocol::WelcomeEnvelope {
                                recipient_npub: carol_npub.clone(),
                                wrapper_event_json: "{\"kind\":1059,\"content\":\"welcome\"}"
                                    .to_string(),
                                server_url: Some("https://chat.example".to_string()),
                                room_id: Some(create_room.room.room_id.clone()),
                            }],
                        })
                        .expect("serialize membership commit request"),
                    ))
                    .expect("build membership commit request"),
            )
            .await
            .expect("membership commit response");
        assert_eq!(commit_response.status(), StatusCode::OK);
        let commit_body = to_bytes(commit_response.into_body(), usize::MAX)
            .await
            .expect("read membership commit body");
        let committed: crate::protocol::SubmitMembershipCommitResponse =
            serde_json::from_slice(&commit_body).expect("decode membership commit body");
        assert_eq!(committed.room.epoch, 1);
        assert_eq!(committed.room.last_seq, 1);
        assert!(committed.room.members.contains(&carol_npub));
        assert_eq!(
            committed.event.event_type,
            crate::protocol::RoomEventType::Commit
        );
        assert_eq!(committed.event.seq, 1);
        assert_eq!(committed.event.epoch, 1);

        let claim_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/welcomes/claim")
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {carol_token}"))
                    .body(Body::empty())
                    .expect("build claim welcomes request"),
            )
            .await
            .expect("claim welcomes response");
        assert_eq!(claim_response.status(), StatusCode::OK);
        let claim_body = to_bytes(claim_response.into_body(), usize::MAX)
            .await
            .expect("read claim welcomes body");
        let claimed: ClaimWelcomesResponse =
            serde_json::from_slice(&claim_body).expect("decode claim welcomes body");
        assert_eq!(claimed.welcomes.len(), 1);
        assert_eq!(claimed.welcomes[0].recipient_npub, carol_npub);
        assert_eq!(
            claimed.welcomes[0].server_url.as_deref(),
            Some("https://chat.example")
        );
        assert_eq!(
            claimed.welcomes[0].room_id.as_deref(),
            Some(create_room.room.room_id.as_str())
        );

        let sync_response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/rooms/{}/events?after_seq=0&limit=10",
                        create_room.room.room_id
                    ))
                    .method("GET")
                    .header(header::AUTHORIZATION, format!("Bearer {carol_token}"))
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
        assert_eq!(sync.room.epoch, 1);
        assert_eq!(sync.events.len(), 1);
        assert_eq!(
            sync.events[0].event_type,
            crate::protocol::RoomEventType::Commit
        );
    }

    #[tokio::test]
    async fn membership_commit_rejects_stale_epoch_without_side_effects() {
        let app = router(test_state());
        let (app, alice_token, alice_npub) = login_token(app, "chat.test").await;
        let (app, _bob_token, bob_npub) = login_token(app, "chat.test").await;
        let (app, carol_token, carol_npub) = login_token(app, "chat.test").await;

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
                            member_npubs: vec![bob_npub.clone()],
                        })
                        .expect("serialize create room request"),
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

        let first_commit_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/rooms/{}/membership-commits",
                        create_room.room.room_id
                    ))
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&crate::protocol::SubmitMembershipCommitRequest {
                            expected_epoch: 0,
                            member_npubs: vec![alice_npub.clone(), bob_npub.clone()],
                            wrapper_event_json: "{\"kind\":1059,\"content\":\"membership-commit\"}"
                                .to_string(),
                            welcomes: vec![],
                        })
                        .expect("serialize first membership commit request"),
                    ))
                    .expect("build first membership commit request"),
            )
            .await
            .expect("first membership commit response");
        assert_eq!(first_commit_response.status(), StatusCode::OK);

        let stale_commit_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/rooms/{}/membership-commits",
                        create_room.room.room_id
                    ))
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&crate::protocol::SubmitMembershipCommitRequest {
                            expected_epoch: 0,
                            member_npubs: vec![alice_npub, bob_npub, carol_npub.clone()],
                            wrapper_event_json: "{\"kind\":1059,\"content\":\"stale-commit\"}"
                                .to_string(),
                            welcomes: vec![crate::protocol::WelcomeEnvelope {
                                recipient_npub: carol_npub.clone(),
                                wrapper_event_json: "{\"kind\":1059,\"content\":\"welcome\"}"
                                    .to_string(),
                                server_url: Some("https://chat.example".to_string()),
                                room_id: Some(create_room.room.room_id.clone()),
                            }],
                        })
                        .expect("serialize stale membership commit request"),
                    ))
                    .expect("build stale membership commit request"),
            )
            .await
            .expect("stale membership commit response");
        assert_eq!(stale_commit_response.status(), StatusCode::CONFLICT);

        let claim_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/welcomes/claim")
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {carol_token}"))
                    .body(Body::empty())
                    .expect("build claim welcomes request"),
            )
            .await
            .expect("claim welcomes response");
        let claim_body = to_bytes(claim_response.into_body(), usize::MAX)
            .await
            .expect("read claim welcomes body");
        let claimed: ClaimWelcomesResponse =
            serde_json::from_slice(&claim_body).expect("decode claim welcomes body");
        assert!(claimed.welcomes.is_empty());

        let sync_response = app
            .clone()
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
        assert_eq!(sync.room.epoch, 1);
        assert_eq!(sync.room.last_seq, 1);
        assert_eq!(sync.events.len(), 1);

        let carol_sync_response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/rooms/{}/events?after_seq=0&limit=10",
                        create_room.room.room_id
                    ))
                    .method("GET")
                    .header(header::AUTHORIZATION, format!("Bearer {carol_token}"))
                    .body(Body::empty())
                    .expect("build sync request"),
            )
            .await
            .expect("carol sync response");
        assert_eq!(carol_sync_response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn member_can_upload_and_claim_key_package() {
        let app = router(test_state());
        let (app, alice_token, _) = login_token(app, "chat.test").await;
        let (app, bob_token, _) = login_token(app, "chat.test").await;

        let upload_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/key-packages")
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&UploadKeyPackageRequest {
                            ciphersuite: Some("mls128".to_string()),
                            payload: "opaque-key-package".to_string(),
                        })
                        .expect("serialize upload key package request"),
                    ))
                    .expect("build upload key package request"),
            )
            .await
            .expect("upload key package response");
        assert_eq!(upload_response.status(), StatusCode::OK);
        let upload_body = to_bytes(upload_response.into_body(), usize::MAX)
            .await
            .expect("read upload key package body");
        let uploaded: UploadKeyPackageResponse =
            serde_json::from_slice(&upload_body).expect("decode upload key package body");
        assert_eq!(uploaded.key_package.claimed_at, None);

        let claim_response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/key-packages/claim")
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&ClaimKeyPackageRequest {
                            owner_npub: uploaded.key_package.owner_npub.clone(),
                            room_id: None,
                        })
                        .expect("serialize claim key package request"),
                    ))
                    .expect("build claim key package request"),
            )
            .await
            .expect("claim key package response");
        assert_eq!(claim_response.status(), StatusCode::OK);
        let claim_body = to_bytes(claim_response.into_body(), usize::MAX)
            .await
            .expect("read claim key package body");
        let claimed: ClaimKeyPackageResponse =
            serde_json::from_slice(&claim_body).expect("decode claim key package body");
        assert_eq!(claimed.key_package.payload, "opaque-key-package");
        assert!(claimed.key_package.claimed_at.is_some());
    }

    #[tokio::test]
    async fn member_can_upload_and_claim_welcome() {
        let app = router(test_state());
        let (app, alice_token, _) = login_token(app, "chat.test").await;
        let (app, bob_token, bob_npub) = login_token(app, "chat.test").await;

        let upload_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/welcomes")
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&UploadWelcomeRequest {
                            recipient_npub: bob_npub.clone(),
                            wrapper_event_json: "{\"kind\":1059}".to_string(),
                            server_url: Some("https://chat.example".to_string()),
                            room_id: Some("room_123".to_string()),
                        })
                        .expect("serialize upload welcome request"),
                    ))
                    .expect("build upload welcome request"),
            )
            .await
            .expect("upload welcome response");
        assert_eq!(upload_response.status(), StatusCode::OK);

        let claim_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/welcomes/claim")
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                    .body(Body::empty())
                    .expect("build claim welcomes request"),
            )
            .await
            .expect("claim welcomes response");
        assert_eq!(claim_response.status(), StatusCode::OK);
        let claim_body = to_bytes(claim_response.into_body(), usize::MAX)
            .await
            .expect("read claim welcomes body");
        let claimed: ClaimWelcomesResponse =
            serde_json::from_slice(&claim_body).expect("decode claim welcomes body");
        assert_eq!(claimed.welcomes.len(), 1);
        assert_eq!(claimed.welcomes[0].recipient_npub, bob_npub);
        assert_eq!(
            claimed.welcomes[0].server_url.as_deref(),
            Some("https://chat.example")
        );
        assert_eq!(claimed.welcomes[0].room_id.as_deref(), Some("room_123"));

        let empty_claim_response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/welcomes/claim")
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                    .body(Body::empty())
                    .expect("build empty claim welcomes request"),
            )
            .await
            .expect("empty claim welcomes response");
        assert_eq!(empty_claim_response.status(), StatusCode::OK);
        let empty_claim_body = to_bytes(empty_claim_response.into_body(), usize::MAX)
            .await
            .expect("read empty claim welcomes body");
        let empty_claimed: ClaimWelcomesResponse =
            serde_json::from_slice(&empty_claim_body).expect("decode empty claim welcomes body");
        assert!(empty_claimed.welcomes.is_empty());
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

    #[tokio::test]
    async fn outsider_cannot_claim_room_scoped_key_package() {
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
                        .expect("serialize create room request"),
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

        let upload_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/key-packages")
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&UploadKeyPackageRequest {
                            ciphersuite: Some("mls128".to_string()),
                            payload: "opaque-key-package".to_string(),
                        })
                        .expect("serialize upload key package request"),
                    ))
                    .expect("build upload key package request"),
            )
            .await
            .expect("upload key package response");
        let upload_body = to_bytes(upload_response.into_body(), usize::MAX)
            .await
            .expect("read upload key package body");
        let uploaded: UploadKeyPackageResponse =
            serde_json::from_slice(&upload_body).expect("decode upload key package body");

        let claim_response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/key-packages/claim")
                    .method("POST")
                    .header(header::AUTHORIZATION, format!("Bearer {mallory_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&ClaimKeyPackageRequest {
                            owner_npub: uploaded.key_package.owner_npub,
                            room_id: Some(create_room.room.room_id),
                        })
                        .expect("serialize claim key package request"),
                    ))
                    .expect("build claim key package request"),
            )
            .await
            .expect("claim key package response");
        assert_eq!(claim_response.status(), StatusCode::FORBIDDEN);
    }
}
