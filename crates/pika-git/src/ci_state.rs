use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::ci_manifest::ForgeLane;

pub const CI_TARGET_HEALTH_INFRA_FAILURE_THRESHOLD: i64 = 2;
pub const CI_TARGET_HEALTH_COOLOFF_MINUTES: i64 = 15;

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
    TargetUnhealthy,
    StaleRecovered,
}

impl CiLaneExecutionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::BlockedByConcurrencyGroup => "blocked_by_concurrency_group",
            Self::WaitingForCapacity => "waiting_for_capacity",
            Self::TargetUnhealthy => "target_unhealthy",
            Self::StaleRecovered => "stale_recovered",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::BlockedByConcurrencyGroup => "blocked by concurrency group",
            Self::WaitingForCapacity => "waiting for scheduler capacity",
            Self::TargetUnhealthy => "blocked by unhealthy target",
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
            "target_unhealthy" => Ok(Self::TargetUnhealthy),
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

    pub const fn counts_toward_target_health(self) -> bool {
        matches!(self, Self::Timeout | Self::Infrastructure)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiTargetHealthState {
    Healthy,
    Unhealthy,
}

impl CiTargetHealthState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Unhealthy => "unhealthy",
        }
    }
}

impl fmt::Display for CiTargetHealthState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CiTargetHealthState {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "healthy" => Ok(Self::Healthy),
            "unhealthy" => Ok(Self::Unhealthy),
            _ => Err("unknown ci target health state"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiTargetHealthSnapshot {
    pub target_id: String,
    pub state: CiTargetHealthState,
    pub consecutive_infra_failure_count: i64,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub last_failure_kind: Option<CiLaneFailureKind>,
    pub cooloff_until: Option<String>,
}

impl CiTargetHealthSnapshot {
    pub fn effective_state(&self, now: DateTime<Utc>) -> CiTargetHealthState {
        if self.state == CiTargetHealthState::Unhealthy
            && self
                .cooloff_until
                .as_deref()
                .and_then(parse_ci_timestamp)
                .is_some_and(|cooloff_until| cooloff_until > now)
        {
            CiTargetHealthState::Unhealthy
        } else {
            CiTargetHealthState::Healthy
        }
    }

    pub fn is_currently_unhealthy(&self, now: DateTime<Utc>) -> bool {
        self.effective_state(now) == CiTargetHealthState::Unhealthy
    }

    pub fn cooloff_active_until(&self, now: DateTime<Utc>) -> Option<String> {
        let until = self.cooloff_until.as_deref().and_then(parse_ci_timestamp)?;
        if until > now {
            Some(self.cooloff_until.clone().unwrap_or_default())
        } else {
            None
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
        lane.staged_linux_target.as_deref(),
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

pub fn next_target_cooloff_until(now: DateTime<Utc>) -> String {
    (now + Duration::minutes(CI_TARGET_HEALTH_COOLOFF_MINUTES))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn parse_ci_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|value| DateTime::<Utc>::from_naive_utc_and_offset(value, Utc))
        })
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
    use super::{
        classify_ci_failure, next_target_cooloff_until, parse_ci_timestamp, CiLaneExecutionReason,
        CiLaneFailureKind, CiLaneStatus, CiTargetHealthSnapshot, CiTargetHealthState,
    };
    use chrono::{Duration, Utc};

    #[test]
    fn typed_states_round_trip_through_serde_names() {
        let status = serde_json::to_string(&CiLaneStatus::Queued).expect("serialize status");
        let reason = serde_json::to_string(&CiLaneExecutionReason::WaitingForCapacity)
            .expect("serialize reason");
        let failure =
            serde_json::to_string(&CiLaneFailureKind::Infrastructure).expect("serialize failure");
        let health =
            serde_json::to_string(&CiTargetHealthState::Unhealthy).expect("serialize health");
        assert_eq!(status, "\"queued\"");
        assert_eq!(reason, "\"waiting_for_capacity\"");
        assert_eq!(failure, "\"infrastructure\"");
        assert_eq!(health, "\"unhealthy\"");
    }

    #[test]
    fn failure_classifier_distinguishes_timeout_infra_and_test_failures() {
        assert_eq!(
            classify_ci_failure(
                "[pikaci] job finished: test · status=failed · timed out after 60s"
            ),
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

    #[test]
    fn target_health_snapshot_only_blocks_while_cooloff_is_active() {
        let now = Utc::now();
        let active_until =
            (now + Duration::minutes(5)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let expired_until =
            (now - Duration::minutes(1)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let active = CiTargetHealthSnapshot {
            target_id: "apple-host".to_string(),
            state: CiTargetHealthState::Unhealthy,
            consecutive_infra_failure_count: 2,
            last_success_at: None,
            last_failure_at: None,
            last_failure_kind: Some(CiLaneFailureKind::Infrastructure),
            cooloff_until: Some(active_until.clone()),
        };
        let expired = CiTargetHealthSnapshot {
            target_id: "apple-host".to_string(),
            state: CiTargetHealthState::Unhealthy,
            consecutive_infra_failure_count: 2,
            last_success_at: None,
            last_failure_at: None,
            last_failure_kind: Some(CiLaneFailureKind::Infrastructure),
            cooloff_until: Some(expired_until),
        };
        assert!(active.is_currently_unhealthy(now));
        assert_eq!(active.cooloff_active_until(now), Some(active_until));
        assert!(!expired.is_currently_unhealthy(now));
    }

    #[test]
    fn next_cooloff_until_is_in_the_future() {
        let now = Utc::now();
        let until = next_target_cooloff_until(now);
        let parsed = parse_ci_timestamp(&until).expect("parse cooloff timestamp");
        assert!(parsed > now);
    }
}
