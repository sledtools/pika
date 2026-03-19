use anyhow::{anyhow, Context};

use crate::branch_store::BranchUpsertInput;
use crate::ci;
use crate::ci_manifest;
use crate::config::Config;
use crate::forge;
use crate::storage::Store;

#[derive(Debug, Default)]
pub struct PollResult {
    pub repos_polled: usize,
    pub branches_seen: usize,
    pub queued_regenerations: usize,
    pub head_sha_changes: usize,
    pub stale_closed: usize,
    pub queued_ci_runs: usize,
}

pub fn poll_once_limited(
    store: &Store,
    config: &Config,
    max_branches: usize,
) -> anyhow::Result<PollResult> {
    let forge_repo = config
        .effective_forge_repo()
        .ok_or_else(|| anyhow!("forge_repo must be configured for hosted forge mode"))?;

    let branches = forge::list_branches(&forge_repo).with_context(|| {
        format!(
            "list canonical branches from {}",
            forge_repo.canonical_git_dir
        )
    })?;

    let mut result = PollResult::default();
    let present_names: Vec<String> = branches
        .iter()
        .map(|branch| branch.branch_name.clone())
        .collect();
    for branch in branches.into_iter().take(if max_branches == 0 {
        usize::MAX
    } else {
        max_branches
    }) {
        let branch_name = branch.branch_name.clone();
        let title = branch.title.clone();
        let head_sha = branch.head_sha.clone();
        let merge_base_sha = branch.merge_base_sha.clone();
        let author_name = branch.author_name.clone();
        let author_email = branch.author_email.clone();
        let updated_at = branch.updated_at.clone();
        let outcome = store
            .upsert_branch_record(&BranchUpsertInput {
                repo: forge_repo.repo.clone(),
                canonical_git_dir: forge_repo.canonical_git_dir.clone(),
                default_branch: forge_repo.default_branch.clone(),
                ci_entrypoint: ci::FORGE_LANE_MANIFEST_PATH.to_string(),
                branch_name: branch_name.clone(),
                title,
                head_sha: head_sha.clone(),
                merge_base_sha: merge_base_sha.clone(),
                author_name,
                author_email,
                updated_at,
            })
            .context("upsert branch record from canonical repo state")?;
        let changed_paths = forge::changed_paths(&forge_repo, &merge_base_sha, &head_sha)
            .with_context(|| format!("compute changed paths for branch {}", branch_name))?;
        let queued_ci =
            match ci::load_manifest_for_head(&forge_repo, &head_sha).and_then(|manifest| {
                if changed_paths
                    .iter()
                    .any(|path| path == ci::FORGE_LANE_MANIFEST_PATH)
                {
                    Ok(manifest.branch_lanes)
                } else {
                    ci_manifest::select_branch_lanes(&manifest, &changed_paths)
                }
            }) {
                Ok(selected_lanes) => store
                    .queue_branch_ci_run_for_head(outcome.branch_id, &head_sha, &selected_lanes)
                    .with_context(|| format!("queue ci suite for branch {}", outcome.branch_id))?,
                Err(err) => {
                    let log = format!("forge ci manifest error: {err:#}");
                    store
                        .queue_branch_ci_failure_for_head(outcome.branch_id, &head_sha, &log)
                        .with_context(|| {
                            format!("queue ci manifest failure for branch {}", outcome.branch_id)
                        })?
                }
            };
        result.branches_seen += 1;
        if outcome.head_changed {
            result.head_sha_changes += 1;
        }
        if outcome.queued_generation {
            result.queued_regenerations += 1;
        }
        if queued_ci {
            result.queued_ci_runs += 1;
        }
    }

    result.stale_closed = store
        .close_missing_open_branches(&forge_repo.repo, &present_names)
        .with_context(|| format!("close stale open branches for {}", forge_repo.repo))?;
    result.repos_polled = 1;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use crate::config::{Config, ForgeRepoConfig};

    use super::poll_once_limited;
    use crate::{ci, storage::Store};

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

    fn config_for_bare_repo(bare: &std::path::Path) -> Config {
        Config {
            repos: vec!["sledtools/pika".to_string()],
            forge_repo: Some(ForgeRepoConfig {
                repo: "sledtools/pika".to_string(),
                canonical_git_dir: bare.to_str().expect("bare path").to_string(),
                default_branch: "master".to_string(),
                mirror_remote: None,
                mirror_poll_interval_secs: None,
                ci_command: vec!["just".to_string(), "pre-merge".to_string()],
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
        }
    }

    #[test]
    fn branch_push_queues_lanes_from_branch_head_manifest() {
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
            fs::set_permissions(seed.join(script), perms).expect("chmod script");
        }
        fs::write(
            seed.join("ci/forge-lanes.toml"),
            r#"
version = 1
nightly_schedule_utc = "08:00"

[[branch.lanes]]
id = "master_lane"
title = "master lane"
entrypoint = "./lane-a.sh"
command = ["./lane-a.sh"]
paths = ["README.md"]
"#,
        )
        .expect("write master manifest");
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

        git(&seed, &["checkout", "-b", "feature/manifest"]);
        fs::write(seed.join("README.md"), "branch\n").expect("update readme");
        fs::write(
            seed.join("ci/forge-lanes.toml"),
            r#"
version = 1
nightly_schedule_utc = "08:00"

[[branch.lanes]]
id = "branch_lane"
title = "branch lane"
entrypoint = "./lane-b.sh"
command = ["./lane-b.sh"]
paths = []
"#,
        )
        .expect("write branch manifest");
        git(&seed, &["add", "README.md", "ci/forge-lanes.toml"]);
        git(&seed, &["commit", "-m", "branch manifest"]);
        git(&seed, &["push", "origin", "feature/manifest"]);

        let config = config_for_bare_repo(&bare);
        let store = Store::open(&db_path).expect("open store");
        poll_once_limited(&store, &config, 0).expect("poll branches");
        let branch = store
            .list_branch_feed_items()
            .expect("list branch feed")
            .into_iter()
            .find(|item| item.branch_name == "feature/manifest")
            .expect("branch feed item");
        let queued = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list branch ci runs");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].lanes.len(), 1);
        assert_eq!(queued[0].lanes[0].lane_id, "branch_lane");

        let pass = ci::run_ci_pass(&store, &config).expect("run ci pass");
        assert_eq!(pass.succeeded, 1);

        let finished = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list finished ci runs");
        assert_eq!(finished[0].lanes[0].lane_id, "branch_lane");
        assert_eq!(finished[0].lanes[0].status, "success");
        assert_eq!(finished[0].lanes[0].log_text.as_deref(), Some("lane-b\n"));
    }

    #[test]
    fn invalid_branch_manifest_records_failed_suite_for_head() {
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
            seed.join("ci/forge-lanes.toml"),
            "version = 1\nnightly_schedule_utc = \"08:00\"\n",
        )
        .expect("write manifest");
        git(&seed, &["add", "README.md", "ci/forge-lanes.toml"]);
        git(&seed, &["commit", "-m", "initial"]);
        git(&seed, &["branch", "-M", "master"]);
        git(
            &seed,
            &["remote", "add", "origin", bare.to_str().expect("bare path")],
        );
        git(&seed, &["push", "origin", "master"]);

        git(&seed, &["checkout", "-b", "feature/bad-manifest"]);
        fs::write(seed.join("ci/forge-lanes.toml"), "not = [valid\n").expect("break manifest");
        git(&seed, &["add", "ci/forge-lanes.toml"]);
        git(&seed, &["commit", "-m", "break manifest"]);
        git(&seed, &["push", "origin", "feature/bad-manifest"]);

        let config = config_for_bare_repo(&bare);
        let store = Store::open(&db_path).expect("open store");
        poll_once_limited(&store, &config, 0).expect("poll branches");
        let branch = store
            .list_branch_feed_items()
            .expect("list branch feed")
            .into_iter()
            .find(|item| item.branch_name == "feature/bad-manifest")
            .expect("branch feed item");
        let failed_suite = store
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT status, log_text
                     FROM branch_ci_runs
                     WHERE branch_id = ?1
                     ORDER BY id DESC
                     LIMIT 1",
                    rusqlite::params![branch.branch_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .map_err(Into::into)
            })
            .expect("load failed suite");
        assert_eq!(failed_suite.0, "failed");
        assert!(failed_suite
            .1
            .as_deref()
            .unwrap_or_default()
            .contains("forge ci manifest error"));
    }
}
