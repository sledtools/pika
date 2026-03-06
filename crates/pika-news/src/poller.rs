use std::collections::HashSet;
use std::env;

use anyhow::Context;

use crate::config::Config;
use crate::github::{GitHubClient, PullRequest};
use crate::storage::{PrUpsertInput, Store};

#[derive(Debug, Default)]
pub struct PollResult {
    pub repos_polled: usize,
    pub prs_seen: usize,
    pub queued_regenerations: usize,
    pub head_sha_changes: usize,
    pub stale_closed: usize,
}

pub fn poll_once_limited(
    store: &Store,
    config: &Config,
    max_prs: usize,
) -> anyhow::Result<PollResult> {
    let token = env::var(&config.github_token_env).ok();
    let github = GitHubClient::new(token).context("initialize GitHub client")?;

    let mut result = PollResult::default();

    for repo in &config.repos {
        let open = github
            .fetch_open_prs(repo)
            .with_context(|| format!("poll open PRs for {}", repo))?;

        // Collect the definitive set of currently-open PR numbers before merging
        // with the merged lookback list, so we can close stale PRs afterward.
        let current_open_numbers: Vec<i64> = open.iter().map(|pr| pr.number).collect();

        let merged = github
            .fetch_merged_lookback(repo, config.merged_lookback_hours)
            .with_context(|| format!("poll merged lookback for {}", repo))?;

        let mut seen = HashSet::new();
        let mut processed = Vec::new();

        for pr in open.into_iter().chain(merged.into_iter()) {
            if seen.insert(pr.number) {
                processed.push(pr);
            }
        }

        if max_prs > 0 {
            processed.truncate(max_prs);
        }

        for pr in &processed {
            let outcome = store
                .upsert_pull_request(&to_upsert(repo, pr))
                .with_context(|| format!("upsert PR {} for {}", pr.number, repo))?;
            result.prs_seen += 1;
            if outcome.queued {
                result.queued_regenerations += 1;
            }
            if outcome.head_changed {
                result.head_sha_changes += 1;
            }
        }

        // Any PR we have stored as "open" but GitHub no longer returns as
        // open must have been closed or merged — mark it closed.
        let closed = store
            .close_stale_open_prs(repo, &current_open_numbers)
            .with_context(|| format!("close stale open PRs for {}", repo))?;
        result.stale_closed += closed;

        let open_cursor = processed
            .iter()
            .filter(|pr| pr.state == "open")
            .map(|pr| pr.updated_at.as_str())
            .max();
        let merged_cursor = processed
            .iter()
            .filter(|pr| pr.state == "merged")
            .map(|pr| pr.updated_at.as_str())
            .max();

        store
            .update_poll_markers(repo, open_cursor, merged_cursor)
            .with_context(|| format!("update poll markers for {}", repo))?;

        result.repos_polled += 1;
    }

    Ok(result)
}

fn to_upsert(repo: &str, pr: &PullRequest) -> PrUpsertInput {
    PrUpsertInput {
        repo: repo.to_string(),
        pr_number: pr.number,
        title: pr.title.clone(),
        url: pr.html_url.clone(),
        state: pr.state.clone(),
        head_sha: pr.head_sha.clone(),
        base_ref: pr.base_ref.clone(),
        author_login: pr.author_login.clone(),
        updated_at: pr.updated_at.clone(),
        merged_at: pr.merged_at.clone(),
    }
}
