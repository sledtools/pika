use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ci_manifest::ForgeLane;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiLaneStatus {
    Queued,
    Running,
    Success,
    Failed,
    Skipped,
}

impl CiLaneStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

impl fmt::Display for CiLaneStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CiLaneStatus {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "success" => Ok(Self::Success),
            "failed" => Ok(Self::Failed),
            "skipped" => Ok(Self::Skipped),
            _ => Err("unknown ci lane status"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CiLaneExecutionReason {
    #[default]
    Queued,
    Running,
    BlockedByConcurrencyGroup,
    WaitingForCapacity,
    StaleRecovered,
}

impl CiLaneExecutionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::BlockedByConcurrencyGroup => "blocked_by_concurrency_group",
            Self::WaitingForCapacity => "waiting_for_capacity",
            Self::StaleRecovered => "stale_recovered",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::BlockedByConcurrencyGroup => "blocked by concurrency group",
            Self::WaitingForCapacity => "waiting for scheduler capacity",
            Self::StaleRecovered => "recovered from stale lease",
        }
    }
}

impl fmt::Display for CiLaneExecutionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CiLaneExecutionReason {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "blocked_by_concurrency_group" => Ok(Self::BlockedByConcurrencyGroup),
            "waiting_for_capacity" => Ok(Self::WaitingForCapacity),
            "stale_recovered" => Ok(Self::StaleRecovered),
            _ => Err("unknown ci lane execution reason"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiLaneFailureKind {
    TestFailure,
    Timeout,
    Infrastructure,
}

impl CiLaneFailureKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TestFailure => "test_failure",
            Self::Timeout => "timeout",
            Self::Infrastructure => "infrastructure",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::TestFailure => "test failure",
            Self::Timeout => "timeout",
            Self::Infrastructure => "infrastructure failure",
        }
    }
}

impl fmt::Display for CiLaneFailureKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CiLaneFailureKind {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "test_failure" => Ok(Self::TestFailure),
            "timeout" => Ok(Self::Timeout),
            "infrastructure" => Ok(Self::Infrastructure),
            _ => Err("unknown ci lane failure kind"),
        }
    }
}

pub fn classify_ci_failure(log_text: &str) -> CiLaneFailureKind {
    let lower = log_text.to_ascii_lowercase();
    let timeout_markers = [
        "timed out after",
        "timeout after",
        "status=timed_out",
        " timed out ",
        "operation timed out",
    ];
    if timeout_markers.iter().any(|marker| lower.contains(marker)) {
        return CiLaneFailureKind::Timeout;
    }

    let infrastructure_markers = [
        "ci runner error:",
        "permission denied",
        "connection refused",
        "connection reset",
        "broken pipe",
        "no route to host",
        "could not resolve host",
        "failed to connect",
        "ssh: ",
        "remote launcher",
        "prepared-output",
        "prepare node `prepare-",
        "runner build failed",
        "missing runner binary",
        "unbound variable",
        "virtiofsd",
        "reported request_path=",
        "refusing local nix copy fallback",
    ];
    if infrastructure_markers
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return CiLaneFailureKind::Infrastructure;
    }

    CiLaneFailureKind::TestFailure
}

pub fn configured_target_key_for_lane(lane: &ForgeLane) -> Option<String> {
    effective_target_key(
        Some(lane.id.as_str()),
        None,
        lane.concurrency_group.as_deref(),
    )
}

pub fn effective_target_key(
    configured_target_key: Option<&str>,
    pikaci_target_id: Option<&str>,
    concurrency_group: Option<&str>,
) -> Option<String> {
    pikaci_target_id
        .and_then(non_empty_trimmed)
        .or_else(|| configured_target_key.and_then(non_empty_trimmed))
        .or_else(|| inferred_target_key_from_concurrency_group(concurrency_group))
}

pub fn inferred_target_key_from_concurrency_group(
    concurrency_group: Option<&str>,
) -> Option<String> {
    match concurrency_group.and_then(non_empty_trimmed).as_deref() {
        Some("apple-host") => Some("apple-host".to_string()),
        Some("nightly-android") => Some("nightly-android".to_string()),
        _ => None,
    }
}

fn non_empty_trimmed(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_ci_failure, CiLaneExecutionReason, CiLaneFailureKind, CiLaneStatus};

    #[test]
    fn typed_states_round_trip_through_serde_names() {
        let status = serde_json::to_string(&CiLaneStatus::Queued).expect("serialize status");
        let reason = serde_json::to_string(&CiLaneExecutionReason::WaitingForCapacity)
            .expect("serialize reason");
        let failure =
            serde_json::to_string(&CiLaneFailureKind::Infrastructure).expect("serialize failure");
        assert_eq!(status, "\"queued\"");
        assert_eq!(reason, "\"waiting_for_capacity\"");
        assert_eq!(failure, "\"infrastructure\"");
    }

    #[test]
    fn failure_classifier_distinguishes_timeout_infra_and_test_failures() {
        assert_eq!(
            classify_ci_failure("[ci] job finished: test · status=failed · timed out after 60s"),
            CiLaneFailureKind::Timeout
        );
        assert_eq!(
            classify_ci_failure("ci runner error: PIKACI_APPLE_SSH_KEY: unbound variable"),
            CiLaneFailureKind::Infrastructure
        );
        assert_eq!(
            classify_ci_failure("test command exited with 101"),
            CiLaneFailureKind::TestFailure
        );
    }
}
