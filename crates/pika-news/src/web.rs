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
use crate::branch_store::{
    BranchCiLaneRecord, BranchCiRunRecord, BranchDetailRecord, BranchFeedItem, NightlyFeedItem,
    NightlyLaneRecord, NightlyRunRecord,
};
use crate::ci;
use crate::config::Config;
use crate::forge;
use crate::model;
use crate::poller;
use crate::render::is_safe_http_url;
use crate::storage::{ChatAllowlistEntry, InboxReviewContext, Store};
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
    history_items: Vec<FeedItemView>,
    nightly_items: Vec<NightlyFeedItemView>,
}

#[derive(Template)]
#[template(path = "detail.html")]
struct DetailTemplate {
    page_title: String,
    repo: String,
    branch_id: i64,
    branch_name: String,
    target_branch: String,
    updated_at: String,
    branch_state: String,
    tutorial_status: String,
    ci_status: String,
    merge_commit_sha: Option<String>,
    executive_html: Option<String>,
    media_links: Vec<MediaLinkView>,
    error_message: Option<String>,
    steps: Vec<StepView>,
    diff_json: Option<String>,
    ci_runs: Vec<CiRunView>,
    review_mode: bool,
}

#[derive(Template)]
#[template(path = "nightly.html")]
struct NightlyTemplate {
    page_title: String,
    repo: String,
    nightly_run_id: i64,
    status: String,
    summary: Option<String>,
    source_ref: String,
    source_head_sha: String,
    scheduled_for: String,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    lanes: Vec<NightlyLaneView>,
}

#[derive(Template)]
#[template(path = "inbox.html")]
struct InboxTemplate {}

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminTemplate {}

#[derive(Clone)]
struct FeedItemView {
    branch_id: i64,
    repo: String,
    branch_name: String,
    title: String,
    state: String,
    updated_at: String,
    tutorial_status: String,
    ci_status: String,
}

#[derive(Clone)]
struct NightlyFeedItemView {
    nightly_run_id: i64,
    repo: String,
    source_head_sha: String,
    status: String,
    summary: Option<String>,
    scheduled_for: String,
    created_at: String,
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

#[derive(Clone)]
struct CiRunView {
    id: i64,
    source_head_sha: String,
    status: String,
    lane_count: usize,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    lanes: Vec<CiLaneView>,
}

#[derive(Clone)]
struct CiLaneView {
    id: i64,
    lane_id: String,
    title: String,
    entrypoint: String,
    status: String,
    log_text: Option<String>,
    retry_count: i64,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
}

#[derive(Clone)]
struct NightlyLaneView {
    lane_id: String,
    title: String,
    entrypoint: String,
    status: String,
    log_text: Option<String>,
    retry_count: i64,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
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
    if let Some(forge_repo) = config.effective_forge_repo() {
        let secret = webhook_secret
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("forge mode requires {}", config.webhook_secret_env))?;
        forge::ensure_canonical_repo(&forge_repo).with_context(|| {
            format!(
                "ensure canonical bare repo exists at {}",
                forge_repo.canonical_git_dir
            )
        })?;
        forge::install_hooks(&forge_repo, secret).with_context(|| {
            format!(
                "install canonical git hooks in {}",
                forge_repo.canonical_git_dir
            )
        })?;
        eprintln!(
            "forge: canonical_repo={} default_branch={} lane_manifest={}",
            forge_repo.canonical_git_dir,
            forge_repo.default_branch,
            ci::FORGE_LANE_MANIFEST_PATH
        );
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
                    ci::run_ci_pass(&state.store, &state.config),
                )
            })
            .await
            {
                Ok((poll_result, worker_result, ci_result)) => {
                    match poll_result {
                        Ok(pr)
                            if pr.branches_seen > 0
                                || pr.queued_regenerations > 0
                                || pr.stale_closed > 0 =>
                        {
                            eprintln!(
                                "poll: repos={} branches_seen={} queued_tutorials={} queued_ci={} head_changes={} stale_closed={}",
                                pr.repos_polled,
                                pr.branches_seen,
                                pr.queued_regenerations,
                                pr.queued_ci_runs,
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
                    match ci_result {
                        Ok(ci)
                            if ci.claimed > 0
                                || ci.nightlies_scheduled > 0
                                || ci.retries_recovered > 0 =>
                        {
                            eprintln!(
                                "ci: claimed={} succeeded={} failed={} nightlies_scheduled={} retries_recovered={}",
                                ci.claimed,
                                ci.succeeded,
                                ci.failed,
                                ci.nightlies_scheduled,
                                ci.retries_recovered
                            );
                        }
                        Ok(_) => {}
                        Err(err) => {
                            eprintln!("pika-news ci runner error: {}", err);
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
        .route("/news/branch/:pr_id", get(detail_handler))
        .route("/news/nightly/:nightly_run_id", get(nightly_handler))
        .route("/news/pr/:pr_id", get(detail_handler))
        .route("/news/branch/:pr_id/merge", post(merge_handler))
        .route("/news/branch/:pr_id/close", post(close_handler))
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
        .route("/news/llms.txt", get(llms_txt_handler))
        .route("/news/api/prs", get(api_prs_handler))
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
    let branch_store = state.store.clone();
    let nightly_store = state.store.clone();
    let items =
        match tokio::task::spawn_blocking(move || branch_store.list_branch_feed_items()).await {
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
    let nightly_items =
        match tokio::task::spawn_blocking(move || nightly_store.list_recent_nightly_runs(12)).await
        {
            Ok(Ok(items)) => items,
            Ok(Err(err)) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to query nightly runs: {}", err),
                )
                    .into_response();
            }
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("nightly worker task failed: {}", err),
                )
                    .into_response();
            }
        };

    let mut open_items = Vec::new();
    let mut history_items = Vec::new();

    for item in items {
        let view = map_feed_item(item);
        if view.state == "open" {
            open_items.push(view);
        } else {
            history_items.push(view);
        }
    }

    let template = FeedTemplate {
        open_items,
        history_items,
        nightly_items: nightly_items
            .into_iter()
            .map(map_nightly_feed_item)
            .collect(),
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

async fn nightly_handler(
    State(state): State<Arc<AppState>>,
    Path(nightly_run_id): Path<i64>,
) -> impl IntoResponse {
    let store = state.store.clone();
    let nightly =
        match tokio::task::spawn_blocking(move || store.get_nightly_run(nightly_run_id)).await {
            Ok(Ok(Some(run))) => run,
            Ok(Ok(None)) => {
                return (
                    StatusCode::NOT_FOUND,
                    format!("nightly run {} not found", nightly_run_id),
                )
                    .into_response();
            }
            Ok(Err(err)) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to query nightly run: {}", err),
                )
                    .into_response();
            }
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("nightly detail worker task failed: {}", err),
                )
                    .into_response();
            }
        };
    let template = render_nightly_template(nightly);
    match template.render() {
        Ok(rendered) => Html(rendered).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to render nightly template: {}", err),
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
    branch_id: i64,
    review_mode: bool,
) -> axum::response::Response {
    let detail_store = state.store.clone();
    let runs_store = state.store.clone();
    let detail = match tokio::task::spawn_blocking(move || {
        detail_store.get_branch_detail(branch_id)
    })
    .await
    {
        Ok(Ok(Some(record))) => record,
        Ok(Ok(None)) => {
            return (
                StatusCode::NOT_FOUND,
                format!("branch {} not found", branch_id),
            )
                .into_response();
        }
        Ok(Err(err)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to query branch detail: {}", err),
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
    let ci_runs =
        match tokio::task::spawn_blocking(move || runs_store.list_branch_ci_runs(branch_id, 8))
            .await
        {
            Ok(Ok(runs)) => runs,
            Ok(Err(err)) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to query branch ci runs: {}", err),
                )
                    .into_response();
            }
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("ci worker task failed: {}", err),
                )
                    .into_response();
            }
        };

    match render_detail_template(detail, ci_runs, review_mode) {
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

fn map_feed_item(item: BranchFeedItem) -> FeedItemView {
    FeedItemView {
        branch_id: item.branch_id,
        repo: item.repo,
        branch_name: item.branch_name,
        title: item.title,
        state: item.state,
        updated_at: item.updated_at,
        tutorial_status: item.tutorial_status,
        ci_status: item.ci_status,
    }
}

fn map_nightly_feed_item(item: NightlyFeedItem) -> NightlyFeedItemView {
    NightlyFeedItemView {
        nightly_run_id: item.nightly_run_id,
        repo: item.repo,
        source_head_sha: item.source_head_sha,
        status: item.status,
        summary: item.summary,
        scheduled_for: item.scheduled_for,
        created_at: item.created_at,
    }
}

fn render_detail_template(
    record: BranchDetailRecord,
    ci_runs: Vec<BranchCiRunRecord>,
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
        page_title: format!(
            "{} #{}: {}",
            record.repo, record.branch_id, record.branch_name
        ),
        repo: record.repo,
        branch_id: record.branch_id,
        branch_name: record.branch_name,
        target_branch: record.target_branch,
        updated_at: record.updated_at,
        branch_state: record.branch_state,
        tutorial_status: record.tutorial_status,
        ci_status: record.ci_status,
        merge_commit_sha: record.merge_commit_sha,
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
        ci_runs: ci_runs
            .into_iter()
            .map(|run| CiRunView {
                id: run.id,
                source_head_sha: run.source_head_sha,
                status: run.status,
                lane_count: run.lane_count,
                created_at: run.created_at,
                started_at: run.started_at,
                finished_at: run.finished_at,
                lanes: run.lanes.into_iter().map(map_ci_lane_view).collect(),
            })
            .collect(),
        review_mode,
    })
}

fn render_nightly_template(run: NightlyRunRecord) -> NightlyTemplate {
    NightlyTemplate {
        page_title: format!("{} nightly #{}", run.repo, run.nightly_run_id),
        repo: run.repo,
        nightly_run_id: run.nightly_run_id,
        status: run.status,
        summary: run.summary,
        source_ref: run.source_ref,
        source_head_sha: run.source_head_sha,
        scheduled_for: run.scheduled_for,
        created_at: run.created_at,
        started_at: run.started_at,
        finished_at: run.finished_at,
        lanes: run.lanes.into_iter().map(map_nightly_lane_view).collect(),
    }
}

fn map_ci_lane_view(lane: BranchCiLaneRecord) -> CiLaneView {
    CiLaneView {
        id: lane.id,
        lane_id: lane.lane_id,
        title: lane.title,
        entrypoint: lane.entrypoint,
        status: lane.status,
        log_text: lane.log_text,
        retry_count: lane.retry_count,
        created_at: lane.created_at,
        started_at: lane.started_at,
        finished_at: lane.finished_at,
    }
}

fn map_nightly_lane_view(lane: NightlyLaneRecord) -> NightlyLaneView {
    NightlyLaneView {
        lane_id: lane.lane_id,
        title: lane.title,
        entrypoint: lane.entrypoint,
        status: lane.status,
        log_text: lane.log_text,
        retry_count: lane.retry_count,
        created_at: lane.created_at,
        started_at: lane.started_at,
        finished_at: lane.finished_at,
    }
}

async fn merge_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_trusted_auth(&state.auth, &headers) {
        Ok(npub) => npub,
        Err(resp) => return resp,
    };
    let Some(forge_repo) = state.config.effective_forge_repo() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "forge repo is not configured"})),
        )
            .into_response();
    };

    let store = state.store.clone();
    let target = match tokio::task::spawn_blocking(move || {
        store.get_branch_action_target(branch_id)
    })
    .await
    {
        Ok(Ok(Some(target))) => target,
        Ok(Ok(None)) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "branch not found"})),
            )
                .into_response();
        }
        Ok(Err(err)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response();
        }
    };
    if target.branch_state != "open" {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "only open branches can be merged"})),
        )
            .into_response();
    }

    let current_head = match forge::current_branch_head(&forge_repo, &target.branch_name) {
        Ok(Some(head)) => head,
        Ok(None) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "branch ref no longer exists"})),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response();
        }
    };
    if current_head != target.head_sha {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "branch head changed; refresh before merging"})),
        )
            .into_response();
    }

    let merge_outcome = match tokio::task::spawn_blocking({
        let forge_repo = forge_repo.clone();
        let branch_name = target.branch_name.clone();
        let expected_head = target.head_sha.clone();
        move || forge::merge_branch(&forge_repo, &branch_name, &expected_head)
    })
    .await
    {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(err)) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response();
        }
    };

    let merge_commit_sha = merge_outcome.merge_commit_sha.clone();
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || {
        store.mark_branch_merged(branch_id, &npub, &merge_commit_sha)
    })
    .await
    {
        Ok(Ok(())) => {
            state.poll_notify.notify_one();
            Json(serde_json::json!({
                "status": "ok",
                "branch_id": branch_id,
                "merge_commit_sha": merge_outcome.merge_commit_sha
            }))
            .into_response()
        }
        Ok(Err(err)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
    }
}

async fn close_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_trusted_auth(&state.auth, &headers) {
        Ok(npub) => npub,
        Err(resp) => return resp,
    };
    let Some(forge_repo) = state.config.effective_forge_repo() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "forge repo is not configured"})),
        )
            .into_response();
    };

    let store = state.store.clone();
    let target = match tokio::task::spawn_blocking(move || {
        store.get_branch_action_target(branch_id)
    })
    .await
    {
        Ok(Ok(Some(target))) => target,
        Ok(Ok(None)) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "branch not found"})),
            )
                .into_response();
        }
        Ok(Err(err)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response();
        }
    };
    if target.branch_state != "open" {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "only open branches can be closed"})),
        )
            .into_response();
    }

    let current_head = match forge::current_branch_head(&forge_repo, &target.branch_name) {
        Ok(Some(head)) => head,
        Ok(None) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "branch ref no longer exists"})),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response();
        }
    };
    if current_head != target.head_sha {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "branch head changed; refresh before closing"})),
        )
            .into_response();
    }

    let close_outcome = match tokio::task::spawn_blocking({
        let forge_repo = forge_repo.clone();
        let branch_name = target.branch_name.clone();
        let expected_head = target.head_sha.clone();
        move || forge::close_branch(&forge_repo, &branch_name, &expected_head)
    })
    .await
    {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(err)) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response();
        }
    };

    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.mark_branch_closed(branch_id, &npub)).await {
        Ok(Ok(())) => {
            state.poll_notify.notify_one();
            Json(serde_json::json!({
                "status": "ok",
                "branch_id": branch_id,
                "deleted": close_outcome.deleted
            }))
            .into_response()
        }
        Ok(Err(err)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
    }
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
            let access = state.auth.access_for_npub(&npub);
            Json(serde_json::json!({
                "token": token,
                "npub": npub,
                "is_admin": is_admin,
                "can_forge_write": access.can_forge_write
            }))
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

// --- LLMs.txt and PR summary API ---

async fn llms_txt_handler() -> impl IntoResponse {
    let body = "\
# Pika News

> AI-generated PR summaries for the Pika project.

Pika News automatically generates structured tutorial-style summaries for every
pull request in the sledtools/pika repository. Summaries include an executive
overview, step-by-step walkthrough, affected files, and evidence snippets.

## API

### GET /news/api/prs

Returns JSON array of PR summaries. Supports filtering:

- `since_pr=N`   — only PRs with pr_number >= N
- `since=DATE`   — only PRs updated on or after DATE (ISO 8601, e.g. 2026-03-07)

Both parameters can be combined. Without filters, returns all tracked PRs.

Response shape:
```json
[
  {
    \"repo\": \"sledtools/pika\",
    \"pr_number\": 482,
    \"title\": \"Fix agent provisioning flow\",
    \"url\": \"https://github.com/sledtools/pika/pull/482\",
    \"state\": \"merged\",
    \"updated_at\": \"2026-03-04T...\",
    \"generation_status\": \"ready\",
    \"executive_summary\": \"...\",
    \"steps\": [
      {
        \"title\": \"...\",
        \"intent\": \"...\",
        \"affected_files\": [\"...\"],
        \"body_markdown\": \"...\"
      }
    ]
  }
]
```

PRs where generation is not yet `ready` will have `executive_summary` and
`steps` set to null.

### GET /news

Human-readable feed of open and recently merged PRs.

### GET /news/pr/:pr_id

Human-readable detail page for a specific PR (by internal ID, not PR number).
";
    ([("content-type", "text/plain; charset=utf-8")], body)
}

#[derive(serde::Deserialize)]
struct PrsQuery {
    since_pr: Option<i64>,
    since: Option<String>,
}

#[derive(serde::Serialize)]
struct PrSummaryResponse {
    repo: String,
    pr_number: i64,
    title: String,
    url: String,
    state: String,
    updated_at: String,
    generation_status: String,
    executive_summary: Option<String>,
    steps: Option<Vec<PrStepResponse>>,
}

#[derive(serde::Serialize)]
struct PrStepResponse {
    title: String,
    intent: String,
    affected_files: Vec<String>,
    body_markdown: String,
}

async fn api_prs_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PrsQuery>,
) -> impl IntoResponse {
    if let Some(ref since) = query.since {
        // Accept ISO 8601 date (YYYY-MM-DD) or datetime prefix; reject garbage.
        if chrono::NaiveDate::parse_from_str(since, "%Y-%m-%d").is_err()
            && chrono::DateTime::parse_from_rfc3339(since).is_err()
        {
            return (
                StatusCode::BAD_REQUEST,
                "invalid 'since' parameter: expected ISO 8601 date (YYYY-MM-DD) or datetime"
                    .to_string(),
            )
                .into_response();
        }
    }

    let store = state.store.clone();
    let since_date = query.since.clone();
    let since_pr = query.since_pr;

    let records = match tokio::task::spawn_blocking(move || {
        store.list_pr_summaries(since_pr, since_date.as_deref())
    })
    .await
    {
        Ok(Ok(records)) => records,
        Ok(Err(err)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to query pr summaries: {}", err),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("pr summaries task failed: {}", err),
            )
                .into_response();
        }
    };

    let items: Vec<PrSummaryResponse> = records
        .into_iter()
        .map(|r| {
            let (executive_summary, steps) = r
                .tutorial_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<TutorialDoc>(json).ok())
                .map(|doc| {
                    let steps = doc
                        .steps
                        .into_iter()
                        .map(|s| PrStepResponse {
                            title: s.title,
                            intent: s.intent,
                            affected_files: s.affected_files,
                            body_markdown: s.body_markdown,
                        })
                        .collect();
                    (Some(doc.executive_summary), Some(steps))
                })
                .unwrap_or((None, None));

            PrSummaryResponse {
                repo: r.repo,
                pr_number: r.pr_number,
                title: r.title,
                url: r.url,
                state: r.state,
                updated_at: r.updated_at,
                generation_status: r.generation_status,
                executive_summary,
                steps,
            }
        })
        .collect();

    Json(items).into_response()
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

    let signature = match headers
        .get("x-pika-signature-256")
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

    if !verify_signature(secret, &body, &signature) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid signature"})),
        )
            .into_response();
    }

    state.poll_notify.notify_one();
    let update_count = String::from_utf8_lossy(&body)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    eprintln!("webhook: received {} ref updates", update_count);

    Json(serde_json::json!({"status": "ok"})).into_response()
}

fn verify_signature(secret: &str, payload: &[u8], signature_header: &str) -> bool {
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
fn require_trusted_auth(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
) -> Result<String, axum::response::Response> {
    let npub = require_auth(auth, headers)?;
    if auth.access_for_npub(&npub).can_forge_write {
        Ok(npub)
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "trusted contributor access required"})),
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
        "can_forge_write": access.can_forge_write,
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
struct AdminAllowlistUpsertRequest {
    npub: String,
    note: Option<String>,
    active: bool,
    #[serde(default)]
    can_forge_write: bool,
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
    let can_forge_write = body.can_forge_write;
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || {
        let existing = store.get_chat_allowlist_entry(&npub)?;
        let entry = store.upsert_chat_allowlist_entry(
            &npub,
            active,
            can_forge_write,
            note.as_deref(),
            &admin_npub,
        )?;
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
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    use askama::Template;

    use super::{
        markdown_to_safe_html, render_detail_template, should_backfill_managed_allowlist_entry,
        verify_signature,
    };
    use crate::ci;
    use crate::config::{Config, ForgeRepoConfig};
    use crate::forge;
    use crate::poller;
    use crate::storage::ChatAllowlistEntry;
    use crate::storage::Store;

    fn git<P: AsRef<std::path::Path>>(cwd: P, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd.as_ref())
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn sanitizes_markdown_html_output() {
        let rendered = markdown_to_safe_html("ok<script>alert('xss')</script>");
        assert!(rendered.contains("ok"));
        assert!(!rendered.contains("<script>"));
    }

    #[test]
    fn valid_signature_accepted() {
        let secret = "test-secret";
        let payload = b"hello world";

        // Compute expected signature.
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(payload);
        let sig = hex::encode(mac.finalize().into_bytes());
        let header = format!("sha256={}", sig);

        assert!(verify_signature(secret, payload, &header));
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

        assert!(!verify_signature("wrong-secret", payload, &header));
    }

    #[test]
    fn missing_prefix_rejected() {
        assert!(!verify_signature("secret", b"body", "bad-header"));
    }

    #[test]
    fn invalid_hex_rejected() {
        assert!(!verify_signature("secret", b"body", "sha256=zzzz"));
    }

    #[test]
    fn managed_allowlist_backfills_only_for_new_active_entries() {
        assert!(should_backfill_managed_allowlist_entry(None, true));
        assert!(!should_backfill_managed_allowlist_entry(None, false));

        let existing_active = ChatAllowlistEntry {
            npub: "npub1existing".to_string(),
            active: true,
            can_forge_write: false,
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

    #[test]
    fn merged_branch_page_renders_after_source_branch_deletion() {
        let root = tempfile::tempdir().expect("create temp root");
        let bare = root.path().join("pika.git");
        let seed = root.path().join("seed");
        let db_path = root.path().join("pika-news.db");

        git(
            root.path(),
            &["init", "--bare", bare.to_str().expect("bare path")],
        );
        git(root.path(), &["init", seed.to_str().expect("seed path")]);
        git(&seed, &["config", "user.name", "Test User"]);
        git(&seed, &["config", "user.email", "test@example.com"]);
        fs::write(seed.join("README.md"), "hello\n").expect("write readme");
        fs::write(
            seed.join("ci.sh"),
            "#!/usr/bin/env bash\nset -euo pipefail\necho branch-ci-ok\n",
        )
        .expect("write ci script");
        fs::create_dir_all(seed.join("ci")).expect("create ci dir");
        fs::write(
            seed.join("ci/forge-lanes.toml"),
            r#"
version = 1
nightly_schedule_utc = "23:59"

[[branch.lanes]]
id = "render_history"
title = "render history"
entrypoint = "./ci.sh"
command = ["./ci.sh"]
paths = ["README.md", "feature.txt", "ci/forge-lanes.toml"]
"#,
        )
        .expect("write forge lane manifest");
        let mut perms = fs::metadata(seed.join("ci.sh"))
            .expect("ci metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(seed.join("ci.sh"), perms).expect("chmod ci script");
        git(&seed, &["add", "README.md", "ci.sh", "ci/forge-lanes.toml"]);
        git(&seed, &["commit", "-m", "initial"]);
        git(&seed, &["branch", "-M", "master"]);
        git(
            &seed,
            &["remote", "add", "origin", bare.to_str().expect("bare path")],
        );
        git(&seed, &["push", "origin", "master"]);
        git(&seed, &["checkout", "-b", "feature/render-history"]);
        fs::write(seed.join("feature.txt"), "branch work\n").expect("write feature file");
        git(&seed, &["add", "feature.txt"]);
        git(&seed, &["commit", "-m", "branch render history"]);
        git(&seed, &["push", "origin", "feature/render-history"]);

        let store = Store::open(&db_path).expect("open store");
        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: bare.to_str().expect("bare path").to_string(),
                default_branch: "master".to_string(),
                ci_command: vec!["./ci.sh".to_string()],
                hook_url: Some("http://127.0.0.1:9999/news/webhook".to_string()),
            }),
            poll_interval_secs: 60,
            model: "test-model".to_string(),
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            github_token_env: "GITHUB_TOKEN".to_string(),
            merged_lookback_hours: 72,
            worker_concurrency: 1,
            retry_backoff_secs: 120,
            webhook_secret_env: "PIKA_NEWS_WEBHOOK_SECRET".to_string(),
            bind_address: "127.0.0.1".to_string(),
            bind_port: 8787,
            allowed_npubs: vec![],
            bootstrap_admin_npubs: vec![],
        };

        poller::poll_once_limited(&store, &config, 0).expect("sync branch from bare repo");
        let branch = store
            .list_branch_feed_items()
            .expect("feed items")
            .into_iter()
            .find(|item| item.branch_name == "feature/render-history")
            .expect("branch item");
        let ci_pass = ci::run_ci_pass(&store, &config).expect("run ci pass");
        assert_eq!(ci_pass.succeeded, 1);

        let artifact_id: i64 = store
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT id
                     FROM branch_artifact_versions
                     WHERE branch_id = ?1
                     ORDER BY version DESC
                     LIMIT 1",
                    rusqlite::params![branch.branch_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("branch artifact id");
        store
            .mark_branch_generation_ready(
                artifact_id,
                r#"{"executive_summary":"ok","media_links":[],"steps":[{"title":"Step","intent":"Intent","affected_files":["feature.txt"],"evidence_snippets":["@@ -0,0 +1 @@"],"body_markdown":"body"}]}"#,
                "<p>ok</p>",
                &branch.head_sha,
                "@@ -0,0 +1 @@",
            )
            .expect("mark artifact ready");

        let forge_repo = config.effective_forge_repo().expect("forge repo");
        let branch_target = store
            .get_branch_action_target(branch.branch_id)
            .expect("branch target")
            .expect("existing branch target");
        let merge = forge::merge_branch(
            &forge_repo,
            &branch_target.branch_name,
            &branch_target.head_sha,
        )
        .expect("merge branch");
        store
            .mark_branch_merged(branch.branch_id, "npub1trusted", &merge.merge_commit_sha)
            .expect("mark merged");
        assert!(
            forge::current_branch_head(&forge_repo, &branch_target.branch_name)
                .expect("resolve branch")
                .is_none()
        );

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail exists");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("ci runs");
        let template =
            render_detail_template(detail, ci_runs, false).expect("render detail template");
        let rendered = template.render().expect("render html");
        assert!(rendered.contains("feature/render-history"));
        assert!(rendered.contains("branch-ci-ok"));
        assert!(rendered.contains("merge commit"));
    }
}
