use anyhow::{anyhow, Context};
use chrono::{DateTime, Duration, Utc};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::Deserialize;

#[derive(Clone)]
pub struct GitHubClient {
    client: Client,
    token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PullRequest {
    pub number: i64,
    pub title: String,
    pub html_url: String,
    pub state: String,
    pub head_sha: String,
    pub base_ref: String,
    pub author_login: Option<String>,
    pub updated_at: String,
    pub merged_at: Option<String>,
}

impl GitHubClient {
    pub fn new(token: Option<String>) -> anyhow::Result<Self> {
        let client = Client::builder()
            .build()
            .context("build GitHub HTTP client")?;
        Ok(Self { client, token })
    }

    pub fn fetch_open_prs(&self, repo: &str) -> anyhow::Result<Vec<PullRequest>> {
        self.fetch_prs(repo, "open")
    }

    pub fn fetch_merged_lookback(
        &self,
        repo: &str,
        lookback_hours: u64,
    ) -> anyhow::Result<Vec<PullRequest>> {
        let cutoff = Utc::now() - Duration::hours(lookback_hours as i64);
        let closed = self.fetch_prs(repo, "closed")?;
        Ok(closed
            .into_iter()
            .filter(|pr| {
                if let Some(merged_at) = &pr.merged_at {
                    if let Ok(ts) = DateTime::parse_from_rfc3339(merged_at) {
                        return ts.with_timezone(&Utc) >= cutoff;
                    }
                }
                false
            })
            .collect())
    }

    fn fetch_prs(&self, repo: &str, state: &str) -> anyhow::Result<Vec<PullRequest>> {
        let url = format!(
            "https://api.github.com/repos/{}/pulls?state={}&sort=updated&direction=desc&per_page=100",
            repo, state
        );

        let mut req = self
            .client
            .get(&url)
            .header(USER_AGENT, "pika-news/0.1")
            .header(ACCEPT, "application/vnd.github+json");

        if let Some(token) = &self.token {
            req = req.header(AUTHORIZATION, format!("Bearer {}", token));
        }

        let response = req
            .send()
            .with_context(|| format!("send GitHub request for repo {} state {}", repo, state))?;

        let status = response.status();
        let body = response
            .text()
            .with_context(|| format!("read GitHub response body for {}", repo))?;

        if !status.is_success() {
            return Err(anyhow!(
                "GitHub API error for {} state {}: HTTP {} body={} ",
                repo,
                state,
                status,
                truncate(&body, 400)
            ));
        }

        let parsed: Vec<GitHubPullResponse> = serde_json::from_str(&body)
            .with_context(|| format!("parse GitHub pull list JSON for {}", repo))?;

        Ok(parsed.into_iter().map(PullRequest::from).collect())
    }

    pub fn fetch_pull_diff(&self, repo: &str, pr_number: i64) -> anyhow::Result<PullDiff> {
        let url = format!(
            "https://api.github.com/repos/{}/pulls/{}/files?per_page=100",
            repo, pr_number
        );

        let mut req = self
            .client
            .get(&url)
            .header(USER_AGENT, "pika-news/0.1")
            .header(ACCEPT, "application/vnd.github+json");

        if let Some(token) = &self.token {
            req = req.header(AUTHORIZATION, format!("Bearer {}", token));
        }

        let response = req
            .send()
            .with_context(|| format!("fetch pull files for {}/{}", repo, pr_number))?;
        let status = response.status();
        let body = response
            .text()
            .with_context(|| format!("read pull files body for {}/{}", repo, pr_number))?;

        if !status.is_success() {
            return Err(anyhow!(
                "GitHub pull files error for {}/{}: HTTP {} body={}",
                repo,
                pr_number,
                status,
                truncate(&body, 400)
            ));
        }

        let files: Vec<GitHubPullFileResponse> = serde_json::from_str(&body)
            .with_context(|| format!("parse pull files JSON for {}/{}", repo, pr_number))?;

        let mut unified_diff = String::new();
        let mut file_list = Vec::with_capacity(files.len());
        for file in files {
            file_list.push(file.filename.clone());
            unified_diff.push_str(&format!(
                "diff --git a/{0} b/{0}\n--- a/{0}\n+++ b/{0}\n",
                file.filename
            ));
            if let Some(patch) = file.patch {
                unified_diff.push_str(&patch);
                unified_diff.push('\n');
            } else {
                unified_diff.push_str("@@ binary or large diff omitted by GitHub @@\n");
            }
            unified_diff.push('\n');
        }

        Ok(PullDiff {
            files: file_list,
            unified_diff,
        })
    }
}

#[derive(Debug, Deserialize)]
struct GitHubPullResponse {
    number: i64,
    title: String,
    html_url: String,
    state: String,
    head: GitRef,
    base: GitRef,
    user: Option<GitUser>,
    updated_at: String,
    merged_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitRef {
    sha: String,
    r#ref: String,
}

#[derive(Debug, Deserialize)]
struct GitUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GitHubPullFileResponse {
    filename: String,
    patch: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PullDiff {
    pub files: Vec<String>,
    pub unified_diff: String,
}

impl From<GitHubPullResponse> for PullRequest {
    fn from(value: GitHubPullResponse) -> Self {
        Self {
            number: value.number,
            title: value.title,
            html_url: value.html_url,
            state: if value.merged_at.is_some() {
                "merged".to_string()
            } else {
                value.state
            },
            head_sha: value.head.sha,
            base_ref: value.base.r#ref,
            author_login: value.user.map(|u| u.login),
            updated_at: value.updated_at,
            merged_at: value.merged_at,
        }
    }
}

fn truncate(input: &str, max: usize) -> String {
    if input.len() <= max {
        input.to_string()
    } else {
        format!("{}...", &input[..max])
    }
}
