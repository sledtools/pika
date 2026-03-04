use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use pulldown_cmark::{html, Options, Parser};

use crate::auth::AuthState;
use crate::config::Config;
use crate::model;
use crate::poller;
use crate::render::is_safe_http_url;
use crate::storage::{FeedItem, PrDetailRecord, Store};
use crate::tutorial::TutorialDoc;
use crate::worker;

#[derive(Clone)]
struct AppState {
    store: Store,
    config: Config,
    max_prs: usize,
    auth: Arc<AuthState>,
}

#[derive(Template)]
#[template(path = "feed.html")]
struct FeedTemplate {
    open_items: Vec<FeedItemView>,
    merged_items: Vec<FeedItemView>,
    chat_enabled: bool,
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
    chat_enabled: bool,
}

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
    let auth = Arc::new(AuthState::new(&config.allowed_npubs));
    let state = Arc::new(AppState {
        store,
        config: config.clone(),
        max_prs,
        auth,
    });

    let background_state = Arc::clone(&state);
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
                        Ok(pr) if pr.prs_seen > 0 || pr.queued_regenerations > 0 => {
                            eprintln!(
                                "poll: repos={} prs_seen={} queued={} head_changes={}",
                                pr.repos_polled,
                                pr.prs_seen,
                                pr.queued_regenerations,
                                pr.head_sha_changes
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
            tokio::time::sleep(Duration::from_secs(config.poll_interval_secs)).await;
        }
    });

    let app = Router::new()
        .route("/", get(feed_handler))
        .route("/news", get(feed_handler))
        .route("/news/pr/:pr_id", get(detail_handler))
        .route(
            "/news/pr/:pr_id/auth/challenge",
            post(auth_challenge_handler),
        )
        .route("/news/pr/:pr_id/auth/verify", post(auth_verify_handler))
        .route(
            "/news/pr/:pr_id/chat",
            get(chat_history_handler).post(chat_send_handler),
        )
        .route("/news/pr/:pr_id/regenerate", post(regenerate_handler))
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
        chat_enabled: state.auth.chat_enabled(),
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

    let chat_enabled = state.auth.chat_enabled();
    match render_detail_template(detail, chat_enabled) {
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
    chat_enabled: bool,
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
        diff_json: record
            .unified_diff
            .map(|d| serde_json::to_string(&d).unwrap_or_default()),
        chat_enabled,
    })
}

// --- Auth handlers ---

async fn auth_challenge_handler(
    State(state): State<Arc<AppState>>,
    Path(_pr_id): Path<i64>,
) -> impl IntoResponse {
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
    Path(_pr_id): Path<i64>,
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
        Ok((token, npub)) => {
            Json(serde_json::json!({"token": token, "npub": npub})).into_response()
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
    let token = match extract_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "missing auth token"})),
            )
                .into_response()
        }
    };
    let npub = match state.auth.validate_token(&token) {
        Some(n) => n,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "invalid or expired token"})),
            )
                .into_response()
        }
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
                .into_response()
        }
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
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
    let token = match extract_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "missing auth token"})),
            )
                .into_response()
        }
    };
    let npub = match state.auth.validate_token(&token) {
        Some(n) => n,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "invalid or expired token"})),
            )
                .into_response()
        }
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
                .into_response()
        }
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
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
                .into_response()
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
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
                .into_response()
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
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
    let token = match extract_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "missing auth token"})),
            )
                .into_response()
        }
    };
    if state.auth.validate_token(&token).is_none() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid or expired token"})),
        )
            .into_response();
    }

    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.reset_artifact_to_pending(pr_id)).await {
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
    use super::markdown_to_safe_html;

    #[test]
    fn sanitizes_markdown_html_output() {
        let rendered = markdown_to_safe_html("ok<script>alert('xss')</script>");
        assert!(rendered.contains("ok"));
        assert!(!rendered.contains("<script>"));
    }
}
