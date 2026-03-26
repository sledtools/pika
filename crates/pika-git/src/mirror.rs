use std::env;

use anyhow::Context;
use chrono::{DateTime, NaiveDateTime, Utc};

use crate::config::Config;
use crate::forge;
use crate::mirror_store::{MirrorStatusRecord, MirrorSyncRunInput, MirrorSyncRunRecord};
use crate::storage::Store;

#[derive(Debug, Default)]
pub struct MirrorPassResult {
    pub attempted: bool,
    pub status: Option<String>,
    pub lagging_ref_count: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MirrorRuntimeStatus {
    pub configured: bool,
    pub remote_name: Option<String>,
    pub background_enabled: bool,
    pub background_interval_secs: Option<u64>,
    pub timeout_secs: Option<u64>,
    pub active_run: Option<forge::MirrorLockStatus>,
    pub github_token_env: String,
}

#[derive(Debug)]
pub struct MirrorAdminStatus {
    pub runtime: MirrorRuntimeStatus,
    pub detail: Option<(MirrorStatusRecord, Vec<MirrorSyncRunRecord>)>,
}

pub fn mirror_runtime_status(config: &Config) -> MirrorRuntimeStatus {
    let forge_repo = config.effective_forge_repo();
    let remote_name = forge_repo
        .as_ref()
        .and_then(|repo| repo.mirror_remote.clone());
    let background_interval_secs = forge_repo.as_ref().and_then(|repo| {
        if repo.mirror_remote.is_some() {
            repo.mirror_poll_interval_secs
        } else {
            None
        }
    });
    let timeout_secs = forge_repo.as_ref().and_then(|repo| {
        if repo.mirror_remote.is_some() {
            repo.mirror_timeout_secs
        } else {
            None
        }
    });
    let active_run = forge_repo
        .as_ref()
        .filter(|repo| repo.mirror_remote.is_some())
        .and_then(forge::current_mirror_lock_status);
    MirrorRuntimeStatus {
        configured: remote_name.is_some(),
        remote_name,
        background_enabled: background_interval_secs.unwrap_or(0) > 0,
        background_interval_secs,
        timeout_secs,
        active_run,
        github_token_env: config.github_token_env.clone(),
    }
}

pub fn run_background_mirror_pass(
    store: &Store,
    config: &Config,
) -> anyhow::Result<MirrorPassResult> {
    let runtime = mirror_runtime_status(config);
    if !runtime.configured || !runtime.background_enabled {
        return Ok(MirrorPassResult::default());
    }
    let interval_secs = runtime.background_interval_secs.unwrap_or_default();
    let Some(forge_repo) = config.effective_forge_repo() else {
        return Ok(MirrorPassResult::default());
    };
    let Some(remote_name) = forge_repo.mirror_remote.as_deref() else {
        return Ok(MirrorPassResult::default());
    };
    let last_attempt_at = store
        .get_mirror_status(&forge_repo.repo, remote_name)
        .context("load mirror status for background schedule")?
        .and_then(|status| status.last_attempt)
        .and_then(|attempt| {
            parse_timestamp(&attempt.finished_at).or_else(|| parse_timestamp(&attempt.created_at))
        });
    if let Some(last_attempt_at) = last_attempt_at {
        let elapsed = Utc::now().signed_duration_since(last_attempt_at);
        if elapsed.num_seconds() >= 0 && elapsed.num_seconds() < interval_secs as i64 {
            return Ok(MirrorPassResult::default());
        }
    }
    run_mirror_pass(store, config, "background")
}

pub fn run_mirror_pass(
    store: &Store,
    config: &Config,
    trigger_source: &str,
) -> anyhow::Result<MirrorPassResult> {
    let Some(forge_repo) = config.effective_forge_repo() else {
        return Ok(MirrorPassResult::default());
    };
    let Some(remote_name) = forge_repo.mirror_remote.as_deref() else {
        return Ok(MirrorPassResult::default());
    };
    let github_token = env::var(&config.github_token_env).ok();
    match forge::sync_mirror(
        &forge_repo,
        remote_name,
        github_token.as_deref(),
        trigger_source,
    ) {
        Ok(outcome) => {
            store.record_mirror_sync_run(&MirrorSyncRunInput {
                repo: forge_repo.repo.clone(),
                canonical_git_dir: forge_repo.canonical_git_dir.clone(),
                default_branch: forge_repo.default_branch.clone(),
                remote_name: outcome.remote_name.clone(),
                trigger_source: trigger_source.to_string(),
                status: "success".to_string(),
                local_default_head: outcome.local_default_head.clone(),
                remote_default_head: outcome.remote_default_head.clone(),
                lagging_ref_count: Some(outcome.lagging_ref_count),
                synced_ref_count: Some(outcome.synced_ref_count),
                error_text: None,
            })?;
            Ok(MirrorPassResult {
                attempted: true,
                status: Some("success".to_string()),
                lagging_ref_count: Some(outcome.lagging_ref_count),
            })
        }
        Err(err) => {
            let inspected = forge::inspect_mirror(&forge_repo, remote_name).ok();
            store.record_mirror_sync_run(&MirrorSyncRunInput {
                repo: forge_repo.repo.clone(),
                canonical_git_dir: forge_repo.canonical_git_dir.clone(),
                default_branch: forge_repo.default_branch.clone(),
                remote_name: remote_name.to_string(),
                trigger_source: trigger_source.to_string(),
                status: "failed".to_string(),
                local_default_head: inspected
                    .as_ref()
                    .and_then(|state| state.local_default_head.clone()),
                remote_default_head: inspected
                    .as_ref()
                    .and_then(|state| state.remote_default_head.clone()),
                lagging_ref_count: inspected.as_ref().map(|state| state.lagging_ref_count),
                synced_ref_count: inspected.as_ref().map(|state| state.synced_ref_count),
                error_text: Some(err.to_string()),
            })?;
            Ok(MirrorPassResult {
                attempted: true,
                status: Some("failed".to_string()),
                lagging_ref_count: inspected.as_ref().map(|state| state.lagging_ref_count),
            })
        }
    }
}

pub fn get_mirror_status(store: &Store, config: &Config) -> anyhow::Result<MirrorAdminStatus> {
    let runtime = mirror_runtime_status(config);
    let Some(forge_repo) = config.effective_forge_repo() else {
        return Ok(MirrorAdminStatus {
            runtime,
            detail: None,
        });
    };
    let Some(remote_name) = forge_repo.mirror_remote.as_deref() else {
        return Ok(MirrorAdminStatus {
            runtime,
            detail: None,
        });
    };
    let status = store
        .get_mirror_status(&forge_repo.repo, remote_name)
        .context("load mirror status")?;
    let Some(status) = status else {
        return Ok(MirrorAdminStatus {
            runtime,
            detail: None,
        });
    };
    let history = store
        .list_recent_mirror_sync_runs(&forge_repo.repo, remote_name, 12)
        .context("load mirror history")?;
    Ok(MirrorAdminStatus {
        runtime,
        detail: Some((status, history)),
    })
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                .map(|dt| dt.and_utc())
                .ok()
        })
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::{run_background_mirror_pass, run_mirror_pass};
    use crate::config::{Config, ForgeRepoConfig};
    use crate::test_support::GitTestRepo;

    fn base_config(canonical_git_dir: &str, mirror_remote: Option<&str>) -> Config {
        Config::test_with_forge_repo(ForgeRepoConfig {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: canonical_git_dir.to_string(),
            default_branch: "master".to_string(),
            ci_concurrency: Some(2),
            mirror_remote: mirror_remote.map(str::to_string),
            mirror_poll_interval_secs: Some(300),
            mirror_timeout_secs: Some(120),
            ci_command: vec!["just".to_string(), "pre-merge".to_string()],
            hook_url: Some("http://127.0.0.1:9999/git/webhook".to_string()),
        })
    }

    #[test]
    fn mirror_pass_records_success_and_zero_lag() {
        let repo = GitTestRepo::new();
        let mirror = repo.seed_path().parent().expect("root").join("mirror.git");
        Command::new("git")
            .args(["init", "--bare", mirror.to_str().expect("mirror path")])
            .current_dir(repo.seed_path().parent().expect("root"))
            .status()
            .expect("init mirror repo");
        repo.write_seed("README.md", "hello\n");
        repo.seed_add(&["README.md"]);
        repo.seed_commit("initial");
        repo.seed_push_master();
        Command::new("git")
            .args([
                "--git-dir",
                repo.bare_path().to_str().expect("bare path"),
                "remote",
                "add",
                "github",
                mirror.to_str().expect("mirror path"),
            ])
            .status()
            .expect("add mirror remote");

        let store = repo.open_store();
        let result = run_mirror_pass(
            &store,
            &base_config(
                repo.bare_path().to_str().expect("bare path"),
                Some("github"),
            ),
            "background",
        )
        .expect("run mirror pass");
        assert!(result.attempted);
        assert_eq!(result.status.as_deref(), Some("success"));
        assert_eq!(result.lagging_ref_count, Some(0));

        let status = store
            .get_mirror_status("sledtools/pika", "github")
            .expect("mirror status")
            .expect("status exists");
        assert_eq!(
            status.last_attempt.as_ref().map(|run| run.status.as_str()),
            Some("success")
        );
    }

    #[test]
    fn mirror_pass_records_failure_for_bad_remote() {
        let repo = GitTestRepo::new();
        let result = run_mirror_pass(
            &repo.open_store(),
            &base_config(
                repo.bare_path().to_str().expect("bare path"),
                Some("github"),
            ),
            "manual",
        )
        .expect("run failed mirror pass");
        assert!(result.attempted);
        assert_eq!(result.status.as_deref(), Some("failed"));

        let status = repo
            .open_store()
            .get_mirror_status("sledtools/pika", "github")
            .expect("mirror status")
            .expect("status exists");
        let attempt = status.last_attempt.expect("attempt");
        assert_eq!(attempt.status, "failed");
        assert_eq!(attempt.failure_kind.as_deref(), Some("config"));
        assert_eq!(status.consecutive_failure_count, 1);
        assert!(status.last_failure_at.is_some());
        assert!(attempt
            .error_text
            .unwrap_or_default()
            .contains("mirror remote"));
    }

    #[test]
    fn background_mirror_pass_respects_configured_interval() {
        let repo = GitTestRepo::new();
        let mirror = repo.seed_path().parent().expect("root").join("mirror.git");
        Command::new("git")
            .args(["init", "--bare", mirror.to_str().expect("mirror path")])
            .current_dir(repo.seed_path().parent().expect("root"))
            .status()
            .expect("init mirror repo");
        repo.write_seed("README.md", "hello\n");
        repo.seed_add(&["README.md"]);
        repo.seed_commit("initial");
        repo.seed_push_master();
        Command::new("git")
            .args([
                "--git-dir",
                repo.bare_path().to_str().expect("bare path"),
                "remote",
                "add",
                "github",
                mirror.to_str().expect("mirror path"),
            ])
            .status()
            .expect("add mirror remote");

        let store = repo.open_store();
        let config = base_config(
            repo.bare_path().to_str().expect("bare path"),
            Some("github"),
        );
        assert!(
            run_background_mirror_pass(&store, &config)
                .expect("first background pass")
                .attempted
        );
        assert!(
            !run_background_mirror_pass(&store, &config)
                .expect("second background pass")
                .attempted
        );
    }
}
