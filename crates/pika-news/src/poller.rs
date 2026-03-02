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
}

pub fn poll_once(store: &Store, config: &Config) -> anyhow::Result<PollResult> {
    let token = env::var(&config.github_token_env).ok();
    let github = GitHubClient::new(token).context("initialize GitHub client")?;

    let mut result = PollResult::default();

    for repo in &config.repos {
        let open = github
            .fetch_open_prs(repo)
            .with_context(|| format!("poll open PRs for {}", repo))?;
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
