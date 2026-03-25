use std::time::Duration;

use anyhow::{Context, anyhow};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

fn encode_query_component(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct ChallengeResponse {
    pub(crate) challenge: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct LoginResponse {
    pub(crate) token: String,
    pub(crate) npub: String,
    pub(crate) is_admin: bool,
    pub(crate) can_forge_write: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct MeResponse {
    pub(crate) npub: String,
    pub(crate) is_admin: bool,
    pub(crate) can_chat: bool,
    pub(crate) can_forge_write: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct BranchResolveResponse {
    pub(crate) branch_id: i64,
    pub(crate) repo: String,
    pub(crate) branch_name: String,
    pub(crate) branch_state: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct BranchDetailResponse {
    pub(crate) branch: BranchSummary,
    pub(crate) ci_runs: Vec<CiRun>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct BranchSummary {
    pub(crate) branch_id: i64,
    pub(crate) repo: String,
    pub(crate) branch_name: String,
    pub(crate) title: String,
    pub(crate) branch_state: String,
    pub(crate) updated_at: String,
    pub(crate) target_branch: String,
    pub(crate) head_sha: String,
    pub(crate) merge_base_sha: String,
    pub(crate) merge_commit_sha: Option<String>,
    pub(crate) tutorial_status: String,
    pub(crate) ci_status: String,
    pub(crate) error_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct CiRun {
    pub(crate) id: i64,
    pub(crate) source_head_sha: String,
    pub(crate) status: String,
    pub(crate) lane_count: usize,
    pub(crate) rerun_of_run_id: Option<i64>,
    pub(crate) created_at: String,
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: Option<String>,
    pub(crate) lanes: Vec<CiLane>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct CiLane {
    pub(crate) id: i64,
    pub(crate) lane_id: String,
    pub(crate) title: String,
    pub(crate) entrypoint: String,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) execution_reason: CiLaneExecutionReason,
    #[serde(default)]
    pub(crate) failure_kind: Option<CiLaneFailureKind>,
    pub(crate) pikaci_run_id: Option<String>,
    pub(crate) pikaci_target_id: Option<String>,
    #[serde(default)]
    pub(crate) ci_target_key: Option<String>,
    #[serde(default)]
    pub(crate) target_health_state: Option<CiTargetHealthState>,
    #[serde(default)]
    pub(crate) target_health_summary: Option<String>,
    pub(crate) log_text: Option<String>,
    pub(crate) retry_count: i64,
    pub(crate) rerun_of_lane_run_id: Option<i64>,
    pub(crate) created_at: String,
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CiLaneExecutionReason {
    #[default]
    Queued,
    Running,
    BlockedByConcurrencyGroup,
    WaitingForCapacity,
    TargetUnhealthy,
    StaleRecovered,
}

impl CiLaneExecutionReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::BlockedByConcurrencyGroup => "blocked_by_concurrency_group",
            Self::WaitingForCapacity => "waiting_for_capacity",
            Self::TargetUnhealthy => "target_unhealthy",
            Self::StaleRecovered => "stale_recovered",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::BlockedByConcurrencyGroup => "blocked by concurrency group",
            Self::WaitingForCapacity => "waiting for capacity",
            Self::TargetUnhealthy => "target unhealthy",
            Self::StaleRecovered => "stale recovered",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CiLaneFailureKind {
    TestFailure,
    Timeout,
    Infrastructure,
}

impl CiLaneFailureKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::TestFailure => "test failure",
            Self::Timeout => "timeout",
            Self::Infrastructure => "infrastructure",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CiTargetHealthState {
    Healthy,
    Unhealthy,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct BranchLogsResponse {
    pub(crate) branch_id: i64,
    pub(crate) branch_name: String,
    pub(crate) run_id: i64,
    pub(crate) lane: CiLane,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct NightlyDetailResponse {
    pub(crate) nightly_run_id: i64,
    pub(crate) repo: String,
    pub(crate) scheduled_for: String,
    pub(crate) created_at: String,
    pub(crate) source_ref: String,
    pub(crate) source_head_sha: String,
    pub(crate) status: String,
    pub(crate) summary: Option<String>,
    pub(crate) rerun_of_run_id: Option<i64>,
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: Option<String>,
    pub(crate) lanes: Vec<CiLane>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct LaneMutationResponse {
    pub(crate) status: String,
    pub(crate) branch_id: Option<i64>,
    pub(crate) nightly_run_id: Option<i64>,
    pub(crate) lane_run_id: i64,
    pub(crate) lane_status: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct RecoverRunResponse {
    pub(crate) status: String,
    pub(crate) branch_id: Option<i64>,
    pub(crate) run_id: Option<i64>,
    pub(crate) nightly_run_id: Option<i64>,
    pub(crate) recovered_lane_count: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct WakeCiResponse {
    pub(crate) status: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct BranchActionResponse {
    pub(crate) status: String,
    pub(crate) branch_id: i64,
    pub(crate) merge_commit_sha: Option<String>,
    pub(crate) deleted: Option<bool>,
}

#[derive(Debug, Serialize)]
struct VerifyRequest<'a> {
    event: &'a str,
}

pub(crate) struct ApiClient {
    base_url: String,
    token: Option<String>,
    client: Client,
}

impl ApiClient {
    pub(crate) fn new(base_url: String, token: Option<String>) -> anyhow::Result<Self> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .context("build ph http client")?;
        Ok(Self {
            base_url,
            token,
            client,
        })
    }

    pub(crate) fn challenge(&self) -> anyhow::Result<ChallengeResponse> {
        self.send(Method::POST, "/news/auth/challenge", None::<&()>, false)
    }

    pub(crate) fn verify(&self, event_json: &str) -> anyhow::Result<LoginResponse> {
        self.send(
            Method::POST,
            "/news/auth/verify",
            Some(&VerifyRequest { event: event_json }),
            false,
        )
    }

    pub(crate) fn me(&self) -> anyhow::Result<MeResponse> {
        self.send(Method::GET, "/news/api/me", None::<&()>, true)
    }

    pub(crate) fn resolve_branch(
        &self,
        branch_name: &str,
    ) -> anyhow::Result<BranchResolveResponse> {
        let path = format!(
            "/news/api/forge/branch/resolve?branch_name={}",
            encode_query_component(branch_name)
        );
        self.send(Method::GET, &path, None::<&()>, true)
    }

    pub(crate) fn branch_detail(&self, branch_id: i64) -> anyhow::Result<BranchDetailResponse> {
        self.send(
            Method::GET,
            &format!("/news/api/forge/branch/{branch_id}"),
            None::<&()>,
            true,
        )
    }

    pub(crate) fn branch_logs(
        &self,
        branch_id: i64,
        lane: Option<&str>,
        lane_run_id: Option<i64>,
    ) -> anyhow::Result<BranchLogsResponse> {
        let mut query = Vec::new();
        if let Some(lane) = lane {
            query.push(format!("lane={}", encode_query_component(lane)));
        }
        if let Some(lane_run_id) = lane_run_id {
            query.push(format!("lane_run_id={lane_run_id}"));
        }
        let suffix = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.send(
            Method::GET,
            &format!("/news/api/forge/branch/{branch_id}/logs{suffix}"),
            None::<&()>,
            true,
        )
    }

    pub(crate) fn nightly_detail(
        &self,
        nightly_run_id: i64,
    ) -> anyhow::Result<NightlyDetailResponse> {
        self.send(
            Method::GET,
            &format!("/news/api/forge/nightly/{nightly_run_id}"),
            None::<&()>,
            true,
        )
    }

    pub(crate) fn merge_branch(&self, branch_id: i64) -> anyhow::Result<BranchActionResponse> {
        self.send(
            Method::POST,
            &format!("/news/api/forge/branch/{branch_id}/merge"),
            Some(&serde_json::json!({})),
            true,
        )
    }

    pub(crate) fn close_branch(&self, branch_id: i64) -> anyhow::Result<BranchActionResponse> {
        self.send(
            Method::POST,
            &format!("/news/api/forge/branch/{branch_id}/close"),
            Some(&serde_json::json!({})),
            true,
        )
    }

    pub(crate) fn fail_branch_ci_lane(
        &self,
        branch_id: i64,
        lane_run_id: i64,
    ) -> anyhow::Result<LaneMutationResponse> {
        self.send(
            Method::POST,
            &format!("/news/branch/{branch_id}/ci/fail/{lane_run_id}"),
            Some(&serde_json::json!({})),
            true,
        )
    }

    pub(crate) fn requeue_branch_ci_lane(
        &self,
        branch_id: i64,
        lane_run_id: i64,
    ) -> anyhow::Result<LaneMutationResponse> {
        self.send(
            Method::POST,
            &format!("/news/branch/{branch_id}/ci/requeue/{lane_run_id}"),
            Some(&serde_json::json!({})),
            true,
        )
    }

    pub(crate) fn recover_branch_ci_run(
        &self,
        branch_id: i64,
        run_id: i64,
    ) -> anyhow::Result<RecoverRunResponse> {
        self.send(
            Method::POST,
            &format!("/news/branch/{branch_id}/ci/recover/{run_id}"),
            Some(&serde_json::json!({})),
            true,
        )
    }

    pub(crate) fn fail_nightly_lane(
        &self,
        nightly_run_id: i64,
        lane_run_id: i64,
    ) -> anyhow::Result<LaneMutationResponse> {
        self.send(
            Method::POST,
            &format!("/news/nightly/{nightly_run_id}/fail/{lane_run_id}"),
            Some(&serde_json::json!({})),
            true,
        )
    }

    pub(crate) fn requeue_nightly_lane(
        &self,
        nightly_run_id: i64,
        lane_run_id: i64,
    ) -> anyhow::Result<LaneMutationResponse> {
        self.send(
            Method::POST,
            &format!("/news/nightly/{nightly_run_id}/requeue/{lane_run_id}"),
            Some(&serde_json::json!({})),
            true,
        )
    }

    pub(crate) fn recover_nightly_run(
        &self,
        nightly_run_id: i64,
    ) -> anyhow::Result<RecoverRunResponse> {
        self.send(
            Method::POST,
            &format!("/news/nightly/{nightly_run_id}/recover"),
            Some(&serde_json::json!({})),
            true,
        )
    }

    pub(crate) fn wake_ci(&self) -> anyhow::Result<WakeCiResponse> {
        self.send(
            Method::POST,
            "/news/api/forge/ci/wake",
            Some(&serde_json::json!({})),
            true,
        )
    }

    fn send<T, B>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        require_auth: bool,
    ) -> anyhow::Result<T>
    where
        T: for<'de> Deserialize<'de>,
        B: Serialize + ?Sized,
    {
        let url = format!("{}{}", self.base_url, path);
        let mut request = self
            .client
            .request(method.clone(), &url)
            .header("Accept", "application/json");
        if require_auth {
            let token = self
                .token
                .as_deref()
                .ok_or_else(|| anyhow!("not logged in; run `ph login` first"))?;
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        send_json(request, method, &url)
    }
}

fn send_json<T>(request: RequestBuilder, method: Method, url: &str) -> anyhow::Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let response = request
        .send()
        .with_context(|| format!("send {} {}", method, url))?;
    let status = response.status();
    let body = response.text().unwrap_or_default();
    if !status.is_success() {
        return Err(http_error(method, url, status, &body));
    }
    serde_json::from_str(&body).with_context(|| format!("parse {} {} response JSON", method, url))
}

fn http_error(method: Method, url: &str, status: StatusCode, body: &str) -> anyhow::Error {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| body.trim().to_string());
    anyhow!(
        "{} {} failed: {} {}",
        method,
        url,
        status.as_u16(),
        if message.is_empty() {
            status
                .canonical_reason()
                .unwrap_or("request failed")
                .to_string()
        } else {
            message
        }
    )
}
