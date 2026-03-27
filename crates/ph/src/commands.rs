use std::thread;
use std::time::Duration;

use anyhow::anyhow;

use crate::api::{
    ApiClient, BranchActionResponse, BranchDetailResponse, BranchLogsResponse, CiLane,
    CiLaneExecutionReason,
};
use crate::resolve::{
    resolve_branch_lane, resolve_branch_ref, resolve_branch_run_id, resolve_nightly_lane,
};
use crate::session::{
    Session, build_nip98_verify_event_json, load_session, login_nsec, remove_session,
    resolve_authenticated_base_url, resolve_base_url, save_session,
};
use crate::{Cli, LaneActionArgs, LoginArgs, RecoverRunArgs};

pub(crate) fn cmd_login(cli: &Cli, args: LoginArgs) -> anyhow::Result<()> {
    let base_url = resolve_base_url(cli.base_url.as_deref(), None)?;
    let nsec = login_nsec(args.nsec.as_deref(), args.nsec_file.as_deref())?;
    let api = ApiClient::new(base_url.clone(), None)?;
    let challenge = api.challenge()?;
    let event = build_nip98_verify_event_json(&nsec, &base_url, &challenge.challenge)?;
    let login = api.verify(&event)?;
    let session = Session {
        base_url,
        token: login.token,
        npub: login.npub,
        is_admin: login.is_admin,
        can_forge_write: login.can_forge_write,
    };
    save_session(&cli.state_dir, &session)?;
    println!(
        "logged in as {} forge_write={} admin={}",
        session.npub, session.can_forge_write, session.is_admin
    );
    Ok(())
}

pub(crate) fn cmd_whoami(cli: &Cli) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    let me = api.me()?;
    println!(
        "{} forge_write={} admin={} chat={}",
        me.npub, me.can_forge_write, me.is_admin, me.can_chat
    );
    Ok(())
}

pub(crate) fn cmd_logout(cli: &Cli) -> anyhow::Result<()> {
    if remove_session(&cli.state_dir)? {
        println!("logged out");
    } else {
        println!("already logged out");
    }
    Ok(())
}

pub(crate) fn cmd_status(cli: &Cli, branch_or_id: Option<&str>) -> anyhow::Result<()> {
    let branch = load_branch_detail(cli, branch_or_id)?;
    print_branch_status(&branch);
    Ok(())
}

pub(crate) fn cmd_wait(
    cli: &Cli,
    branch_or_id: Option<&str>,
    poll_secs: u64,
) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    let resolved = resolve_branch_ref(&api, branch_or_id)?;
    let mut last_snapshot = None;
    loop {
        let branch = api.branch_detail(resolved.branch_id)?;
        let snapshot = branch_wait_snapshot(&branch);
        if last_snapshot.as_ref() != Some(&snapshot) {
            print_branch_status(&branch);
            last_snapshot = Some(snapshot);
        }
        if !branch_ci_active(&branch) {
            return if let Some(run) = stale_branch_ci_run(&branch) {
                Err(anyhow!(
                    "branch ci settled on stale head {} while branch head is {} (latest run #{})",
                    short_sha(&run.source_head_sha),
                    short_sha(&branch.branch.head_sha),
                    run.id
                ))
            } else if branch.branch.ci_status.is_success() {
                Ok(())
            } else {
                Err(anyhow!(
                    "branch ci settled with status {}",
                    branch.branch.ci_status
                ))
            };
        }
        thread::sleep(Duration::from_secs(poll_secs.max(1)));
    }
}

pub(crate) fn cmd_logs(
    cli: &Cli,
    branch_or_id: Option<&str>,
    lane: Option<&str>,
    lane_run_id: Option<i64>,
) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    let resolved = resolve_branch_ref(&api, branch_or_id)?;
    let logs = api.branch_logs(resolved.branch_id, lane, lane_run_id)?;
    print_branch_logs(&logs);
    Ok(())
}

pub(crate) fn cmd_merge(cli: &Cli, branch_or_id: Option<&str>, force: bool) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    let resolved = resolve_branch_ref(&api, branch_or_id)?;
    let branch = api.branch_detail(resolved.branch_id)?;
    ensure_merge_ci_guard(&branch, force)?;
    let response = api.merge_branch(resolved.branch_id)?;
    print_branch_action("merged", &response);
    Ok(())
}

pub(crate) fn cmd_close(cli: &Cli, branch_or_id: Option<&str>) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    let resolved = resolve_branch_ref(&api, branch_or_id)?;
    let response = api.close_branch(resolved.branch_id)?;
    println!(
        "closed branch #{} deleted={}",
        response.branch_id,
        response.deleted.unwrap_or(false)
    );
    Ok(())
}

pub(crate) fn cmd_url(cli: &Cli, branch_or_id: Option<&str>) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url.clone(), Some(session.token))?;
    let resolved = resolve_branch_ref(&api, branch_or_id)?;
    println!(
        "{}/git/branch/{}",
        base_url.trim_end_matches('/'),
        resolved.branch_id
    );
    Ok(())
}

pub(crate) fn cmd_fail_lane(cli: &Cli, args: &LaneActionArgs) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    execute_lane_action(&api, args, LaneActionKind::Fail)
}

pub(crate) fn cmd_requeue_lane(cli: &Cli, args: &LaneActionArgs) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    execute_lane_action(&api, args, LaneActionKind::Requeue)
}

pub(crate) fn cmd_recover_run(cli: &Cli, args: &RecoverRunArgs) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    if let Some(nightly_run_id) = args.nightly_run_id {
        let response = api.recover_nightly_run(nightly_run_id)?;
        println!(
            "recovered nightly #{} lanes={}",
            nightly_run_id, response.recovered_lane_count
        );
        return Ok(());
    }

    let resolved = resolve_branch_ref(&api, args.branch_or_id.as_deref())?;
    let branch = api.branch_detail(resolved.branch_id)?;
    let run_id = resolve_branch_run_id(&branch, args.run_id)?;
    let response = api.recover_branch_ci_run(resolved.branch_id, run_id)?;
    println!(
        "recovered branch #{} run #{} lanes={}",
        resolved.branch_id, run_id, response.recovered_lane_count
    );
    Ok(())
}

pub(crate) fn cmd_wake_ci(cli: &Cli) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    let response = api.wake_ci()?;
    println!("{}", response.message);
    Ok(())
}

pub(crate) fn load_branch_detail(
    cli: &Cli,
    branch_or_id: Option<&str>,
) -> anyhow::Result<BranchDetailResponse> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    let resolved = resolve_branch_ref(&api, branch_or_id)?;
    api.branch_detail(resolved.branch_id)
}

#[derive(Debug, Clone, Copy)]
enum LaneActionKind {
    Fail,
    Requeue,
}

impl LaneActionKind {
    fn verb(self) -> &'static str {
        match self {
            Self::Fail => "failed",
            Self::Requeue => "requeued",
        }
    }
}

fn execute_lane_action(
    api: &ApiClient,
    args: &LaneActionArgs,
    action: LaneActionKind,
) -> anyhow::Result<()> {
    if let Some(nightly_run_id) = args.nightly_run_id {
        let nightly = api.nightly_detail(nightly_run_id)?;
        let lane = resolve_nightly_lane(&nightly, args.lane.as_deref(), args.lane_run_id)?;
        let response = match action {
            LaneActionKind::Fail => api.fail_nightly_lane(nightly_run_id, lane.id)?,
            LaneActionKind::Requeue => api.requeue_nightly_lane(nightly_run_id, lane.id)?,
        };
        println!(
            "{} nightly #{} lane #{} {}",
            action.verb(),
            nightly_run_id,
            response.lane_run_id,
            lane.lane_id
        );
        return Ok(());
    }

    let resolved = resolve_branch_ref(api, args.branch_or_id.as_deref())?;
    let branch = api.branch_detail(resolved.branch_id)?;
    let lane = resolve_branch_lane(&branch, args.lane.as_deref(), args.lane_run_id)?;
    let response = match action {
        LaneActionKind::Fail => api.fail_branch_ci_lane(resolved.branch_id, lane.id)?,
        LaneActionKind::Requeue => api.requeue_branch_ci_lane(resolved.branch_id, lane.id)?,
    };
    println!(
        "{} branch #{} lane #{} {}",
        action.verb(),
        resolved.branch_id,
        response.lane_run_id,
        lane.lane_id
    );
    Ok(())
}

pub(crate) fn branch_wait_snapshot(branch: &BranchDetailResponse) -> String {
    let active = branch
        .ci_runs
        .iter()
        .flat_map(|run| run.lanes.iter().map(render_lane_snapshot_fragment))
        .collect::<Vec<_>>()
        .join(",");
    let latest_run_head = latest_branch_run(branch)
        .map(|run| run.source_head_sha.as_str())
        .unwrap_or_default();
    format!(
        "{}|{}|{}|{}|{}",
        branch.branch.ci_status,
        branch.ci_runs.len(),
        branch.branch.head_sha,
        latest_run_head,
        active
    )
}

pub(crate) fn branch_ci_active(branch: &BranchDetailResponse) -> bool {
    branch.branch.ci_status.is_active()
        || branch.ci_runs.iter().any(|run| {
            run.status.is_active() || run.lanes.iter().any(|lane| lane.status.is_active())
        })
}

fn ensure_merge_ci_guard(branch: &BranchDetailResponse, force: bool) -> anyhow::Result<()> {
    if force {
        return Ok(());
    }
    if let Some(run) = stale_branch_ci_run(branch) {
        return Err(anyhow!(
            "refusing to merge branch #{} because the latest ci run #{} is for head {} while the branch head is {}. Rerun CI for the current head, or pass --force if absolutely necessary; --force is discouraged under normal circumstances.",
            branch.branch.branch_id,
            run.id,
            short_sha(&run.source_head_sha),
            short_sha(&branch.branch.head_sha)
        ));
    }
    if branch.branch.ci_status.is_success() || branch_ci_active(branch) {
        return Ok(());
    }
    Err(anyhow!(
        "refusing to merge branch #{} because ci is {}. Rerun or fix CI, or pass --force if absolutely necessary; --force is discouraged under normal circumstances.",
        branch.branch.branch_id,
        branch.branch.ci_status
    ))
}

fn latest_branch_run(branch: &BranchDetailResponse) -> Option<&crate::api::CiRun> {
    branch.ci_runs.first()
}

fn stale_branch_ci_run(branch: &BranchDetailResponse) -> Option<&crate::api::CiRun> {
    latest_branch_run(branch).filter(|run| run.source_head_sha != branch.branch.head_sha)
}

fn active_lane_titles(branch: &BranchDetailResponse) -> Vec<String> {
    let mut active = Vec::new();
    for run in &branch.ci_runs {
        for lane in &run.lanes {
            if lane.status.is_active() {
                active.push(lane.lane_id.clone());
            }
        }
    }
    active
}

pub(crate) fn print_branch_status(branch: &BranchDetailResponse) {
    print!("{}", render_branch_status(branch));
}

pub(crate) fn render_branch_status(branch: &BranchDetailResponse) -> String {
    let mut lines = vec![format!(
        "branch #{} {} {} tutorial={} ci={}",
        branch.branch.branch_id,
        branch.branch.branch_name,
        branch.branch.branch_state,
        branch.branch.tutorial_status,
        branch.branch.ci_status
    )];
    if let Some(run) = branch.ci_runs.first() {
        lines.push(format!(
            "run #{} {} head {}",
            run.id,
            run.status,
            short_sha(&run.source_head_sha)
        ));
        if run.source_head_sha != branch.branch.head_sha {
            lines.push(format!(
                "warning: latest ci run is for head {} but branch head is {}",
                short_sha(&run.source_head_sha),
                short_sha(&branch.branch.head_sha)
            ));
        }
        let active = active_lane_titles(branch);
        if active.is_empty() {
            lines.push("active lanes: none".to_string());
        } else {
            lines.push(format!("active lanes: {}", active.join(", ")));
        }
        for lane in &run.lanes {
            lines.push(format!("- {}", render_lane_status_line(lane)));
        }
    } else {
        lines.push("ci runs: none yet".to_string());
    }
    format!("{}\n", lines.join("\n"))
}

fn print_branch_logs(logs: &BranchLogsResponse) {
    println!(
        "branch #{} {} run #{} lane #{} {} {}",
        logs.branch_id,
        logs.branch_name,
        logs.run_id,
        logs.lane.id,
        logs.lane.lane_id,
        logs.lane.status
    );
    if let Some(run_id) = &logs.lane.pikaci_run_id {
        println!("pikaci run {run_id}");
    }
    if let Some(target) = &logs.lane.pikaci_target_id {
        println!("pikaci target {target}");
    }
    match logs.lane.log_text.as_deref() {
        Some(text) if !text.trim().is_empty() => {
            println!();
            print!("{text}");
            if !text.ends_with('\n') {
                println!();
            }
        }
        _ => println!("no log text available"),
    }
}

fn print_branch_action(verb: &str, response: &BranchActionResponse) {
    println!(
        "{verb} branch #{}{}",
        response.branch_id,
        response
            .merge_commit_sha
            .as_deref()
            .map(|sha| format!(" {sha}"))
            .unwrap_or_default()
    );
}

fn render_lane_status_line(lane: &CiLane) -> String {
    let mut line = format!("{} {}", lane.lane_id, lane.status);
    if lane.status.is_active()
        && lane.execution_reason.as_str() != CiLaneExecutionReason::Queued.as_str()
        && lane.execution_reason.as_str() != lane.status.as_str()
    {
        line.push_str(" · ");
        line.push_str(lane.execution_reason.label());
    }
    if let Some(failure_kind) = lane.failure_kind.as_ref() {
        line.push_str(" · failure=");
        line.push_str(failure_kind.label());
    }
    let target = lane
        .pikaci_target_id
        .as_deref()
        .or(lane.ci_target_key.as_deref());
    match (&lane.pikaci_run_id, target) {
        (Some(run_id), Some(target)) => {
            line.push_str(&format!(" [{target} {run_id}]"));
        }
        (Some(run_id), None) => {
            line.push_str(&format!(" [pikaci {run_id}]"));
        }
        (None, Some(target)) => {
            line.push_str(&format!(" [{target}]"));
        }
        (None, None) => {}
    }
    line
}

fn render_lane_snapshot_fragment(lane: &CiLane) -> String {
    format!(
        "{}:{}:{}:{}",
        lane.id,
        lane.status,
        lane.execution_reason.as_str(),
        lane.failure_kind
            .as_ref()
            .map(|kind| kind.label().to_string())
            .unwrap_or_default(),
    )
}

fn short_sha(sha: &str) -> &str {
    let len = sha.len().min(12);
    &sha[..len]
}
