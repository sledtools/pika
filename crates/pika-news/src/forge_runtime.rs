use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use tokio::sync::Notify;

use crate::branch_store::MirrorStatusRecord;
use crate::ci;
use crate::config::{Config, ForgeRepoConfig};
use crate::forge;
use crate::live::CiLiveUpdates;
use crate::mirror;
use crate::poller;
use crate::storage::Store;
use crate::worker;

#[derive(Clone)]
pub(crate) struct ForgeRuntimeContext {
    pub(crate) store: Store,
    pub(crate) config: Config,
    pub(crate) max_prs: usize,
    pub(crate) live_updates: CiLiveUpdates,
    pub(crate) webhook_secret: Option<String>,
}

#[derive(Clone)]
pub(crate) struct ForgeRuntime {
    enabled: bool,
    wake_notify: Arc<Notify>,
    last_wake_reason: Arc<Mutex<Option<WakeReason>>>,
    mirror_requested: Arc<AtomicBool>,
    mirror_running: Arc<AtomicBool>,
    ci_running: Arc<AtomicBool>,
    forge_health: Arc<Mutex<ForgeHealthState>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WakeReason {
    Ci,
    CiFollowUp,
    ManualMirrorComplete,
    MirrorRequested,
    Webhook,
}

impl WakeReason {
    fn log_label(self) -> Option<&'static str> {
        match self {
            Self::Webhook => Some("webhook"),
            Self::Ci | Self::CiFollowUp | Self::ManualMirrorComplete | Self::MirrorRequested => {
                None
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ForgeHealthIssue {
    pub(crate) severity: String,
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct ForgeSubsystemStatus {
    pub(crate) state: String,
    pub(crate) last_checked_at: Option<String>,
    pub(crate) last_activity_at: Option<String>,
    pub(crate) last_error_at: Option<String>,
    pub(crate) summary: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct ForgeMirrorHealthStatus {
    pub(crate) state: String,
    pub(crate) background_enabled: bool,
    pub(crate) background_interval_secs: Option<u64>,
    pub(crate) last_success_at: Option<String>,
    pub(crate) last_failure_at: Option<String>,
    pub(crate) summary: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct ForgeHealthSnapshot {
    pub(crate) enabled: bool,
    pub(crate) issues: Vec<ForgeHealthIssue>,
    pub(crate) poller: ForgeSubsystemStatus,
    pub(crate) generation_worker: ForgeSubsystemStatus,
    pub(crate) ci: ForgeSubsystemStatus,
    pub(crate) mirror: ForgeMirrorHealthStatus,
}

#[derive(Clone, Debug)]
pub(crate) struct ForgeHealthState {
    enabled: bool,
    issues: Vec<ForgeHealthIssue>,
    poller: ForgeSubsystemTracker,
    generation_worker: ForgeSubsystemTracker,
    ci: ForgeSubsystemTracker,
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

pub(crate) enum ManualMirrorPassStatus {
    AlreadyRunning,
    Attempted(mirror::MirrorPassResult),
    Unavailable,
}

impl ForgeRuntime {
    pub(crate) fn new(config: &Config, webhook_secret: Option<&str>) -> Self {
        let enabled = config.effective_forge_repo().is_some();
        let runtime = Self::blank(enabled);
        runtime.replace_issues(current_forge_runtime_issues(config, webhook_secret));
        runtime
    }

    pub(crate) fn blank(enabled: bool) -> Self {
        Self {
            enabled,
            wake_notify: Arc::new(Notify::new()),
            last_wake_reason: Arc::new(Mutex::new(None)),
            mirror_requested: Arc::new(AtomicBool::new(false)),
            mirror_running: Arc::new(AtomicBool::new(false)),
            ci_running: Arc::new(AtomicBool::new(false)),
            forge_health: Arc::new(Mutex::new(ForgeHealthState::new(enabled))),
        }
    }

    pub(crate) fn issues(&self) -> Vec<ForgeHealthIssue> {
        self.forge_health
            .lock()
            .map(|health| health.issues.clone())
            .unwrap_or_default()
    }

    pub(crate) fn health_snapshot(
        &self,
        config: &Config,
        mirror_status: Option<&MirrorStatusRecord>,
    ) -> ForgeHealthSnapshot {
        self.forge_health
            .lock()
            .map(|health| health.snapshot(config, mirror_status))
            .unwrap_or_else(|_| ForgeHealthState::new(self.enabled).snapshot(config, mirror_status))
    }

    pub(crate) fn wake_ci(&self) {
        self.notify_with_reason(WakeReason::Ci);
    }

    pub(crate) fn wake_webhook(&self) {
        self.notify_with_reason(WakeReason::Webhook);
    }

    pub(crate) fn request_mirror(&self) {
        self.mirror_requested.store(true, Ordering::Release);
        self.notify_with_reason(WakeReason::MirrorRequested);
    }

    pub(crate) fn start_background(self: &Arc<Self>, context: ForgeRuntimeContext) {
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let iteration_context = context.clone();
                let iteration_runtime = Arc::clone(&runtime);
                match tokio::task::spawn_blocking(move || {
                    (
                        current_forge_runtime_issues(
                            &iteration_context.config,
                            iteration_context.webhook_secret.as_deref(),
                        ),
                        poller::poll_once_limited_with_updates(
                            &iteration_context.store,
                            &iteration_context.config,
                            iteration_context.max_prs,
                            Some(&iteration_context.live_updates),
                        ),
                        worker::run_generation_pass(
                            &iteration_context.store,
                            &iteration_context.config,
                        ),
                        iteration_runtime.run_scheduled_mirror_pass(
                            &iteration_context.store,
                            &iteration_context.config,
                        ),
                    )
                })
                .await
                {
                    Ok((issues, poll_result, worker_result, mirror_result)) => {
                        runtime.replace_issues(issues);
                        runtime.handle_poll_result(poll_result);
                        runtime.handle_worker_result(worker_result);
                        runtime.handle_mirror_result(mirror_result);
                    }
                    Err(err) => {
                        eprintln!("pika-news background task join error: {}", err);
                    }
                }

                runtime.maybe_start_background_ci_pass(context.clone());
                runtime
                    .wait_for_next_wake(Duration::from_secs(context.config.poll_interval_secs))
                    .await;
            }
        });
    }

    pub(crate) async fn run_manual_mirror_pass(
        &self,
        store: Store,
        config: Config,
    ) -> anyhow::Result<ManualMirrorPassStatus> {
        if self
            .mirror_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(ManualMirrorPassStatus::AlreadyRunning);
        }

        let result =
            tokio::task::spawn_blocking(move || mirror::run_mirror_pass(&store, &config, "manual"))
                .await;

        match result {
            Ok(Ok(result)) if result.attempted => {
                self.finish_manual_mirror_pass(true);
                Ok(ManualMirrorPassStatus::Attempted(result))
            }
            Ok(Ok(_)) => {
                self.finish_manual_mirror_pass(false);
                Ok(ManualMirrorPassStatus::Unavailable)
            }
            Ok(Err(err)) => {
                self.finish_manual_mirror_pass(false);
                Err(err)
            }
            Err(err) => {
                self.finish_manual_mirror_pass(false);
                Err(err.into())
            }
        }
    }

    fn replace_issues(&self, issues: Vec<ForgeHealthIssue>) {
        if let Ok(mut health) = self.forge_health.lock() {
            health.replace_issues(issues);
        }
    }

    fn maybe_start_background_ci_pass(self: &Arc<Self>, context: ForgeRuntimeContext) {
        if self
            .ci_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let ci_context = context.clone();
            let scheduler_notify = Arc::clone(&runtime.wake_notify);
            let ci_result = tokio::task::spawn_blocking(move || {
                ci::schedule_ci_pass_with_updates(
                    &ci_context.store,
                    &ci_context.config,
                    Some(&ci_context.live_updates),
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
                    if let Ok(mut health) = runtime.forge_health.lock() {
                        let active = ci.claimed > 0
                            || ci.nightlies_scheduled > 0
                            || ci.retries_recovered > 0;
                        health.ci.mark_success(ci_summary(&ci), active);
                    }
                    runtime.ci_running.store(false, Ordering::Release);
                    if should_wake_follow_up {
                        runtime.notify_with_reason(WakeReason::CiFollowUp);
                    }
                }
                Ok(Err(err)) => {
                    eprintln!("pika-news ci runner error: {}", err);
                    if let Ok(mut health) = runtime.forge_health.lock() {
                        health.ci.mark_error(err.to_string());
                    }
                    runtime.ci_running.store(false, Ordering::Release);
                }
                Err(err) => {
                    eprintln!("pika-news ci runner task join error: {}", err);
                    if let Ok(mut health) = runtime.forge_health.lock() {
                        health.ci.mark_error(err.to_string());
                    }
                    runtime.ci_running.store(false, Ordering::Release);
                }
            }
        });
    }

    fn run_scheduled_mirror_pass(
        &self,
        store: &Store,
        config: &Config,
    ) -> anyhow::Result<mirror::MirrorPassResult> {
        let force_requested = self.mirror_requested.load(Ordering::Acquire);
        let acquired = self
            .mirror_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if !acquired {
            return Ok(mirror::MirrorPassResult::default());
        }

        let result = if force_requested {
            self.mirror_requested.store(false, Ordering::Release);
            mirror::run_mirror_pass(store, config, "post-mutation")
        } else {
            mirror::run_background_mirror_pass(store, config)
        };

        self.mirror_running.store(false, Ordering::Release);
        result
    }

    fn finish_manual_mirror_pass(&self, attempted: bool) {
        self.mirror_running.store(false, Ordering::Release);
        if attempted && self.mirror_requested.load(Ordering::Acquire) {
            self.notify_with_reason(WakeReason::MirrorRequested);
        }
        if attempted {
            self.notify_with_reason(WakeReason::ManualMirrorComplete);
        }
    }

    fn handle_poll_result(&self, poll_result: anyhow::Result<poller::PollResult>) {
        match poll_result {
            Ok(pr) => {
                if pr.branches_seen > 0 || pr.queued_regenerations > 0 || pr.stale_closed > 0 {
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
                if let Ok(mut health) = self.forge_health.lock() {
                    let active = pr.queued_regenerations > 0
                        || pr.queued_ci_runs > 0
                        || pr.head_sha_changes > 0
                        || pr.stale_closed > 0;
                    health.poller.mark_success(poller_summary(&pr), active);
                }
                if pr.queued_ci_runs > 0 {
                    self.wake_ci();
                }
            }
            Err(err) => {
                eprintln!("pika-news background poller error: {}", err);
                if let Ok(mut health) = self.forge_health.lock() {
                    health.poller.mark_error(err.to_string());
                }
            }
        }
    }

    fn handle_worker_result(&self, worker_result: anyhow::Result<worker::WorkerPassResult>) {
        match worker_result {
            Ok(wr) => {
                if wr.claimed > 0 {
                    eprintln!(
                        "worker: claimed={} ready={} failed={} retry={}",
                        wr.claimed, wr.ready, wr.failed, wr.retry_scheduled
                    );
                }
                if let Ok(mut health) = self.forge_health.lock() {
                    let active =
                        wr.claimed > 0 || wr.ready > 0 || wr.failed > 0 || wr.retry_scheduled > 0;
                    health
                        .generation_worker
                        .mark_success(worker_summary(&wr), active);
                }
            }
            Err(err) => {
                eprintln!("pika-news background worker error: {}", err);
                if let Ok(mut health) = self.forge_health.lock() {
                    health.generation_worker.mark_error(err.to_string());
                }
            }
        }
    }

    fn handle_mirror_result(&self, mirror_result: anyhow::Result<mirror::MirrorPassResult>) {
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

    fn notify_with_reason(&self, reason: WakeReason) {
        if let Ok(mut wake_reason) = self.last_wake_reason.lock() {
            *wake_reason = Some(reason);
        }
        self.wake_notify.notify_one();
    }

    fn take_wake_reason(&self) -> Option<WakeReason> {
        self.last_wake_reason
            .lock()
            .ok()
            .and_then(|mut wake_reason| wake_reason.take())
    }

    async fn wait_for_next_wake(&self, poll_interval: Duration) {
        tokio::select! {
            _ = tokio::time::sleep(poll_interval) => {}
            _ = self.wake_notify.notified() => {
                if let Some(label) = self.take_wake_reason().and_then(WakeReason::log_label) {
                    eprintln!("poll: woken by {label}");
                }
            }
        }
    }
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
    pub(crate) fn new(enabled: bool) -> Self {
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

fn now_string() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
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

pub(crate) fn ci_pass_needs_follow_up_wake(ci: &ci::CiPassResult) -> bool {
    ci.claimed > 0
        || ci.succeeded > 0
        || ci.failed > 0
        || ci.nightlies_scheduled > 0
        || ci.retries_recovered > 0
}

pub(crate) fn build_mirror_health_status(
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

pub(crate) fn collect_forge_startup_issues(
    config: &Config,
    forge_repo: &ForgeRepoConfig,
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

pub(crate) fn current_forge_runtime_issues(
    config: &Config,
    webhook_secret: Option<&str>,
) -> Vec<ForgeHealthIssue> {
    let Some(forge_repo) = config.effective_forge_repo() else {
        return Vec::new();
    };
    collect_forge_startup_issues(config, &forge_repo, webhook_secret)
}
