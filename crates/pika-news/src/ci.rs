use anyhow::{anyhow, Context};
use chrono::{DateTime, NaiveTime, Utc};

use crate::branch_store::{PendingBranchCiLaneJob, PendingNightlyLaneJob};
use crate::ci_manifest::{self, ForgeCiManifest};
use crate::config::Config;
use crate::forge;
use crate::storage::Store;

pub const FORGE_LANE_MANIFEST_PATH: &str = "ci/forge-lanes.toml";

#[derive(Debug, Default)]
pub struct CiPassResult {
    pub claimed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub nightlies_scheduled: usize,
    pub retries_recovered: usize,
}

pub fn load_manifest_from_default_branch(
    forge_repo: &crate::config::ForgeRepoConfig,
) -> anyhow::Result<ForgeCiManifest> {
    let default_ref = format!("refs/heads/{}", forge_repo.default_branch);
    let raw = forge::read_file_at_ref(forge_repo, &default_ref, FORGE_LANE_MANIFEST_PATH)?;
    ci_manifest::parse_manifest(&raw)
}

pub fn run_ci_pass(store: &Store, config: &Config) -> anyhow::Result<CiPassResult> {
    let Some(forge_repo) = config.effective_forge_repo() else {
        return Ok(CiPassResult::default());
    };

    let manifest = load_manifest_from_default_branch(&forge_repo)?;
    let mut result = CiPassResult {
        retries_recovered: store
            .recover_stale_ci_lanes()
            .context("recover stale ci lanes")?,
        nightlies_scheduled: schedule_due_nightlies(store, &forge_repo, &manifest)
            .context("schedule due nightlies")?,
        ..CiPassResult::default()
    };

    let branch_jobs = store
        .claim_pending_branch_ci_lane_runs(1)
        .context("claim pending branch ci lanes")?;
    for job in branch_jobs {
        result.claimed += 1;
        execute_branch_job(store, &forge_repo, job, &mut result)?;
    }

    let nightly_jobs = store
        .claim_pending_nightly_lane_runs(1)
        .context("claim pending nightly lanes")?;
    for job in nightly_jobs {
        result.claimed += 1;
        execute_nightly_job(store, &forge_repo, job, &mut result)?;
    }

    Ok(result)
}

fn execute_branch_job(
    store: &Store,
    forge_repo: &crate::config::ForgeRepoConfig,
    job: PendingBranchCiLaneJob,
    result: &mut CiPassResult,
) -> anyhow::Result<()> {
    let exec = forge::run_ci_command_for_head(forge_repo, &job.source_head_sha, &job.command)
        .with_context(|| {
            format!(
                "run branch lane {} for branch {}",
                job.lane_id, job.branch_id
            )
        });
    match exec {
        Ok(exec) => {
            let status = if exec.success { "success" } else { "failed" };
            store
                .finish_branch_ci_lane_run(job.lane_run_id, status, &exec.log)
                .with_context(|| format!("persist branch lane result {}", job.lane_run_id))?;
            if exec.success {
                result.succeeded += 1;
            } else {
                result.failed += 1;
            }
        }
        Err(err) => {
            let log = format!("ci runner error: {}", err);
            store
                .finish_branch_ci_lane_run(job.lane_run_id, "failed", &log)
                .with_context(|| format!("persist branch lane failure {}", job.lane_run_id))?;
            result.failed += 1;
        }
    }
    Ok(())
}

fn execute_nightly_job(
    store: &Store,
    forge_repo: &crate::config::ForgeRepoConfig,
    job: PendingNightlyLaneJob,
    result: &mut CiPassResult,
) -> anyhow::Result<()> {
    let exec = forge::run_ci_command_for_head(forge_repo, &job.source_head_sha, &job.command)
        .with_context(|| {
            format!(
                "run nightly lane {} for nightly {}",
                job.lane_id, job.nightly_run_id
            )
        });
    match exec {
        Ok(exec) => {
            let status = if exec.success { "success" } else { "failed" };
            store
                .finish_nightly_lane_run(job.lane_run_id, status, &exec.log)
                .with_context(|| format!("persist nightly lane result {}", job.lane_run_id))?;
            if exec.success {
                result.succeeded += 1;
            } else {
                result.failed += 1;
            }
        }
        Err(err) => {
            let log = format!("ci runner error: {}", err);
            store
                .finish_nightly_lane_run(job.lane_run_id, "failed", &log)
                .with_context(|| format!("persist nightly lane failure {}", job.lane_run_id))?;
            result.failed += 1;
        }
    }
    Ok(())
}

fn schedule_due_nightlies(
    store: &Store,
    forge_repo: &crate::config::ForgeRepoConfig,
    manifest: &ForgeCiManifest,
) -> anyhow::Result<usize> {
    schedule_due_nightlies_at(store, forge_repo, manifest, Utc::now())
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

    use chrono::TimeZone;

    use super::{load_manifest_from_default_branch, schedule_due_nightlies_at};
    use crate::config::ForgeRepoConfig;
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
    fn nightly_schedule_creates_durable_history_once_per_slot() {
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
nightly_schedule_utc = "08:00"

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
            ci_command: vec!["just".to_string(), "pre-merge".to_string()],
            hook_url: Some("http://127.0.0.1:9999/news/webhook".to_string()),
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
        assert_eq!(detail.status, "queued");
        assert_eq!(detail.lanes.len(), 1);
        assert_eq!(detail.lanes[0].lane_id, "nightly_smoke");
    }
}
