async fn auth_challenge_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if !state.auth.auth_enabled() {
        return auth_json_error(StatusCode::FORBIDDEN, "auth not enabled");
    }
    let nonce = state.auth.create_challenge();
    Json(serde_json::json!({"challenge": nonce})).into_response()
}

#[derive(serde::Deserialize)]
struct VerifyRequest {
    event: String,
}

async fn auth_verify_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<VerifyRequest>,
) -> impl IntoResponse {
    if !state.auth.auth_enabled() {
        return auth_json_error(StatusCode::FORBIDDEN, "auth not enabled");
    }
    match state.auth.verify_event(&body.event) {
        Ok((token, npub, is_admin)) => {
            let access = state.auth.access_for_npub(&npub);
            let store = state.store.clone();
            let npub_for_backfill = npub.clone();
            match tokio::task::spawn_blocking(move || {
                store.backfill_branch_inbox_for_npub(&npub_for_backfill)
            })
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(err)) => {
                    eprintln!("warning: auth inbox backfill failed: {}", err);
                }
                Err(err) => {
                    eprintln!("warning: auth inbox backfill task failed: {}", err);
                }
            }
            Json(serde_json::json!({
                "token": token,
                "npub": npub,
                "is_admin": is_admin,
                "can_forge_write": access.can_forge_write
            }))
            .into_response()
        }
        Err(msg) => auth_json_error(StatusCode::UNAUTHORIZED, &msg),
    }
}

#[derive(Clone, Copy)]
enum AccessRequirement {
    Authenticated,
    Chat,
    Inbox,
    Trusted,
    Admin,
}

impl AccessRequirement {
    fn allows(self, access: crate::auth::AccessState) -> bool {
        match self {
            AccessRequirement::Authenticated => true,
            AccessRequirement::Chat => access.can_chat,
            AccessRequirement::Inbox => access.can_chat || access.can_forge_write,
            AccessRequirement::Trusted => access.can_forge_write,
            AccessRequirement::Admin => access.is_admin,
        }
    }

    fn forbidden_message(self) -> Option<&'static str> {
        match self {
            AccessRequirement::Authenticated => None,
            AccessRequirement::Chat => Some("chat access revoked"),
            AccessRequirement::Inbox => Some("inbox access revoked"),
            AccessRequirement::Trusted => Some("trusted contributor access required"),
            AccessRequirement::Admin => Some("admin access required"),
        }
    }
}

fn auth_json_error(status: StatusCode, message: &str) -> axum::response::Response {
    (status, Json(serde_json::json!({"error": message}))).into_response()
}

fn extract_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

#[allow(clippy::result_large_err)]
fn require_access(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
    requirement: AccessRequirement,
) -> Result<String, axum::response::Response> {
    let token = extract_token(headers)
        .ok_or_else(|| auth_json_error(StatusCode::UNAUTHORIZED, "missing auth token"))?;
    let npub = auth
        .validate_token(&token)
        .ok_or_else(|| auth_json_error(StatusCode::UNAUTHORIZED, "invalid or expired token"))?;
    let access = auth.access_for_npub(&npub);
    if requirement.allows(access) {
        Ok(npub)
    } else {
        Err(auth_json_error(
            StatusCode::FORBIDDEN,
            requirement.forbidden_message().unwrap_or("access denied"),
        ))
    }
}

#[allow(clippy::result_large_err)]
fn require_auth(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
) -> Result<String, axum::response::Response> {
    require_access(auth, headers, AccessRequirement::Authenticated)
}

#[allow(clippy::result_large_err)]
fn require_chat_auth(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
) -> Result<String, axum::response::Response> {
    require_access(auth, headers, AccessRequirement::Chat)
}

#[allow(clippy::result_large_err)]
fn require_inbox_auth(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
) -> Result<String, axum::response::Response> {
    require_access(auth, headers, AccessRequirement::Inbox)
}

#[allow(clippy::result_large_err)]
fn require_trusted_auth(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
) -> Result<String, axum::response::Response> {
    require_access(auth, headers, AccessRequirement::Trusted)
}

#[allow(clippy::result_large_err)]
fn require_admin_auth(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
) -> Result<String, axum::response::Response> {
    require_access(auth, headers, AccessRequirement::Admin)
}

async fn api_me_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let access = state.auth.access_for_npub(&npub);
    Json(serde_json::json!({
        "npub": npub,
        "is_admin": access.is_admin,
        "can_chat": access.can_chat,
        "can_forge_write": access.can_forge_write,
    }))
    .into_response()
}
