use std::env;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use askama::Template;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use hmac::{Hmac, Mac};
use pulldown_cmark::{html, Options, Parser};
use sha2::Sha256;
use tokio::sync::Notify;

use crate::auth::{normalize_npub, AuthState};
use crate::config::Config;
use crate::model;
use crate::poller;
use crate::render::is_safe_http_url;
use crate::storage::{ChatAllowlistEntry, FeedItem, InboxReviewContext, PrDetailRecord, Store};
use crate::tutorial::TutorialDoc;
use crate::worker;

#[derive(Clone)]
struct AppState {
    store: Store,
    config: Config,
    max_prs: usize,
    auth: Arc<AuthState>,
    poll_notify: Arc<Notify>,
    webhook_secret: Option<String>,
}

#[derive(Template)]
#[template(path = "feed.html")]
struct FeedTemplate {
    open_items: Vec<FeedItemView>,
    merged_items: Vec<FeedItemView>,
}

#[derive(Template)]
#[template(path = "detail.html")]
struct DetailTemplate {
    page_title: String,
    repo: String,
    pr_number: i64,
    github_url: String,
    updated_at: String,
    pr_state: String,
    generation_status: String,
    executive_html: Option<String>,
    media_links: Vec<MediaLinkView>,
    error_message: Option<String>,
    steps: Vec<StepView>,
    diff_json: Option<String>,
    review_mode: bool,
}

#[derive(Template)]
#[template(path = "inbox.html")]
struct InboxTemplate {}

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminTemplate {}

#[derive(Clone)]
struct FeedItemView {
    pr_id: i64,
    repo: String,
    pr_number: i64,
    title: String,
    url: String,
    state: String,
    updated_at: String,
    generation_status: String,
}

#[derive(Clone)]
struct StepView {
    title: String,
    intent: String,
    affected_files: String,
    evidence_snippets: Vec<String>,
    body_html: String,
}

#[derive(Clone)]
struct MediaLinkView {
    href: String,
    label: String,
}

pub async fn serve(
    store: Store,
    config: Config,
    bind_addr: String,
    max_prs: usize,
) -> anyhow::Result<()> {
    let bootstrap_admin_npubs = config.effective_bootstrap_admin_npubs();
    let legacy_allowed_npubs = config.allowed_npubs.clone();
    let auth = Arc::new(AuthState::new(
        &bootstrap_admin_npubs,
        &legacy_allowed_npubs,
        store.clone(),
    ));

    if bootstrap_admin_npubs.is_empty() && !legacy_allowed_npubs.is_empty() {
        eprintln!(
            "warning: allowed_npubs grants chat access only; set bootstrap_admin_npubs to enable /news/admin"
        );
    }

    if let Err(err) = store.canonicalize_inbox_npubs() {
        eprintln!("warning: failed to canonicalize inbox owners: {}", err);
    }

    let poll_notify = Arc::new(Notify::new());
    let webhook_secret = env::var(&config.webhook_secret_env).ok();
    if webhook_secret.is_some() {
        eprintln!("webhook: signature verification enabled");
    } else {
        eprintln!("webhook: no secret configured, endpoint disabled");
    }
    let state = Arc::new(AppState {
        store,
        config: config.clone(),
        max_prs,
        auth,
        poll_notify: Arc::clone(&poll_notify),
        webhook_secret,
    });

    let background_state = Arc::clone(&state);
    let background_notify = Arc::clone(&poll_notify);
    tokio::spawn(async move {
        loop {
            let state = Arc::clone(&background_state);
            match tokio::task::spawn_blocking(move || {
                (
                    poller::poll_once_limited(&state.store, &state.config, state.max_prs),
                    worker::run_generation_pass(&state.store, &state.config),
                )
            })
            .await
            {
                Ok((poll_result, worker_result)) => {
                    match poll_result {
                        Ok(pr)
                            if pr.prs_seen > 0
                                || pr.queued_regenerations > 0
                                || pr.stale_closed > 0 =>
                        {
                            eprintln!(
                                "poll: repos={} prs_seen={} queued={} head_changes={} stale_closed={}",
                                pr.repos_polled,
                                pr.prs_seen,
                                pr.queued_regenerations,
                                pr.head_sha_changes,
                                pr.stale_closed
                            );
                        }
                        Ok(_) => {}
                        Err(err) => {
                            eprintln!("pika-news background poller error: {}", err);
                        }
                    }
                    match worker_result {
                        Ok(wr) if wr.claimed > 0 => {
                            eprintln!(
                                "worker: claimed={} ready={} failed={} retry={}",
                                wr.claimed, wr.ready, wr.failed, wr.retry_scheduled
                            );
                        }
                        Ok(_) => {}
                        Err(err) => {
                            eprintln!("pika-news background worker error: {}", err);
                        }
                    }
                }
                Err(err) => {
                    eprintln!("pika-news background task join error: {}", err);
                }
            }
            // Wait for the poll interval OR an early wake-up from a webhook.
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(config.poll_interval_secs)) => {}
                _ = background_notify.notified() => {
                    eprintln!("poll: woken by webhook");
                }
            }
        }
    });

    let app = Router::new()
        .route("/", get(feed_handler))
        .route("/news", get(feed_handler))
        .route("/news/pr/:pr_id", get(detail_handler))
        .route("/news/inbox", get(inbox_handler))
        .route("/news/admin", get(admin_handler))
        .route("/news/inbox/review/:pr_id", get(inbox_review_handler))
        .route("/news/api/inbox", get(api_inbox_list_handler))
        .route("/news/api/inbox/count", get(api_inbox_count_handler))
        .route("/news/api/inbox/dismiss", post(api_inbox_dismiss_handler))
        .route("/news/api/me", get(api_me_handler))
        .route(
            "/news/api/admin/allowlist",
            get(api_admin_allowlist_handler).post(api_admin_allowlist_upsert_handler),
        )
        .route(
            "/news/api/inbox/neighbors/:pr_id",
            get(api_inbox_neighbors_handler),
        )
        .route("/news/auth/challenge", post(auth_challenge_handler))
        .route("/news/auth/verify", post(auth_verify_handler))
        .route(
            "/news/pr/:pr_id/chat",
            get(chat_history_handler).post(chat_send_handler),
        )
        .route("/news/pr/:pr_id/regenerate", post(regenerate_handler))
        .route("/news/webhook", post(webhook_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("bind hosted server on {}", bind_addr))?;

    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serve hosted UI")?;

    Ok(())
}

async fn feed_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let store = state.store.clone();
    let items = match tokio::task::spawn_blocking(move || store.list_feed_items()).await {
        Ok(Ok(items)) => items,
        Ok(Err(err)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to query feed items: {}", err),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("feed worker task failed: {}", err),
            )
                .into_response();
        }
    };

    let mut open_items = Vec::new();
    let mut merged_items = Vec::new();

    for item in items {
        let view = map_feed_item(item);
        if view.state == "open" {
            open_items.push(view);
        } else if view.state == "merged" {
            merged_items.push(view);
        }
    }

    let template = FeedTemplate {
        open_items,
        merged_items,
    };

    match template.render() {
        Ok(rendered) => Html(rendered).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to render feed template: {}", err),
        )
            .into_response(),
    }
}

async fn detail_handler(
    State(state): State<Arc<AppState>>,
    Path(pr_id): Path<i64>,
) -> impl IntoResponse {
    detail_page(state, pr_id, false).await
}

async fn inbox_review_handler(
    State(state): State<Arc<AppState>>,
    Path(pr_id): Path<i64>,
) -> impl IntoResponse {
    detail_page(state, pr_id, true).await
}

async fn detail_page(
    state: Arc<AppState>,
    pr_id: i64,
    review_mode: bool,
) -> axum::response::Response {
    let store = state.store.clone();
    let detail = match tokio::task::spawn_blocking(move || store.get_pr_detail(pr_id)).await {
        Ok(Ok(Some(record))) => record,
        Ok(Ok(None)) => {
            return (StatusCode::NOT_FOUND, format!("PR {} not found", pr_id)).into_response();
        }
        Ok(Err(err)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to query PR detail: {}", err),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("detail worker task failed: {}", err),
            )
                .into_response();
        }
    };

    match render_detail_template(detail, review_mode) {
        Ok(template) => match template.render() {
            Ok(rendered) => Html(rendered).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to render detail template: {}", err),
            )
                .into_response(),
        },
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to build detail view: {}", err),
        )
            .into_response(),
    }
}

fn map_feed_item(item: FeedItem) -> FeedItemView {
    FeedItemView {
        pr_id: item.pr_id,
        repo: item.repo,
        pr_number: item.pr_number,
        title: item.title,
        url: item.url,
        state: item.state,
        updated_at: item.updated_at,
        generation_status: item.generation_status,
    }
}

fn render_detail_template(
    record: PrDetailRecord,
    review_mode: bool,
) -> anyhow::Result<DetailTemplate> {
    let mut steps = Vec::new();
    let mut executive_html = None;
    let mut media_links = Vec::new();

    if let Some(tutorial_json) = &record.tutorial_json {
        let tutorial: TutorialDoc = serde_json::from_str(tutorial_json)
            .context("parse stored tutorial JSON for detail page")?;

        executive_html = Some(markdown_to_safe_html(&tutorial.executive_summary));
        media_links = tutorial
            .media_links
            .into_iter()
            .map(|link| MediaLinkView {
                href: if is_safe_http_url(&link) {
                    link.clone()
                } else {
                    "#".to_string()
                },
                label: link,
            })
            .collect();
        for step in tutorial.steps {
            steps.push(StepView {
                title: step.title,
                intent: step.intent,
                affected_files: step.affected_files.join(", "),
                evidence_snippets: step.evidence_snippets,
                body_html: markdown_to_safe_html(&step.body_markdown),
            });
        }
    }

    Ok(DetailTemplate {
        page_title: format!("{} #{}: {}", record.repo, record.pr_number, record.title),
        repo: record.repo,
        pr_number: record.pr_number,
        github_url: if is_safe_http_url(&record.url) {
            record.url
        } else {
            "#".to_string()
        },
        updated_at: record.updated_at,
        pr_state: record.pr_state,
        generation_status: record.generation_status,
        executive_html,
        media_links,
        error_message: record.error_message,
        steps,
        diff_json: record.unified_diff.map(|d| {
            // Escape `</` as `<\/` to prevent the browser HTML parser from
            // prematurely closing the <script> tag when the diff contains
            // literal `</script>` sequences (e.g. from HTML source diffs).
            // `<\/` is valid JSON so JSON.parse still recovers the original.
            serde_json::to_string(&d)
                .unwrap_or_default()
                .replace("</", r"<\/")
        }),
        review_mode,
    })
}

// --- Auth handlers ---

async fn auth_challenge_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if !state.auth.chat_enabled() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "chat not enabled"})),
        )
            .into_response();
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
    if !state.auth.chat_enabled() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "chat not enabled"})),
        )
            .into_response();
    }
    match state.auth.verify_event(&body.event) {
        Ok((token, npub, is_admin)) => {
            Json(serde_json::json!({"token": token, "npub": npub, "is_admin": is_admin}))
                .into_response()
        }
        Err(msg) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": msg})),
        )
            .into_response(),
    }
}

// --- Chat handlers ---

fn extract_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

async fn chat_history_handler(
    State(state): State<Arc<AppState>>,
    Path(pr_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_chat_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };

    let store = state.store.clone();
    let base_session_id = match tokio::task::spawn_blocking({
        let store = store.clone();
        move || store.get_artifact_session_id(pr_id)
    })
    .await
    {
        Ok(Ok(Some(sid))) => sid,
        Ok(Ok(None)) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "no session for this tutorial"})),
            )
                .into_response();
        }
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    let result = tokio::task::spawn_blocking({
        let store = store.clone();
        let npub = npub.clone();
        move || store.get_or_create_chat_session(pr_id, &npub, &base_session_id)
    })
    .await;

    match result {
        Ok(Ok((_session_id, messages))) => {
            Json(serde_json::json!({"messages": messages})).into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
struct ChatSendRequest {
    message: String,
}

async fn chat_send_handler(
    State(state): State<Arc<AppState>>,
    Path(pr_id): Path<i64>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ChatSendRequest>,
) -> impl IntoResponse {
    let npub = match require_chat_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };

    let store = state.store.clone();

    // Get the artifact's base session id
    let base_session_id = match tokio::task::spawn_blocking({
        let store = store.clone();
        move || store.get_artifact_session_id(pr_id)
    })
    .await
    {
        Ok(Ok(Some(sid))) => sid,
        Ok(Ok(None)) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "no session for this tutorial"})),
            )
                .into_response();
        }
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Get or create chat session
    let (session_id, _messages) = match tokio::task::spawn_blocking({
        let store = store.clone();
        let npub = npub.clone();
        let base_session_id = base_session_id.clone();
        move || store.get_or_create_chat_session(pr_id, &npub, &base_session_id)
    })
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Get the current claude session id for this user's chat
    let claude_session_id = match tokio::task::spawn_blocking({
        let store = store.clone();
        move || store.get_chat_claude_session_id(session_id)
    })
    .await
    {
        Ok(Ok(sid)) => sid,
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Save user message
    if let Err(e) = tokio::task::spawn_blocking({
        let store = store.clone();
        let msg = body.message.clone();
        move || store.append_chat_message(session_id, "user", &msg)
    })
    .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response();
    }

    // Call claude -r with the session
    let message = body.message.clone();
    let chat_result =
        tokio::task::spawn_blocking(move || model::chat_with_session(&claude_session_id, &message))
            .await;

    match chat_result {
        Ok(Ok(response)) => {
            // Update the claude session id for next turn
            let new_session_id = response.session_id.clone();
            let response_text = response.text.clone();
            let _ = tokio::task::spawn_blocking({
                let store = store.clone();
                move || {
                    let _ = store.update_chat_claude_session_id(session_id, &new_session_id);
                    let _ = store.append_chat_message(session_id, "assistant", &response_text);
                }
            })
            .await;

            Json(serde_json::json!({
                "role": "assistant",
                "content": response.text
            }))
            .into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("claude error: {}", e)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// --- Regenerate handler ---

async fn regenerate_handler(
    State(state): State<Arc<AppState>>,
    Path(pr_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_admin_auth(&state.auth, &headers) {
        return resp;
    }

    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.queue_regeneration(pr_id)).await {
        Ok(Ok(true)) => {
            Json(serde_json::json!({"status": "queued", "message": "Tutorial regeneration queued. Refresh in a minute."}))
                .into_response()
        }
        Ok(Ok(false)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "no artifact found for this PR"})),
        )
            .into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// --- Webhook handler ---

async fn webhook_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let secret = match &state.webhook_secret {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "webhook not configured"})),
            )
                .into_response();
        }
    };

    // Validate X-Hub-Signature-256 header.
    let signature = match headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
    {
        Some(s) => s.to_string(),
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "missing signature"})),
            )
                .into_response();
        }
    };

    if !verify_github_signature(secret, &body, &signature) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid signature"})),
        )
            .into_response();
    }

    // Signature valid — wake the poller.
    state.poll_notify.notify_one();
    let event_type = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    eprintln!("webhook: received {} event", event_type);

    Json(serde_json::json!({"status": "ok"})).into_response()
}

fn verify_github_signature(secret: &str, payload: &[u8], signature_header: &str) -> bool {
    let hex_sig = match signature_header.strip_prefix("sha256=") {
        Some(h) => h,
        None => return false,
    };
    let sig_bytes = match hex::decode(hex_sig) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
    mac.update(payload);
    mac.verify_slice(&sig_bytes).is_ok()
}

// --- Inbox handlers ---

async fn inbox_handler(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let template = InboxTemplate {};
    match template.render() {
        Ok(rendered) => Html(rendered).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to render inbox template: {}", err),
        )
            .into_response(),
    }
}

async fn admin_handler(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let template = AdminTemplate {};
    match template.render() {
        Ok(rendered) => Html(rendered).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to render admin template: {}", err),
        )
            .into_response(),
    }
}

#[allow(clippy::result_large_err)]
fn require_auth(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
) -> Result<String, axum::response::Response> {
    let token = extract_token(headers).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "missing auth token"})),
        )
            .into_response()
    })?;
    auth.validate_token(&token).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid or expired token"})),
        )
            .into_response()
    })
}

#[allow(clippy::result_large_err)]
fn require_chat_auth(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
) -> Result<String, axum::response::Response> {
    let npub = require_auth(auth, headers)?;
    if auth.access_for_npub(&npub).can_chat {
        Ok(npub)
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "chat access revoked"})),
        )
            .into_response())
    }
}

#[allow(clippy::result_large_err)]
fn require_admin_auth(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
) -> Result<String, axum::response::Response> {
    let npub = require_auth(auth, headers)?;
    if auth.access_for_npub(&npub).is_admin {
        Ok(npub)
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "admin access required"})),
        )
            .into_response())
    }
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
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
struct AdminAllowlistUpsertRequest {
    npub: String,
    note: Option<String>,
    active: bool,
}

async fn api_admin_allowlist_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let _admin_npub = match require_admin_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };

    let store = state.store.clone();
    let bootstrap_admin_npubs = state.auth.bootstrap_admin_npubs();
    let legacy_allowed_npubs = state.auth.legacy_allowed_npubs();
    match tokio::task::spawn_blocking(move || {
        let entries = store.list_chat_allowlist_entries()?;
        Ok::<_, anyhow::Error>((entries, bootstrap_admin_npubs, legacy_allowed_npubs))
    })
    .await
    {
        Ok(Ok((entries, bootstrap_admin_npubs, legacy_allowed_npubs))) => Json(serde_json::json!({
            "bootstrap_admin_npubs": bootstrap_admin_npubs,
            "legacy_allowed_npubs": legacy_allowed_npubs,
            "entries": entries,
        }))
        .into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn api_admin_allowlist_upsert_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<AdminAllowlistUpsertRequest>,
) -> impl IntoResponse {
    let admin_npub = match require_admin_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };

    let npub = match normalize_npub(&body.npub) {
        Ok(value) => value,
        Err(msg) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": msg})),
            )
                .into_response();
        }
    };

    if state.auth.is_config_managed_chat_principal(&npub) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "This pubkey is managed by config and cannot be changed from the admin page"
            })),
        )
            .into_response();
    }

    let note = body
        .note
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let active = body.active;
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || {
        let existing = store.get_chat_allowlist_entry(&npub)?;
        let entry =
            store.upsert_chat_allowlist_entry(&npub, active, note.as_deref(), &admin_npub)?;
        let backfilled = if should_backfill_managed_allowlist_entry(existing.as_ref(), active) {
            store.backfill_inbox_for_npub(&npub)?
        } else {
            0
        };
        Ok::<_, anyhow::Error>((entry, backfilled))
    })
    .await
    {
        Ok(Ok((entry, backfilled))) => Json(serde_json::json!({
            "entry": entry,
            "backfilled": backfilled,
        }))
        .into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

fn should_backfill_managed_allowlist_entry(
    existing: Option<&ChatAllowlistEntry>,
    active: bool,
) -> bool {
    active && existing.map(|entry| !entry.active).unwrap_or(true)
}

#[derive(serde::Deserialize)]
struct InboxListParams {
    page: Option<i64>,
}

async fn api_inbox_list_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(params): Query<InboxListParams>,
) -> impl IntoResponse {
    let npub = match require_chat_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let page = params.page.unwrap_or(1).max(1);
    let offset = (page - 1) * 50;
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || {
        let items = store.list_inbox(&npub, 50, offset)?;
        let count = store.inbox_count(&npub)?;
        Ok::<_, anyhow::Error>((items, count))
    })
    .await
    {
        Ok(Ok((items, total))) => {
            Json(serde_json::json!({"items": items, "total": total, "page": page})).into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn api_inbox_count_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_chat_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.inbox_count(&npub)).await {
        Ok(Ok(count)) => Json(serde_json::json!({"count": count})).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
struct InboxDismissRequest {
    pr_ids: Option<Vec<i64>>,
    all: Option<bool>,
}

async fn api_inbox_dismiss_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<InboxDismissRequest>,
) -> impl IntoResponse {
    let npub = match require_chat_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let store = state.store.clone();
    let dismissed = if body.all.unwrap_or(false) {
        tokio::task::spawn_blocking(move || store.dismiss_all_inbox(&npub)).await
    } else {
        let pr_ids = body.pr_ids.unwrap_or_default();
        tokio::task::spawn_blocking(move || store.dismiss_inbox_items(&npub, &pr_ids)).await
    };
    match dismissed {
        Ok(Ok(count)) => Json(serde_json::json!({"dismissed": count})).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn api_inbox_neighbors_handler(
    State(state): State<Arc<AppState>>,
    Path(pr_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_chat_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.inbox_review_context(&npub, pr_id)).await {
        Ok(Ok(Some(InboxReviewContext {
            prev,
            next,
            position,
            total,
        }))) => Json(
            serde_json::json!({"prev": prev, "next": next, "position": position, "total": total}),
        )
        .into_response(),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "inbox item not found"})),
        )
            .into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

fn markdown_to_safe_html(markdown: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(markdown, opts);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    let mut builder = ammonia::Builder::default();
    builder.add_tags(&["table", "thead", "tbody", "tr", "th", "td"]);
    builder.clean(&html_output).to_string()
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::{
        markdown_to_safe_html, should_backfill_managed_allowlist_entry, verify_github_signature,
    };
    use crate::storage::ChatAllowlistEntry;

    #[test]
    fn sanitizes_markdown_html_output() {
        let rendered = markdown_to_safe_html("ok<script>alert('xss')</script>");
        assert!(rendered.contains("ok"));
        assert!(!rendered.contains("<script>"));
    }

    #[test]
    fn valid_github_signature_accepted() {
        let secret = "test-secret";
        let payload = b"hello world";

        // Compute expected signature.
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(payload);
        let sig = hex::encode(mac.finalize().into_bytes());
        let header = format!("sha256={}", sig);

        assert!(verify_github_signature(secret, payload, &header));
    }

    #[test]
    fn wrong_secret_rejected() {
        let payload = b"hello world";

        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"right-secret").unwrap();
        mac.update(payload);
        let sig = hex::encode(mac.finalize().into_bytes());
        let header = format!("sha256={}", sig);

        assert!(!verify_github_signature("wrong-secret", payload, &header));
    }

    #[test]
    fn missing_prefix_rejected() {
        assert!(!verify_github_signature("secret", b"body", "bad-header"));
    }

    #[test]
    fn invalid_hex_rejected() {
        assert!(!verify_github_signature("secret", b"body", "sha256=zzzz"));
    }

    #[test]
    fn managed_allowlist_backfills_only_for_new_active_entries() {
        assert!(should_backfill_managed_allowlist_entry(None, true));
        assert!(!should_backfill_managed_allowlist_entry(None, false));

        let existing_active = ChatAllowlistEntry {
            npub: "npub1existing".to_string(),
            active: true,
            note: Some("note".to_string()),
            updated_by: "npub1admin".to_string(),
            updated_at: "2026-03-08 00:00:00".to_string(),
        };
        assert!(!should_backfill_managed_allowlist_entry(
            Some(&existing_active),
            true
        ));
        assert!(!should_backfill_managed_allowlist_entry(
            Some(&existing_active),
            false
        ));

        let existing_inactive = ChatAllowlistEntry {
            active: false,
            ..existing_active
        };
        assert!(should_backfill_managed_allowlist_entry(
            Some(&existing_inactive),
            true
        ));
    }
}
