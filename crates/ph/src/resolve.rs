use std::process::Command as ProcessCommand;

use anyhow::{Context, anyhow, bail};

use crate::api::{ApiClient, BranchDetailResponse, CiLane, NightlyDetailResponse};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BranchRef {
    pub(crate) branch_id: i64,
    pub(crate) branch_name: Option<String>,
}

pub(crate) fn resolve_branch_ref(
    api: &ApiClient,
    branch_or_id: Option<&str>,
) -> anyhow::Result<BranchRef> {
    match branch_or_id {
        Some(value) => resolve_branch_value(api, value.trim()),
        None => {
            let branch_name = infer_current_branch()?;
            resolve_branch_value(api, &branch_name)
        }
    }
}

fn resolve_branch_value(api: &ApiClient, value: &str) -> anyhow::Result<BranchRef> {
    if value.is_empty() {
        bail!("branch name or id is required");
    }
    if let Ok(branch_id) = value.parse::<i64>() {
        return Ok(BranchRef {
            branch_id,
            branch_name: None,
        });
    }
    let resolved = api.resolve_branch(value)?;
    Ok(BranchRef {
        branch_id: resolved.branch_id,
        branch_name: Some(resolved.branch_name),
    })
}

pub(crate) fn infer_current_branch() -> anyhow::Result<String> {
    let output = ProcessCommand::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .context("run git rev-parse --abbrev-ref HEAD")?;
    if !output.status.success() {
        bail!(
            "failed to infer current Git branch: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let branch = String::from_utf8(output.stdout)
        .context("decode git branch name")?
        .trim()
        .to_string();
    if branch.is_empty() || branch == "HEAD" {
        bail!("current Git branch is detached; pass a branch name or id explicitly");
    }
    Ok(branch)
}

pub(crate) fn resolve_branch_run_id(
    branch: &BranchDetailResponse,
    requested: Option<i64>,
) -> anyhow::Result<i64> {
    if let Some(run_id) = requested {
        if branch.ci_runs.iter().any(|run| run.id == run_id) {
            return Ok(run_id);
        }
        bail!("run #{run_id} was not found on this branch");
    }
    branch
        .ci_runs
        .first()
        .map(|run| run.id)
        .ok_or_else(|| anyhow!("branch has no recorded ci runs"))
}

pub(crate) fn resolve_branch_lane<'a>(
    branch: &'a BranchDetailResponse,
    lane: Option<&str>,
    lane_run_id: Option<i64>,
) -> anyhow::Result<&'a CiLane> {
    let selector = lane_selector(lane, lane_run_id)?;
    for run in &branch.ci_runs {
        if let Ok(found) = resolve_lane_selector(&run.lanes, selector.0, selector.1) {
            return Ok(found);
        }
    }
    match selector {
        (Some(lane), None) => bail!("lane `{lane}` was not found on this branch"),
        (None, Some(lane_run_id)) => bail!("lane run #{lane_run_id} was not found on this branch"),
        _ => unreachable!("lane selector is validated"),
    }
}

pub(crate) fn resolve_nightly_lane<'a>(
    nightly: &'a NightlyDetailResponse,
    lane: Option<&str>,
    lane_run_id: Option<i64>,
) -> anyhow::Result<&'a CiLane> {
    resolve_lane_selector(&nightly.lanes, lane, lane_run_id)
}

fn lane_selector(
    lane: Option<&str>,
    lane_run_id: Option<i64>,
) -> anyhow::Result<(Option<&str>, Option<i64>)> {
    match (
        lane.map(str::trim).filter(|value| !value.is_empty()),
        lane_run_id,
    ) {
        (Some(lane), None) => Ok((Some(lane), None)),
        (None, Some(lane_run_id)) => Ok((None, Some(lane_run_id))),
        _ => bail!("pass exactly one of --lane or --lane-run-id"),
    }
}

pub(crate) fn resolve_lane_selector<'a>(
    lanes: &'a [CiLane],
    lane: Option<&str>,
    lane_run_id: Option<i64>,
) -> anyhow::Result<&'a CiLane> {
    match lane_selector(lane, lane_run_id)? {
        (Some(lane_id), None) => lanes
            .iter()
            .find(|lane| lane.lane_id == lane_id)
            .ok_or_else(|| anyhow!("lane `{lane_id}` was not found")),
        (None, Some(lane_run_id)) => lanes
            .iter()
            .find(|lane| lane.id == lane_run_id)
            .ok_or_else(|| anyhow!("lane run #{lane_run_id} was not found")),
        _ => unreachable!("lane selector is validated"),
    }
}
