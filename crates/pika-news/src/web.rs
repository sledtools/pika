use std::collections::BTreeSet;
use std::convert::Infallible;
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use futures::stream;
use hmac::{Hmac, Mac};
use pulldown_cmark::{html, Options, Parser};
use sha2::Sha256;
use tokio::sync::{broadcast::error::RecvError, Notify};

use crate::auth::{normalize_npub, AuthState};
use crate::branch_store::{
    BranchCiLaneRecord, BranchCiRunRecord, BranchDetailRecord, BranchFeedItem, MirrorStatusRecord,
    NightlyFeedItem, NightlyLaneRecord, NightlyRunRecord,
};
use crate::ci;
use crate::ci_state::{
    CiLaneExecutionReason, CiLaneFailureKind, CiTargetHealthSnapshot, CiTargetHealthState,
};
use crate::config::Config;
use crate::forge;
use crate::live::{CiLiveUpdate, CiLiveUpdates};
use crate::mirror;
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
    mirror_requested: Arc<AtomicBool>,
    mirror_running: Arc<AtomicBool>,
    live_updates: CiLiveUpdates,
    webhook_secret: Option<String>,
    forge_health: Arc<Mutex<ForgeHealthState>>,
}

fn maybe_start_background_ci_pass(
    state: Arc<AppState>,
    notify: Arc<Notify>,
    ci_running: Arc<AtomicBool>,
) {
    if ci_running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    tokio::spawn(async move {
        let state_for_ci = Arc::clone(&state);
        let scheduler_notify = Arc::clone(&notify);
        let ci_result = tokio::task::spawn_blocking(move || {
            ci::schedule_ci_pass_with_updates(
                &state_for_ci.store,
                &state_for_ci.config,
                Some(&state_for_ci.live_updates),
                Some(scheduler_notify),
            )
        })
        .await;

        match ci_result {
            Ok(Ok(ci)) => {
                let should_wake_follow_up = ci_pass_needs_follow_up_wake(&ci);
                if ci.claimed > 0 || ci.nightlies_scheduled > 0 || ci.retries_recovered > 0 {
                    eprintln!(
                        "ci: claimed={} succeeded={} failed={} nightlies_scheduled={} retries_recovered={}",
                        ci.claimed,
                        ci.succeeded,
                        ci.failed,
                        ci.nightlies_scheduled,
                        ci.retries_recovered
                    );
                }
                if let Ok(mut health) = state.forge_health.lock() {
                    let active =
                        ci.claimed > 0 || ci.nightlies_scheduled > 0 || ci.retries_recovered > 0;
                    health.ci.mark_success(ci_summary(&ci), active);
                }
                ci_running.store(false, Ordering::Release);
                if should_wake_follow_up {
                    notify.notify_one();
                }
            }
            Ok(Err(err)) => {
                eprintln!("pika-news ci runner error: {}", err);
                if let Ok(mut health) = state.forge_health.lock() {
                    health.ci.mark_error(err.to_string());
                }
                ci_running.store(false, Ordering::Release);
            }
            Err(err) => {
                eprintln!("pika-news ci runner task join error: {}", err);
                if let Ok(mut health) = state.forge_health.lock() {
                    health.ci.mark_error(err.to_string());
                }
                ci_running.store(false, Ordering::Release);
            }
        }
    });
}

fn run_scheduled_mirror_pass(state: &AppState) -> anyhow::Result<mirror::MirrorPassResult> {
    let force_requested = state.mirror_requested.load(Ordering::Acquire);
    let acquired = state
        .mirror_running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok();
    if !acquired {
        return Ok(mirror::MirrorPassResult::default());
    }

    let result = if force_requested {
        state.mirror_requested.store(false, Ordering::Release);
        mirror::run_mirror_pass(&state.store, &state.config, "post-mutation")
    } else {
        mirror::run_background_mirror_pass(&state.store, &state.config)
    };

    state.mirror_running.store(false, Ordering::Release);
    result
}

fn ci_pass_needs_follow_up_wake(ci: &ci::CiPassResult) -> bool {
    ci.claimed > 0
        || ci.succeeded > 0
        || ci.failed > 0
        || ci.nightlies_scheduled > 0
        || ci.retries_recovered > 0
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
struct ForgeHealthIssue {
    severity: String,
    code: String,
    message: String,
}

#[derive(Clone, Debug, serde::Serialize)]
struct ForgeSubsystemStatus {
    state: String,
    last_checked_at: Option<String>,
    last_activity_at: Option<String>,
    last_error_at: Option<String>,
    summary: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
struct ForgeMirrorHealthStatus {
    state: String,
    background_enabled: bool,
    background_interval_secs: Option<u64>,
    last_success_at: Option<String>,
    last_failure_at: Option<String>,
    summary: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
struct ForgeHealthSnapshot {
    enabled: bool,
    issues: Vec<ForgeHealthIssue>,
    poller: ForgeSubsystemStatus,
    generation_worker: ForgeSubsystemStatus,
    ci: ForgeSubsystemStatus,
    mirror: ForgeMirrorHealthStatus,
}

#[derive(Clone, Debug)]
struct ForgeSubsystemTracker {
    enabled: bool,
    state: &'static str,
    last_checked_at: Option<String>,
    last_activity_at: Option<String>,
    last_error_at: Option<String>,
    summary: Option<String>,
}

#[derive(Clone, Debug)]
struct ForgeHealthState {
    enabled: bool,
    issues: Vec<ForgeHealthIssue>,
    poller: ForgeSubsystemTracker,
    generation_worker: ForgeSubsystemTracker,
    ci: ForgeSubsystemTracker,
}

impl ForgeSubsystemTracker {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            state: if enabled { "idle" } else { "disabled" },
            last_checked_at: None,
            last_activity_at: None,
            last_error_at: None,
            summary: None,
        }
    }

    fn mark_success(&mut self, summary: String, active: bool) {
        if !self.enabled {
            return;
        }
        let now = now_string();
        self.state = if active { "active" } else { "idle" };
        self.last_checked_at = Some(now.clone());
        if active {
            self.last_activity_at = Some(now);
        }
        self.summary = Some(summary);
    }

    fn mark_error(&mut self, message: String) {
        if !self.enabled {
            return;
        }
        let now = now_string();
        self.state = "error";
        self.last_checked_at = Some(now.clone());
        self.last_error_at = Some(now);
        self.summary = Some(message);
    }

    fn snapshot(&self) -> ForgeSubsystemStatus {
        ForgeSubsystemStatus {
            state: self.state.to_string(),
            last_checked_at: self.last_checked_at.clone(),
            last_activity_at: self.last_activity_at.clone(),
            last_error_at: self.last_error_at.clone(),
            summary: self.summary.clone(),
        }
    }
}

impl ForgeHealthState {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            issues: Vec::new(),
            poller: ForgeSubsystemTracker::new(enabled),
            generation_worker: ForgeSubsystemTracker::new(enabled),
            ci: ForgeSubsystemTracker::new(enabled),
        }
    }

    fn replace_issues(&mut self, issues: Vec<ForgeHealthIssue>) {
        self.issues = issues;
    }

    fn snapshot(
        &self,
        config: &Config,
        mirror_status: Option<&MirrorStatusRecord>,
    ) -> ForgeHealthSnapshot {
        let mirror_runtime = mirror::mirror_runtime_status(config);
        ForgeHealthSnapshot {
            enabled: self.enabled,
            issues: self.issues.clone(),
            poller: self.poller.snapshot(),
            generation_worker: self.generation_worker.snapshot(),
            ci: self.ci.snapshot(),
            mirror: build_mirror_health_status(&mirror_runtime, mirror_status),
        }
    }
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
    title: String,
    target_branch: String,
    updated_at: String,
    branch_state: String,
    tutorial_status: String,
    ci_status: String,
    head_sha: String,
    merge_base_sha: String,
    merge_commit_sha: Option<String>,
    executive_html: Option<String>,
    media_links: Vec<MediaLinkView>,
    error_message: Option<String>,
    steps: Vec<StepView>,
    diff_json: Option<String>,
    branch_ci_summary_html: String,
    branch_ci_summary_enabled: bool,
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

#[derive(serde::Serialize)]
struct ForgeBranchResolveResponse {
    branch_id: i64,
    repo: String,
    branch_name: String,
    branch_state: String,
}

#[derive(serde::Serialize)]
struct ForgeBranchSummaryResponse {
    branch_id: i64,
    repo: String,
    branch_name: String,
    title: String,
    branch_state: String,
    updated_at: String,
    target_branch: String,
    head_sha: String,
    merge_base_sha: String,
    merge_commit_sha: Option<String>,
    tutorial_status: String,
    ci_status: String,
    error_message: Option<String>,
}

#[derive(serde::Serialize)]
struct ForgeBranchDetailResponse {
    branch: ForgeBranchSummaryResponse,
    ci_runs: Vec<CiRunView>,
}

#[derive(serde::Serialize)]
struct ForgeBranchLogsResponse {
    branch_id: i64,
    branch_name: String,
    run_id: i64,
    lane: CiLaneView,
}

#[derive(serde::Serialize)]
struct ForgeNightlyDetailResponse {
    nightly_run_id: i64,
    repo: String,
    scheduled_for: String,
    created_at: String,
    source_ref: String,
    source_head_sha: String,
    status: String,
    summary: Option<String>,
    rerun_of_run_id: Option<i64>,
    started_at: Option<String>,
    finished_at: Option<String>,
    lanes: Vec<NightlyLaneView>,
}

fn now_string() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
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
        format!("/news/branch/{}/ci?review=true", branch_id)
    } else {
        format!("/news/branch/{}/ci", branch_id)
    }
}

fn branch_detail_path(branch_id: i64, review_mode: bool) -> String {
    if review_mode {
        format!("/news/inbox/review/{}", branch_id)
    } else {
        format!("/news/branch/{}", branch_id)
    }
}

fn ci_lane_counts(run: &BranchCiRunRecord) -> (usize, usize, usize) {
    let mut success_count = 0;
    let mut active_count = 0;
    let mut failed_count = 0;
    for lane in &run.lanes {
        match ci_status_tone(&lane.status) {
            "success" => success_count += 1,
            "warning" => active_count += 1,
            "danger" => failed_count += 1,
            _ => {}
        }
    }
    (success_count, active_count, failed_count)
}

fn forge_issue(severity: &str, code: &str, message: impl Into<String>) -> ForgeHealthIssue {
    ForgeHealthIssue {
        severity: severity.to_string(),
        code: code.to_string(),
        message: message.into(),
    }
}

fn poller_summary(result: &poller::PollResult) -> String {
    format!(
        "repos {} · branches {} · queued tutorials {} · queued ci {} · stale closed {}",
        result.repos_polled,
        result.branches_seen,
        result.queued_regenerations,
        result.queued_ci_runs,
        result.stale_closed
    )
}

fn worker_summary(result: &worker::WorkerPassResult) -> String {
    format!(
        "claimed {} · ready {} · failed {} · retry {}",
        result.claimed, result.ready, result.failed, result.retry_scheduled
    )
}

fn ci_summary(result: &ci::CiPassResult) -> String {
    format!(
        "claimed {} · succeeded {} · failed {} · nightlies {} · recovered {}",
        result.claimed,
        result.succeeded,
        result.failed,
        result.nightlies_scheduled,
        result.retries_recovered
    )
}

fn build_mirror_health_status(
    runtime: &mirror::MirrorRuntimeStatus,
    status: Option<&MirrorStatusRecord>,
) -> ForgeMirrorHealthStatus {
    if !runtime.configured {
        return ForgeMirrorHealthStatus {
            state: "disabled".to_string(),
            background_enabled: false,
            background_interval_secs: None,
            last_success_at: None,
            last_failure_at: None,
            summary: Some("mirror remote not configured".to_string()),
        };
    }
    if !runtime.background_enabled {
        return ForgeMirrorHealthStatus {
            state: "disabled".to_string(),
            background_enabled: false,
            background_interval_secs: runtime.background_interval_secs,
            last_success_at: status.and_then(|s| s.last_success_at.clone()),
            last_failure_at: status.and_then(|s| s.last_failure_at.clone()),
            summary: Some("background sync disabled; manual sync only".to_string()),
        };
    }
    if let Some(active_run) = runtime.active_run.as_ref() {
        let elapsed = active_run
            .age_secs
            .map(|age| format!("{age}s"))
            .unwrap_or_else(|| "unknown".to_string());
        let trigger = active_run
            .trigger_source
            .as_deref()
            .unwrap_or("unknown trigger");
        let pid = active_run
            .pid
            .map(|value| format!("pid {value}"))
            .unwrap_or_else(|| "unknown pid".to_string());
        let summary = if active_run.state == "stale" {
            format!(
                "stale mirror run still holds the repo lock ({trigger}, {pid}, elapsed {elapsed})"
            )
        } else {
            format!("mirror sync currently running ({trigger}, {pid}, elapsed {elapsed})")
        };
        return ForgeMirrorHealthStatus {
            state: if active_run.state == "stale" {
                "error".to_string()
            } else {
                "active".to_string()
            },
            background_enabled: true,
            background_interval_secs: runtime.background_interval_secs,
            last_success_at: status.and_then(|s| s.last_success_at.clone()),
            last_failure_at: status.and_then(|s| s.last_failure_at.clone()),
            summary: Some(summary),
        };
    }
    if let Some(status) = status {
        let state = if matches!(
            status.current_failure_kind.as_deref(),
            Some("config" | "stale" | "timeout")
        ) {
            "error"
        } else if matches!(
            status.current_failure_kind.as_deref(),
            Some("busy" | "obsolete")
        ) {
            "active"
        } else {
            "idle"
        };
        let summary = match status.current_failure_kind.as_deref() {
            Some("busy") => Some("another mirror run was already active".to_string()),
            Some("obsolete") => Some(
                "another mirror run already completed the needed sync; this trigger was obsolete"
                    .to_string(),
            ),
            Some(kind) => Some(format!("last background attempt failed ({kind})")),
            None => Some("background sync enabled".to_string()),
        };
        return ForgeMirrorHealthStatus {
            state: state.to_string(),
            background_enabled: true,
            background_interval_secs: runtime.background_interval_secs,
            last_success_at: status.last_success_at.clone(),
            last_failure_at: status.last_failure_at.clone(),
            summary,
        };
    }
    ForgeMirrorHealthStatus {
        state: "idle".to_string(),
        background_enabled: true,
        background_interval_secs: runtime.background_interval_secs,
        last_success_at: None,
        last_failure_at: None,
        summary: Some("background sync enabled; no attempts recorded yet".to_string()),
    }
}

fn collect_forge_startup_issues(
    config: &Config,
    forge_repo: &crate::config::ForgeRepoConfig,
    webhook_secret: Option<&str>,
) -> Vec<ForgeHealthIssue> {
    let mut issues = Vec::new();

    if webhook_secret.is_none() {
        issues.push(forge_issue(
            "error",
            "webhook_secret_missing",
            format!(
                "{} is not set. Install hooks and webhook-triggered refresh stay disabled until it is configured.",
                config.webhook_secret_env
            ),
        ));
    }

    match forge::ensure_canonical_repo(forge_repo) {
        Ok(()) => {
            if let Some(secret) = webhook_secret {
                if let Err(err) = forge::install_hooks(forge_repo, secret) {
                    issues.push(forge_issue(
                        "error",
                        "hook_install_failed",
                        format!(
                            "Could not install forge hooks in {}: {}",
                            forge_repo.canonical_git_dir, err
                        ),
                    ));
                }
            }
        }
        Err(err) => {
            issues.push(forge_issue(
                "error",
                "canonical_repo_unavailable",
                format!(
                    "Canonical repo path {} is not usable: {}",
                    forge_repo.canonical_git_dir, err
                ),
            ));
        }
    }

    match forge_repo.mirror_remote.as_deref() {
        None => issues.push(forge_issue(
            "warning",
            "mirror_remote_missing",
            "Mirror remote is not configured. GitHub stays disabled until forge_repo.mirror_remote is set.",
        )),
        Some(remote_name) => match forge::mirror_remote_url(forge_repo, remote_name) {
            Ok(remote_url) => {
                let token_missing = env::var(&config.github_token_env)
                    .ok()
                    .is_none_or(|value| value.trim().is_empty());
                if remote_url.contains("github.com") && token_missing {
                    issues.push(forge_issue(
                        "warning",
                        "mirror_auth_missing",
                        format!(
                            "Mirror remote `{remote_name}` points at GitHub, but {} is not set. Background and manual sync will fail until credentials are available.",
                            config.github_token_env
                        ),
                    ));
                }
            }
            Err(err) => issues.push(forge_issue(
                "error",
                "mirror_remote_invalid",
                format!("Mirror remote `{remote_name}` could not be resolved: {err}"),
            )),
        },
    }

    issues
}

fn current_forge_runtime_issues(
    config: &Config,
    webhook_secret: Option<&str>,
) -> Vec<ForgeHealthIssue> {
    let Some(forge_repo) = config.effective_forge_repo() else {
        return Vec::new();
    };
    collect_forge_startup_issues(config, &forge_repo, webhook_secret)
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

fn branch_page_notices(state: &AppState) -> Vec<PageNoticeView> {
    let Ok(health) = state.forge_health.lock() else {
        return Vec::new();
    };
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
            "The summary generator is unhealthy. New tutorials across the forge may be delayed until it recovers.",
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
    let Ok(health) = state.forge_health.lock() else {
        return Vec::new();
    };
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
    let mirror_requested = Arc::new(AtomicBool::new(false));
    let mirror_running = Arc::new(AtomicBool::new(false));
    let live_updates = CiLiveUpdates::new(256);
    let webhook_secret = env::var(&config.webhook_secret_env).ok();
    let forge_mode = config.effective_forge_repo().is_some();
    let forge_health = Arc::new(Mutex::new(ForgeHealthState::new(forge_mode)));
    if let Some(forge_repo) = config.effective_forge_repo() {
        let startup_issues = current_forge_runtime_issues(&config, webhook_secret.as_deref());
        if let Ok(mut health) = forge_health.lock() {
            health.replace_issues(startup_issues.clone());
        }
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
        max_prs,
        auth,
        poll_notify: Arc::clone(&poll_notify),
        mirror_requested: Arc::clone(&mirror_requested),
        mirror_running: Arc::clone(&mirror_running),
        live_updates: live_updates.clone(),
        webhook_secret,
        forge_health: Arc::clone(&forge_health),
    });

    let background_state = Arc::clone(&state);
    let background_notify = Arc::clone(&poll_notify);
    let background_ci_running = Arc::new(AtomicBool::new(false));
    tokio::spawn(async move {
        loop {
            let state = Arc::clone(&background_state);
            match tokio::task::spawn_blocking(move || {
                (
                    current_forge_runtime_issues(&state.config, state.webhook_secret.as_deref()),
                    poller::poll_once_limited_with_updates(
                        &state.store,
                        &state.config,
                        state.max_prs,
                        Some(&state.live_updates),
                    ),
                    worker::run_generation_pass(&state.store, &state.config),
                    run_scheduled_mirror_pass(&state),
                )
            })
            .await
            {
                Ok((issues, poll_result, worker_result, mirror_result)) => {
                    if let Ok(mut health) = background_state.forge_health.lock() {
                        health.replace_issues(issues);
                    }
                    match poll_result {
                        Ok(pr) => {
                            if pr.branches_seen > 0
                                || pr.queued_regenerations > 0
                                || pr.stale_closed > 0
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
                            if let Ok(mut health) = background_state.forge_health.lock() {
                                let active = pr.queued_regenerations > 0
                                    || pr.queued_ci_runs > 0
                                    || pr.head_sha_changes > 0
                                    || pr.stale_closed > 0;
                                health.poller.mark_success(poller_summary(&pr), active);
                            }
                            if pr.queued_ci_runs > 0 {
                                background_notify.notify_one();
                            }
                        }
                        Err(err) => {
                            eprintln!("pika-news background poller error: {}", err);
                            if let Ok(mut health) = background_state.forge_health.lock() {
                                health.poller.mark_error(err.to_string());
                            }
                        }
                    }
                    match worker_result {
                        Ok(wr) => {
                            if wr.claimed > 0 {
                                eprintln!(
                                    "worker: claimed={} ready={} failed={} retry={}",
                                    wr.claimed, wr.ready, wr.failed, wr.retry_scheduled
                                );
                            }
                            if let Ok(mut health) = background_state.forge_health.lock() {
                                let active = wr.claimed > 0
                                    || wr.ready > 0
                                    || wr.failed > 0
                                    || wr.retry_scheduled > 0;
                                health
                                    .generation_worker
                                    .mark_success(worker_summary(&wr), active);
                            }
                        }
                        Err(err) => {
                            eprintln!("pika-news background worker error: {}", err);
                            if let Ok(mut health) = background_state.forge_health.lock() {
                                health.generation_worker.mark_error(err.to_string());
                            }
                        }
                    }
                    match mirror_result {
                        Ok(mirror)
                            if mirror.attempted
                                && (mirror.status.as_deref() != Some("success")
                                    || mirror.lagging_ref_count.unwrap_or(0) > 0) =>
                        {
                            eprintln!(
                                "mirror: status={} lagging_refs={}",
                                mirror.status.as_deref().unwrap_or("unknown"),
                                mirror.lagging_ref_count.unwrap_or(-1)
                            );
                        }
                        Ok(_) => {}
                        Err(err) => {
                            eprintln!("pika-news mirror runner error: {}", err);
                        }
                    }
                }
                Err(err) => {
                    eprintln!("pika-news background task join error: {}", err);
                }
            }
            maybe_start_background_ci_pass(
                Arc::clone(&background_state),
                Arc::clone(&background_notify),
                Arc::clone(&background_ci_running),
            );
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
        .route("/news/branch/:branch_id/ci", get(branch_ci_page_handler))
        .route("/news/nightly/:nightly_run_id", get(nightly_handler))
        .route(
            "/news/branch/:branch_id/ci/stream",
            get(branch_ci_stream_handler),
        )
        .route(
            "/news/branch/:branch_id/ci/stream/full",
            get(branch_ci_full_stream_handler),
        )
        .route(
            "/news/nightly/:nightly_run_id/stream",
            get(nightly_stream_handler),
        )
        .route("/news/pr/:pr_id", get(detail_handler))
        .route("/news/branch/:pr_id/merge", post(merge_handler))
        .route("/news/branch/:pr_id/close", post(close_handler))
        .route(
            "/news/branch/:branch_id/ci/rerun/:lane_run_id",
            post(rerun_branch_ci_lane_handler),
        )
        .route(
            "/news/branch/:branch_id/ci/fail/:lane_run_id",
            post(fail_branch_ci_lane_handler),
        )
        .route(
            "/news/branch/:branch_id/ci/requeue/:lane_run_id",
            post(requeue_branch_ci_lane_handler),
        )
        .route(
            "/news/branch/:branch_id/ci/recover/:run_id",
            post(recover_branch_ci_run_handler),
        )
        .route(
            "/news/nightly/:nightly_run_id/rerun/:lane_run_id",
            post(rerun_nightly_lane_handler),
        )
        .route(
            "/news/nightly/:nightly_run_id/fail/:lane_run_id",
            post(fail_nightly_lane_handler),
        )
        .route(
            "/news/nightly/:nightly_run_id/requeue/:lane_run_id",
            post(requeue_nightly_lane_handler),
        )
        .route(
            "/news/nightly/:nightly_run_id/recover",
            post(recover_nightly_run_handler),
        )
        .route("/news/inbox", get(inbox_handler))
        .route("/news/admin", get(admin_handler))
        .route("/news/inbox/review/:pr_id", get(inbox_review_handler))
        .route("/news/api/inbox", get(api_inbox_list_handler))
        .route("/news/api/inbox/count", get(api_inbox_count_handler))
        .route("/news/api/inbox/dismiss", post(api_inbox_dismiss_handler))
        .route("/news/api/me", get(api_me_handler))
        .route(
            "/news/api/forge/branch/resolve",
            get(api_forge_branch_resolve_handler),
        )
        .route(
            "/news/api/forge/branch/:branch_id",
            get(api_forge_branch_detail_handler),
        )
        .route(
            "/news/api/forge/branch/:branch_id/logs",
            get(api_forge_branch_logs_handler),
        )
        .route(
            "/news/api/forge/nightly/:nightly_run_id",
            get(api_forge_nightly_detail_handler),
        )
        .route(
            "/news/api/forge/branch/:branch_id/merge",
            post(merge_handler),
        )
        .route(
            "/news/api/forge/branch/:branch_id/close",
            post(close_handler),
        )
        .route("/news/api/forge/ci/wake", post(wake_ci_handler))
        .route(
            "/news/api/admin/allowlist",
            get(api_admin_allowlist_handler).post(api_admin_allowlist_upsert_handler),
        )
        .route(
            "/news/api/admin/forge-status",
            get(api_admin_forge_status_handler),
        )
        .route(
            "/news/api/admin/mirror/sync",
            post(api_admin_mirror_sync_handler),
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
    let template = render_nightly_template_with_notices(nightly, nightly_page_notices(&state));
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

async fn branch_ci_page_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
    Query(query): Query<ReviewModeQuery>,
) -> impl IntoResponse {
    let (detail, ci_runs) =
        match load_branch_detail_and_runs(Arc::clone(&state), branch_id, 8).await {
            Ok(Some(result)) => result,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    format!("branch {} not found", branch_id),
                )
                    .into_response();
            }
            Err((status, message)) => return (status, message).into_response(),
        };

    match render_branch_ci_template_with_notices(
        detail,
        ci_runs,
        branch_page_notices(&state),
        query.review,
    ) {
        Ok(template) => match template.render() {
            Ok(rendered) => Html(rendered).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to render branch ci template: {}", err),
            )
                .into_response(),
        },
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to build branch ci view: {}", err),
        )
            .into_response(),
    }
}

async fn inbox_review_handler(
    State(state): State<Arc<AppState>>,
    Path(review_id): Path<i64>,
) -> impl IntoResponse {
    let response = detail_page(Arc::clone(&state), review_id, true).await;
    if state.config.effective_forge_repo().is_some() && response.status() == StatusCode::NOT_FOUND {
        let store = state.store.clone();
        let legacy_exists =
            match tokio::task::spawn_blocking(move || store.get_pr_detail(review_id)).await {
                Ok(Ok(Some(_))) => true,
                Ok(Ok(None)) => false,
                Ok(Err(err)) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("failed to query legacy inbox detail: {}", err),
                    )
                        .into_response();
                }
                Err(err) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("legacy inbox worker task failed: {}", err),
                    )
                        .into_response();
                }
            };
        if legacy_exists {
            return Redirect::to("/news/inbox").into_response();
        }
    }
    response
}

async fn detail_page(
    state: Arc<AppState>,
    branch_id: i64,
    review_mode: bool,
) -> axum::response::Response {
    let (detail, ci_runs) =
        match load_branch_detail_and_runs(Arc::clone(&state), branch_id, 8).await {
            Ok(Some(result)) => result,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    format!("branch {} not found", branch_id),
                )
                    .into_response();
            }
            Err((status, message)) => return (status, message).into_response(),
        };

    match render_detail_template_with_notices(
        detail,
        ci_runs,
        review_mode,
        branch_page_notices(&state),
    ) {
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

async fn load_branch_detail_and_runs(
    state: Arc<AppState>,
    branch_id: i64,
    run_limit: usize,
) -> Result<Option<(BranchDetailRecord, Vec<BranchCiRunRecord>)>, (StatusCode, String)> {
    let detail_store = state.store.clone();
    let runs_store = state.store.clone();
    let detail = match tokio::task::spawn_blocking(move || {
        detail_store.get_branch_detail(branch_id)
    })
    .await
    {
        Ok(Ok(Some(record))) => record,
        Ok(Ok(None)) => return Ok(None),
        Ok(Err(err)) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to query branch detail: {}", err),
            ));
        }
        Err(err) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("detail worker task failed: {}", err),
            ));
        }
    };
    let ci_runs = match tokio::task::spawn_blocking(move || {
        runs_store.list_branch_ci_runs(branch_id, run_limit)
    })
    .await
    {
        Ok(Ok(runs)) => runs,
        Ok(Err(err)) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to query branch ci runs: {}", err),
            ));
        }
        Err(err) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("ci worker task failed: {}", err),
            ));
        }
    };
    Ok(Some((detail, ci_runs)))
}

struct BranchCiLiveSnapshot {
    html: String,
    active: bool,
}

struct BranchCiSummarySnapshot {
    html: String,
    active: bool,
}

struct NightlyLiveSnapshot {
    html: String,
    active: bool,
}

async fn load_branch_ci_summary_snapshot(
    state: Arc<AppState>,
    branch_id: i64,
    review_mode: bool,
) -> Result<Option<BranchCiSummarySnapshot>, (StatusCode, String)> {
    let detail_store = state.store.clone();
    let runs_store = state.store.clone();
    let detail = match tokio::task::spawn_blocking(move || {
        detail_store.get_branch_detail(branch_id)
    })
    .await
    {
        Ok(Ok(Some(record))) => record,
        Ok(Ok(None)) => return Ok(None),
        Ok(Err(err)) => return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string())),
        Err(err) => return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string())),
    };
    let ci_runs =
        match tokio::task::spawn_blocking(move || runs_store.list_branch_ci_runs(branch_id, 8))
            .await
        {
            Ok(Ok(runs)) => runs,
            Ok(Err(err)) => return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string())),
            Err(err) => return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string())),
        };
    let html =
        render_branch_ci_summary_html(&detail, &ci_runs, &branch_page_notices(&state), review_mode)
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let active = branch_ci_runs_are_active(&ci_runs);
    Ok(Some(BranchCiSummarySnapshot { html, active }))
}

async fn load_branch_ci_live_snapshot(
    state: Arc<AppState>,
    branch_id: i64,
) -> Result<Option<BranchCiLiveSnapshot>, (StatusCode, String)> {
    let detail_store = state.store.clone();
    let runs_store = state.store.clone();
    let detail = match tokio::task::spawn_blocking(move || {
        detail_store.get_branch_detail(branch_id)
    })
    .await
    {
        Ok(Ok(Some(record))) => record,
        Ok(Ok(None)) => return Ok(None),
        Ok(Err(err)) => return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string())),
        Err(err) => return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string())),
    };
    let ci_runs =
        match tokio::task::spawn_blocking(move || runs_store.list_branch_ci_runs(branch_id, 8))
            .await
        {
            Ok(Ok(runs)) => runs,
            Ok(Err(err)) => return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string())),
            Err(err) => return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string())),
        };
    let html = render_branch_ci_live_html(&detail, &ci_runs, &branch_page_notices(&state))
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let active = branch_ci_runs_are_active(&ci_runs);
    Ok(Some(BranchCiLiveSnapshot { html, active }))
}

async fn load_nightly_live_snapshot(
    state: Arc<AppState>,
    nightly_run_id: i64,
) -> Result<Option<NightlyLiveSnapshot>, (StatusCode, String)> {
    let store = state.store.clone();
    let nightly =
        match tokio::task::spawn_blocking(move || store.get_nightly_run(nightly_run_id)).await {
            Ok(Ok(Some(run))) => run,
            Ok(Ok(None)) => return Ok(None),
            Ok(Err(err)) => return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string())),
            Err(err) => return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string())),
        };
    let html = render_nightly_live_html(&nightly, &nightly_page_notices(&state))
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let active = nightly_run_is_active(&nightly);
    Ok(Some(NightlyLiveSnapshot { html, active }))
}

fn live_html_event(html: String) -> Result<Event, Infallible> {
    let payload = serde_json::to_string(&LiveHtmlPayload { html }).unwrap_or_else(|_| {
        serde_json::json!({"html": "<p class=\"muted\">Failed to encode live update.</p>"})
            .to_string()
    });
    Ok(Event::default().event("ci-update").data(payload))
}

fn branch_live_update_error_html(status: StatusCode, message: &str) -> String {
    format!(
        "<section class=\"panel\"><h2>CI</h2><p class=\"muted\">Live update failed: {} {}</p></section>",
        status.as_u16(),
        message
    )
}

fn nightly_live_update_error_html(status: StatusCode, message: &str) -> String {
    format!(
        "<section class=\"panel\"><h2>Lanes</h2><p class=\"muted\">Live update failed: {} {}</p></section>",
        status.as_u16(),
        message
    )
}

async fn next_branch_ci_summary_snapshot(
    receiver: &mut tokio::sync::broadcast::Receiver<CiLiveUpdate>,
    state: Arc<AppState>,
    branch_id: i64,
    review_mode: bool,
) -> Option<BranchCiSummarySnapshot> {
    loop {
        match receiver.recv().await {
            Ok(CiLiveUpdate::BranchChanged {
                branch_id: updated_branch_id,
                ..
            }) if updated_branch_id == branch_id => {
                return match load_branch_ci_summary_snapshot(
                    Arc::clone(&state),
                    branch_id,
                    review_mode,
                )
                .await
                {
                    Ok(Some(snapshot)) => Some(snapshot),
                    Ok(None) => None,
                    Err((status, message)) => Some(BranchCiSummarySnapshot {
                        html: branch_live_update_error_html(status, &message),
                        active: false,
                    }),
                };
            }
            Ok(_) => continue,
            Err(RecvError::Lagged(_)) => {
                return match load_branch_ci_summary_snapshot(
                    Arc::clone(&state),
                    branch_id,
                    review_mode,
                )
                .await
                {
                    Ok(Some(snapshot)) => Some(snapshot),
                    Ok(None) => None,
                    Err((status, message)) => Some(BranchCiSummarySnapshot {
                        html: branch_live_update_error_html(status, &message),
                        active: false,
                    }),
                };
            }
            Err(RecvError::Closed) => return None,
        }
    }
}

async fn next_branch_ci_live_snapshot(
    receiver: &mut tokio::sync::broadcast::Receiver<CiLiveUpdate>,
    state: Arc<AppState>,
    branch_id: i64,
) -> Option<BranchCiLiveSnapshot> {
    loop {
        match receiver.recv().await {
            Ok(CiLiveUpdate::BranchChanged {
                branch_id: updated_branch_id,
                ..
            }) if updated_branch_id == branch_id => {
                return match load_branch_ci_live_snapshot(Arc::clone(&state), branch_id).await {
                    Ok(Some(snapshot)) => Some(snapshot),
                    Ok(None) => None,
                    Err((status, message)) => Some(BranchCiLiveSnapshot {
                        html: branch_live_update_error_html(status, &message),
                        active: false,
                    }),
                };
            }
            Ok(_) => continue,
            Err(RecvError::Lagged(_)) => {
                return match load_branch_ci_live_snapshot(Arc::clone(&state), branch_id).await {
                    Ok(Some(snapshot)) => Some(snapshot),
                    Ok(None) => None,
                    Err((status, message)) => Some(BranchCiLiveSnapshot {
                        html: branch_live_update_error_html(status, &message),
                        active: false,
                    }),
                };
            }
            Err(RecvError::Closed) => return None,
        }
    }
}

async fn next_nightly_live_snapshot(
    receiver: &mut tokio::sync::broadcast::Receiver<CiLiveUpdate>,
    state: Arc<AppState>,
    nightly_run_id: i64,
) -> Option<NightlyLiveSnapshot> {
    loop {
        match receiver.recv().await {
            Ok(CiLiveUpdate::NightlyChanged {
                nightly_run_id: updated_nightly_run_id,
                ..
            }) if updated_nightly_run_id == nightly_run_id => {
                return match load_nightly_live_snapshot(Arc::clone(&state), nightly_run_id).await {
                    Ok(Some(snapshot)) => Some(snapshot),
                    Ok(None) => None,
                    Err((status, message)) => Some(NightlyLiveSnapshot {
                        html: nightly_live_update_error_html(status, &message),
                        active: false,
                    }),
                };
            }
            Ok(_) => continue,
            Err(RecvError::Lagged(_)) => {
                return match load_nightly_live_snapshot(Arc::clone(&state), nightly_run_id).await {
                    Ok(Some(snapshot)) => Some(snapshot),
                    Ok(None) => None,
                    Err((status, message)) => Some(NightlyLiveSnapshot {
                        html: nightly_live_update_error_html(status, &message),
                        active: false,
                    }),
                };
            }
            Err(RecvError::Closed) => return None,
        }
    }
}

async fn branch_ci_stream_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
    Query(query): Query<ReviewModeQuery>,
) -> impl IntoResponse {
    let review_mode = query.review;
    let initial =
        match load_branch_ci_summary_snapshot(Arc::clone(&state), branch_id, review_mode).await {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    format!("branch {} not found", branch_id),
                )
                    .into_response();
            }
            Err((status, message)) => return (status, message).into_response(),
        };
    let receiver = state.live_updates.subscribe();
    let stream = stream::unfold(
        Some((Some(initial), receiver, state, branch_id)),
        move |state| async move {
            let (pending, mut receiver, state, branch_id) = match state {
                Some(state) => state,
                None => return None,
            };
            if let Some(snapshot) = pending {
                let next_state = if snapshot.active {
                    Some((None, receiver, state, branch_id))
                } else {
                    None
                };
                return Some((live_html_event(snapshot.html), next_state));
            }
            let snapshot = match next_branch_ci_summary_snapshot(
                &mut receiver,
                Arc::clone(&state),
                branch_id,
                review_mode,
            )
            .await
            {
                Some(snapshot) => snapshot,
                None => return None,
            };
            let next_state = if snapshot.active {
                Some((None, receiver, state, branch_id))
            } else {
                None
            };
            Some((live_html_event(snapshot.html), next_state))
        },
    );
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

async fn branch_ci_full_stream_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
) -> impl IntoResponse {
    let initial = match load_branch_ci_live_snapshot(Arc::clone(&state), branch_id).await {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                format!("branch {} not found", branch_id),
            )
                .into_response();
        }
        Err((status, message)) => return (status, message).into_response(),
    };
    let receiver = state.live_updates.subscribe();
    let stream = stream::unfold(
        Some((Some(initial), receiver, state, branch_id)),
        |state| async move {
            let (pending, mut receiver, state, branch_id) = match state {
                Some(state) => state,
                None => return None,
            };
            if let Some(snapshot) = pending {
                let next_state = if snapshot.active {
                    Some((None, receiver, state, branch_id))
                } else {
                    None
                };
                return Some((live_html_event(snapshot.html), next_state));
            }
            let snapshot =
                match next_branch_ci_live_snapshot(&mut receiver, Arc::clone(&state), branch_id)
                    .await
                {
                    Some(snapshot) => snapshot,
                    None => return None,
                };
            let next_state = if snapshot.active {
                Some((None, receiver, state, branch_id))
            } else {
                None
            };
            Some((live_html_event(snapshot.html), next_state))
        },
    );
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

async fn nightly_stream_handler(
    State(state): State<Arc<AppState>>,
    Path(nightly_run_id): Path<i64>,
) -> impl IntoResponse {
    let initial = match load_nightly_live_snapshot(Arc::clone(&state), nightly_run_id).await {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                format!("nightly run {} not found", nightly_run_id),
            )
                .into_response();
        }
        Err((status, message)) => return (status, message).into_response(),
    };
    let receiver = state.live_updates.subscribe();
    let stream = stream::unfold(
        Some((Some(initial), receiver, state, nightly_run_id)),
        |state| async move {
            let (pending, mut receiver, state, nightly_run_id) = match state {
                Some(state) => state,
                None => return None,
            };
            if let Some(snapshot) = pending {
                let next_state = if snapshot.active {
                    Some((None, receiver, state, nightly_run_id))
                } else {
                    None
                };
                return Some((live_html_event(snapshot.html), next_state));
            }
            let snapshot =
                match next_nightly_live_snapshot(&mut receiver, Arc::clone(&state), nightly_run_id)
                    .await
                {
                    Some(snapshot) => snapshot,
                    None => return None,
                };
            let next_state = if snapshot.active {
                Some((None, receiver, state, nightly_run_id))
            } else {
                None
            };
            Some((live_html_event(snapshot.html), next_state))
        },
    );
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
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

#[cfg(test)]
fn render_detail_template(
    record: BranchDetailRecord,
    ci_runs: Vec<BranchCiRunRecord>,
    review_mode: bool,
) -> anyhow::Result<DetailTemplate> {
    render_detail_template_with_notices(record, ci_runs, review_mode, Vec::new())
}

fn render_detail_template_with_notices(
    record: BranchDetailRecord,
    ci_runs: Vec<BranchCiRunRecord>,
    review_mode: bool,
    page_notices: Vec<PageNoticeView>,
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

    let branch_ci_summary_html =
        render_branch_ci_summary_html(&record, &ci_runs, &page_notices, review_mode)?;
    let branch_ci_summary_enabled = branch_ci_runs_are_active(&ci_runs);

    Ok(DetailTemplate {
        page_title: format!(
            "{} #{}: {}",
            record.repo, record.branch_id, record.branch_name
        ),
        repo: record.repo,
        branch_id: record.branch_id,
        branch_name: record.branch_name,
        title: record.title,
        target_branch: record.target_branch,
        updated_at: record.updated_at,
        branch_state: record.branch_state.clone(),
        tutorial_status: record.tutorial_status,
        ci_status: record.ci_status,
        head_sha: record.head_sha,
        merge_base_sha: record.merge_base_sha,
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
        branch_ci_summary_html,
        branch_ci_summary_enabled,
        review_mode,
    })
}

fn render_branch_ci_template_with_notices(
    record: BranchDetailRecord,
    ci_runs: Vec<BranchCiRunRecord>,
    page_notices: Vec<PageNoticeView>,
    review_mode: bool,
) -> anyhow::Result<BranchCiTemplate> {
    let branch_ci_live_html = render_branch_ci_live_html(&record, &ci_runs, &page_notices)?;
    let branch_ci_live_enabled = branch_ci_runs_are_active(&ci_runs);
    Ok(BranchCiTemplate {
        page_title: format!("{} #{} CI", record.repo, record.branch_id),
        repo: record.repo,
        branch_id: record.branch_id,
        branch_name: record.branch_name,
        title: record.title,
        target_branch: record.target_branch,
        updated_at: record.updated_at,
        branch_state: record.branch_state,
        head_sha: record.head_sha,
        merge_base_sha: record.merge_base_sha,
        review_mode,
        back_href: branch_detail_path(record.branch_id, review_mode),
        branch_ci_live_html,
        branch_ci_live_enabled,
    })
}

#[cfg(test)]
fn render_nightly_template(run: NightlyRunRecord) -> NightlyTemplate {
    render_nightly_template_with_notices(run, Vec::new())
}

fn render_nightly_template_with_notices(
    run: NightlyRunRecord,
    page_notices: Vec<PageNoticeView>,
) -> NightlyTemplate {
    let nightly_live_html = render_nightly_live_html(&run, &page_notices)
        .unwrap_or_else(|_| "<section class=\"panel\"><h2>Lanes</h2><p class=\"muted\">Failed to render nightly lane state.</p></section>".to_string());
    let nightly_live_enabled = nightly_run_is_active(&run);
    NightlyTemplate {
        page_title: format!("{} nightly #{}", run.repo, run.nightly_run_id),
        repo: run.repo,
        nightly_run_id: run.nightly_run_id,
        summary: run.summary,
        scheduled_for: run.scheduled_for,
        created_at: run.created_at,
        nightly_live_html,
        nightly_live_enabled,
    }
}

fn branch_ci_runs_are_active(ci_runs: &[BranchCiRunRecord]) -> bool {
    ci_runs.iter().any(|run| {
        matches!(run.status.as_str(), "queued" | "running")
            || run
                .lanes
                .iter()
                .any(|lane| matches!(lane.status.as_str(), "queued" | "running"))
    })
}

fn nightly_run_is_active(run: &NightlyRunRecord) -> bool {
    matches!(run.status.as_str(), "queued" | "running")
        || run
            .lanes
            .iter()
            .any(|lane| matches!(lane.status.as_str(), "queued" | "running"))
}

fn render_branch_ci_live_html(
    record: &BranchDetailRecord,
    ci_runs: &[BranchCiRunRecord],
    page_notices: &[PageNoticeView],
) -> anyhow::Result<String> {
    let latest_failed_lane_count = ci_runs
        .first()
        .map(|run| {
            run.lanes
                .iter()
                .filter(|lane| lane.status == "failed")
                .count()
        })
        .unwrap_or(0);
    BranchCiLiveTemplate {
        branch_id: record.branch_id,
        branch_state: record.branch_state.clone(),
        tutorial_status: record.tutorial_status.clone(),
        ci_status: record.ci_status.clone(),
        ci_status_tone: ci_status_tone(&record.ci_status).to_string(),
        live_active: branch_ci_runs_are_active(ci_runs),
        ci_runs: ci_runs.iter().cloned().map(map_ci_run_view).collect(),
        page_notices: page_notices.to_vec(),
        latest_failed_lane_count,
    }
    .render()
    .context("render branch ci live template")
}

fn render_branch_ci_summary_html(
    record: &BranchDetailRecord,
    ci_runs: &[BranchCiRunRecord],
    page_notices: &[PageNoticeView],
    review_mode: bool,
) -> anyhow::Result<String> {
    let latest_run = ci_runs.first().map(map_ci_summary_run);
    BranchCiSummaryTemplate {
        ci_status: record.ci_status.clone(),
        ci_status_tone: ci_status_tone(&record.ci_status).to_string(),
        live_active: branch_ci_runs_are_active(ci_runs),
        ci_details_path: branch_ci_page_path(record.branch_id, review_mode),
        latest_run,
        page_notices: page_notices.to_vec(),
    }
    .render()
    .context("render branch ci summary template")
}

fn render_nightly_live_html(
    run: &NightlyRunRecord,
    page_notices: &[PageNoticeView],
) -> anyhow::Result<String> {
    let failed_lane_count = run
        .lanes
        .iter()
        .filter(|lane| lane.status == "failed")
        .count();
    NightlyLiveTemplate {
        nightly_run_id: run.nightly_run_id,
        status: run.status.clone(),
        live_active: nightly_run_is_active(run),
        source_ref: run.source_ref.clone(),
        source_head_sha: run.source_head_sha.clone(),
        rerun_of_run_id: run.rerun_of_run_id,
        started_at: run.started_at.clone(),
        finished_at: run.finished_at.clone(),
        lanes: run
            .lanes
            .iter()
            .cloned()
            .map(map_nightly_lane_view)
            .collect(),
        page_notices: page_notices.to_vec(),
        failed_lane_count,
    }
    .render()
    .context("render nightly live template")
}

fn map_ci_run_view(run: BranchCiRunRecord) -> CiRunView {
    let status_tone = ci_status_tone(&run.status).to_string();
    CiRunView {
        id: run.id,
        source_head_sha: run.source_head_sha,
        status: run.status,
        status_tone,
        lane_count: run.lane_count,
        rerun_of_run_id: run.rerun_of_run_id,
        created_at: run.created_at,
        started_at: run.started_at,
        finished_at: run.finished_at,
        lanes: run.lanes.into_iter().map(map_ci_lane_view).collect(),
    }
}

fn map_ci_lane_view(lane: BranchCiLaneRecord) -> CiLaneView {
    let target_health_summary =
        lane_target_health_summary(lane.ci_target_key.as_deref(), lane.target_health.as_ref());
    let operator_hint = lane_operator_hint(&LaneHintContext {
        status: &lane.status,
        execution_reason: lane.execution_reason,
        failure_kind: lane.failure_kind,
        ci_target_key: lane.ci_target_key.as_deref(),
        target_health: lane.target_health.as_ref(),
        created_at: &lane.created_at,
        started_at: lane.started_at.as_deref(),
        finished_at: lane.finished_at.as_deref(),
        last_heartbeat_at: lane.last_heartbeat_at.as_deref(),
        lease_expires_at: lane.lease_expires_at.as_deref(),
    });
    let status_tone = ci_status_tone(&lane.status).to_string();
    let failure_kind = lane.failure_kind.map(|kind| kind.as_str().to_string());
    let failure_kind_label = lane.failure_kind.map(|kind| kind.label().to_string());
    let target_health_state = lane
        .target_health
        .as_ref()
        .map(|snapshot| snapshot.effective_state(Utc::now()).as_str().to_string());
    CiLaneView {
        id: lane.id,
        lane_id: lane.lane_id,
        title: lane.title,
        entrypoint: lane.entrypoint,
        status: lane.status,
        status_tone,
        execution_reason: lane.execution_reason.as_str().to_string(),
        execution_reason_label: lane.execution_reason.label().to_string(),
        failure_kind,
        failure_kind_label,
        pikaci_run_id: lane.pikaci_run_id,
        pikaci_target_id: lane.pikaci_target_id,
        ci_target_key: lane.ci_target_key,
        target_health_state,
        target_health_summary,
        log_text: lane.log_text,
        retry_count: lane.retry_count,
        rerun_of_lane_run_id: lane.rerun_of_lane_run_id,
        created_at: lane.created_at,
        started_at: lane.started_at,
        finished_at: lane.finished_at,
        last_heartbeat_at: lane.last_heartbeat_at,
        lease_expires_at: lane.lease_expires_at,
        operator_hint,
    }
}

fn map_ci_summary_run(run: &BranchCiRunRecord) -> CiSummaryRunView {
    let (success_count, active_count, failed_count) = ci_lane_counts(run);
    CiSummaryRunView {
        id: run.id,
        status: run.status.clone(),
        status_tone: ci_status_tone(&run.status).to_string(),
        lane_count: run.lane_count,
        created_at: run.created_at.clone(),
        source_head_sha: run.source_head_sha.clone(),
        rerun_of_run_id: run.rerun_of_run_id,
        success_count,
        active_count,
        failed_count,
        lanes: run
            .lanes
            .iter()
            .map(|lane| CiSummaryLaneView {
                title: lane.title.clone(),
                status: lane.status.clone(),
                status_tone: ci_status_tone(&lane.status).to_string(),
            })
            .collect(),
    }
}

fn map_nightly_lane_view(lane: NightlyLaneRecord) -> NightlyLaneView {
    let status_badge_class = lane_status_badge_class(&lane.status).to_string();
    let is_failed = lane.status == "failed";
    let target_health_summary =
        lane_target_health_summary(lane.ci_target_key.as_deref(), lane.target_health.as_ref());
    let operator_hint = lane_operator_hint(&LaneHintContext {
        status: &lane.status,
        execution_reason: lane.execution_reason,
        failure_kind: lane.failure_kind,
        ci_target_key: lane.ci_target_key.as_deref(),
        target_health: lane.target_health.as_ref(),
        created_at: &lane.created_at,
        started_at: lane.started_at.as_deref(),
        finished_at: lane.finished_at.as_deref(),
        last_heartbeat_at: lane.last_heartbeat_at.as_deref(),
        lease_expires_at: lane.lease_expires_at.as_deref(),
    });
    let failure_kind = lane.failure_kind.map(|kind| kind.as_str().to_string());
    let failure_kind_label = lane.failure_kind.map(|kind| kind.label().to_string());
    let target_health_state = lane
        .target_health
        .as_ref()
        .map(|snapshot| snapshot.effective_state(Utc::now()).as_str().to_string());
    NightlyLaneView {
        id: lane.id,
        lane_id: lane.lane_id,
        title: lane.title,
        entrypoint: lane.entrypoint,
        status: lane.status,
        status_badge_class,
        is_failed,
        execution_reason: lane.execution_reason.as_str().to_string(),
        execution_reason_label: lane.execution_reason.label().to_string(),
        failure_kind,
        failure_kind_label,
        pikaci_run_id: lane.pikaci_run_id,
        pikaci_target_id: lane.pikaci_target_id,
        ci_target_key: lane.ci_target_key,
        target_health_state,
        target_health_summary,
        log_text: lane.log_text,
        retry_count: lane.retry_count,
        rerun_of_lane_run_id: lane.rerun_of_lane_run_id,
        created_at: lane.created_at,
        started_at: lane.started_at,
        finished_at: lane.finished_at,
        last_heartbeat_at: lane.last_heartbeat_at,
        lease_expires_at: lane.lease_expires_at,
        operator_hint,
    }
}

fn lane_status_badge_class(status: &str) -> &'static str {
    match status {
        "failed" => "status-failed",
        "success" => "status-success",
        "running" => "status-running",
        "queued" => "status-queued",
        "skipped" => "status-skipped",
        _ => "status-neutral",
    }
}

fn parse_ci_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|value| DateTime::<Utc>::from_naive_utc_and_offset(value, Utc))
        })
}

struct LaneHintContext<'a> {
    status: &'a str,
    execution_reason: CiLaneExecutionReason,
    failure_kind: Option<CiLaneFailureKind>,
    ci_target_key: Option<&'a str>,
    target_health: Option<&'a CiTargetHealthSnapshot>,
    created_at: &'a str,
    started_at: Option<&'a str>,
    finished_at: Option<&'a str>,
    last_heartbeat_at: Option<&'a str>,
    lease_expires_at: Option<&'a str>,
}

fn lane_operator_hint(context: &LaneHintContext<'_>) -> Option<String> {
    match context.status {
        "queued" => match context.execution_reason {
            CiLaneExecutionReason::BlockedByConcurrencyGroup => {
                Some("Blocked by another lane in the same concurrency group.".to_string())
            }
            CiLaneExecutionReason::WaitingForCapacity => Some(
                "Waiting for scheduler capacity. Other runnable lanes are already consuming the active worker slots."
                    .to_string(),
            ),
            CiLaneExecutionReason::TargetUnhealthy => {
                let target = context.ci_target_key.unwrap_or("unknown-target");
                let detail = context
                    .target_health
                    .and_then(|snapshot| {
                        snapshot.cooloff_active_until(Utc::now()).map(|cooloff_until| {
                            format!(
                                " after {} consecutive infra failures until {}",
                                snapshot.consecutive_infra_failure_count, cooloff_until
                            )
                        })
                    })
                    .unwrap_or_default();
                Some(format!("Target {target} is currently unhealthy{detail}."))
            }
            CiLaneExecutionReason::StaleRecovered => Some(
                "Recovered after a stale lease expired. This lane is ready to be reclaimed."
                    .to_string(),
            ),
            _ => {
                let age = parse_ci_timestamp(context.created_at)
                    .map(|created| Utc::now().signed_duration_since(created).num_minutes());
                if age.is_some_and(|minutes| minutes >= 15) {
                    Some(format!(
                        "Queued too long since {}. Wake CI or requeue if the scheduler is wedged.",
                        context.created_at
                    ))
                } else {
                    Some(format!("Queued since {}.", context.created_at))
                }
            }
        },
        "running" => {
            let lease_note = context.lease_expires_at.map_or_else(
                || "Running with no lease metadata.".to_string(),
                |lease| {
                    let prefix = match parse_ci_timestamp(lease) {
                        Some(expires_at) if expires_at <= Utc::now() => {
                            "Running with an expired lease"
                        }
                        _ => "Running with lease",
                    };
                    format!("{prefix} until {lease}.")
                },
            );
            let heartbeat_note = context
                .last_heartbeat_at
                .map(|heartbeat| format!(" Last heartbeat {heartbeat}."))
                .unwrap_or_default();
            Some(format!("{lease_note}{heartbeat_note}"))
        }
        "failed" => {
            let failure_detail = context
                .failure_kind
                .map(|kind| format!(" Classified as {}.", kind.label()))
                .unwrap_or_default();
            Some(match context.finished_at {
                Some(finished_at) => format!("Failed at {finished_at}.{failure_detail}"),
                None => format!("Failed.{failure_detail}"),
            })
        }
        "success" | "skipped" => None,
        _ => context
            .started_at
            .map(|started_at| format!("State updated after start at {started_at}."))
            .or_else(|| Some(format!("Current state: {}.", context.status))),
    }
}

fn lane_target_health_summary(
    ci_target_key: Option<&str>,
    target_health: Option<&CiTargetHealthSnapshot>,
) -> Option<String> {
    let snapshot = target_health?;
    if snapshot.effective_state(Utc::now()) != CiTargetHealthState::Unhealthy {
        return None;
    }
    let target = ci_target_key.unwrap_or(&snapshot.target_id);
    let cooloff_suffix = snapshot
        .cooloff_active_until(Utc::now())
        .map(|cooloff_until| format!(" · cooloff until {cooloff_until}"))
        .unwrap_or_default();
    Some(format!(
        "target {target} unhealthy · consecutive infra failures {}{cooloff_suffix}",
        snapshot.consecutive_infra_failure_count
    ))
}
#[derive(serde::Deserialize)]
struct ForgeBranchResolveQuery {
    branch_name: String,
}

#[derive(serde::Deserialize)]
struct ForgeBranchLogsQuery {
    lane: Option<String>,
    lane_run_id: Option<i64>,
}

fn map_forge_branch_summary(detail: BranchDetailRecord) -> ForgeBranchSummaryResponse {
    ForgeBranchSummaryResponse {
        branch_id: detail.branch_id,
        repo: detail.repo,
        branch_name: detail.branch_name,
        title: detail.title,
        branch_state: detail.branch_state,
        updated_at: detail.updated_at,
        target_branch: detail.target_branch,
        head_sha: detail.head_sha,
        merge_base_sha: detail.merge_base_sha,
        merge_commit_sha: detail.merge_commit_sha,
        tutorial_status: detail.tutorial_status,
        ci_status: detail.ci_status,
        error_message: detail.error_message,
    }
}

fn select_branch_log_lane(
    ci_runs: &[BranchCiRunRecord],
    lane_id: Option<&str>,
    lane_run_id: Option<i64>,
) -> Option<(i64, BranchCiLaneRecord)> {
    if let Some(lane_run_id) = lane_run_id {
        return ci_runs.iter().find_map(|run| {
            run.lanes
                .iter()
                .find(|lane| lane.id == lane_run_id)
                .cloned()
                .map(|lane| (run.id, lane))
        });
    }
    if let Some(lane_id) = lane_id {
        return ci_runs.iter().find_map(|run| {
            run.lanes
                .iter()
                .find(|lane| lane.lane_id == lane_id)
                .cloned()
                .map(|lane| (run.id, lane))
        });
    }
    ci_runs
        .iter()
        .find_map(|run| {
            run.lanes
                .iter()
                .find(|lane| lane.status == "failed")
                .cloned()
                .map(|lane| (run.id, lane))
        })
        .or_else(|| {
            ci_runs.iter().find_map(|run| {
                run.lanes
                    .iter()
                    .find(|lane| {
                        lane.log_text
                            .as_ref()
                            .is_some_and(|text| !text.trim().is_empty())
                    })
                    .cloned()
                    .map(|lane| (run.id, lane))
            })
        })
        .or_else(|| {
            ci_runs
                .first()
                .and_then(|run| run.lanes.first().cloned().map(|lane| (run.id, lane)))
        })
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
            state.mirror_requested.store(true, Ordering::Release);
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
            state.mirror_requested.store(true, Ordering::Release);
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

async fn rerun_branch_ci_lane_handler(
    State(state): State<Arc<AppState>>,
    Path((branch_id, lane_run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.rerun_branch_ci_lane(branch_id, lane_run_id))
        .await
    {
        Ok(Ok(Some(rerun_suite_id))) => {
            state.live_updates.branch_changed(branch_id, "rerun_queued");
            state.poll_notify.notify_one();
            Json(serde_json::json!({
                "status": "ok",
                "branch_id": branch_id,
                "rerun_suite_id": rerun_suite_id
            }))
            .into_response()
        }
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "branch lane not found"})),
        )
            .into_response(),
        Ok(Err(err)) => (
            StatusCode::CONFLICT,
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

async fn rerun_nightly_lane_handler(
    State(state): State<Arc<AppState>>,
    Path((nightly_run_id, lane_run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.rerun_nightly_lane(nightly_run_id, lane_run_id))
        .await
    {
        Ok(Ok(Some(rerun_run_id))) => {
            state
                .live_updates
                .nightly_changed(nightly_run_id, "rerun_queued");
            state.poll_notify.notify_one();
            Json(serde_json::json!({
                "status": "ok",
                "nightly_run_id": nightly_run_id,
                "rerun_run_id": rerun_run_id
            }))
            .into_response()
        }
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "nightly lane not found"})),
        )
            .into_response(),
        Ok(Err(err)) => (
            StatusCode::CONFLICT,
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

async fn fail_branch_ci_lane_handler(
    State(state): State<Arc<AppState>>,
    Path((branch_id, lane_run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_trusted_auth(&state.auth, &headers) {
        Ok(npub) => npub,
        Err(resp) => return resp,
    };
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || {
        store.fail_branch_ci_lane(branch_id, lane_run_id, &npub)
    })
    .await
    {
        Ok(Ok(Some(()))) => {
            state.live_updates.branch_changed(branch_id, "lane_failed");
            state.poll_notify.notify_one();
            Json(serde_json::json!({
                "status": "ok",
                "branch_id": branch_id,
                "lane_run_id": lane_run_id,
                "lane_status": "failed"
            }))
            .into_response()
        }
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "branch lane not found"})),
        )
            .into_response(),
        Ok(Err(err)) => (
            StatusCode::CONFLICT,
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

async fn requeue_branch_ci_lane_handler(
    State(state): State<Arc<AppState>>,
    Path((branch_id, lane_run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.requeue_branch_ci_lane(branch_id, lane_run_id))
        .await
    {
        Ok(Ok(Some(()))) => {
            state
                .live_updates
                .branch_changed(branch_id, "lane_requeued");
            state.poll_notify.notify_one();
            Json(serde_json::json!({
                "status": "ok",
                "branch_id": branch_id,
                "lane_run_id": lane_run_id,
                "lane_status": "queued"
            }))
            .into_response()
        }
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "branch lane not found"})),
        )
            .into_response(),
        Ok(Err(err)) => (
            StatusCode::CONFLICT,
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

async fn recover_branch_ci_run_handler(
    State(state): State<Arc<AppState>>,
    Path((branch_id, run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.recover_branch_ci_run(branch_id, run_id)).await
    {
        Ok(Ok(Some(recovered_lane_count))) => {
            state
                .live_updates
                .branch_changed(branch_id, "run_recovered");
            state.poll_notify.notify_one();
            Json(serde_json::json!({
                "status": "ok",
                "branch_id": branch_id,
                "run_id": run_id,
                "recovered_lane_count": recovered_lane_count
            }))
            .into_response()
        }
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "branch run not found"})),
        )
            .into_response(),
        Ok(Err(err)) => (
            StatusCode::CONFLICT,
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

async fn fail_nightly_lane_handler(
    State(state): State<Arc<AppState>>,
    Path((nightly_run_id, lane_run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_trusted_auth(&state.auth, &headers) {
        Ok(npub) => npub,
        Err(resp) => return resp,
    };
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || {
        store.fail_nightly_lane(nightly_run_id, lane_run_id, &npub)
    })
    .await
    {
        Ok(Ok(Some(()))) => {
            state
                .live_updates
                .nightly_changed(nightly_run_id, "lane_failed");
            state.poll_notify.notify_one();
            Json(serde_json::json!({
                "status": "ok",
                "nightly_run_id": nightly_run_id,
                "lane_run_id": lane_run_id,
                "lane_status": "failed"
            }))
            .into_response()
        }
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "nightly lane not found"})),
        )
            .into_response(),
        Ok(Err(err)) => (
            StatusCode::CONFLICT,
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

async fn requeue_nightly_lane_handler(
    State(state): State<Arc<AppState>>,
    Path((nightly_run_id, lane_run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || {
        store.requeue_nightly_lane(nightly_run_id, lane_run_id)
    })
    .await
    {
        Ok(Ok(Some(()))) => {
            state
                .live_updates
                .nightly_changed(nightly_run_id, "lane_requeued");
            state.poll_notify.notify_one();
            Json(serde_json::json!({
                "status": "ok",
                "nightly_run_id": nightly_run_id,
                "lane_run_id": lane_run_id,
                "lane_status": "queued"
            }))
            .into_response()
        }
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "nightly lane not found"})),
        )
            .into_response(),
        Ok(Err(err)) => (
            StatusCode::CONFLICT,
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

async fn recover_nightly_run_handler(
    State(state): State<Arc<AppState>>,
    Path(nightly_run_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.recover_nightly_run(nightly_run_id)).await {
        Ok(Ok(Some(recovered_lane_count))) => {
            state
                .live_updates
                .nightly_changed(nightly_run_id, "run_recovered");
            state.poll_notify.notify_one();
            Json(serde_json::json!({
                "status": "ok",
                "nightly_run_id": nightly_run_id,
                "recovered_lane_count": recovered_lane_count
            }))
            .into_response()
        }
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "nightly run not found"})),
        )
            .into_response(),
        Ok(Err(err)) => (
            StatusCode::CONFLICT,
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

async fn wake_ci_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    state.poll_notify.notify_one();
    Json(serde_json::json!({
        "status": "ok",
        "message": "scheduler wake requested"
    }))
    .into_response()
}

async fn api_forge_branch_resolve_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ForgeBranchResolveQuery>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_auth(&state.auth, &headers) {
        return resp;
    }
    let branch_name = query.branch_name.trim().to_string();
    if branch_name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "branch_name is required"})),
        )
            .into_response();
    }
    let repo = state
        .config
        .effective_forge_repo()
        .map(|repo| repo.repo)
        .unwrap_or_else(|| "sledtools/pika".to_string());
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.find_branch_by_name(&repo, &branch_name)).await
    {
        Ok(Ok(Some(branch))) => Json(ForgeBranchResolveResponse {
            branch_id: branch.branch_id,
            repo: branch.repo,
            branch_name: branch.branch_name,
            branch_state: branch.branch_state,
        })
        .into_response(),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "branch not found"})),
        )
            .into_response(),
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

async fn api_forge_branch_detail_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_auth(&state.auth, &headers) {
        return resp;
    }
    match load_branch_detail_and_runs(Arc::clone(&state), branch_id, 8).await {
        Ok(Some((detail, ci_runs))) => Json(ForgeBranchDetailResponse {
            branch: map_forge_branch_summary(detail),
            ci_runs: ci_runs.into_iter().map(map_ci_run_view).collect(),
        })
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "branch not found"})),
        )
            .into_response(),
        Err((status, message)) => {
            (status, Json(serde_json::json!({"error": message}))).into_response()
        }
    }
}

async fn api_forge_branch_logs_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
    Query(query): Query<ForgeBranchLogsQuery>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_auth(&state.auth, &headers) {
        return resp;
    }
    match load_branch_detail_and_runs(Arc::clone(&state), branch_id, 8).await {
        Ok(Some((detail, ci_runs))) => {
            let Some((run_id, lane)) =
                select_branch_log_lane(&ci_runs, query.lane.as_deref(), query.lane_run_id)
            else {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({"error": "no matching lane logs found"})),
                )
                    .into_response();
            };
            Json(ForgeBranchLogsResponse {
                branch_id: detail.branch_id,
                branch_name: detail.branch_name,
                run_id,
                lane: map_ci_lane_view(lane),
            })
            .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "branch not found"})),
        )
            .into_response(),
        Err((status, message)) => {
            (status, Json(serde_json::json!({"error": message}))).into_response()
        }
    }
}

async fn api_forge_nightly_detail_handler(
    State(state): State<Arc<AppState>>,
    Path(nightly_run_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_auth(&state.auth, &headers) {
        return resp;
    }
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.get_nightly_run(nightly_run_id)).await {
        Ok(Ok(Some(run))) => Json(ForgeNightlyDetailResponse {
            nightly_run_id: run.nightly_run_id,
            repo: run.repo,
            scheduled_for: run.scheduled_for,
            created_at: run.created_at,
            source_ref: run.source_ref,
            source_head_sha: run.source_head_sha,
            status: run.status,
            summary: run.summary,
            rerun_of_run_id: run.rerun_of_run_id,
            started_at: run.started_at,
            finished_at: run.finished_at,
            lanes: run.lanes.into_iter().map(map_nightly_lane_view).collect(),
        })
        .into_response(),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "nightly run not found"})),
        )
            .into_response(),
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
    if !state.auth.auth_enabled() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "auth not enabled"})),
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
    if !state.auth.auth_enabled() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "auth not enabled"})),
        )
            .into_response();
    }
    match state.auth.verify_event(&body.event) {
        Ok((token, npub, is_admin)) => {
            let access = state.auth.access_for_npub(&npub);
            let store = state.store.clone();
            let forge_mode = state.config.effective_forge_repo().is_some();
            let npub_for_backfill = npub.clone();
            match tokio::task::spawn_blocking(move || {
                if forge_mode {
                    store.backfill_branch_inbox_for_npub(&npub_for_backfill)
                } else {
                    store.backfill_inbox_for_npub(&npub_for_backfill)
                }
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
            Json(serde_json::json!({"error": if state.config.effective_forge_repo().is_some() {
                "no tutorial artifact found for this branch"
            } else {
                "no artifact found for this PR"
            }})),
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

async fn api_admin_forge_status_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let _admin_npub = match require_admin_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };

    let store = state.store.clone();
    let mirror_config = state.config.clone();
    let health_config = state.config.clone();
    let forge_health_state = Arc::clone(&state.forge_health);
    match tokio::task::spawn_blocking(move || mirror::get_mirror_status(&store, &mirror_config))
        .await
    {
        Ok(Ok(mirror_admin)) => {
            let forge_health = forge_health_state
                .lock()
                .map(|health| {
                    let mirror_status = mirror_admin.detail.as_ref().map(|(status, _)| status);
                    health.snapshot(&health_config, mirror_status)
                })
                .unwrap_or_else(|_| {
                    ForgeHealthState::new(health_config.effective_forge_repo().is_some())
                        .snapshot(&health_config, None)
                });
            let mirror_runtime = mirror_admin.runtime;
            match mirror_admin.detail {
                Some((mirror_status, mirror_history)) => Json(serde_json::json!({
                    "forge_health": forge_health,
                    "mirror_runtime": mirror_runtime,
                    "mirror_status": mirror_status,
                    "mirror_history": mirror_history,
                }))
                .into_response(),
                None => Json(serde_json::json!({
                    "forge_health": forge_health,
                    "mirror_runtime": mirror_runtime,
                    "mirror_status": null,
                    "mirror_history": [],
                }))
                .into_response(),
            }
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

async fn api_admin_mirror_sync_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let _admin_npub = match require_admin_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };

    let store = state.store.clone();
    let config = state.config.clone();
    if state
        .mirror_running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "mirror sync already running"
            })),
        )
            .into_response();
    }
    match tokio::task::spawn_blocking(move || mirror::run_mirror_pass(&store, &config, "manual"))
        .await
    {
        Ok(Ok(result)) if result.attempted => {
            state.mirror_running.store(false, Ordering::Release);
            if state.mirror_requested.load(Ordering::Acquire) {
                state.poll_notify.notify_one();
            }
            state.poll_notify.notify_one();
            Json(serde_json::json!({
                "attempted": result.attempted,
                "status": result.status,
                "lagging_ref_count": result.lagging_ref_count,
            }))
            .into_response()
        }
        Ok(Ok(_)) => {
            state.mirror_running.store(false, Ordering::Release);
            (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "mirror sync is unavailable; configure forge_repo.mirror_remote to enable mirroring"
                })),
            )
                .into_response()
        }
        Ok(Err(err)) => {
            state.mirror_running.store(false, Ordering::Release);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response()
        }
        Err(err) => {
            state.mirror_running.store(false, Ordering::Release);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response()
        }
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
    let forge_mode = state.config.effective_forge_repo().is_some();
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
            if forge_mode {
                store.backfill_branch_inbox_for_npub(&npub)?
            } else {
                store.backfill_inbox_for_npub(&npub)?
            }
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
    let forge_mode = state.config.effective_forge_repo().is_some();
    match tokio::task::spawn_blocking(move || {
        let items = if forge_mode {
            store.list_branch_inbox(&npub, 50, offset)?
        } else {
            store.list_inbox(&npub, 50, offset)?
        };
        let count = if forge_mode {
            store.branch_inbox_count(&npub)?
        } else {
            store.inbox_count(&npub)?
        };
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
    let forge_mode = state.config.effective_forge_repo().is_some();
    match tokio::task::spawn_blocking(move || {
        if forge_mode {
            store.branch_inbox_count(&npub)
        } else {
            store.inbox_count(&npub)
        }
    })
    .await
    {
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
    branch_ids: Option<Vec<i64>>,
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
    let forge_mode = state.config.effective_forge_repo().is_some();
    let dismissed = if body.all.unwrap_or(false) {
        tokio::task::spawn_blocking(move || {
            if forge_mode {
                store.dismiss_all_branch_inbox(&npub)
            } else {
                store.dismiss_all_inbox(&npub)
            }
        })
        .await
    } else {
        let review_ids = body.branch_ids.or(body.pr_ids).unwrap_or_default();
        tokio::task::spawn_blocking(move || {
            if forge_mode {
                store.dismiss_branch_inbox_items(&npub, &review_ids)
            } else {
                store.dismiss_inbox_items(&npub, &review_ids)
            }
        })
        .await
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
    Path(review_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_chat_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let store = state.store.clone();
    let forge_mode = state.config.effective_forge_repo().is_some();
    match tokio::task::spawn_blocking(move || {
        if forge_mode {
            store.branch_inbox_review_context(&npub, review_id)
        } else {
            store.inbox_review_context(&npub, review_id)
        }
    })
    .await
    {
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
    use std::sync::{Arc, Mutex};

    use askama::Template;
    use axum::body::to_bytes;
    use axum::extract::{Path, Query, State};
    use axum::http::{header::AUTHORIZATION, HeaderMap, HeaderValue, StatusCode};
    use axum::response::IntoResponse;
    use tokio::sync::Notify;

    use super::{
        api_forge_branch_detail_handler, api_forge_branch_logs_handler,
        api_forge_branch_resolve_handler, auth_challenge_handler, branch_ci_stream_handler,
        build_mirror_health_status, collect_forge_startup_issues, current_forge_runtime_issues,
        fail_branch_ci_lane_handler, fail_nightly_lane_handler, inbox_review_handler,
        load_branch_ci_live_snapshot, load_nightly_live_snapshot, markdown_to_safe_html,
        next_branch_ci_live_snapshot, next_nightly_live_snapshot, nightly_stream_handler,
        recover_branch_ci_run_handler, render_branch_ci_template_with_notices,
        render_detail_template, render_detail_template_with_notices, render_nightly_template,
        rerun_branch_ci_lane_handler, rerun_nightly_lane_handler,
        should_backfill_managed_allowlist_entry, verify_signature, wake_ci_handler, AppState,
        CiLiveUpdates, ForgeBranchLogsQuery, ForgeBranchResolveQuery, ForgeHealthState,
        PageNoticeView, ReviewModeQuery,
    };
    use crate::auth::AuthState;
    use crate::branch_store::{BranchUpsertInput, MirrorStatusRecord, MirrorSyncRunRecord};
    use crate::ci;
    use crate::config::{Config, ForgeRepoConfig};
    use crate::forge;
    use crate::mirror::MirrorRuntimeStatus;
    use crate::poller;
    use crate::storage::ChatAllowlistEntry;
    use crate::storage::Store;

    const TRUSTED_NPUB: &str = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y";

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

    fn branch_upsert_input(branch_name: &str, head_sha: &str) -> BranchUpsertInput {
        BranchUpsertInput {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: "/tmp/pika.git".to_string(),
            default_branch: "master".to_string(),
            ci_entrypoint: "just pre-merge".to_string(),
            branch_name: branch_name.to_string(),
            title: format!("{branch_name} title"),
            head_sha: head_sha.to_string(),
            merge_base_sha: "base123".to_string(),
            author_name: Some("alice".to_string()),
            author_email: Some("alice@example.com".to_string()),
            updated_at: "2026-03-18T12:00:00Z".to_string(),
        }
    }

    fn forge_test_config() -> Config {
        Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: Some(2),
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
            bootstrap_admin_npubs: vec![TRUSTED_NPUB.to_string()],
        }
    }

    fn test_state_with_live_buffer(
        store: Store,
        config: Config,
        live_buffer: usize,
    ) -> Arc<AppState> {
        let bootstrap_admin_npubs = config.effective_bootstrap_admin_npubs();
        let legacy_allowed_npubs = config.allowed_npubs.clone();
        let forge_mode = config.effective_forge_repo().is_some();
        Arc::new(AppState {
            auth: Arc::new(AuthState::new(
                &bootstrap_admin_npubs,
                &legacy_allowed_npubs,
                store.clone(),
            )),
            store,
            config,
            max_prs: 10,
            poll_notify: Arc::new(Notify::new()),
            mirror_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            mirror_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            live_updates: CiLiveUpdates::new(live_buffer),
            webhook_secret: None,
            forge_health: Arc::new(Mutex::new(ForgeHealthState::new(forge_mode))),
        })
    }

    fn test_state(store: Store, config: Config) -> Arc<AppState> {
        test_state_with_live_buffer(store, config, 64)
    }

    fn trusted_headers(store: &Store, npub: &str) -> HeaderMap {
        let token = "test-token";
        store
            .insert_auth_token(token, npub)
            .expect("insert auth token");
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("auth header"),
        );
        headers
    }

    #[test]
    fn ci_follow_up_wake_only_for_material_progress() {
        assert!(!super::ci_pass_needs_follow_up_wake(
            &ci::CiPassResult::default()
        ));

        assert!(super::ci_pass_needs_follow_up_wake(&ci::CiPassResult {
            claimed: 1,
            ..Default::default()
        }));
        assert!(super::ci_pass_needs_follow_up_wake(&ci::CiPassResult {
            succeeded: 1,
            ..Default::default()
        }));
        assert!(super::ci_pass_needs_follow_up_wake(&ci::CiPassResult {
            failed: 1,
            ..Default::default()
        }));
        assert!(super::ci_pass_needs_follow_up_wake(&ci::CiPassResult {
            nightlies_scheduled: 1,
            ..Default::default()
        }));
        assert!(super::ci_pass_needs_follow_up_wake(&ci::CiPassResult {
            retries_recovered: 1,
            ..Default::default()
        }));
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
    fn forge_startup_issues_surface_missing_secret_and_mirror_remote() {
        let root = tempfile::tempdir().expect("create temp root");
        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: root.path().join("pika.git").display().to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: Some(2),
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: Some("http://127.0.0.1:8788/news/webhook".to_string()),
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
        let forge_repo = config.effective_forge_repo().expect("forge repo");
        let issues = collect_forge_startup_issues(&config, &forge_repo, None);
        let codes: Vec<&str> = issues.iter().map(|issue| issue.code.as_str()).collect();
        assert!(codes.contains(&"webhook_secret_missing"));
        assert!(codes.contains(&"mirror_remote_missing"));
        assert!(!codes.contains(&"canonical_repo_unavailable"));
    }

    #[test]
    fn forge_runtime_issues_clear_after_hook_install_recovery() {
        let root = tempfile::tempdir().expect("create temp root");
        let canonical = root.path().join("recovered.git");
        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: canonical.display().to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: Some(2),
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: Some("http://127.0.0.1:8788/news/webhook".to_string()),
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

        let output = Command::new("git")
            .args([
                "init",
                "--bare",
                canonical.to_str().expect("canonical path"),
            ])
            .output()
            .expect("init bare repo");
        assert!(
            output.status.success(),
            "git init --bare failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::remove_dir_all(canonical.join("hooks")).expect("remove hooks dir");
        fs::write(canonical.join("hooks"), "blocked").expect("create blocking hooks file");

        let issues = current_forge_runtime_issues(&config, Some("secret"));
        assert!(issues
            .iter()
            .any(|issue| issue.code == "hook_install_failed"));

        fs::remove_file(canonical.join("hooks")).expect("remove blocking hooks file");

        let issues = current_forge_runtime_issues(&config, Some("secret"));
        assert!(!issues
            .iter()
            .any(|issue| issue.code == "hook_install_failed"));
    }

    #[test]
    fn mirror_health_distinguishes_disabled_and_error_states() {
        let disabled = build_mirror_health_status(
            &MirrorRuntimeStatus {
                configured: true,
                remote_name: Some("github".to_string()),
                background_enabled: false,
                background_interval_secs: Some(0),
                timeout_secs: Some(120),
                active_run: None,
                github_token_env: "GITHUB_TOKEN".to_string(),
            },
            None,
        );
        assert_eq!(disabled.state, "disabled");

        let errored = build_mirror_health_status(
            &MirrorRuntimeStatus {
                configured: true,
                remote_name: Some("github".to_string()),
                background_enabled: true,
                background_interval_secs: Some(300),
                timeout_secs: Some(120),
                active_run: None,
                github_token_env: "GITHUB_TOKEN".to_string(),
            },
            Some(&MirrorStatusRecord {
                remote_name: "github".to_string(),
                last_attempt: Some(MirrorSyncRunRecord {
                    id: 1,
                    remote_name: "github".to_string(),
                    trigger_source: "background".to_string(),
                    status: "failed".to_string(),
                    failure_kind: Some("config".to_string()),
                    local_default_head: None,
                    remote_default_head: None,
                    lagging_ref_count: None,
                    synced_ref_count: None,
                    error_text: Some("boom".to_string()),
                    created_at: "2026-03-19T10:00:00Z".to_string(),
                    finished_at: "2026-03-19T10:00:01Z".to_string(),
                }),
                last_success_at: None,
                last_failure_at: Some("2026-03-19T10:00:01Z".to_string()),
                consecutive_failure_count: 1,
                current_lagging_ref_count: None,
                current_failure_kind: Some("config".to_string()),
            }),
        );
        assert_eq!(errored.state, "error");
    }

    #[test]
    fn mirror_health_surfaces_active_and_stale_lock_state() {
        let active = build_mirror_health_status(
            &MirrorRuntimeStatus {
                configured: true,
                remote_name: Some("github".to_string()),
                background_enabled: true,
                background_interval_secs: Some(300),
                timeout_secs: Some(120),
                active_run: Some(crate::forge::MirrorLockStatus {
                    state: "active".to_string(),
                    pid: Some(4242),
                    trigger_source: Some("post-mutation".to_string()),
                    operation: Some("git push --prune mirror".to_string()),
                    started_at: Some("2026-03-24T12:00:00Z".to_string()),
                    age_secs: Some(7),
                }),
                github_token_env: "GITHUB_TOKEN".to_string(),
            },
            None,
        );
        assert_eq!(active.state, "active");

        let stale = build_mirror_health_status(
            &MirrorRuntimeStatus {
                configured: true,
                remote_name: Some("github".to_string()),
                background_enabled: true,
                background_interval_secs: Some(300),
                timeout_secs: Some(120),
                active_run: Some(crate::forge::MirrorLockStatus {
                    state: "stale".to_string(),
                    pid: Some(4242),
                    trigger_source: Some("background".to_string()),
                    operation: Some("git push --prune mirror".to_string()),
                    started_at: Some("2026-03-24T12:00:00Z".to_string()),
                    age_secs: Some(999),
                }),
                github_token_env: "GITHUB_TOKEN".to_string(),
            },
            None,
        );
        assert_eq!(stale.state, "error");
        assert!(stale
            .summary
            .unwrap_or_default()
            .contains("stale mirror run"));
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

    #[tokio::test]
    async fn inbox_review_route_resolves_branch_ids_in_forge_mode() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/review", "head-1"))
            .expect("insert branch");
        let artifact_id = store
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
                r#"{"executive_summary":"ok","media_links":[],"steps":[]}"#,
                "<p>ok</p>",
                "head-1",
                "diff",
            )
            .expect("mark ready");
        store
            .populate_branch_inbox(artifact_id, &["npub1reviewer".to_string()])
            .expect("populate branch inbox");

        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: Some(2),
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let state = test_state(store, config);

        let response = inbox_review_handler(State(state), Path(branch.branch_id))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn inbox_review_legacy_pr_id_redirects_to_inbox_in_forge_mode() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        store
            .upsert_pull_request(&crate::storage::PrUpsertInput {
                repo: "sledtools/pika".to_string(),
                pr_number: 264,
                title: "legacy review item".to_string(),
                url: "https://github.com/sledtools/pika/pull/264".to_string(),
                state: "open".to_string(),
                head_sha: "legacy-head".to_string(),
                base_ref: "master".to_string(),
                author_login: Some("alice".to_string()),
                updated_at: "2026-03-18T12:00:00Z".to_string(),
                merged_at: None,
            })
            .expect("insert legacy pr");
        let legacy_pr_id = store
            .list_feed_items()
            .expect("list feed")
            .into_iter()
            .find(|item| item.pr_number == 264)
            .expect("legacy pr row")
            .pr_id;

        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: Some(2),
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let state = test_state(store, config);

        let response = inbox_review_handler(State(state), Path(legacy_pr_id))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("/news/inbox")
        );
    }

    #[tokio::test]
    async fn api_forge_branch_resolve_returns_open_branch() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/api-resolve", "head-resolve"))
            .expect("insert branch");
        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: None,
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let headers = trusted_headers(&store, TRUSTED_NPUB);
        let state = test_state(store, config);

        let response = api_forge_branch_resolve_handler(
            State(state),
            Query(ForgeBranchResolveQuery {
                branch_name: "feature/api-resolve".to_string(),
            }),
            headers,
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
        assert_eq!(json["branch_id"], branch.branch_id);
        assert_eq!(json["branch_name"], "feature/api-resolve");
        assert_eq!(json["branch_state"], "open");
    }

    #[tokio::test]
    async fn api_forge_branch_resolve_returns_closed_branch_history() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/api-history", "head-history"))
            .expect("insert branch");
        store
            .mark_branch_closed(branch.branch_id, TRUSTED_NPUB)
            .expect("close branch");
        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: None,
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let headers = trusted_headers(&store, TRUSTED_NPUB);
        let state = test_state(store, config);

        let response = api_forge_branch_resolve_handler(
            State(state),
            Query(ForgeBranchResolveQuery {
                branch_name: "feature/api-history".to_string(),
            }),
            headers,
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
        assert_eq!(json["branch_id"], branch.branch_id);
        assert_eq!(json["branch_name"], "feature/api-history");
        assert_eq!(json["branch_state"], "closed");
    }

    #[tokio::test]
    async fn auth_challenge_handler_allows_forge_only_auth_mode() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        store
            .upsert_chat_allowlist_entry(
                TRUSTED_NPUB,
                false,
                true,
                Some("forge-only"),
                "npub1admin",
            )
            .expect("upsert forge-only allowlist entry");
        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: None,
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let state = test_state(store, config);

        let response = auth_challenge_handler(State(state)).await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_forge_branch_detail_returns_ci_summary() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/api-detail", "head-detail"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-detail",
                &[crate::ci_manifest::ForgeLane {
                    id: "pika".to_string(),
                    title: "check-pika".to_string(),
                    entrypoint: "just checks::pre-merge-pika".to_string(),
                    command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                    staged_linux_target: Some("pre-merge-pika-rust".to_string()),
                }],
            )
            .expect("queue ci");
        let claimed = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim lane")
            .into_iter()
            .next()
            .expect("claimed lane");
        store
            .record_branch_ci_lane_pikaci_run(
                claimed.lane_run_id,
                claimed.claim_token,
                "pikaci-api-detail",
                Some("pre-merge-pika-rust"),
            )
            .expect("record pikaci metadata");
        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: None,
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let headers = trusted_headers(&store, TRUSTED_NPUB);
        let state = test_state(store, config);

        let response =
            api_forge_branch_detail_handler(State(state), Path(branch.branch_id), headers)
                .await
                .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
        assert_eq!(json["branch"]["branch_name"], "feature/api-detail");
        assert_eq!(
            json["ci_runs"][0]["lanes"][0]["pikaci_run_id"],
            "pikaci-api-detail"
        );
        assert_eq!(
            json["ci_runs"][0]["lanes"][0]["pikaci_target_id"],
            "pre-merge-pika-rust"
        );
        assert_eq!(
            json["ci_runs"][0]["lanes"][0]["execution_reason"],
            "running"
        );
        assert_eq!(
            json["ci_runs"][0]["lanes"][0]["ci_target_key"],
            "pre-merge-pika-rust"
        );
    }

    #[tokio::test]
    async fn api_forge_branch_detail_exposes_waiting_and_unhealthy_lane_state() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/api-waiting", "head-waiting"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-waiting",
                &[
                    crate::ci_manifest::ForgeLane {
                        id: "wait-capacity".to_string(),
                        title: "wait-capacity".to_string(),
                        entrypoint: "just checks::wait-capacity".to_string(),
                        command: vec!["just".to_string(), "checks::wait-capacity".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                    crate::ci_manifest::ForgeLane {
                        id: "apple-sanity".to_string(),
                        title: "apple-sanity".to_string(),
                        entrypoint: "just checks::apple-sanity".to_string(),
                        command: vec!["just".to_string(), "checks::apple-sanity".to_string()],
                        paths: vec![],
                        concurrency_group: Some("apple-host".to_string()),
                        staged_linux_target: Some("apple-host".to_string()),
                    },
                ],
            )
            .expect("queue ci");
        store
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE branch_ci_run_lanes
                     SET execution_reason = 'waiting_for_capacity'
                     WHERE lane_id = 'wait-capacity'",
                    [],
                )?;
                conn.execute(
                    "UPDATE branch_ci_run_lanes
                     SET execution_reason = 'target_unhealthy'
                     WHERE lane_id = 'apple-sanity'",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO ci_target_health(
                        target_id,
                        state,
                        consecutive_infra_failure_count,
                        last_failure_at,
                        last_failure_kind,
                        cooloff_until,
                        updated_at
                     ) VALUES (?1, 'unhealthy', 2, CURRENT_TIMESTAMP, 'infrastructure', datetime('now', '+15 minutes'), CURRENT_TIMESTAMP)",
                    rusqlite::params!["apple-host"],
                )?;
                Ok::<(), anyhow::Error>(())
            })
            .expect("set waiting/unhealthy state");
        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: Some(1),
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let headers = trusted_headers(&store, TRUSTED_NPUB);
        let state = test_state(store, config);

        let response =
            api_forge_branch_detail_handler(State(state), Path(branch.branch_id), headers)
                .await
                .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
        assert_eq!(
            json["ci_runs"][0]["lanes"][0]["execution_reason"],
            "waiting_for_capacity"
        );
        assert_eq!(
            json["ci_runs"][0]["lanes"][1]["execution_reason"],
            "target_unhealthy"
        );
        assert_eq!(
            json["ci_runs"][0]["lanes"][1]["target_health_state"],
            "unhealthy"
        );
        assert!(json["ci_runs"][0]["lanes"][1]["target_health_summary"]
            .as_str()
            .unwrap_or_default()
            .contains("apple-host"));
    }

    #[tokio::test]
    async fn api_forge_branch_logs_defaults_to_latest_failed_lane() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/api-logs", "head-logs"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-logs",
                &[
                    crate::ci_manifest::ForgeLane {
                        id: "pika".to_string(),
                        title: "check-pika".to_string(),
                        entrypoint: "just checks::pre-merge-pika".to_string(),
                        command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                    crate::ci_manifest::ForgeLane {
                        id: "fixture".to_string(),
                        title: "check-fixture".to_string(),
                        entrypoint: "just checks::pre-merge-fixture".to_string(),
                        command: vec!["just".to_string(), "checks::pre-merge-fixture".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                ],
            )
            .expect("queue ci");
        let claimed = store
            .claim_pending_branch_ci_lane_runs(2, 120)
            .expect("claim lanes");
        let success_lane = claimed
            .iter()
            .find(|lane| lane.lane_id == "pika")
            .expect("success lane");
        store
            .finish_branch_ci_lane_run(
                success_lane.lane_run_id,
                success_lane.claim_token,
                "success",
                "ok",
            )
            .expect("finish success lane");
        let failed_lane = claimed
            .iter()
            .find(|lane| lane.lane_id == "fixture")
            .expect("failed lane");
        store
            .finish_branch_ci_lane_run(
                failed_lane.lane_run_id,
                failed_lane.claim_token,
                "failed",
                "fixture boom",
            )
            .expect("finish failed lane");
        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: None,
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let headers = trusted_headers(&store, TRUSTED_NPUB);
        let state = test_state(store, config);

        let response = api_forge_branch_logs_handler(
            State(state),
            Path(branch.branch_id),
            Query(ForgeBranchLogsQuery {
                lane: None,
                lane_run_id: None,
            }),
            headers,
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
        assert_eq!(json["lane"]["lane_id"], "fixture");
        assert_eq!(json["lane"]["status"], "failed");
        assert_eq!(json["lane"]["log_text"], "fixture boom");
    }

    #[tokio::test]
    async fn rerun_branch_handler_rejects_lane_from_another_branch() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let first = store
            .upsert_branch_record(&branch_upsert_input("feature/one", "head-1"))
            .expect("insert first branch");
        let second = store
            .upsert_branch_record(&branch_upsert_input("feature/two", "head-2"))
            .expect("insert second branch");
        store
            .queue_branch_ci_run_for_head(
                first.branch_id,
                "head-1",
                &[crate::ci_manifest::ForgeLane {
                    id: "pika".to_string(),
                    title: "check-pika".to_string(),
                    entrypoint: "just checks::pre-merge-pika".to_string(),
                    command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                    staged_linux_target: None,
                }],
            )
            .expect("queue branch ci");
        let job = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim branch job")
            .into_iter()
            .next()
            .expect("job");
        store
            .finish_branch_ci_lane_run(job.lane_run_id, job.claim_token, "failed", "boom")
            .expect("finish lane");

        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: Some(2),
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
            bootstrap_admin_npubs: vec![TRUSTED_NPUB.to_string()],
        };
        let headers = trusted_headers(&store, TRUSTED_NPUB);
        let state = test_state(store, config);
        let response = rerun_branch_ci_lane_handler(
            State(state),
            Path((second.branch_id, job.lane_run_id)),
            headers,
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn rerun_nightly_handler_rejects_lane_from_another_run() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        let lane = crate::ci_manifest::ForgeLane {
            id: "nightly_pika".to_string(),
            title: "nightly-pika".to_string(),
            entrypoint: "just checks::nightly-pika-e2e".to_string(),
            command: vec!["just".to_string(), "checks::nightly-pika-e2e".to_string()],
            paths: vec![],
            concurrency_group: None,
            staged_linux_target: None,
        };
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "head-a",
                "2026-03-17T08:00:00Z",
                std::slice::from_ref(&lane),
            )
            .expect("queue first nightly");
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "head-b",
                "2026-03-18T08:00:00Z",
                std::slice::from_ref(&lane),
            )
            .expect("queue second nightly");
        let job = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim nightly job")
            .into_iter()
            .next()
            .expect("job");
        store
            .finish_nightly_lane_run(job.lane_run_id, job.claim_token, "failed", "boom")
            .expect("finish lane");
        let wrong_nightly = store
            .list_recent_nightly_runs(8)
            .expect("list nightly runs")
            .into_iter()
            .find(|run| run.nightly_run_id != job.nightly_run_id)
            .expect("other nightly");

        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: Some(2),
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
            bootstrap_admin_npubs: vec![TRUSTED_NPUB.to_string()],
        };
        let headers = trusted_headers(&store, TRUSTED_NPUB);
        let state = test_state(store, config);
        let response = rerun_nightly_lane_handler(
            State(state),
            Path((wrong_nightly.nightly_run_id, job.lane_run_id)),
            headers,
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn fail_branch_handler_requires_trusted_access() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/forbidden", "head-1"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-1",
                &[crate::ci_manifest::ForgeLane {
                    id: "pika".to_string(),
                    title: "check-pika".to_string(),
                    entrypoint: "just checks::pre-merge-pika".to_string(),
                    command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                    staged_linux_target: None,
                }],
            )
            .expect("queue branch ci");
        let lane = store
            .list_branch_ci_runs(branch.branch_id, 1)
            .expect("list branch runs")[0]
            .lanes[0]
            .id;
        let mut headers = HeaderMap::new();
        store
            .insert_auth_token("reader-token", "npub1reader")
            .expect("insert reader token");
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer reader-token"),
        );
        let state = test_state(store, forge_test_config());

        let response =
            fail_branch_ci_lane_handler(State(state), Path((branch.branch_id, lane)), headers)
                .await
                .into_response();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn recover_branch_handler_rejects_run_from_another_branch() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let first = store
            .upsert_branch_record(&branch_upsert_input("feature/one", "head-1"))
            .expect("insert first branch");
        let second = store
            .upsert_branch_record(&branch_upsert_input("feature/two", "head-2"))
            .expect("insert second branch");
        let lane = crate::ci_manifest::ForgeLane {
            id: "pika".to_string(),
            title: "check-pika".to_string(),
            entrypoint: "just checks::pre-merge-pika".to_string(),
            command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
            paths: vec![],
            concurrency_group: None,
            staged_linux_target: None,
        };
        store
            .queue_branch_ci_run_for_head(first.branch_id, "head-1", std::slice::from_ref(&lane))
            .expect("queue first branch");
        store
            .queue_branch_ci_run_for_head(second.branch_id, "head-2", std::slice::from_ref(&lane))
            .expect("queue second branch");
        let wrong_run_id = store
            .list_branch_ci_runs(first.branch_id, 1)
            .expect("first runs")[0]
            .id;

        let headers = trusted_headers(&store, TRUSTED_NPUB);
        let state = test_state(store, forge_test_config());
        let response = recover_branch_ci_run_handler(
            State(state),
            Path((second.branch_id, wrong_run_id)),
            headers,
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn fail_nightly_handler_rejects_lane_from_another_run() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        let lane = crate::ci_manifest::ForgeLane {
            id: "nightly_pika".to_string(),
            title: "nightly-pika".to_string(),
            entrypoint: "just checks::nightly-pika-e2e".to_string(),
            command: vec!["just".to_string(), "checks::nightly-pika-e2e".to_string()],
            paths: vec![],
            concurrency_group: None,
            staged_linux_target: None,
        };
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "head-a",
                "2026-03-17T08:00:00Z",
                std::slice::from_ref(&lane),
            )
            .expect("queue first nightly");
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "head-b",
                "2026-03-18T08:00:00Z",
                std::slice::from_ref(&lane),
            )
            .expect("queue second nightly");
        let wrong_nightly = store
            .list_recent_nightly_runs(8)
            .expect("list nightly runs")
            .into_iter()
            .max_by_key(|run| run.nightly_run_id)
            .expect("other nightly");
        let lane_run_id = store
            .get_nightly_run(wrong_nightly.nightly_run_id)
            .expect("nightly detail")
            .expect("nightly run")
            .lanes[0]
            .id;
        let other_nightly_id = store
            .list_recent_nightly_runs(8)
            .expect("list nightly runs")
            .into_iter()
            .find(|run| run.nightly_run_id != wrong_nightly.nightly_run_id)
            .expect("mismatched nightly")
            .nightly_run_id;

        let headers = trusted_headers(&store, TRUSTED_NPUB);
        let state = test_state(store, forge_test_config());
        let response =
            fail_nightly_lane_handler(State(state), Path((other_nightly_id, lane_run_id)), headers)
                .await
                .into_response();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn wake_ci_handler_requires_trusted_access() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let state = test_state(store.clone(), forge_test_config());
        store
            .insert_auth_token("reader-token", "npub1reader")
            .expect("insert reader token");
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer reader-token"),
        );

        let response = wake_ci_handler(State(state), headers).await.into_response();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn completed_branch_ci_stream_returns_initial_snapshot_and_closes() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/live-branch", "head-live"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-live",
                &[crate::ci_manifest::ForgeLane {
                    id: "pika".to_string(),
                    title: "check-pika".to_string(),
                    entrypoint: "just checks::pre-merge-pika".to_string(),
                    command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                    staged_linux_target: None,
                }],
            )
            .expect("queue branch ci");
        let lane = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim branch lane")
            .into_iter()
            .next()
            .expect("branch lane");
        store
            .finish_branch_ci_lane_run(lane.lane_run_id, lane.claim_token, "success", "ok")
            .expect("finish branch lane");

        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: None,
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let state = test_state(store, config);
        let response = branch_ci_stream_handler(
            State(state),
            Path(branch.branch_id),
            Query(ReviewModeQuery::default()),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read branch stream body");
        let text = String::from_utf8(body.to_vec()).expect("decode branch stream");
        assert!(text.contains("event: ci-update"));
        assert!(text.contains("\"html\":"));
        assert!(text.contains("check-pika"));
        assert!(text.contains("CI: success"));
    }

    #[tokio::test]
    async fn completed_nightly_stream_returns_initial_snapshot_and_closes() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        let lane = crate::ci_manifest::ForgeLane {
            id: "nightly_pika".to_string(),
            title: "nightly-pika".to_string(),
            entrypoint: "just checks::nightly-pika-e2e".to_string(),
            command: vec!["just".to_string(), "checks::nightly-pika-e2e".to_string()],
            paths: vec![],
            concurrency_group: None,
            staged_linux_target: None,
        };
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "nightly-head",
                "2026-03-19T08:00:00Z",
                std::slice::from_ref(&lane),
            )
            .expect("queue nightly");
        let claimed = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim nightly lane")
            .into_iter()
            .next()
            .expect("nightly lane");
        store
            .finish_nightly_lane_run(claimed.lane_run_id, claimed.claim_token, "failed", "boom")
            .expect("finish nightly lane");

        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: None,
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let nightly_run_id = claimed.nightly_run_id;
        let state = test_state(store, config);
        let response = nightly_stream_handler(State(state), Path(nightly_run_id))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read nightly stream body");
        let text = String::from_utf8(body.to_vec()).expect("decode nightly stream");
        assert!(text.contains("event: ci-update"));
        assert!(text.contains("nightly-pika"));
        assert!(text.contains("nightly: failed"));
    }

    #[tokio::test]
    async fn branch_live_snapshot_html_updates_across_lane_transitions() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input(
                "feature/live-progress",
                "head-progress",
            ))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-progress",
                &[crate::ci_manifest::ForgeLane {
                    id: "pika".to_string(),
                    title: "check-pika".to_string(),
                    entrypoint: "just checks::pre-merge-pika".to_string(),
                    command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                    staged_linux_target: None,
                }],
            )
            .expect("queue ci");
        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: None,
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let state = test_state(store.clone(), config);

        let queued = load_branch_ci_live_snapshot(Arc::clone(&state), branch.branch_id)
            .await
            .expect("load queued snapshot")
            .expect("queued snapshot exists");
        assert!(queued.html.contains("CI: queued"));
        assert!(queued.html.contains("data-branch-ci-active=\"true\""));

        let claimed = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim branch lane")
            .into_iter()
            .next()
            .expect("lane");
        let running = load_branch_ci_live_snapshot(Arc::clone(&state), branch.branch_id)
            .await
            .expect("load running snapshot")
            .expect("running snapshot exists");
        assert!(running.html.contains("running"));
        assert!(running.html.contains("data-branch-ci-active=\"true\""));

        store
            .finish_branch_ci_lane_run(claimed.lane_run_id, claimed.claim_token, "success", "ok")
            .expect("finish lane");
        let finished = load_branch_ci_live_snapshot(state, branch.branch_id)
            .await
            .expect("load finished snapshot")
            .expect("finished snapshot exists");
        assert!(finished.html.contains("CI: success"));
        assert!(finished.html.contains("data-branch-ci-active=\"false\""));
    }

    #[tokio::test]
    async fn nightly_live_snapshot_html_updates_across_lane_transitions() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        let lane = crate::ci_manifest::ForgeLane {
            id: "nightly_pika".to_string(),
            title: "nightly-pika".to_string(),
            entrypoint: "just checks::nightly-pika-e2e".to_string(),
            command: vec!["just".to_string(), "checks::nightly-pika-e2e".to_string()],
            paths: vec![],
            concurrency_group: None,
            staged_linux_target: None,
        };
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "nightly-live-head",
                "2026-03-19T08:00:00Z",
                std::slice::from_ref(&lane),
            )
            .expect("queue nightly");
        let nightly_run_id = store
            .list_recent_nightly_runs(1)
            .expect("nightly feed")
            .into_iter()
            .next()
            .expect("nightly run")
            .nightly_run_id;
        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: None,
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let state = test_state(store.clone(), config);

        let queued = load_nightly_live_snapshot(Arc::clone(&state), nightly_run_id)
            .await
            .expect("load queued nightly")
            .expect("nightly exists");
        assert!(queued.html.contains("nightly: queued"));
        assert!(queued.html.contains("data-nightly-active=\"true\""));

        let claimed = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim nightly")
            .into_iter()
            .next()
            .expect("nightly lane");
        let running = load_nightly_live_snapshot(Arc::clone(&state), nightly_run_id)
            .await
            .expect("load running nightly")
            .expect("nightly exists");
        assert!(running.html.contains("running"));
        assert!(running.html.contains("data-nightly-active=\"true\""));

        store
            .finish_nightly_lane_run(claimed.lane_run_id, claimed.claim_token, "failed", "boom")
            .expect("finish nightly");
        let finished = load_nightly_live_snapshot(state, nightly_run_id)
            .await
            .expect("load finished nightly")
            .expect("nightly exists");
        assert!(finished.html.contains("nightly: failed"));
        assert!(finished.html.contains("data-nightly-active=\"false\""));
    }

    #[tokio::test]
    async fn branch_live_stream_recovers_with_fresh_snapshot_after_lag() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/live-lag", "head-lag"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-lag",
                &[crate::ci_manifest::ForgeLane {
                    id: "pika".to_string(),
                    title: "check-pika".to_string(),
                    entrypoint: "just checks::pre-merge-pika".to_string(),
                    command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                    staged_linux_target: None,
                }],
            )
            .expect("queue ci");
        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: None,
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let state = test_state_with_live_buffer(store.clone(), config, 1);
        let mut receiver = state.live_updates.subscribe();
        let claimed = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim branch lane")
            .into_iter()
            .next()
            .expect("claimed lane");
        store
            .finish_branch_ci_lane_run(claimed.lane_run_id, claimed.claim_token, "success", "ok")
            .expect("finish branch lane");

        state
            .live_updates
            .branch_changed(branch.branch_id, "lane_claimed");
        state
            .live_updates
            .branch_changed(branch.branch_id, "lane_finished");

        let snapshot = next_branch_ci_live_snapshot(&mut receiver, state, branch.branch_id)
            .await
            .expect("lagged snapshot");
        assert!(snapshot.html.contains("CI: success"));
        assert!(!snapshot.active);
    }

    #[tokio::test]
    async fn nightly_live_stream_recovers_with_fresh_snapshot_after_lag() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        let lane = crate::ci_manifest::ForgeLane {
            id: "nightly_pika".to_string(),
            title: "nightly-pika".to_string(),
            entrypoint: "just checks::nightly-pika-e2e".to_string(),
            command: vec!["just".to_string(), "checks::nightly-pika-e2e".to_string()],
            paths: vec![],
            concurrency_group: None,
            staged_linux_target: None,
        };
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "nightly-lag-head",
                "2026-03-19T08:00:00Z",
                std::slice::from_ref(&lane),
            )
            .expect("queue nightly");
        let nightly_run_id = store
            .list_recent_nightly_runs(1)
            .expect("nightly feed")
            .into_iter()
            .next()
            .expect("nightly run")
            .nightly_run_id;
        let config = Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: "/tmp/pika.git".to_string(),
                default_branch: "master".to_string(),
                ci_concurrency: None,
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
                hook_url: None,
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
        let state = test_state_with_live_buffer(store.clone(), config, 1);
        let mut receiver = state.live_updates.subscribe();
        let claimed = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim nightly")
            .into_iter()
            .next()
            .expect("claimed nightly lane");
        store
            .finish_nightly_lane_run(claimed.lane_run_id, claimed.claim_token, "failed", "boom")
            .expect("finish nightly lane");

        state
            .live_updates
            .nightly_changed(nightly_run_id, "lane_claimed");
        state
            .live_updates
            .nightly_changed(nightly_run_id, "lane_finished");

        let snapshot = next_nightly_live_snapshot(&mut receiver, state, nightly_run_id)
            .await
            .expect("lagged nightly snapshot");
        assert!(snapshot.html.contains("nightly: failed"));
        assert!(!snapshot.active);
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
                ci_concurrency: Some(2),
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                mirror_timeout_secs: None,
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
        let detail_rendered = render_detail_template(detail.clone(), ci_runs.clone(), false)
            .expect("render detail template")
            .render()
            .expect("render detail html");
        assert!(detail_rendered.contains("feature/render-history"));
        assert!(detail_rendered.contains("Open CI Details"));
        assert!(!detail_rendered.contains("branch-ci-ok"));
        assert!(detail_rendered.contains("Merge Commit"));

        let ci_rendered =
            render_branch_ci_template_with_notices(detail, ci_runs, Vec::new(), false)
                .expect("render branch ci template")
                .render()
                .expect("render branch ci html");
        assert!(ci_rendered.contains("branch-ci-ok"));
        assert!(ci_rendered.contains("Run History"));
    }

    #[test]
    fn branch_detail_renders_manual_rerun_provenance() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/rerun-ui", "head-rerun"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-rerun",
                &[crate::ci_manifest::ForgeLane {
                    id: "pika".to_string(),
                    title: "check-pika".to_string(),
                    entrypoint: "just checks::pre-merge-pika".to_string(),
                    command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                    staged_linux_target: None,
                }],
            )
            .expect("queue ci");
        let failed = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim ci")
            .into_iter()
            .next()
            .expect("ci lane");
        store
            .finish_branch_ci_lane_run(failed.lane_run_id, failed.claim_token, "failed", "boom")
            .expect("finish ci");
        store
            .rerun_branch_ci_lane(branch.branch_id, failed.lane_run_id)
            .expect("rerun ci")
            .expect("rerun suite");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let detail_rendered = render_detail_template(detail.clone(), ci_runs.clone(), false)
            .expect("render detail template")
            .render()
            .expect("render detail html");
        assert!(detail_rendered.contains("manual rerun of run #"));
        assert!(!detail_rendered.contains("manual rerun of lane #"));

        let ci_rendered =
            render_branch_ci_template_with_notices(detail, ci_runs, Vec::new(), false)
                .expect("render branch ci template")
                .render()
                .expect("render branch ci html");
        assert!(ci_rendered.contains("manual rerun of lane #"));
    }

    #[test]
    fn branch_detail_renders_pikaci_run_metadata() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/pikaci-ui", "head-pikaci"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-pikaci",
                &[crate::ci_manifest::ForgeLane {
                    id: "pika".to_string(),
                    title: "check-pika".to_string(),
                    entrypoint: "just checks::pre-merge-pika".to_string(),
                    command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                    staged_linux_target: Some("pre-merge-pika-rust".to_string()),
                }],
            )
            .expect("queue ci");
        let running = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim ci")
            .into_iter()
            .next()
            .expect("ci lane");
        store
            .record_branch_ci_lane_pikaci_run(
                running.lane_run_id,
                running.claim_token,
                "pikaci-run-branch-ui",
                Some("pre-merge-pika-rust"),
            )
            .expect("persist branch pikaci metadata");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let detail_rendered = render_detail_template(detail.clone(), ci_runs.clone(), false)
            .expect("render detail template")
            .render()
            .expect("render detail html");
        assert!(!detail_rendered.contains("pikaci run"));

        let ci_rendered =
            render_branch_ci_template_with_notices(detail, ci_runs, Vec::new(), false)
                .expect("render branch ci template")
                .render()
                .expect("render branch ci html");
        assert!(ci_rendered.contains("pikaci run"));
        assert!(ci_rendered.contains("pikaci-run-branch-ui"));
        assert!(ci_rendered.contains("pre-merge-pika-rust"));
    }

    #[test]
    fn branch_ci_page_renders_waiting_and_unhealthy_lane_state() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/state-ui", "head-state-ui"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-state-ui",
                &[
                    crate::ci_manifest::ForgeLane {
                        id: "wait-capacity".to_string(),
                        title: "wait-capacity".to_string(),
                        entrypoint: "just checks::wait-capacity".to_string(),
                        command: vec!["just".to_string(), "checks::wait-capacity".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                    crate::ci_manifest::ForgeLane {
                        id: "apple-sanity".to_string(),
                        title: "apple-sanity".to_string(),
                        entrypoint: "just checks::apple-sanity".to_string(),
                        command: vec!["just".to_string(), "checks::apple-sanity".to_string()],
                        paths: vec![],
                        concurrency_group: Some("apple-host".to_string()),
                        staged_linux_target: Some("apple-host".to_string()),
                    },
                ],
            )
            .expect("queue ci");
        store
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE branch_ci_run_lanes
                     SET execution_reason = 'waiting_for_capacity'
                     WHERE lane_id = 'wait-capacity'",
                    [],
                )?;
                conn.execute(
                    "UPDATE branch_ci_run_lanes
                     SET execution_reason = 'target_unhealthy'
                     WHERE lane_id = 'apple-sanity'",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO ci_target_health(
                        target_id,
                        state,
                        consecutive_infra_failure_count,
                        last_failure_at,
                        last_failure_kind,
                        cooloff_until,
                        updated_at
                     ) VALUES (?1, 'unhealthy', 2, CURRENT_TIMESTAMP, 'infrastructure', datetime('now', '+15 minutes'), CURRENT_TIMESTAMP)",
                    rusqlite::params!["apple-host"],
                )?;
                Ok::<(), anyhow::Error>(())
            })
            .expect("set waiting/unhealthy state");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let rendered = render_branch_ci_template_with_notices(detail, ci_runs, Vec::new(), false)
            .expect("render branch ci template")
            .render()
            .expect("render branch ci html");
        assert!(rendered.contains("waiting for scheduler capacity"));
        assert!(rendered.contains("target apple-host unhealthy"));
    }

    #[test]
    fn review_mode_ci_links_preserve_inbox_context() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/review-ci", "head-review-ci"))
            .expect("insert branch");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");

        let detail_rendered = render_detail_template(detail.clone(), ci_runs.clone(), true)
            .expect("render detail template")
            .render()
            .expect("render detail html");
        assert!(
            detail_rendered.contains(&format!("/news/branch/{}/ci?review=true", branch.branch_id))
        );

        let ci_rendered = render_branch_ci_template_with_notices(detail, ci_runs, Vec::new(), true)
            .expect("render branch ci template")
            .render()
            .expect("render branch ci html");
        assert!(ci_rendered.contains(&format!("href=\"/news/inbox/review/{}\"", branch.branch_id)));
    }

    #[test]
    fn review_mode_query_accepts_numeric_and_text_bools() {
        let uri_numeric: axum::http::Uri =
            "/news/branch/7/ci?review=1".parse().expect("numeric uri");
        let numeric =
            Query::<ReviewModeQuery>::try_from_uri(&uri_numeric).expect("numeric review query");
        assert!(numeric.0.review);

        let uri_text: axum::http::Uri = "/news/branch/7/ci?review=true".parse().expect("text uri");
        let text = Query::<ReviewModeQuery>::try_from_uri(&uri_text).expect("text review query");
        assert!(text.0.review);

        let uri_missing: axum::http::Uri = "/news/branch/7/ci".parse().expect("missing uri");
        let missing =
            Query::<ReviewModeQuery>::try_from_uri(&uri_missing).expect("missing review query");
        assert!(!missing.0.review);
    }

    #[test]
    fn branch_detail_distinguishes_global_generator_health_from_branch_failure() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input(
                "feature/tutorial-failure",
                "head-tutorial",
            ))
            .expect("insert branch");
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
            .mark_branch_generation_failed(artifact_id, "model output malformed", false, 0)
            .expect("mark tutorial failed");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let rendered = render_detail_template_with_notices(
            detail,
            ci_runs,
            false,
            vec![PageNoticeView {
                tone: "warning".to_string(),
                message: "The summary generator is unhealthy. New tutorials across the forge may be delayed until it recovers.".to_string(),
            }],
        )
        .expect("render detail template")
        .render()
        .expect("render detail html");

        assert!(rendered.contains("The summary generator is unhealthy."));
        assert!(rendered.contains("Branch Tutorial Generation Failed"));
        assert!(rendered.contains("This branch tutorial is unavailable because generation failed."));
    }

    #[test]
    fn branch_detail_places_diff_after_review_layout() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/full-width-diff", "head-diff"))
            .expect("insert branch");
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
                r#"{"executive_summary":"summary","steps":[],"media_links":[]}"#,
                "<p>ok</p>",
                "head-diff",
                "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n",
            )
            .expect("mark tutorial ready");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let rendered = render_detail_template(detail, ci_runs, false)
            .expect("render detail template")
            .render()
            .expect("render detail html");

        let review_layout = rendered
            .find("class=\"review-layout\"")
            .expect("review layout");
        let diff_row = rendered
            .find("class=\"panel diff-panel diff-row\"")
            .expect("diff row");
        assert!(diff_row > review_layout);
    }

    #[test]
    fn branch_detail_renders_skipped_lane_badges_in_summary_and_body() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/skipped-ui", "head-skipped"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-skipped",
                &[crate::ci_manifest::ForgeLane {
                    id: "pikachat_typescript".to_string(),
                    title: "check-pikachat-typescript".to_string(),
                    entrypoint:
                        "./scripts/pikaci-staged-linux-remote.sh run pre-merge-pikachat-typescript"
                            .to_string(),
                    command: vec![
                        "./scripts/pikaci-staged-linux-remote.sh".to_string(),
                        "run".to_string(),
                        "pre-merge-pikachat-typescript".to_string(),
                    ],
                    paths: vec![],
                    concurrency_group: Some(
                        "staged-linux:pre-merge-pikachat-typescript".to_string(),
                    ),
                    staged_linux_target: Some("pre-merge-pikachat-typescript".to_string()),
                }],
            )
            .expect("queue ci");
        let skipped = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim ci")
            .into_iter()
            .next()
            .expect("ci lane");
        store
            .finish_branch_ci_lane_run(
                skipped.lane_run_id,
                skipped.claim_token,
                "skipped",
                "skipped; no changed files matched target filters",
            )
            .expect("finish skipped lane");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let detail_rendered = render_detail_template(detail.clone(), ci_runs.clone(), false)
            .expect("render detail template")
            .render()
            .expect("render detail html");
        assert!(detail_rendered.contains("check-pikachat-typescript"));
        assert!(detail_rendered.contains("skipped"));

        let ci_rendered =
            render_branch_ci_template_with_notices(detail, ci_runs, Vec::new(), false)
                .expect("render branch ci template")
                .render()
                .expect("render branch ci html");
        assert!(ci_rendered.contains("check-pikachat-typescript"));
        assert!(ci_rendered.contains("skipped"));
    }

    #[test]
    fn nightly_page_renders_manual_rerun_provenance() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        let lane = crate::ci_manifest::ForgeLane {
            id: "nightly_pika".to_string(),
            title: "nightly-pika".to_string(),
            entrypoint: "just checks::nightly-pika-e2e".to_string(),
            command: vec!["just".to_string(), "checks::nightly-pika-e2e".to_string()],
            paths: vec![],
            concurrency_group: None,
            staged_linux_target: None,
        };
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "nightly-head",
                "2026-03-19T08:00:00Z",
                std::slice::from_ref(&lane),
            )
            .expect("queue nightly");
        let failed = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim nightly")
            .into_iter()
            .next()
            .expect("nightly lane");
        store
            .finish_nightly_lane_run(failed.lane_run_id, failed.claim_token, "failed", "boom")
            .expect("finish nightly");
        let rerun_run_id = store
            .rerun_nightly_lane(failed.nightly_run_id, failed.lane_run_id)
            .expect("rerun nightly")
            .expect("rerun run");

        let run = store
            .get_nightly_run(rerun_run_id)
            .expect("nightly detail")
            .expect("nightly run");
        let rendered = render_nightly_template(run)
            .render()
            .expect("render nightly html");
        assert!(rendered.contains("manual rerun of nightly #"));
        assert!(rendered.contains("manual rerun of lane #"));
    }

    #[test]
    fn nightly_page_renders_pikaci_run_metadata() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        let lane = crate::ci_manifest::ForgeLane {
            id: "nightly_pika".to_string(),
            title: "nightly-pika".to_string(),
            entrypoint: "just checks::nightly-pika-e2e".to_string(),
            command: vec!["just".to_string(), "checks::nightly-pika-e2e".to_string()],
            paths: vec![],
            concurrency_group: None,
            staged_linux_target: Some("pre-merge-pika-rust".to_string()),
        };
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "nightly-pikaci-head",
                "2026-03-19T08:00:00Z",
                std::slice::from_ref(&lane),
            )
            .expect("queue nightly");
        let running = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim nightly")
            .into_iter()
            .next()
            .expect("nightly lane");
        store
            .record_nightly_lane_pikaci_run(
                running.lane_run_id,
                running.claim_token,
                "pikaci-run-nightly-ui",
                Some("pre-merge-pika-rust"),
            )
            .expect("persist nightly pikaci metadata");

        let run = store
            .get_nightly_run(running.nightly_run_id)
            .expect("nightly detail")
            .expect("nightly run");
        let rendered = render_nightly_template(run)
            .render()
            .expect("render nightly html");
        assert!(rendered.contains("pikaci run"));
        assert!(rendered.contains("pikaci-run-nightly-ui"));
        assert!(rendered.contains("pre-merge-pika-rust"));
    }
}
