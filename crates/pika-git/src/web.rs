use std::collections::BTreeSet;
use std::convert::Infallible;
use std::env;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use askama::Template;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Json, Redirect};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};
use futures::stream;
use hmac::{Hmac, Mac};
use pika_forge_model::{
    BranchActionResponse, BranchDetailResponse as ForgeBranchDetailResponse,
    BranchLogsResponse as SharedForgeBranchLogsResponse,
    BranchResolveResponse as ForgeBranchResolveResponse,
    BranchSummary as ForgeBranchSummaryResponse, CiLane,
    CiLaneExecutionReason as ForgeCiLaneExecutionReason,
    CiLaneFailureKind as ForgeCiLaneFailureKind, CiLaneStatus as ForgeCiLaneStatus, CiRun,
    CiTargetHealthState as ForgeApiTargetHealthState, ForgeCiStatus, LaneMutationResponse,
    NightlyDetailResponse as ForgeNightlyDetailResponse, RecoverRunResponse, WakeCiResponse,
};
use pikaci::{LogKind, PreparedOutputsRecord, RunLogsMetadata, RunRecord};
use pulldown_cmark::{html, Options, Parser};
use sha2::Sha256;
use tokio::sync::broadcast::error::RecvError;

use crate::auth::{normalize_npub, AuthState};
use crate::branch_store::{BranchDetailRecord, BranchFeedItem};
use crate::ci;
use crate::ci_state::{
    CiLaneExecutionReason, CiLaneFailureKind, CiLaneStatus, CiTargetHealthSnapshot,
    CiTargetHealthState,
};
use crate::ci_store::{
    BranchCiLaneRecord, BranchCiRunRecord, NightlyFeedItem, NightlyLaneRecord, NightlyRunRecord,
};
use crate::config::Config;
use crate::forge_runtime::{ForgeRuntime, ForgeRuntimeContext, ManualMirrorPassStatus};
use crate::forge_service::{
    BranchDetailAndRuns, BranchLaneMutationResult, BranchLaneRerunResult, BranchRunRecoveryResult,
    CloseBranchResult, ForgeService, ForgeServiceError, MergeBranchResult,
    NightlyLaneMutationResult, NightlyLaneRerunResult, NightlyRunRecoveryResult,
};
use crate::live::{CiLiveUpdate, CiLiveUpdates};
use crate::mirror;
use crate::model;
use crate::pikaci_store::{require_pikaci_run_store, PikaciRunStore};
use crate::render::is_safe_http_url;
use crate::storage::{ChatAllowlistEntry, InboxReviewContext, Store};
use crate::tutorial::TutorialDoc;

type ForgeBranchLogsResponse =
    SharedForgeBranchLogsResponse<CiLane, RunRecord, RunLogsMetadata, PreparedOutputsRecord>;

#[derive(Clone)]
struct AppState {
    store: Store,
    config: Config,
    pikaci_run_store: Option<PikaciRunStore>,
    auth: Arc<AuthState>,
    live_updates: CiLiveUpdates,
    webhook_secret: Option<String>,
    forge_runtime: Arc<ForgeRuntime>,
    forge_service: Arc<ForgeService>,
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
    branch_chat_artifact_id: Option<i64>,
    branch_name: String,
    title: String,
    target_branch: String,
    branch_state: String,
    merge_commit_sha: Option<String>,
    tutorial_status: String,
    ci_status: String,
    executive_html: Option<String>,
    media_links: Vec<MediaLinkView>,
    error_message: Option<String>,
    steps: Vec<StepView>,
    diff_json: Option<String>,
    branch_ci_summary_html: String,
    branch_ci_summary_enabled: bool,
    branch_chat_ready: bool,
    review_mode: bool,
}

#[derive(Template)]
#[template(path = "nightly.html")]
struct NightlyTemplate {
    page_title: String,
    repo: String,
    nightly_run_id: i64,
    summary: Option<String>,
    scheduled_for: String,
    created_at: String,
    nightly_live_html: String,
    nightly_live_enabled: bool,
}

#[derive(Template)]
#[template(path = "inbox.html")]
struct InboxTemplate {}

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminTemplate {}

#[derive(Template)]
#[template(path = "branch_ci.html")]
struct BranchCiTemplate {
    page_title: String,
    repo: String,
    branch_id: i64,
    branch_name: String,
    title: String,
    target_branch: String,
    updated_at: String,
    branch_state: String,
    head_sha: String,
    merge_base_sha: String,
    review_mode: bool,
    back_href: String,
    branch_ci_live_html: String,
    branch_ci_live_enabled: bool,
}

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

#[derive(Clone, serde::Serialize)]
struct PageNoticeView {
    tone: String,
    message: String,
}

#[derive(Clone, serde::Serialize)]
struct CiSummaryRunView {
    id: i64,
    status: String,
    status_tone: String,
    lane_count: usize,
    created_at: String,
    source_head_sha: String,
    rerun_of_run_id: Option<i64>,
    timing_summary: Option<String>,
    success_count: usize,
    active_count: usize,
    failed_count: usize,
    lanes: Vec<CiSummaryLaneView>,
}

#[derive(Clone, serde::Serialize)]
struct CiSummaryLaneView {
    title: String,
    status: String,
    status_tone: String,
}

#[derive(Clone, serde::Serialize)]
struct CiRunView {
    id: i64,
    source_head_sha: String,
    status: String,
    status_tone: String,
    lane_count: usize,
    rerun_of_run_id: Option<i64>,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    timing_summary: Option<String>,
    lanes: Vec<CiLaneView>,
}

#[derive(Clone, serde::Serialize)]
struct CiLaneView {
    id: i64,
    lane_id: String,
    title: String,
    entrypoint: String,
    status: String,
    status_tone: String,
    execution_reason: String,
    execution_reason_label: String,
    failure_kind: Option<String>,
    failure_kind_label: Option<String>,
    pikaci_run_id: Option<String>,
    pikaci_target_id: Option<String>,
    ci_target_key: Option<String>,
    target_health_state: Option<String>,
    target_health_summary: Option<String>,
    log_text: Option<String>,
    retry_count: i64,
    rerun_of_lane_run_id: Option<i64>,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    timing_summary: Option<String>,
    last_heartbeat_at: Option<String>,
    lease_expires_at: Option<String>,
    operator_hint: Option<String>,
}

#[derive(Clone, serde::Serialize)]
struct NightlyLaneView {
    id: i64,
    lane_id: String,
    title: String,
    entrypoint: String,
    status: String,
    status_badge_class: String,
    is_failed: bool,
    execution_reason: String,
    execution_reason_label: String,
    failure_kind: Option<String>,
    failure_kind_label: Option<String>,
    pikaci_run_id: Option<String>,
    pikaci_target_id: Option<String>,
    ci_target_key: Option<String>,
    target_health_state: Option<String>,
    target_health_summary: Option<String>,
    log_text: Option<String>,
    retry_count: i64,
    rerun_of_lane_run_id: Option<i64>,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    last_heartbeat_at: Option<String>,
    lease_expires_at: Option<String>,
    operator_hint: Option<String>,
}

#[derive(Template)]
#[template(path = "branch_ci_live.html")]
struct BranchCiLiveTemplate {
    branch_id: i64,
    branch_state: String,
    tutorial_status: String,
    ci_status: String,
    ci_status_tone: String,
    live_active: bool,
    ci_runs: Vec<CiRunView>,
    page_notices: Vec<PageNoticeView>,
    latest_failed_lane_count: usize,
}

#[derive(Template)]
#[template(path = "branch_ci_summary.html")]
struct BranchCiSummaryTemplate {
    ci_status: String,
    ci_status_tone: String,
    live_active: bool,
    ci_details_path: String,
    latest_run: Option<CiSummaryRunView>,
    page_notices: Vec<PageNoticeView>,
}

#[derive(Template)]
#[template(path = "nightly_live.html")]
struct NightlyLiveTemplate {
    nightly_run_id: i64,
    status: String,
    live_active: bool,
    source_ref: String,
    source_head_sha: String,
    rerun_of_run_id: Option<i64>,
    started_at: Option<String>,
    finished_at: Option<String>,
    lanes: Vec<NightlyLaneView>,
    page_notices: Vec<PageNoticeView>,
    failed_lane_count: usize,
}

#[derive(serde::Serialize)]
struct LiveHtmlPayload {
    html: String,
}

fn deserialize_boolish<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct BoolishVisitor;

    impl<'de> serde::de::Visitor<'de> for BoolishVisitor {
        type Value = bool;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a boolean or boolean-like value")
        }

        fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
            Ok(value)
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            match value {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(E::custom("expected 0 or 1")),
            }
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            match value {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(E::custom("expected 0 or 1")),
            }
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            match value {
                "1" | "true" | "TRUE" | "True" => Ok(true),
                "0" | "false" | "FALSE" | "False" => Ok(false),
                _ => Err(E::custom("expected true/false or 1/0")),
            }
        }

        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            self.visit_str(&value)
        }
    }

    deserializer.deserialize_any(BoolishVisitor)
}

#[derive(Clone, Copy, Default, serde::Deserialize)]
struct ReviewModeQuery {
    #[serde(default, deserialize_with = "deserialize_boolish")]
    review: bool,
}

#[derive(Clone, serde::Deserialize)]
struct ForgePikaciLogsQuery {
    job: Option<String>,
    #[serde(default)]
    kind: ForgePikaciLogKind,
}

#[derive(Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ForgePikaciLogKind {
    Host,
    Guest,
    #[default]
    Both,
}

#[derive(serde::Serialize)]
struct ForgePikaciLogsResponse {
    run_id: String,
    job: Option<String>,
    host: Option<String>,
    guest: Option<String>,
}

#[derive(serde::Serialize)]
struct ForgePikaciPreparedOutputsResponse {
    run_id: String,
    prepared_outputs: PreparedOutputsRecord,
}

fn ci_status_tone(status: &str) -> &'static str {
    match status {
        "success" | "passed" | "succeeded" | "ready" => "success",
        "failed" | "error" | "lost" | "timed_out" | "timeout" | "cancelled" => "danger",
        "queued"
        | "running"
        | "pending"
        | "waiting"
        | "waiting_for_capacity"
        | "blocked_by_concurrency_group"
        | "target_unhealthy"
        | "blocked_by_target_health"
        | "needs_attention" => "warning",
        _ => "neutral",
    }
}

fn branch_ci_page_path(branch_id: i64, review_mode: bool) -> String {
    if review_mode {
        format!("/git/branch/{}/ci?review=true", branch_id)
    } else {
        format!("/git/branch/{}/ci", branch_id)
    }
}

fn branch_detail_path(branch_id: i64, review_mode: bool) -> String {
    if review_mode {
        format!("/git/inbox/review/{}", branch_id)
    } else {
        format!("/git/branch/{}", branch_id)
    }
}

fn ci_lane_counts(run: &BranchCiRunRecord) -> (usize, usize, usize) {
    let mut success_count = 0;
    let mut active_count = 0;
    let mut failed_count = 0;
    for lane in &run.lanes {
        match ci_status_tone(lane.status.as_str()) {
            "success" => success_count += 1,
            "warning" => active_count += 1,
            "danger" => failed_count += 1,
            _ => {}
        }
    }
    (success_count, active_count, failed_count)
}

fn push_page_notice(
    notices: &mut Vec<PageNoticeView>,
    seen: &mut BTreeSet<String>,
    tone: &str,
    message: &str,
) {
    if seen.insert(message.to_string()) {
        notices.push(PageNoticeView {
            tone: tone.to_string(),
            message: message.to_string(),
        });
    }
}

fn map_forge_service_error(err: ForgeServiceError) -> (StatusCode, String) {
    match err {
        ForgeServiceError::NotFound(message) => (StatusCode::NOT_FOUND, message),
        ForgeServiceError::Conflict(message) => (StatusCode::CONFLICT, message),
        ForgeServiceError::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
    }
}

fn forge_service_json_error(err: ForgeServiceError) -> axum::response::Response {
    let (status, message) = map_forge_service_error(err);
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

fn branch_page_notices(state: &AppState) -> Vec<PageNoticeView> {
    let health = state.forge_runtime.health_snapshot(&state.config, None);
    if !health.enabled {
        return Vec::new();
    }
    let mut notices = Vec::new();
    let mut seen = BTreeSet::new();

    for issue in &health.issues {
        match issue.code.as_str() {
            "canonical_repo_unavailable" => push_page_notice(
                &mut notices,
                &mut seen,
                "error",
                "Forge repo access is degraded. Branch state and CI may be stale until canonical repo access is fixed.",
            ),
            "webhook_secret_missing" | "hook_install_failed" => push_page_notice(
                &mut notices,
                &mut seen,
                "warning",
                "Webhook refresh is unavailable. New pushes may appear on the next poll instead of immediately.",
            ),
            _ => {}
        }
    }

    if health.poller.state == "error" {
        push_page_notice(
            &mut notices,
            &mut seen,
            "warning",
            "Forge polling hit an error. Recent branch updates may be stale until it recovers.",
        );
    }
    if health.generation_worker.state == "error" {
        push_page_notice(
            &mut notices,
            &mut seen,
            "warning",
            "Forge health warning: the tutorial generator worker is unhealthy. New tutorials on any branch may be delayed until it recovers; this forge-wide warning does not mean this branch's last tutorial generation attempt failed.",
        );
    }
    if health.ci.state == "error" {
        push_page_notice(
            &mut notices,
            &mut seen,
            "error",
            "Forge CI hit an error. New lane runs may stay queued until the runner recovers.",
        );
    }

    notices
}

fn nightly_page_notices(state: &AppState) -> Vec<PageNoticeView> {
    let health = state.forge_runtime.health_snapshot(&state.config, None);
    if !health.enabled {
        return Vec::new();
    }
    let mut notices = Vec::new();
    let mut seen = BTreeSet::new();

    for issue in &health.issues {
        if issue.code.as_str() == "canonical_repo_unavailable" {
            push_page_notice(
                &mut notices,
                &mut seen,
                "error",
                "Forge repo access is degraded. Nightly state may be stale until canonical repo access is fixed.",
            );
        }
    }

    if health.ci.state == "error" {
        push_page_notice(
            &mut notices,
            &mut seen,
            "error",
            "Forge CI hit an error. New nightly lanes may stay queued until the runner recovers.",
        );
    }

    notices
}

pub async fn serve(
    store: Store,
    config: Config,
    bind_addr: String,
    max_prs: usize,
) -> anyhow::Result<()> {
    let auth = Arc::new(AuthState::new(&config.bootstrap_admin_npubs, store.clone()));

    if let Err(err) = store.canonicalize_inbox_npubs() {
        eprintln!("warning: failed to canonicalize inbox owners: {}", err);
    }

    let live_updates = CiLiveUpdates::new(256);
    let webhook_secret = env::var(&config.webhook_secret_env).ok();
    let forge_runtime = Arc::new(ForgeRuntime::new(&config, webhook_secret.as_deref()));
    let forge_service = Arc::new(ForgeService::new(
        store.clone(),
        config.clone(),
        live_updates.clone(),
        Arc::clone(&forge_runtime),
    ));
    if let Some(forge_repo) = config.effective_forge_repo() {
        let startup_issues = forge_runtime.issues();
        for issue in &startup_issues {
            eprintln!(
                "forge startup {} [{}]: {}",
                issue.severity, issue.code, issue.message
            );
        }
        eprintln!(
            "forge: canonical_repo={} default_branch={} lane_manifest={}",
            forge_repo.canonical_git_dir,
            forge_repo.default_branch,
            ci::FORGE_LANE_MANIFEST_PATH
        );
        if let Some(remote_name) = forge_repo.mirror_remote.as_deref() {
            let interval = forge_repo.mirror_poll_interval_secs.unwrap_or(0);
            eprintln!(
                "forge: mirror_remote={} background_enabled={} background_interval_secs={}",
                remote_name,
                interval > 0,
                interval
            );
        } else {
            eprintln!("forge: mirror_remote=disabled");
        }
    }
    let state = Arc::new(AppState {
        store,
        config: config.clone(),
        pikaci_run_store: PikaciRunStore::from_config(&config),
        auth,
        live_updates: live_updates.clone(),
        webhook_secret,
        forge_runtime: Arc::clone(&forge_runtime),
        forge_service,
    });
    forge_runtime.start_background(ForgeRuntimeContext {
        store: state.store.clone(),
        config: state.config.clone(),
        max_prs,
        live_updates: state.live_updates.clone(),
        webhook_secret: state.webhook_secret.clone(),
    });

    let git_routes = Router::new()
        .route("/", get(feed_handler))
        .route("/branch/:branch_id", get(detail_handler))
        .route("/branch/:branch_id/ci", get(branch_ci_page_handler))
        .route("/nightly/:nightly_run_id", get(nightly_handler))
        .route(
            "/branch/:branch_id/ci/stream",
            get(branch_ci_stream_handler),
        )
        .route(
            "/branch/:branch_id/ci/stream/full",
            get(branch_ci_full_stream_handler),
        )
        .route(
            "/nightly/:nightly_run_id/stream",
            get(nightly_stream_handler),
        )
        .route("/branch/:branch_id/merge", post(merge_handler))
        .route("/branch/:branch_id/close", post(close_handler))
        .route(
            "/branch/:branch_id/ci/rerun/:lane_run_id",
            post(rerun_branch_ci_lane_handler),
        )
        .route(
            "/branch/:branch_id/ci/fail/:lane_run_id",
            post(fail_branch_ci_lane_handler),
        )
        .route(
            "/branch/:branch_id/ci/requeue/:lane_run_id",
            post(requeue_branch_ci_lane_handler),
        )
        .route(
            "/branch/:branch_id/ci/recover/:run_id",
            post(recover_branch_ci_run_handler),
        )
        .route(
            "/nightly/:nightly_run_id/rerun/:lane_run_id",
            post(rerun_nightly_lane_handler),
        )
        .route(
            "/nightly/:nightly_run_id/fail/:lane_run_id",
            post(fail_nightly_lane_handler),
        )
        .route(
            "/nightly/:nightly_run_id/requeue/:lane_run_id",
            post(requeue_nightly_lane_handler),
        )
        .route(
            "/nightly/:nightly_run_id/recover",
            post(recover_nightly_run_handler),
        )
        .route("/inbox", get(inbox_handler))
        .route("/admin", get(admin_handler))
        .route("/inbox/review/:review_id", get(inbox_review_handler))
        .route("/api/inbox", get(api_inbox_list_handler))
        .route("/api/inbox/count", get(api_inbox_count_handler))
        .route("/api/inbox/dismiss", post(api_inbox_dismiss_handler))
        .route(
            "/api/inbox/reviewed/:review_id",
            post(api_inbox_mark_reviewed_handler),
        )
        .route("/api/me", get(api_me_handler))
        .route(
            "/api/forge/branch/resolve",
            get(api_forge_branch_resolve_handler),
        )
        .route(
            "/api/forge/branch/:branch_id",
            get(api_forge_branch_detail_handler),
        )
        .route(
            "/api/forge/branch/:branch_id/logs",
            get(api_forge_branch_logs_handler),
        )
        .route(
            "/api/forge/pikaci/run/:run_id",
            get(api_forge_pikaci_run_handler),
        )
        .route(
            "/api/forge/pikaci/logs/:run_id",
            get(api_forge_pikaci_logs_handler),
        )
        .route(
            "/api/forge/pikaci/prepared-outputs/:run_id",
            get(api_forge_pikaci_prepared_outputs_handler),
        )
        .route(
            "/api/forge/nightly/:nightly_run_id",
            get(api_forge_nightly_detail_handler),
        )
        .route("/api/forge/branch/:branch_id/merge", post(merge_handler))
        .route("/api/forge/branch/:branch_id/close", post(close_handler))
        .route("/api/forge/ci/wake", post(wake_ci_handler))
        .route(
            "/api/admin/allowlist",
            get(api_admin_allowlist_handler).post(api_admin_allowlist_upsert_handler),
        )
        .route(
            "/api/admin/forge-status",
            get(api_admin_forge_status_handler),
        )
        .route(
            "/api/admin/mirror/sync",
            post(api_admin_mirror_sync_handler),
        )
        .route(
            "/api/inbox/neighbors/:review_id",
            get(api_inbox_neighbors_handler),
        )
        .route("/auth/challenge", post(auth_challenge_handler))
        .route("/auth/verify", post(auth_verify_handler))
        .route(
            "/branch/:branch_id/chat",
            get(branch_chat_history_handler).post(branch_chat_send_handler),
        )
        .route("/webhook", post(webhook_handler));

    let app = Router::new()
        .route("/", get(|| async { Redirect::permanent("/git") }))
        .nest("/git", git_routes.clone())
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

include!("web/views.rs");

include!("web/pages.rs");

include!("web/live.rs");

include!("web/api.rs");

include!("web/auth.rs");

include!("web/chat.rs");

include!("web/webhook.rs");

include!("web/admin.rs");

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests;
