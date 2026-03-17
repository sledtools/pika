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

    let manifest = ci::load_manifest_from_default_branch(&forge_repo)
        .context("load forge lane manifest from default branch")?;
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
        let selected_lanes = ci_manifest::select_branch_lanes(&manifest, &changed_paths)
            .with_context(|| format!("select ci lanes for branch {}", branch_name))?;
        let queued_ci = store
            .queue_branch_ci_run_for_head(outcome.branch_id, &head_sha, &selected_lanes)
            .with_context(|| format!("queue ci suite for branch {}", outcome.branch_id))?;
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
