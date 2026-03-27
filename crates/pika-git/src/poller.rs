use anyhow::{anyhow, Context};

use crate::branch_store::BranchUpsertInput;
use crate::ci;
use crate::ci_manifest;
use crate::config::Config;
use crate::forge;
use crate::live::CiLiveUpdates;
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

#[cfg_attr(not(test), allow(dead_code))]
pub fn poll_once_limited(
    store: &Store,
    config: &Config,
    max_branches: usize,
) -> anyhow::Result<PollResult> {
    poll_once_limited_with_updates(store, config, max_branches, None)
}

pub fn poll_once_limited_with_updates(
    store: &Store,
    config: &Config,
    max_branches: usize,
    live_updates: Option<&CiLiveUpdates>,
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
                ci_entrypoint: ci::FORGE_LANE_DEFINITION_PATH.to_string(),
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
                    .any(|path| path == ci::FORGE_LANE_DEFINITION_PATH)
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
            if let Some(live_updates) = live_updates {
                live_updates.branch_changed(outcome.branch_id, "run_queued");
            }
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
    use crate::config::{Config, ForgeRepoConfig};
    use crate::test_support::GitTestRepo;

    use super::poll_once_limited;
    use crate::{ci, ci_manifest};

    fn config_for_bare_repo(bare: &std::path::Path) -> Config {
        Config::test_with_forge_repo(ForgeRepoConfig {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: bare.to_str().expect("bare path").to_string(),
            default_branch: "master".to_string(),
            ci_concurrency: Some(2),
            mirror_remote: None,
            mirror_poll_interval_secs: None,
            mirror_timeout_secs: None,
            ci_command: vec!["just".to_string(), "pre-merge".to_string()],
            hook_url: Some("http://127.0.0.1:9999/git/webhook".to_string()),
        })
    }

    #[test]
    fn branch_push_queues_compiled_lanes_for_changed_paths() {
        let repo = GitTestRepo::new();
        repo.write_seed("README.md", "hello\n");
        repo.write_seed("rust/src/lib.rs", "pub fn feature_branch() {}\n");
        repo.seed_add(&["README.md", "rust/src/lib.rs"]);
        repo.seed_commit("initial");
        repo.seed_push_master();

        repo.seed_checkout_new_branch("feature/rust");
        repo.write_seed(
            "rust/src/lib.rs",
            "pub fn feature_branch() { println!(\"hi\"); }\n",
        );
        repo.seed_add(&["rust/src/lib.rs"]);
        repo.seed_commit("touch rust core");
        repo.seed_push_branch("feature/rust");

        let config = config_for_bare_repo(repo.bare_path());
        let store = repo.open_store();
        poll_once_limited(&store, &config, 0).expect("poll branches");
        let branch = store
            .list_branch_feed_items()
            .expect("list branch feed")
            .into_iter()
            .find(|item| item.branch_name == "feature/rust")
            .expect("branch feed item");
        let queued = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list branch ci runs");
        assert_eq!(queued.len(), 1);
        let lane_ids = queued[0]
            .lanes
            .iter()
            .map(|lane| lane.lane_id.as_str())
            .collect::<Vec<_>>();
        assert!(lane_ids.contains(&"pika_rust"));
        assert!(lane_ids.contains(&"pika_followup"));
        assert!(lane_ids.contains(&"pikachat"));
        assert!(lane_ids.contains(&"apple_host_sanity"));
    }

    #[test]
    fn branch_push_of_lane_definition_file_queues_all_branch_lanes() {
        let repo = GitTestRepo::new();
        repo.write_seed(ci::FORGE_LANE_DEFINITION_PATH, "// lane source of truth\n");
        repo.seed_add(&[ci::FORGE_LANE_DEFINITION_PATH]);
        repo.seed_commit("initial");
        repo.seed_push_master();

        repo.seed_checkout_new_branch("feature/lanes");
        repo.write_seed(
            ci::FORGE_LANE_DEFINITION_PATH,
            "// lane source of truth changed\n",
        );
        repo.seed_add(&[ci::FORGE_LANE_DEFINITION_PATH]);
        repo.seed_commit("touch lane source");
        repo.seed_push_branch("feature/lanes");

        let config = config_for_bare_repo(repo.bare_path());
        let store = repo.open_store();
        poll_once_limited(&store, &config, 0).expect("poll branches");
        let branch = store
            .list_branch_feed_items()
            .expect("list branch feed")
            .into_iter()
            .find(|item| item.branch_name == "feature/lanes")
            .expect("branch feed item");
        let queued = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list branch ci runs");
        assert_eq!(queued.len(), 1);
        let manifest = ci_manifest::compiled_forge_ci_manifest();
        assert_eq!(queued[0].lanes.len(), manifest.branch_lanes.len());
    }
}
