use anyhow::{anyhow, Context};
use chrono::{DateTime, NaiveTime, Utc};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::sync::Notify;

use crate::ci_manifest::{self, ForgeCiManifest};
use crate::ci_state::CiLaneStatus;
use crate::ci_store::{PendingBranchCiLaneJob, PendingNightlyLaneJob, CI_LANE_LEASE_LOST};
use crate::config::Config;
use crate::forge;
use crate::live::CiLiveUpdates;
use crate::storage::Store;

pub const FORGE_LANE_MANIFEST_PATH: &str = "ci/forge-lanes.toml";
pub const CI_LANE_LEASE_SECS: u64 = 120;
pub const CI_LANE_HEARTBEAT_INTERVAL_SECS: u64 = 30;

#[derive(Debug, Default)]
pub struct CiPassResult {
    pub claimed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub nightlies_scheduled: usize,
    pub retries_recovered: usize,
}

#[derive(Debug)]
enum ClaimedLaneJob {
    Branch(PendingBranchCiLaneJob),
    Nightly(PendingNightlyLaneJob),
}

#[derive(Debug, Default)]
struct LaneJobOutcome {
    succeeded: usize,
    failed: usize,
}

pub fn load_manifest_from_default_branch(
    forge_repo: &crate::config::ForgeRepoConfig,
) -> anyhow::Result<ForgeCiManifest> {
    let default_ref = format!("refs/heads/{}", forge_repo.default_branch);
    load_manifest_at_ref(forge_repo, &default_ref)
}

pub fn load_manifest_for_head(
    forge_repo: &crate::config::ForgeRepoConfig,
    head_sha: &str,
) -> anyhow::Result<ForgeCiManifest> {
    load_manifest_at_ref(forge_repo, head_sha)
}

fn load_manifest_at_ref(
    forge_repo: &crate::config::ForgeRepoConfig,
    git_ref: &str,
) -> anyhow::Result<ForgeCiManifest> {
    let raw = forge::read_file_at_ref(forge_repo, git_ref, FORGE_LANE_MANIFEST_PATH)?;
    ci_manifest::parse_manifest(&raw)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn run_ci_pass(store: &Store, config: &Config) -> anyhow::Result<CiPassResult> {
    run_ci_pass_with_updates(store, config, None)
}

pub fn run_ci_pass_with_updates(
    store: &Store,
    config: &Config,
    live_updates: Option<&CiLiveUpdates>,
) -> anyhow::Result<CiPassResult> {
    run_ci_pass_with_timing(
        store,
        config,
        Duration::from_secs(CI_LANE_HEARTBEAT_INTERVAL_SECS),
        CI_LANE_LEASE_SECS,
        live_updates,
    )
}

pub fn schedule_ci_pass_with_updates(
    store: &Store,
    config: &Config,
    live_updates: Option<&CiLiveUpdates>,
    wake_notify: Option<Arc<Notify>>,
) -> anyhow::Result<CiPassResult> {
    schedule_ci_pass_with_timing_at(
        store,
        config,
        Duration::from_secs(CI_LANE_HEARTBEAT_INTERVAL_SECS),
        CI_LANE_LEASE_SECS,
        Utc::now(),
        live_updates,
        wake_notify,
    )
}

fn run_ci_pass_with_timing(
    store: &Store,
    config: &Config,
    heartbeat_interval: Duration,
    lease_secs: u64,
    live_updates: Option<&CiLiveUpdates>,
) -> anyhow::Result<CiPassResult> {
    run_ci_pass_with_timing_at(
        store,
        config,
        heartbeat_interval,
        lease_secs,
        Utc::now(),
        live_updates,
    )
}

fn run_ci_pass_with_timing_at(
    store: &Store,
    config: &Config,
    heartbeat_interval: Duration,
    lease_secs: u64,
    now: DateTime<Utc>,
    live_updates: Option<&CiLiveUpdates>,
) -> anyhow::Result<CiPassResult> {
    let Some(forge_repo) = config.effective_forge_repo() else {
        return Ok(CiPassResult::default());
    };
    let ci_concurrency = forge_repo.ci_concurrency.filter(|limit| *limit > 0);

    let mut result = CiPassResult {
        retries_recovered: store
            .recover_stale_ci_lanes()
            .context("recover stale ci lanes")?,
        ..CiPassResult::default()
    };

    let nightly_schedule_result = load_manifest_from_default_branch(&forge_repo)
        .and_then(|manifest| schedule_due_nightlies_at(store, &forge_repo, &manifest, now))
        .context("schedule due nightlies");
    if let Ok(nightlies_scheduled) = nightly_schedule_result.as_ref() {
        result.nightlies_scheduled = *nightlies_scheduled;
    }
    store
        .refresh_ci_lane_execution_reasons(ci_concurrency)
        .context("refresh ci execution reasons before ci pass")?;

    let (tx, rx) = mpsc::channel::<anyhow::Result<LaneJobOutcome>>();
    let mut active_workers = 0_usize;

    loop {
        while has_capacity(ci_concurrency, active_workers) {
            let claim_limit = next_claim_limit(store, ci_concurrency, active_workers)?;
            if claim_limit == 0 {
                store
                    .refresh_ci_lane_execution_reasons(ci_concurrency)
                    .context("refresh ci execution reasons with no available worker slots")?;
                break;
            }
            let claimed_jobs = claim_lane_jobs(store, lease_secs, claim_limit)?;
            if claimed_jobs.is_empty() {
                store
                    .refresh_ci_lane_execution_reasons(ci_concurrency)
                    .context("refresh ci execution reasons with no claimable lanes")?;
                break;
            }
            store
                .refresh_ci_lane_execution_reasons(ci_concurrency)
                .context("refresh ci execution reasons after claims")?;
            for job in claimed_jobs {
                result.claimed += 1;
                active_workers += 1;
                let store = store.clone();
                let forge_repo = forge_repo.clone();
                let tx = tx.clone();
                let live_updates = live_updates.cloned();
                match &job {
                    ClaimedLaneJob::Branch(job) => {
                        if let Some(live_updates) = live_updates.as_ref() {
                            live_updates.branch_changed(job.branch_id, "lane_claimed");
                        }
                    }
                    ClaimedLaneJob::Nightly(job) => {
                        if let Some(live_updates) = live_updates.as_ref() {
                            live_updates.nightly_changed(job.nightly_run_id, "lane_claimed");
                        }
                    }
                }
                thread::spawn(move || {
                    let outcome = match job {
                        ClaimedLaneJob::Branch(job) => execute_branch_job(
                            &store,
                            &forge_repo,
                            job,
                            heartbeat_interval,
                            lease_secs,
                            live_updates.clone(),
                        ),
                        ClaimedLaneJob::Nightly(job) => execute_nightly_job(
                            &store,
                            &forge_repo,
                            job,
                            heartbeat_interval,
                            lease_secs,
                            live_updates.clone(),
                        ),
                    };
                    let _ = tx.send(outcome);
                });
            }
        }

        if active_workers == 0 {
            break;
        }

        let outcome = rx.recv().context("wait for ci lane worker")??;
        active_workers -= 1;
        result.succeeded += outcome.succeeded;
        result.failed += outcome.failed;
        store
            .refresh_ci_lane_execution_reasons(ci_concurrency)
            .context("refresh ci execution reasons after worker completion")?;
    }

    nightly_schedule_result?;

    Ok(result)
}

fn schedule_ci_pass_with_timing_at(
    store: &Store,
    config: &Config,
    heartbeat_interval: Duration,
    lease_secs: u64,
    now: DateTime<Utc>,
    live_updates: Option<&CiLiveUpdates>,
    wake_notify: Option<Arc<Notify>>,
) -> anyhow::Result<CiPassResult> {
    let Some(forge_repo) = config.effective_forge_repo() else {
        return Ok(CiPassResult::default());
    };
    let ci_concurrency = forge_repo.ci_concurrency.filter(|limit| *limit > 0);

    let mut result = CiPassResult {
        retries_recovered: store
            .recover_stale_ci_lanes()
            .context("recover stale ci lanes")?,
        ..CiPassResult::default()
    };

    let nightly_schedule_result = load_manifest_from_default_branch(&forge_repo)
        .and_then(|manifest| schedule_due_nightlies_at(store, &forge_repo, &manifest, now))
        .context("schedule due nightlies");
    if let Ok(nightlies_scheduled) = nightly_schedule_result.as_ref() {
        result.nightlies_scheduled = *nightlies_scheduled;
    }
    store
        .refresh_ci_lane_execution_reasons(ci_concurrency)
        .context("refresh ci execution reasons before scheduler pass")?;

    let claim_limit = next_scheduler_claim_limit(store, ci_concurrency)?;
    if claim_limit > 0 {
        let claimed_jobs = claim_lane_jobs(store, lease_secs, claim_limit)?;
        result.claimed = claimed_jobs.len();
        store
            .refresh_ci_lane_execution_reasons(ci_concurrency)
            .context("refresh ci execution reasons after scheduler claims")?;
        for job in claimed_jobs {
            launch_claimed_job(
                store.clone(),
                forge_repo.clone(),
                job,
                heartbeat_interval,
                lease_secs,
                live_updates.cloned(),
                wake_notify.clone(),
            );
        }
    } else {
        store
            .refresh_ci_lane_execution_reasons(ci_concurrency)
            .context("refresh ci execution reasons with no scheduler capacity")?;
    }

    nightly_schedule_result?;

    Ok(result)
}

fn claim_lane_jobs(
    store: &Store,
    lease_secs: u64,
    available_slots: usize,
) -> anyhow::Result<Vec<ClaimedLaneJob>> {
    if available_slots == 0 {
        return Ok(Vec::new());
    }
    let mut jobs = Vec::new();
    let nightly_jobs = store
        .claim_pending_nightly_lane_runs(available_slots, lease_secs)
        .context("claim pending nightly lanes")?;
    jobs.extend(nightly_jobs.into_iter().map(ClaimedLaneJob::Nightly));
    if jobs.len() < available_slots {
        let branch_jobs = store
            .claim_pending_branch_ci_lane_runs(available_slots - jobs.len(), lease_secs)
            .context("claim pending branch ci lanes")?;
        jobs.extend(branch_jobs.into_iter().map(ClaimedLaneJob::Branch));
    }
    Ok(jobs)
}

fn launch_claimed_job(
    store: Store,
    forge_repo: crate::config::ForgeRepoConfig,
    job: ClaimedLaneJob,
    heartbeat_interval: Duration,
    lease_secs: u64,
    live_updates: Option<CiLiveUpdates>,
    wake_notify: Option<Arc<Notify>>,
) {
    match &job {
        ClaimedLaneJob::Branch(job) => {
            if let Some(live_updates) = live_updates.as_ref() {
                live_updates.branch_changed(job.branch_id, "lane_claimed");
            }
        }
        ClaimedLaneJob::Nightly(job) => {
            if let Some(live_updates) = live_updates.as_ref() {
                live_updates.nightly_changed(job.nightly_run_id, "lane_claimed");
            }
        }
    }
    thread::spawn(move || {
        let _ = match job {
            ClaimedLaneJob::Branch(job) => execute_branch_job(
                &store,
                &forge_repo,
                job,
                heartbeat_interval,
                lease_secs,
                live_updates.clone(),
            ),
            ClaimedLaneJob::Nightly(job) => execute_nightly_job(
                &store,
                &forge_repo,
                job,
                heartbeat_interval,
                lease_secs,
                live_updates.clone(),
            ),
        };
        if let Some(notify) = wake_notify.as_ref() {
            notify.notify_one();
        }
    });
}

fn has_capacity(ci_concurrency: Option<usize>, active_workers: usize) -> bool {
    match ci_concurrency {
        Some(limit) => active_workers < limit,
        None => true,
    }
}

fn next_claim_limit(
    store: &Store,
    ci_concurrency: Option<usize>,
    active_workers: usize,
) -> anyhow::Result<usize> {
    match ci_concurrency {
        Some(limit) => Ok(limit.saturating_sub(active_workers)),
        None => store.count_queued_ci_lane_runs(),
    }
}

fn next_scheduler_claim_limit(
    store: &Store,
    ci_concurrency: Option<usize>,
) -> anyhow::Result<usize> {
    match ci_concurrency {
        Some(limit) => {
            let running = store.count_running_ci_lane_runs()?;
            Ok(limit.saturating_sub(running))
        }
        None => store.count_queued_ci_lane_runs(),
    }
}

fn execute_branch_job(
    store: &Store,
    forge_repo: &crate::config::ForgeRepoConfig,
    job: PendingBranchCiLaneJob,
    heartbeat_interval: Duration,
    lease_secs: u64,
    live_updates: Option<CiLiveUpdates>,
) -> anyhow::Result<LaneJobOutcome> {
    let exec = forge::run_ci_command_for_head_with_heartbeat(
        forge_repo,
        &job.source_head_sha,
        &job.command,
        job.structured_pikaci_target_id.as_deref(),
        heartbeat_interval,
        || store.heartbeat_branch_ci_lane_run(job.lane_run_id, job.claim_token, lease_secs),
        |event| {
            match event {
                forge::CiExecutionEvent::PikaciRunStarted { run_id, target_id } => {
                    store.record_branch_ci_lane_pikaci_run(
                        job.lane_run_id,
                        job.claim_token,
                        &run_id,
                        target_id.as_deref(),
                    )?;
                    if let Some(live_updates) = live_updates.as_ref() {
                        live_updates.branch_changed(job.branch_id, "pikaci_run_started");
                    }
                }
            }
            Ok(())
        },
    )
    .with_context(|| {
        format!(
            "run branch lane {} for branch {}",
            job.lane_id, job.branch_id
        )
    });
    let mut outcome = LaneJobOutcome::default();
    match exec {
        Ok(exec) => {
            let status = if exec.success {
                CiLaneStatus::Success
            } else {
                CiLaneStatus::Failed
            };
            match store.finish_branch_ci_lane_run(
                job.lane_run_id,
                job.claim_token,
                status,
                &exec.log,
            ) {
                Ok(()) => {}
                Err(err) if is_lease_lost(&err) => return Ok(outcome),
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("persist branch lane result {}", job.lane_run_id))
                }
            }
            if exec.success {
                outcome.succeeded += 1;
            } else {
                outcome.failed += 1;
            }
            if let Some(live_updates) = live_updates.as_ref() {
                live_updates.branch_changed(job.branch_id, "lane_finished");
            }
        }
        Err(err) if is_lease_lost(&err) => return Ok(outcome),
        Err(err) => {
            let log = format!("ci runner error: {}", err);
            match store.finish_branch_ci_lane_run(
                job.lane_run_id,
                job.claim_token,
                CiLaneStatus::Failed,
                &log,
            ) {
                Ok(()) => {}
                Err(finish_err) if is_lease_lost(&finish_err) => return Ok(outcome),
                Err(finish_err) => {
                    return Err(finish_err).with_context(|| {
                        format!("persist branch lane failure {}", job.lane_run_id)
                    })
                }
            }
            outcome.failed += 1;
            if let Some(live_updates) = live_updates.as_ref() {
                live_updates.branch_changed(job.branch_id, "lane_finished");
            }
        }
    }
    Ok(outcome)
}

fn execute_nightly_job(
    store: &Store,
    forge_repo: &crate::config::ForgeRepoConfig,
    job: PendingNightlyLaneJob,
    heartbeat_interval: Duration,
    lease_secs: u64,
    live_updates: Option<CiLiveUpdates>,
) -> anyhow::Result<LaneJobOutcome> {
    let exec = forge::run_ci_command_for_head_with_heartbeat(
        forge_repo,
        &job.source_head_sha,
        &job.command,
        job.structured_pikaci_target_id.as_deref(),
        heartbeat_interval,
        || store.heartbeat_nightly_lane_run(job.lane_run_id, job.claim_token, lease_secs),
        |event| {
            match event {
                forge::CiExecutionEvent::PikaciRunStarted { run_id, target_id } => {
                    store.record_nightly_lane_pikaci_run(
                        job.lane_run_id,
                        job.claim_token,
                        &run_id,
                        target_id.as_deref(),
                    )?;
                    if let Some(live_updates) = live_updates.as_ref() {
                        live_updates.nightly_changed(job.nightly_run_id, "pikaci_run_started");
                    }
                }
            }
            Ok(())
        },
    )
    .with_context(|| {
        format!(
            "run nightly lane {} for nightly {}",
            job.lane_id, job.nightly_run_id
        )
    });
    let mut outcome = LaneJobOutcome::default();
    match exec {
        Ok(exec) => {
            let status = if exec.success {
                CiLaneStatus::Success
            } else {
                CiLaneStatus::Failed
            };
            match store.finish_nightly_lane_run(job.lane_run_id, job.claim_token, status, &exec.log)
            {
                Ok(()) => {}
                Err(err) if is_lease_lost(&err) => return Ok(outcome),
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!("persist nightly lane result {}", job.lane_run_id)
                    })
                }
            }
            if exec.success {
                outcome.succeeded += 1;
            } else {
                outcome.failed += 1;
            }
            if let Some(live_updates) = live_updates.as_ref() {
                live_updates.nightly_changed(job.nightly_run_id, "lane_finished");
            }
        }
        Err(err) if is_lease_lost(&err) => return Ok(outcome),
        Err(err) => {
            let log = format!("ci runner error: {}", err);
            match store.finish_nightly_lane_run(
                job.lane_run_id,
                job.claim_token,
                CiLaneStatus::Failed,
                &log,
            ) {
                Ok(()) => {}
                Err(finish_err) if is_lease_lost(&finish_err) => return Ok(outcome),
                Err(finish_err) => {
                    return Err(finish_err).with_context(|| {
                        format!("persist nightly lane failure {}", job.lane_run_id)
                    })
                }
            }
            outcome.failed += 1;
            if let Some(live_updates) = live_updates.as_ref() {
                live_updates.nightly_changed(job.nightly_run_id, "lane_finished");
            }
        }
    }
    Ok(outcome)
}

fn is_lease_lost(err: &anyhow::Error) -> bool {
    err.to_string().contains(CI_LANE_LEASE_LOST)
}

fn schedule_due_nightlies_at(
    store: &Store,
    forge_repo: &crate::config::ForgeRepoConfig,
    manifest: &ForgeCiManifest,
    now: DateTime<Utc>,
) -> anyhow::Result<usize> {
    let scheduled_for = current_scheduled_slot(manifest, now)?;
    if now < scheduled_for {
        return Ok(0);
    }
    let source_ref = format!("refs/heads/{}", forge_repo.default_branch);
    let source_head_sha = forge::current_branch_head(forge_repo, &forge_repo.default_branch)?
        .ok_or_else(|| {
            anyhow!(
                "default branch `{}` no longer exists",
                forge_repo.default_branch
            )
        })?;
    let repo_id = store.ensure_forge_repo_metadata(
        &forge_repo.repo,
        &forge_repo.canonical_git_dir,
        &forge_repo.default_branch,
        FORGE_LANE_MANIFEST_PATH,
    )?;
    let queued = store.queue_nightly_run(
        repo_id,
        &source_ref,
        &source_head_sha,
        &scheduled_for.to_rfc3339(),
        &manifest.nightly_lanes,
    )?;
    Ok(if queued { 1 } else { 0 })
}

fn current_scheduled_slot(
    manifest: &ForgeCiManifest,
    now: DateTime<Utc>,
) -> anyhow::Result<DateTime<Utc>> {
    let time = NaiveTime::from_hms_opt(manifest.nightly_hour_utc, manifest.nightly_minute_utc, 0)
        .ok_or_else(|| anyhow!("invalid nightly schedule time"))?;
    let slot = now.date_naive().and_time(time).and_utc();
    Ok(slot)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    use chrono::TimeZone;

    use super::{
        load_manifest_from_default_branch, run_ci_pass, run_ci_pass_with_timing,
        run_ci_pass_with_timing_at, schedule_due_nightlies_at,
    };
    use crate::ci_manifest::ForgeLane;
    use crate::ci_state::CiLaneStatus;
    use crate::config::{Config, ForgeRepoConfig};
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

    fn git_stdout<P: AsRef<std::path::Path>>(cwd: P, args: &[&str]) -> String {
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
        String::from_utf8(output.stdout)
            .expect("decode git stdout")
            .trim()
            .to_string()
    }

    fn wait_for_log_line_count(
        path: &std::path::Path,
        expected: usize,
        timeout: Duration,
    ) -> String {
        let start = std::time::Instant::now();
        loop {
            if let Ok(contents) = fs::read_to_string(path) {
                if contents.lines().count() >= expected {
                    return contents;
                }
            }
            assert!(
                start.elapsed() < timeout,
                "timed out waiting for {expected} log lines in {}",
                path.display()
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn wait_for_log_contains(path: &std::path::Path, needle: &str, timeout: Duration) -> String {
        let start = std::time::Instant::now();
        loop {
            if let Ok(contents) = fs::read_to_string(path) {
                if contents.contains(needle) {
                    return contents;
                }
            }
            assert!(
                start.elapsed() < timeout,
                "timed out waiting for `{needle}` in {}",
                path.display()
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn nightly_schedule_creates_durable_history_once_per_slot() {
        let root = tempfile::tempdir().expect("create temp root");
        let bare = root.path().join("pika.git");
        let seed = root.path().join("seed");
        let db_path = root.path().join("pika-git.db");

        git(
            root.path(),
            &["init", "--bare", bare.to_str().expect("bare path")],
        );
        git(root.path(), &["init", seed.to_str().expect("seed path")]);
        git(&seed, &["config", "user.name", "Test User"]);
        git(&seed, &["config", "user.email", "test@example.com"]);
        fs::create_dir_all(seed.join("ci")).expect("create ci dir");
        fs::write(seed.join("README.md"), "hello\n").expect("write readme");
        fs::write(
            seed.join("nightly.sh"),
            "#!/usr/bin/env bash\nset -euo pipefail\necho nightly-ok\n",
        )
        .expect("write nightly script");
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(seed.join("nightly.sh"))
            .expect("nightly metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(seed.join("nightly.sh"), perms).expect("chmod nightly script");
        fs::write(
            seed.join("ci/forge-lanes.toml"),
            r#"
version = 1
nightly_schedule_utc = "00:00"

[[nightly.lanes]]
id = "nightly_smoke"
title = "nightly smoke"
entrypoint = "./nightly.sh"
command = ["./nightly.sh"]
"#,
        )
        .expect("write nightly manifest");
        git(
            &seed,
            &["add", "README.md", "nightly.sh", "ci/forge-lanes.toml"],
        );
        git(&seed, &["commit", "-m", "initial"]);
        git(&seed, &["branch", "-M", "master"]);
        git(
            &seed,
            &["remote", "add", "origin", bare.to_str().expect("bare path")],
        );
        git(&seed, &["push", "origin", "master"]);

        let forge_repo = ForgeRepoConfig {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: bare.to_str().expect("bare path").to_string(),
            default_branch: "master".to_string(),
            ci_concurrency: Some(2),
            mirror_remote: None,
            mirror_poll_interval_secs: None,
            mirror_timeout_secs: None,
            ci_command: vec!["just".to_string(), "pre-merge".to_string()],
            hook_url: Some("http://127.0.0.1:9999/git/webhook".to_string()),
        };
        let manifest = load_manifest_from_default_branch(&forge_repo).expect("load manifest");
        let store = Store::open(&db_path).expect("open store");
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 3, 17, 9, 30, 0)
            .single()
            .expect("fixed timestamp");

        assert_eq!(
            schedule_due_nightlies_at(&store, &forge_repo, &manifest, now).expect("queue nightly"),
            1
        );
        assert_eq!(
            schedule_due_nightlies_at(&store, &forge_repo, &manifest, now).expect("dedupe nightly"),
            0
        );

        let feed = store.list_recent_nightly_runs(4).expect("nightly feed");
        assert_eq!(feed.len(), 1);
        let detail = store
            .get_nightly_run(feed[0].nightly_run_id)
            .expect("nightly detail")
            .expect("nightly exists");
        assert_eq!(detail.status, pika_forge_model::ForgeCiStatus::Queued);
        assert_eq!(detail.lanes.len(), 1);
        assert_eq!(detail.lanes[0].lane_id, "nightly_smoke");
    }

    #[test]
    fn run_ci_pass_drains_all_queued_branch_lanes_without_waiting_for_next_poll() {
        let root = tempfile::tempdir().expect("create temp root");
        let bare = root.path().join("pika.git");
        let seed = root.path().join("seed");
        let db_path = root.path().join("pika-git.db");

        git(
            root.path(),
            &["init", "--bare", bare.to_str().expect("bare path")],
        );
        git(root.path(), &["init", seed.to_str().expect("seed path")]);
        git(&seed, &["config", "user.name", "Test User"]);
        git(&seed, &["config", "user.email", "test@example.com"]);
        fs::create_dir_all(seed.join("ci")).expect("create ci dir");
        fs::write(seed.join("README.md"), "hello\n").expect("write readme");
        fs::write(
            seed.join("lane-a.sh"),
            "#!/usr/bin/env bash\nset -euo pipefail\necho lane-a\n",
        )
        .expect("write lane a script");
        fs::write(
            seed.join("lane-b.sh"),
            "#!/usr/bin/env bash\nset -euo pipefail\necho lane-b\n",
        )
        .expect("write lane b script");
        use std::os::unix::fs::PermissionsExt;
        for script in ["lane-a.sh", "lane-b.sh"] {
            let mut perms = fs::metadata(seed.join(script))
                .expect("script metadata")
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(seed.join(script), perms).expect("chmod ci script");
        }
        fs::write(
            seed.join("ci/forge-lanes.toml"),
            r#"
version = 1
nightly_schedule_utc = "08:00"
"#,
        )
        .expect("write manifest");
        git(
            &seed,
            &[
                "add",
                "README.md",
                "lane-a.sh",
                "lane-b.sh",
                "ci/forge-lanes.toml",
            ],
        );
        git(&seed, &["commit", "-m", "initial"]);
        git(&seed, &["branch", "-M", "master"]);
        git(
            &seed,
            &["remote", "add", "origin", bare.to_str().expect("bare path")],
        );
        git(&seed, &["push", "origin", "master"]);
        let head_sha = git_stdout(&seed, &["rev-parse", "HEAD"]);

        let forge_repo = ForgeRepoConfig {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: bare.to_str().expect("bare path").to_string(),
            default_branch: "master".to_string(),
            ci_concurrency: Some(2),
            mirror_remote: None,
            mirror_poll_interval_secs: None,
            mirror_timeout_secs: None,
            ci_command: vec!["just".to_string(), "pre-merge".to_string()],
            hook_url: Some("http://127.0.0.1:9999/git/webhook".to_string()),
        };
        let config = Config::test_with_forge_repo(forge_repo.clone());
        let store = Store::open(&db_path).expect("open store");
        let repo_id = store
            .ensure_forge_repo_metadata(
                &forge_repo.repo,
                &forge_repo.canonical_git_dir,
                &forge_repo.default_branch,
                super::FORGE_LANE_MANIFEST_PATH,
            )
            .expect("ensure repo metadata");
        let branch = store
            .upsert_branch_record(&crate::branch_store::BranchUpsertInput {
                repo: forge_repo.repo.clone(),
                canonical_git_dir: forge_repo.canonical_git_dir.clone(),
                default_branch: forge_repo.default_branch.clone(),
                ci_entrypoint: super::FORGE_LANE_MANIFEST_PATH.to_string(),
                branch_name: "feature/drain".to_string(),
                title: "feature/drain".to_string(),
                head_sha: head_sha.clone(),
                merge_base_sha: head_sha.clone(),
                author_name: None,
                author_email: None,
                updated_at: "2026-03-17T00:00:00Z".to_string(),
            })
            .expect("insert branch");
        assert_eq!(repo_id, 1);
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                &head_sha,
                &[
                    ForgeLane {
                        id: "lane_a".to_string(),
                        title: "lane a".to_string(),
                        entrypoint: "./lane-a.sh".to_string(),
                        command: vec!["./lane-a.sh".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                    ForgeLane {
                        id: "lane_b".to_string(),
                        title: "lane b".to_string(),
                        entrypoint: "./lane-b.sh".to_string(),
                        command: vec!["./lane-b.sh".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                ],
            )
            .expect("queue branch suite");

        let result = run_ci_pass(&store, &config).expect("run ci pass");
        assert_eq!(result.claimed, 2);
        assert_eq!(result.succeeded, 2);

        let runs = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list branch suites");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, pika_forge_model::ForgeCiStatus::Success);
        assert_eq!(runs[0].lanes.len(), 2);
        assert!(runs[0]
            .lanes
            .iter()
            .all(|lane| lane.status == CiLaneStatus::Success));
    }

    #[test]
    fn run_ci_pass_respects_configured_parallel_branch_cap() {
        let root = tempfile::tempdir().expect("create temp root");
        let bare = root.path().join("pika.git");
        let seed = root.path().join("seed");
        let db_path = root.path().join("pika-git.db");

        git(
            root.path(),
            &["init", "--bare", bare.to_str().expect("bare path")],
        );
        git(root.path(), &["init", seed.to_str().expect("seed path")]);
        git(&seed, &["config", "user.name", "Test User"]);
        git(&seed, &["config", "user.email", "test@example.com"]);
        fs::create_dir_all(seed.join("ci")).expect("create ci dir");
        fs::write(seed.join("README.md"), "hello\n").expect("write readme");
        let start_log = seed.join("starts.log");
        for script in ["lane-a.sh", "lane-b.sh", "lane-c.sh"] {
            fs::write(
                seed.join(script),
                format!(
                    "#!/usr/bin/env bash\nset -euo pipefail\necho {script} >> \"{}\"\nsleep 2\necho {script}-ok\n",
                    start_log.display()
                ),
            )
            .expect("write lane script");
        }
        use std::os::unix::fs::PermissionsExt;
        for script in ["lane-a.sh", "lane-b.sh", "lane-c.sh"] {
            let mut perms = fs::metadata(seed.join(script))
                .expect("script metadata")
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(seed.join(script), perms).expect("chmod ci script");
        }
        fs::write(
            seed.join("ci/forge-lanes.toml"),
            "version = 1\nnightly_schedule_utc = \"08:00\"\n",
        )
        .expect("write manifest");
        git(
            &seed,
            &[
                "add",
                "README.md",
                "lane-a.sh",
                "lane-b.sh",
                "lane-c.sh",
                "ci/forge-lanes.toml",
            ],
        );
        git(&seed, &["commit", "-m", "initial"]);
        git(&seed, &["branch", "-M", "master"]);
        git(
            &seed,
            &["remote", "add", "origin", bare.to_str().expect("bare path")],
        );
        git(&seed, &["push", "origin", "master"]);
        let head_sha = git_stdout(&seed, &["rev-parse", "HEAD"]);

        let forge_repo = ForgeRepoConfig {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: bare.to_str().expect("bare path").to_string(),
            default_branch: "master".to_string(),
            ci_concurrency: Some(2),
            mirror_remote: None,
            mirror_poll_interval_secs: None,
            mirror_timeout_secs: None,
            ci_command: vec!["just".to_string(), "pre-merge".to_string()],
            hook_url: Some("http://127.0.0.1:9999/git/webhook".to_string()),
        };
        let config = Config::test_with_forge_repo(forge_repo.clone());
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&crate::branch_store::BranchUpsertInput {
                repo: forge_repo.repo.clone(),
                canonical_git_dir: forge_repo.canonical_git_dir.clone(),
                default_branch: forge_repo.default_branch.clone(),
                ci_entrypoint: super::FORGE_LANE_MANIFEST_PATH.to_string(),
                branch_name: "feature/parallel-cap".to_string(),
                title: "feature/parallel-cap".to_string(),
                head_sha: head_sha.clone(),
                merge_base_sha: head_sha.clone(),
                author_name: None,
                author_email: None,
                updated_at: "2026-03-17T00:00:00Z".to_string(),
            })
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                &head_sha,
                &[
                    ForgeLane {
                        id: "lane_a".to_string(),
                        title: "lane a".to_string(),
                        entrypoint: "./lane-a.sh".to_string(),
                        command: vec!["./lane-a.sh".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                    ForgeLane {
                        id: "lane_b".to_string(),
                        title: "lane b".to_string(),
                        entrypoint: "./lane-b.sh".to_string(),
                        command: vec!["./lane-b.sh".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                    ForgeLane {
                        id: "lane_c".to_string(),
                        title: "lane c".to_string(),
                        entrypoint: "./lane-c.sh".to_string(),
                        command: vec!["./lane-c.sh".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                ],
            )
            .expect("queue branch suite");

        let runner_store = store.clone();
        let runner_config = config.clone();
        let handle = thread::spawn(move || run_ci_pass(&runner_store, &runner_config));

        let started = wait_for_log_line_count(&start_log, 2, Duration::from_secs(5));
        assert_eq!(started.lines().count(), 2);

        let result = handle.join().expect("join runner").expect("run ci pass");
        assert_eq!(result.claimed, 3);
        assert_eq!(result.succeeded, 3);
    }

    #[test]
    fn run_ci_pass_starts_all_claimable_branch_lanes_when_unbounded() {
        let root = tempfile::tempdir().expect("create temp root");
        let bare = root.path().join("pika.git");
        let seed = root.path().join("seed");
        let db_path = root.path().join("pika-git.db");

        git(
            root.path(),
            &["init", "--bare", bare.to_str().expect("bare path")],
        );
        git(root.path(), &["init", seed.to_str().expect("seed path")]);
        git(&seed, &["config", "user.name", "Test User"]);
        git(&seed, &["config", "user.email", "test@example.com"]);
        fs::create_dir_all(seed.join("ci")).expect("create ci dir");
        fs::write(seed.join("README.md"), "hello\n").expect("write readme");
        let start_log = seed.join("starts.log");
        for script in ["lane-a.sh", "lane-b.sh", "lane-c.sh"] {
            fs::write(
                seed.join(script),
                format!(
                    "#!/usr/bin/env bash\nset -euo pipefail\necho {script} >> \"{}\"\nsleep 2\necho {script}-ok\n",
                    start_log.display()
                ),
            )
            .expect("write lane script");
        }
        use std::os::unix::fs::PermissionsExt;
        for script in ["lane-a.sh", "lane-b.sh", "lane-c.sh"] {
            let mut perms = fs::metadata(seed.join(script))
                .expect("script metadata")
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(seed.join(script), perms).expect("chmod ci script");
        }
        fs::write(
            seed.join("ci/forge-lanes.toml"),
            "version = 1\nnightly_schedule_utc = \"08:00\"\n",
        )
        .expect("write manifest");
        git(
            &seed,
            &[
                "add",
                "README.md",
                "lane-a.sh",
                "lane-b.sh",
                "lane-c.sh",
                "ci/forge-lanes.toml",
            ],
        );
        git(&seed, &["commit", "-m", "initial"]);
        git(&seed, &["branch", "-M", "master"]);
        git(
            &seed,
            &["remote", "add", "origin", bare.to_str().expect("bare path")],
        );
        git(&seed, &["push", "origin", "master"]);
        let head_sha = git_stdout(&seed, &["rev-parse", "HEAD"]);

        let forge_repo = ForgeRepoConfig {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: bare.to_str().expect("bare path").to_string(),
            default_branch: "master".to_string(),
            ci_concurrency: None,
            mirror_remote: None,
            mirror_poll_interval_secs: None,
            mirror_timeout_secs: None,
            ci_command: vec!["just".to_string(), "pre-merge".to_string()],
            hook_url: Some("http://127.0.0.1:9999/git/webhook".to_string()),
        };
        let config = Config::test_with_forge_repo(forge_repo.clone());
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&crate::branch_store::BranchUpsertInput {
                repo: forge_repo.repo.clone(),
                canonical_git_dir: forge_repo.canonical_git_dir.clone(),
                default_branch: forge_repo.default_branch.clone(),
                ci_entrypoint: super::FORGE_LANE_MANIFEST_PATH.to_string(),
                branch_name: "feature/unbounded".to_string(),
                title: "feature/unbounded".to_string(),
                head_sha: head_sha.clone(),
                merge_base_sha: head_sha.clone(),
                author_name: None,
                author_email: None,
                updated_at: "2026-03-17T00:00:00Z".to_string(),
            })
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                &head_sha,
                &[
                    ForgeLane {
                        id: "lane_a".to_string(),
                        title: "lane a".to_string(),
                        entrypoint: "./lane-a.sh".to_string(),
                        command: vec!["./lane-a.sh".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                    ForgeLane {
                        id: "lane_b".to_string(),
                        title: "lane b".to_string(),
                        entrypoint: "./lane-b.sh".to_string(),
                        command: vec!["./lane-b.sh".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                    ForgeLane {
                        id: "lane_c".to_string(),
                        title: "lane c".to_string(),
                        entrypoint: "./lane-c.sh".to_string(),
                        command: vec!["./lane-c.sh".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                ],
            )
            .expect("queue branch suite");

        let runner_store = store.clone();
        let runner_config = config.clone();
        let handle = thread::spawn(move || run_ci_pass(&runner_store, &runner_config));

        let started = wait_for_log_line_count(&start_log, 3, Duration::from_secs(5));
        assert_eq!(started.lines().count(), 3);

        let result = handle.join().expect("join runner").expect("run ci pass");
        assert_eq!(result.claimed, 3);
        assert_eq!(result.succeeded, 3);
    }

    #[test]
    fn scheduler_pass_returns_while_running_lane_does_not_block_later_claims() {
        let root = tempfile::tempdir().expect("create temp root");
        let bare = root.path().join("pika.git");
        let seed = root.path().join("seed");
        let db_path = root.path().join("pika-git.db");

        git(
            root.path(),
            &["init", "--bare", bare.to_str().expect("bare path")],
        );
        git(root.path(), &["init", seed.to_str().expect("seed path")]);
        git(&seed, &["config", "user.name", "Test User"]);
        git(&seed, &["config", "user.email", "test@example.com"]);
        fs::create_dir_all(seed.join("ci")).expect("create ci dir");
        fs::write(seed.join("README.md"), "hello\n").expect("write readme");
        let start_log = seed.join("starts.log");
        fs::write(
            seed.join("slow.sh"),
            format!(
                "#!/usr/bin/env bash\nset -euo pipefail\necho slow >> \"{}\"\nsleep 3\necho slow-ok\n",
                start_log.display()
            ),
        )
        .expect("write slow lane");
        fs::write(
            seed.join("fast.sh"),
            format!(
                "#!/usr/bin/env bash\nset -euo pipefail\necho fast >> \"{}\"\necho fast-ok\n",
                start_log.display()
            ),
        )
        .expect("write fast lane");
        use std::os::unix::fs::PermissionsExt;
        for script in ["slow.sh", "fast.sh"] {
            let mut perms = fs::metadata(seed.join(script))
                .expect("script metadata")
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(seed.join(script), perms).expect("chmod ci script");
        }
        fs::write(
            seed.join("ci/forge-lanes.toml"),
            "version = 1\nnightly_schedule_utc = \"08:00\"\n",
        )
        .expect("write manifest");
        git(
            &seed,
            &[
                "add",
                "README.md",
                "slow.sh",
                "fast.sh",
                "ci/forge-lanes.toml",
            ],
        );
        git(&seed, &["commit", "-m", "initial"]);
        git(&seed, &["branch", "-M", "master"]);
        git(
            &seed,
            &["remote", "add", "origin", bare.to_str().expect("bare path")],
        );
        git(&seed, &["push", "origin", "master"]);
        let head_sha = git_stdout(&seed, &["rev-parse", "HEAD"]);

        let forge_repo = ForgeRepoConfig {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: bare.to_str().expect("bare path").to_string(),
            default_branch: "master".to_string(),
            ci_concurrency: None,
            mirror_remote: None,
            mirror_poll_interval_secs: None,
            mirror_timeout_secs: None,
            ci_command: vec!["just".to_string(), "pre-merge".to_string()],
            hook_url: Some("http://127.0.0.1:9999/git/webhook".to_string()),
        };
        let config = Config::test_with_forge_repo(forge_repo.clone());
        let store = Store::open(&db_path).expect("open store");

        let slow_branch = store
            .upsert_branch_record(&crate::branch_store::BranchUpsertInput {
                repo: forge_repo.repo.clone(),
                canonical_git_dir: forge_repo.canonical_git_dir.clone(),
                default_branch: forge_repo.default_branch.clone(),
                ci_entrypoint: super::FORGE_LANE_MANIFEST_PATH.to_string(),
                branch_name: "feature/slow".to_string(),
                title: "feature/slow".to_string(),
                head_sha: head_sha.clone(),
                merge_base_sha: head_sha.clone(),
                author_name: None,
                author_email: None,
                updated_at: "2026-03-17T00:00:00Z".to_string(),
            })
            .expect("insert slow branch");
        store
            .queue_branch_ci_run_for_head(
                slow_branch.branch_id,
                &head_sha,
                &[ForgeLane {
                    id: "slow".to_string(),
                    title: "slow".to_string(),
                    entrypoint: "./slow.sh".to_string(),
                    command: vec!["./slow.sh".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                    staged_linux_target: None,
                }],
            )
            .expect("queue slow suite");

        let first = super::schedule_ci_pass_with_updates(&store, &config, None, None)
            .expect("run first scheduler pass");
        assert_eq!(first.claimed, 1);
        let started = wait_for_log_contains(&start_log, "slow", Duration::from_secs(5));
        assert!(started.contains("slow"));

        let fast_branch = store
            .upsert_branch_record(&crate::branch_store::BranchUpsertInput {
                repo: forge_repo.repo.clone(),
                canonical_git_dir: forge_repo.canonical_git_dir.clone(),
                default_branch: forge_repo.default_branch.clone(),
                ci_entrypoint: super::FORGE_LANE_MANIFEST_PATH.to_string(),
                branch_name: "feature/fast".to_string(),
                title: "feature/fast".to_string(),
                head_sha: head_sha.clone(),
                merge_base_sha: head_sha.clone(),
                author_name: None,
                author_email: None,
                updated_at: "2026-03-17T00:00:00Z".to_string(),
            })
            .expect("insert fast branch");
        store
            .queue_branch_ci_run_for_head(
                fast_branch.branch_id,
                &head_sha,
                &[ForgeLane {
                    id: "fast".to_string(),
                    title: "fast".to_string(),
                    entrypoint: "./fast.sh".to_string(),
                    command: vec!["./fast.sh".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                    staged_linux_target: None,
                }],
            )
            .expect("queue fast suite");

        let second = super::schedule_ci_pass_with_updates(&store, &config, None, None)
            .expect("run second scheduler pass");
        assert_eq!(second.claimed, 1);
        let started = wait_for_log_contains(&start_log, "fast", Duration::from_secs(5));
        assert!(started.contains("slow"));
        assert!(started.contains("fast"));

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let slow_runs = store
                .list_branch_ci_runs(slow_branch.branch_id, 2)
                .expect("list slow runs");
            let fast_runs = store
                .list_branch_ci_runs(fast_branch.branch_id, 2)
                .expect("list fast runs");
            if slow_runs[0].status == pika_forge_model::ForgeCiStatus::Success
                && fast_runs[0].status == pika_forge_model::ForgeCiStatus::Success
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "scheduler-launched lanes did not finish in time"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn run_ci_pass_refills_parallel_slots_when_a_lane_finishes_early() {
        let root = tempfile::tempdir().expect("create temp root");
        let bare = root.path().join("pika.git");
        let seed = root.path().join("seed");
        let db_path = root.path().join("pika-git.db");

        git(
            root.path(),
            &["init", "--bare", bare.to_str().expect("bare path")],
        );
        git(root.path(), &["init", seed.to_str().expect("seed path")]);
        git(&seed, &["config", "user.name", "Test User"]);
        git(&seed, &["config", "user.email", "test@example.com"]);
        fs::create_dir_all(seed.join("ci")).expect("create ci dir");
        fs::write(seed.join("README.md"), "hello\n").expect("write readme");
        let start_log = seed.join("starts.log");
        fs::write(
            seed.join("slow.sh"),
            format!(
                "#!/usr/bin/env bash\nset -euo pipefail\necho slow >> \"{}\"\nsleep 3\necho slow-ok\n",
                start_log.display()
            ),
        )
        .expect("write slow script");
        fs::write(
            seed.join("fast-fail.sh"),
            format!(
                "#!/usr/bin/env bash\nset -euo pipefail\necho fast-fail >> \"{}\"\necho fail >&2\nexit 1\n",
                start_log.display()
            ),
        )
        .expect("write fast fail script");
        fs::write(
            seed.join("fast-ok.sh"),
            format!(
                "#!/usr/bin/env bash\nset -euo pipefail\necho fast-ok >> \"{}\"\necho fast-ok\n",
                start_log.display()
            ),
        )
        .expect("write fast ok script");
        use std::os::unix::fs::PermissionsExt;
        for script in ["slow.sh", "fast-fail.sh", "fast-ok.sh"] {
            let mut perms = fs::metadata(seed.join(script))
                .expect("script metadata")
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(seed.join(script), perms).expect("chmod script");
        }
        fs::write(
            seed.join("ci/forge-lanes.toml"),
            "version = 1\nnightly_schedule_utc = \"08:00\"\n",
        )
        .expect("write manifest");
        git(
            &seed,
            &[
                "add",
                "README.md",
                "slow.sh",
                "fast-fail.sh",
                "fast-ok.sh",
                "ci/forge-lanes.toml",
            ],
        );
        git(&seed, &["commit", "-m", "initial"]);
        git(&seed, &["branch", "-M", "master"]);
        git(
            &seed,
            &["remote", "add", "origin", bare.to_str().expect("bare path")],
        );
        git(&seed, &["push", "origin", "master"]);
        let head_sha = git_stdout(&seed, &["rev-parse", "HEAD"]);

        let forge_repo = ForgeRepoConfig {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: bare.to_str().expect("bare path").to_string(),
            default_branch: "master".to_string(),
            ci_concurrency: Some(2),
            mirror_remote: None,
            mirror_poll_interval_secs: None,
            mirror_timeout_secs: None,
            ci_command: vec!["just".to_string(), "pre-merge".to_string()],
            hook_url: Some("http://127.0.0.1:9999/git/webhook".to_string()),
        };
        let config = Config::test_with_forge_repo(forge_repo.clone());
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&crate::branch_store::BranchUpsertInput {
                repo: forge_repo.repo.clone(),
                canonical_git_dir: forge_repo.canonical_git_dir.clone(),
                default_branch: forge_repo.default_branch.clone(),
                ci_entrypoint: super::FORGE_LANE_MANIFEST_PATH.to_string(),
                branch_name: "feature/refill".to_string(),
                title: "feature/refill".to_string(),
                head_sha: head_sha.clone(),
                merge_base_sha: head_sha.clone(),
                author_name: None,
                author_email: None,
                updated_at: "2026-03-17T00:00:00Z".to_string(),
            })
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                &head_sha,
                &[
                    ForgeLane {
                        id: "slow".to_string(),
                        title: "slow".to_string(),
                        entrypoint: "./slow.sh".to_string(),
                        command: vec!["./slow.sh".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                    ForgeLane {
                        id: "fast_fail".to_string(),
                        title: "fast fail".to_string(),
                        entrypoint: "./fast-fail.sh".to_string(),
                        command: vec!["./fast-fail.sh".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                    ForgeLane {
                        id: "fast_ok".to_string(),
                        title: "fast ok".to_string(),
                        entrypoint: "./fast-ok.sh".to_string(),
                        command: vec!["./fast-ok.sh".to_string()],
                        paths: vec![],
                        concurrency_group: None,
                        staged_linux_target: None,
                    },
                ],
            )
            .expect("queue branch suite");

        let runner_store = store.clone();
        let runner_config = config.clone();
        let handle = thread::spawn(move || run_ci_pass(&runner_store, &runner_config));

        let started = wait_for_log_contains(&start_log, "fast-ok", Duration::from_secs(5));
        assert!(started.contains("slow"));
        assert!(started.contains("fast-fail"));
        assert!(started.contains("fast-ok"));

        let result = handle.join().expect("join runner").expect("run ci pass");
        assert_eq!(result.claimed, 3);
        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 1);
        let runs = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list branch runs");
        assert_eq!(runs[0].status, pika_forge_model::ForgeCiStatus::Failed);
        assert!(runs[0]
            .lanes
            .iter()
            .all(|lane| matches!(lane.status.as_str(), "success" | "failed")));
    }

    #[test]
    fn live_heartbeat_prevents_long_running_lane_from_being_requeued() {
        let root = tempfile::tempdir().expect("create temp root");
        let bare = root.path().join("pika.git");
        let seed = root.path().join("seed");
        let db_path = root.path().join("pika-git.db");

        git(
            root.path(),
            &["init", "--bare", bare.to_str().expect("bare path")],
        );
        git(root.path(), &["init", seed.to_str().expect("seed path")]);
        git(&seed, &["config", "user.name", "Test User"]);
        git(&seed, &["config", "user.email", "test@example.com"]);
        fs::create_dir_all(seed.join("ci")).expect("create ci dir");
        fs::write(seed.join("README.md"), "hello\n").expect("write readme");
        fs::write(
            seed.join("slow-lane.sh"),
            "#!/usr/bin/env bash\nset -euo pipefail\nsleep 4\necho slow-ok\n",
        )
        .expect("write slow lane script");
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(seed.join("slow-lane.sh"))
            .expect("slow lane metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(seed.join("slow-lane.sh"), perms).expect("chmod slow lane script");
        fs::write(
            seed.join("ci/forge-lanes.toml"),
            r#"
version = 1
nightly_schedule_utc = "08:00"
"#,
        )
        .expect("write manifest");
        git(
            &seed,
            &["add", "README.md", "slow-lane.sh", "ci/forge-lanes.toml"],
        );
        git(&seed, &["commit", "-m", "initial"]);
        git(&seed, &["branch", "-M", "master"]);
        git(
            &seed,
            &["remote", "add", "origin", bare.to_str().expect("bare path")],
        );
        git(&seed, &["push", "origin", "master"]);
        let head_sha = git_stdout(&seed, &["rev-parse", "HEAD"]);

        let forge_repo = ForgeRepoConfig {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: bare.to_str().expect("bare path").to_string(),
            default_branch: "master".to_string(),
            ci_concurrency: Some(2),
            mirror_remote: None,
            mirror_poll_interval_secs: None,
            mirror_timeout_secs: None,
            ci_command: vec!["just".to_string(), "pre-merge".to_string()],
            hook_url: Some("http://127.0.0.1:9999/git/webhook".to_string()),
        };
        let config = Config::test_with_forge_repo(forge_repo.clone());
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&crate::branch_store::BranchUpsertInput {
                repo: forge_repo.repo.clone(),
                canonical_git_dir: forge_repo.canonical_git_dir.clone(),
                default_branch: forge_repo.default_branch.clone(),
                ci_entrypoint: super::FORGE_LANE_MANIFEST_PATH.to_string(),
                branch_name: "feature/heartbeat".to_string(),
                title: "feature/heartbeat".to_string(),
                head_sha: head_sha.clone(),
                merge_base_sha: head_sha.clone(),
                author_name: None,
                author_email: None,
                updated_at: "2026-03-17T00:00:00Z".to_string(),
            })
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                &head_sha,
                &[ForgeLane {
                    id: "slow_lane".to_string(),
                    title: "slow lane".to_string(),
                    entrypoint: "./slow-lane.sh".to_string(),
                    command: vec!["./slow-lane.sh".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                    staged_linux_target: None,
                }],
            )
            .expect("queue branch suite");

        let runner_store = store.clone();
        let runner_config = config.clone();
        let handle = thread::spawn(move || {
            run_ci_pass_with_timing(
                &runner_store,
                &runner_config,
                Duration::from_secs(1),
                2,
                None,
            )
            .expect("run ci pass with heartbeat")
        });

        thread::sleep(Duration::from_millis(2600));
        assert_eq!(
            store.recover_stale_ci_lanes().expect("recover stale work"),
            0
        );

        let result = handle.join().expect("join runner");
        assert_eq!(result.succeeded, 1);
        let runs = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list branch runs");
        assert_eq!(runs[0].status, pika_forge_model::ForgeCiStatus::Success);
        assert_eq!(runs[0].lanes[0].retry_count, 0);
    }

    #[test]
    fn due_nightly_is_queued_and_run_in_same_ci_pass() {
        let root = tempfile::tempdir().expect("create temp root");
        let bare = root.path().join("pika.git");
        let seed = root.path().join("seed");
        let db_path = root.path().join("pika-git.db");

        git(
            root.path(),
            &["init", "--bare", bare.to_str().expect("bare path")],
        );
        git(root.path(), &["init", seed.to_str().expect("seed path")]);
        git(&seed, &["config", "user.name", "Test User"]);
        git(&seed, &["config", "user.email", "test@example.com"]);
        fs::create_dir_all(seed.join("ci")).expect("create ci dir");
        fs::write(seed.join("README.md"), "hello\n").expect("write readme");
        fs::write(
            seed.join("branch-lane.sh"),
            "#!/usr/bin/env bash\nset -euo pipefail\necho branch-ok\n",
        )
        .expect("write branch lane script");
        fs::write(
            seed.join("nightly.sh"),
            "#!/usr/bin/env bash\nset -euo pipefail\necho nightly-ok\n",
        )
        .expect("write nightly script");
        use std::os::unix::fs::PermissionsExt;
        for script in ["branch-lane.sh", "nightly.sh"] {
            let mut perms = fs::metadata(seed.join(script))
                .expect("script metadata")
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(seed.join(script), perms).expect("chmod script");
        }
        fs::write(
            seed.join("ci/forge-lanes.toml"),
            r#"
version = 1
nightly_schedule_utc = "08:00"

[[nightly.lanes]]
id = "nightly_smoke"
title = "nightly smoke"
entrypoint = "./nightly.sh"
command = ["./nightly.sh"]
"#,
        )
        .expect("write manifest");
        git(
            &seed,
            &[
                "add",
                "README.md",
                "branch-lane.sh",
                "nightly.sh",
                "ci/forge-lanes.toml",
            ],
        );
        git(&seed, &["commit", "-m", "initial"]);
        git(&seed, &["branch", "-M", "master"]);
        git(
            &seed,
            &["remote", "add", "origin", bare.to_str().expect("bare path")],
        );
        git(&seed, &["push", "origin", "master"]);
        let head_sha = git_stdout(&seed, &["rev-parse", "HEAD"]);

        let forge_repo = ForgeRepoConfig {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: bare.to_str().expect("bare path").to_string(),
            default_branch: "master".to_string(),
            ci_concurrency: Some(2),
            mirror_remote: None,
            mirror_poll_interval_secs: None,
            mirror_timeout_secs: None,
            ci_command: vec!["just".to_string(), "pre-merge".to_string()],
            hook_url: Some("http://127.0.0.1:9999/git/webhook".to_string()),
        };
        let config = Config::test_with_forge_repo(forge_repo.clone());
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&crate::branch_store::BranchUpsertInput {
                repo: forge_repo.repo.clone(),
                canonical_git_dir: forge_repo.canonical_git_dir.clone(),
                default_branch: forge_repo.default_branch.clone(),
                ci_entrypoint: super::FORGE_LANE_MANIFEST_PATH.to_string(),
                branch_name: "feature/nightly-priority".to_string(),
                title: "feature/nightly-priority".to_string(),
                head_sha: head_sha.clone(),
                merge_base_sha: head_sha.clone(),
                author_name: None,
                author_email: None,
                updated_at: "2026-03-17T00:00:00Z".to_string(),
            })
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                &head_sha,
                &[ForgeLane {
                    id: "branch_lane".to_string(),
                    title: "branch lane".to_string(),
                    entrypoint: "./branch-lane.sh".to_string(),
                    command: vec!["./branch-lane.sh".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                    staged_linux_target: None,
                }],
            )
            .expect("queue branch suite");

        let now = chrono::Utc
            .with_ymd_and_hms(2026, 3, 17, 9, 30, 0)
            .single()
            .expect("fixed timestamp");
        let result =
            run_ci_pass_with_timing_at(&store, &config, Duration::from_secs(1), 2, now, None)
                .expect("run ci pass");
        assert_eq!(result.nightlies_scheduled, 1);

        let nightly_manifest = load_manifest_from_default_branch(&forge_repo).expect("manifest");
        assert_eq!(
            schedule_due_nightlies_at(&store, &forge_repo, &nightly_manifest, now)
                .expect("nightly dedupe"),
            0
        );
        let nightlies = store.list_recent_nightly_runs(4).expect("nightly feed");
        let nightly = store
            .get_nightly_run(nightlies[0].nightly_run_id)
            .expect("nightly detail")
            .expect("nightly exists");
        assert_eq!(nightly.status, pika_forge_model::ForgeCiStatus::Success);
        assert_eq!(nightly.lanes.len(), 1);
        assert_eq!(nightly.lanes[0].status, CiLaneStatus::Success);
        assert_eq!(nightly.lanes[0].log_text.as_deref(), Some("nightly-ok\n"));
    }
}
