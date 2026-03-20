use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use clap::{Parser, Subcommand};
use nostr::{EventBuilder, Keys, Kind, Tag, TagKind};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use url::Url;

const DEFAULT_BASE_URL: &str = "https://git.pikachat.org";
const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        PhCommand::Login(args) => cmd_login(&cli, args.clone()),
        PhCommand::Whoami => cmd_whoami(&cli),
        PhCommand::Logout => cmd_logout(&cli),
        PhCommand::Status { branch_or_id } => cmd_status(&cli, branch_or_id.as_deref()),
        PhCommand::Wait {
            branch_or_id,
            poll_secs,
        } => cmd_wait(&cli, branch_or_id.as_deref(), *poll_secs),
        PhCommand::Logs {
            branch_or_id,
            lane,
            lane_run_id,
        } => cmd_logs(&cli, branch_or_id.as_deref(), lane.as_deref(), *lane_run_id),
        PhCommand::Merge { branch_or_id } => cmd_merge(&cli, branch_or_id.as_deref()),
        PhCommand::Close { branch_or_id } => cmd_close(&cli, branch_or_id.as_deref()),
        PhCommand::Url { branch_or_id } => cmd_url(&cli, branch_or_id.as_deref()),
        PhCommand::FailLane(args) => cmd_fail_lane(&cli, args),
        PhCommand::RequeueLane(args) => cmd_requeue_lane(&cli, args),
        PhCommand::RecoverRun(args) => cmd_recover_run(&cli, args),
        PhCommand::WakeCi => cmd_wake_ci(&cli),
    }
}

#[derive(Debug, Parser)]
#[command(name = "ph")]
#[command(version, propagate_version = true)]
#[command(about = "Thin forge control-plane client")]
pub struct Cli {
    #[arg(long, global = true, env = "PH_BASE_URL")]
    base_url: Option<String>,

    #[arg(long, global = true, default_value_os_t = default_state_dir())]
    state_dir: PathBuf,

    #[command(subcommand)]
    command: PhCommand,
}

#[derive(Debug, Subcommand)]
enum PhCommand {
    Login(LoginArgs),
    Whoami,
    Logout,
    Status {
        branch_or_id: Option<String>,
    },
    Wait {
        branch_or_id: Option<String>,
        #[arg(long, default_value_t = DEFAULT_POLL_INTERVAL_SECS)]
        poll_secs: u64,
    },
    Logs {
        branch_or_id: Option<String>,
        #[arg(long)]
        lane: Option<String>,
        #[arg(long)]
        lane_run_id: Option<i64>,
    },
    Merge {
        branch_or_id: Option<String>,
    },
    Close {
        branch_or_id: Option<String>,
    },
    Url {
        branch_or_id: Option<String>,
    },
    FailLane(LaneActionArgs),
    RequeueLane(LaneActionArgs),
    RecoverRun(RecoverRunArgs),
    WakeCi,
}

#[derive(Debug, Clone, clap::Args)]
struct LoginArgs {
    #[arg(long, conflicts_with = "nsec_file")]
    nsec: Option<String>,
    #[arg(long, conflicts_with = "nsec")]
    nsec_file: Option<PathBuf>,
}

#[derive(Debug, Clone, clap::Args)]
struct LaneActionArgs {
    branch_or_id: Option<String>,
    #[arg(long, conflicts_with = "branch_or_id")]
    nightly_run_id: Option<i64>,
    #[arg(
        long,
        conflicts_with = "lane_run_id",
        required_unless_present = "lane_run_id"
    )]
    lane: Option<String>,
    #[arg(long, conflicts_with = "lane", required_unless_present = "lane")]
    lane_run_id: Option<i64>,
}

#[derive(Debug, Clone, clap::Args)]
struct RecoverRunArgs {
    branch_or_id: Option<String>,
    #[arg(long, conflicts_with = "branch_or_id")]
    nightly_run_id: Option<i64>,
    #[arg(long, conflicts_with = "nightly_run_id")]
    run_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Session {
    base_url: String,
    token: String,
    npub: String,
    is_admin: bool,
    can_forge_write: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct ChallengeResponse {
    challenge: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct LoginResponse {
    token: String,
    npub: String,
    is_admin: bool,
    can_forge_write: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct MeResponse {
    npub: String,
    is_admin: bool,
    can_chat: bool,
    can_forge_write: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct BranchResolveResponse {
    branch_id: i64,
    repo: String,
    branch_name: String,
    branch_state: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct BranchDetailResponse {
    branch: BranchSummary,
    ci_runs: Vec<CiRun>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct BranchSummary {
    branch_id: i64,
    repo: String,
    branch_name: String,
    title: String,
    branch_state: String,
    updated_at: String,
    target_branch: String,
    head_sha: String,
    merge_base_sha: String,
    merge_commit_sha: Option<String>,
    tutorial_status: String,
    ci_status: String,
    error_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct CiRun {
    id: i64,
    source_head_sha: String,
    status: String,
    lane_count: usize,
    rerun_of_run_id: Option<i64>,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    lanes: Vec<CiLane>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct CiLane {
    id: i64,
    lane_id: String,
    title: String,
    entrypoint: String,
    status: String,
    pikaci_run_id: Option<String>,
    pikaci_target_id: Option<String>,
    log_text: Option<String>,
    retry_count: i64,
    rerun_of_lane_run_id: Option<i64>,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct BranchLogsResponse {
    branch_id: i64,
    branch_name: String,
    run_id: i64,
    lane: CiLane,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct NightlyDetailResponse {
    nightly_run_id: i64,
    repo: String,
    scheduled_for: String,
    created_at: String,
    source_ref: String,
    source_head_sha: String,
    status: String,
    summary: Option<String>,
    rerun_of_run_id: Option<i64>,
    started_at: Option<String>,
    finished_at: Option<String>,
    lanes: Vec<CiLane>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct LaneMutationResponse {
    status: String,
    branch_id: Option<i64>,
    nightly_run_id: Option<i64>,
    lane_run_id: i64,
    lane_status: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct RecoverRunResponse {
    status: String,
    branch_id: Option<i64>,
    run_id: Option<i64>,
    nightly_run_id: Option<i64>,
    recovered_lane_count: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct WakeCiResponse {
    status: String,
    message: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct BranchActionResponse {
    status: String,
    branch_id: i64,
    merge_commit_sha: Option<String>,
    deleted: Option<bool>,
}

#[derive(Debug, Serialize)]
struct VerifyRequest<'a> {
    event: &'a str,
}

fn cmd_login(cli: &Cli, args: LoginArgs) -> anyhow::Result<()> {
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

fn cmd_whoami(cli: &Cli) -> anyhow::Result<()> {
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

fn cmd_logout(cli: &Cli) -> anyhow::Result<()> {
    let path = session_path(&cli.state_dir);
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        println!("logged out");
    } else {
        println!("already logged out");
    }
    Ok(())
}

fn cmd_status(cli: &Cli, branch_or_id: Option<&str>) -> anyhow::Result<()> {
    let branch = load_branch_detail(cli, branch_or_id)?;
    print_branch_status(&branch);
    Ok(())
}

fn cmd_wait(cli: &Cli, branch_or_id: Option<&str>, poll_secs: u64) -> anyhow::Result<()> {
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
            return if branch.branch.ci_status == "success" {
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

fn cmd_logs(
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
    Ok(())
}

fn cmd_merge(cli: &Cli, branch_or_id: Option<&str>) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    let resolved = resolve_branch_ref(&api, branch_or_id)?;
    let response = api.merge_branch(resolved.branch_id)?;
    println!(
        "merged branch #{}{}",
        response.branch_id,
        response
            .merge_commit_sha
            .as_deref()
            .map(|sha| format!(" {}", sha))
            .unwrap_or_default()
    );
    Ok(())
}

fn cmd_close(cli: &Cli, branch_or_id: Option<&str>) -> anyhow::Result<()> {
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

fn cmd_url(cli: &Cli, branch_or_id: Option<&str>) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url.clone(), Some(session.token))?;
    let resolved = resolve_branch_ref(&api, branch_or_id)?;
    println!(
        "{}/news/branch/{}",
        base_url.trim_end_matches('/'),
        resolved.branch_id
    );
    Ok(())
}

fn cmd_fail_lane(cli: &Cli, args: &LaneActionArgs) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    execute_lane_action(&api, args, LaneActionKind::Fail)
}

fn cmd_requeue_lane(cli: &Cli, args: &LaneActionArgs) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    execute_lane_action(&api, args, LaneActionKind::Requeue)
}

fn cmd_recover_run(cli: &Cli, args: &RecoverRunArgs) -> anyhow::Result<()> {
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

fn cmd_wake_ci(cli: &Cli) -> anyhow::Result<()> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    let response = api.wake_ci()?;
    println!("{}", response.message);
    Ok(())
}

fn load_branch_detail(
    cli: &Cli,
    branch_or_id: Option<&str>,
) -> anyhow::Result<BranchDetailResponse> {
    let session = load_session(&cli.state_dir)?;
    let base_url = resolve_authenticated_base_url(cli.base_url.as_deref(), &session)?;
    let api = ApiClient::new(base_url, Some(session.token))?;
    let resolved = resolve_branch_ref(&api, branch_or_id)?;
    api.branch_detail(resolved.branch_id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BranchRef {
    branch_id: i64,
    branch_name: Option<String>,
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

fn resolve_branch_ref(api: &ApiClient, branch_or_id: Option<&str>) -> anyhow::Result<BranchRef> {
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

fn infer_current_branch() -> anyhow::Result<String> {
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

fn execute_lane_action(
    api: &ApiClient,
    args: &LaneActionArgs,
    action: LaneActionKind,
) -> anyhow::Result<()> {
    if let Some(nightly_run_id) = args.nightly_run_id {
        let nightly = api.nightly_detail(nightly_run_id)?;
        let lane = resolve_lane_selector(&nightly.lanes, args.lane.as_deref(), args.lane_run_id)?;
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

fn resolve_branch_run_id(
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

fn resolve_branch_lane<'a>(
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

fn resolve_lane_selector<'a>(
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

fn branch_wait_snapshot(branch: &BranchDetailResponse) -> String {
    let active = active_lane_titles(branch).join(",");
    format!(
        "{}|{}|{}",
        branch.branch.ci_status,
        branch.ci_runs.len(),
        active
    )
}

fn branch_ci_active(branch: &BranchDetailResponse) -> bool {
    matches!(branch.branch.ci_status.as_str(), "queued" | "running")
        || branch.ci_runs.iter().any(|run| {
            matches!(run.status.as_str(), "queued" | "running")
                || run
                    .lanes
                    .iter()
                    .any(|lane| matches!(lane.status.as_str(), "queued" | "running"))
        })
}

fn active_lane_titles(branch: &BranchDetailResponse) -> Vec<String> {
    let mut active = Vec::new();
    for run in &branch.ci_runs {
        for lane in &run.lanes {
            if matches!(lane.status.as_str(), "queued" | "running") {
                active.push(lane.lane_id.clone());
            }
        }
    }
    active
}

fn print_branch_status(branch: &BranchDetailResponse) {
    println!(
        "branch #{} {} {} tutorial={} ci={}",
        branch.branch.branch_id,
        branch.branch.branch_name,
        branch.branch.branch_state,
        branch.branch.tutorial_status,
        branch.branch.ci_status
    );
    if let Some(run) = branch.ci_runs.first() {
        println!(
            "run #{} {} head {}",
            run.id,
            run.status,
            short_sha(&run.source_head_sha)
        );
        let active = active_lane_titles(branch);
        if active.is_empty() {
            println!("active lanes: none");
        } else {
            println!("active lanes: {}", active.join(", "));
        }
        for lane in &run.lanes {
            match (&lane.pikaci_run_id, &lane.pikaci_target_id) {
                (Some(run_id), Some(target)) => {
                    println!("- {} {} [{} {}]", lane.lane_id, lane.status, target, run_id);
                }
                (Some(run_id), None) => {
                    println!("- {} {} [pikaci {}]", lane.lane_id, lane.status, run_id);
                }
                _ => println!("- {} {}", lane.lane_id, lane.status),
            }
        }
    } else {
        println!("ci runs: none yet");
    }
}

fn short_sha(sha: &str) -> &str {
    let len = sha.len().min(12);
    &sha[..len]
}

fn build_nip98_verify_event_json(
    nsec: &str,
    base_url: &str,
    challenge: &str,
) -> anyhow::Result<String> {
    let keys = Keys::parse(nsec).context("parse Nostr signing key")?;
    let verify_url = format!("{}/news/auth/verify", base_url.trim_end_matches('/'));
    let event = EventBuilder::new(Kind::Custom(27235), challenge)
        .tags([
            Tag::custom(TagKind::custom("u"), [verify_url.as_str()]),
            Tag::custom(TagKind::custom("method"), ["POST"]),
        ])
        .sign_with_keys(&keys)
        .context("sign NIP-98 auth event")?;
    serde_json::to_string(&event).context("serialize signed NIP-98 auth event")
}

fn login_nsec(nsec: Option<&str>, nsec_file: Option<&Path>) -> anyhow::Result<String> {
    match (nsec, nsec_file) {
        (Some(value), None) => Ok(value.trim().to_string()),
        (None, Some(path)) => read_nsec_file(path),
        (None, None) => bail!("nsec is required; pass --nsec or --nsec-file"),
        (Some(_), Some(_)) => unreachable!("clap enforces conflicts"),
    }
}

fn read_nsec_file(path: &Path) -> anyhow::Result<String> {
    let raw = if path == Path::new("-") {
        let mut stdin = String::new();
        std::io::stdin()
            .read_to_string(&mut stdin)
            .context("read nsec from stdin")?;
        stdin
    } else {
        fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
    };
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        bail!("nsec file is empty");
    }
    Ok(trimmed)
}

fn default_state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_STATE_HOME") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("ph");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed)
                .join(".local")
                .join("state")
                .join("ph");
        }
    }
    PathBuf::from(".ph")
}

fn session_path(state_dir: &Path) -> PathBuf {
    state_dir.join("session.json")
}

fn save_session(state_dir: &Path, session: &Session) -> anyhow::Result<()> {
    fs::create_dir_all(state_dir).with_context(|| format!("create {}", state_dir.display()))?;
    let path = session_path(state_dir);
    let bytes = serde_json::to_vec_pretty(session).context("serialize session")?;
    let mut file = fs::File::create(&path).with_context(|| format!("create {}", path.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod {}", path.display()))?;
    }
    Ok(())
}

fn load_session(state_dir: &Path) -> anyhow::Result<Session> {
    let path = session_path(state_dir);
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("read {} (run `ph login` first)", path.display()))?;
    serde_json::from_str(&raw).context("parse ph session")
}

fn resolve_base_url(explicit: Option<&str>, session: Option<&Session>) -> anyhow::Result<String> {
    let base = explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| session.map(|session| session.base_url.clone()))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let url = Url::parse(&base).with_context(|| format!("parse base url {}", base))?;
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn resolve_authenticated_base_url(
    explicit: Option<&str>,
    session: &Session,
) -> anyhow::Result<String> {
    let session_base_url = resolve_base_url(None, Some(session))?;
    let explicit_base_url = resolve_base_url(explicit, None)?;
    if let Some(explicit) = explicit
        && !explicit.trim().is_empty()
        && explicit_base_url != session_base_url
    {
        bail!(
            "saved session belongs to {}; refusing to reuse its token for {}",
            session_base_url,
            explicit_base_url
        );
    }
    Ok(session_base_url)
}

fn encode_query_component(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

struct ApiClient {
    base_url: String,
    token: Option<String>,
    client: Client,
}

impl ApiClient {
    fn new(base_url: String, token: Option<String>) -> anyhow::Result<Self> {
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

    fn challenge(&self) -> anyhow::Result<ChallengeResponse> {
        self.send(Method::POST, "/news/auth/challenge", None::<&()>, false)
    }

    fn verify(&self, event_json: &str) -> anyhow::Result<LoginResponse> {
        self.send(
            Method::POST,
            "/news/auth/verify",
            Some(&VerifyRequest { event: event_json }),
            false,
        )
    }

    fn me(&self) -> anyhow::Result<MeResponse> {
        self.send(Method::GET, "/news/api/me", None::<&()>, true)
    }

    fn resolve_branch(&self, branch_name: &str) -> anyhow::Result<BranchResolveResponse> {
        let path = format!(
            "/news/api/forge/branch/resolve?branch_name={}",
            encode_query_component(branch_name)
        );
        self.send(Method::GET, &path, None::<&()>, true)
    }

    fn branch_detail(&self, branch_id: i64) -> anyhow::Result<BranchDetailResponse> {
        self.send(
            Method::GET,
            &format!("/news/api/forge/branch/{branch_id}"),
            None::<&()>,
            true,
        )
    }

    fn branch_logs(
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

    fn nightly_detail(&self, nightly_run_id: i64) -> anyhow::Result<NightlyDetailResponse> {
        self.send(
            Method::GET,
            &format!("/news/api/forge/nightly/{nightly_run_id}"),
            None::<&()>,
            true,
        )
    }

    fn merge_branch(&self, branch_id: i64) -> anyhow::Result<BranchActionResponse> {
        self.send(
            Method::POST,
            &format!("/news/api/forge/branch/{branch_id}/merge"),
            Some(&serde_json::json!({})),
            true,
        )
    }

    fn close_branch(&self, branch_id: i64) -> anyhow::Result<BranchActionResponse> {
        self.send(
            Method::POST,
            &format!("/news/api/forge/branch/{branch_id}/close"),
            Some(&serde_json::json!({})),
            true,
        )
    }

    fn fail_branch_ci_lane(
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

    fn requeue_branch_ci_lane(
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

    fn recover_branch_ci_run(
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

    fn fail_nightly_lane(
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

    fn requeue_nightly_lane(
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

    fn recover_nightly_run(&self, nightly_run_id: i64) -> anyhow::Result<RecoverRunResponse> {
        self.send(
            Method::POST,
            &format!("/news/nightly/{nightly_run_id}/recover"),
            Some(&serde_json::json!({})),
            true,
        )
    }

    fn wake_ci(&self) -> anyhow::Result<WakeCiResponse> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::routing::{get, post};
    use axum::{Json, Router};
    use nostr::ToBech32;
    use tempfile::tempdir;

    fn cwd_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn infer_current_branch_reads_git_worktree() {
        let _guard = cwd_test_lock().lock().expect("lock cwd test");
        let dir = tempdir().expect("temp dir");
        git(dir.path(), &["init"]);
        git(dir.path(), &["config", "user.name", "Test User"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        fs::write(dir.path().join("README.md"), "hello\n").expect("write file");
        git(dir.path(), &["add", "README.md"]);
        git(dir.path(), &["commit", "-m", "init"]);
        git(dir.path(), &["checkout", "-b", "feature/ph"]);

        let cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(dir.path()).expect("chdir");
        let branch = infer_current_branch().expect("infer branch");
        std::env::set_current_dir(cwd).expect("restore cwd");

        assert_eq!(branch, "feature/ph");
    }

    #[test]
    fn login_persists_session_against_auth_flow() {
        let state_dir = tempdir().expect("state dir");
        let keys = Keys::generate();
        let nsec = keys.secret_key().to_secret_hex();
        let expected_npub = keys.public_key().to_bech32().expect("npub");
        let base_url = spawn_test_server(
            Router::new()
                .route(
                    "/news/auth/challenge",
                    post(|| async { Json(serde_json::json!({"challenge": "nonce-123"})) }),
                )
                .route("/news/auth/verify", {
                    let expected_npub = expected_npub.clone();
                    post(move |Json(body): Json<serde_json::Value>| {
                        let expected_npub = expected_npub.clone();
                        async move {
                            let event_raw = body["event"].as_str().expect("event json");
                            let event: serde_json::Value =
                                serde_json::from_str(event_raw).expect("parse event");
                            assert_eq!(event["content"], "nonce-123");
                            Json(serde_json::json!({
                                "token": "bearer-123",
                                "npub": expected_npub,
                                "is_admin": false,
                                "can_forge_write": true
                            }))
                        }
                    })
                }),
        );

        let cli = Cli::parse_from([
            "ph",
            "--state-dir",
            state_dir.path().to_str().expect("state dir path"),
            "--base-url",
            &base_url,
            "login",
            "--nsec",
            &nsec,
        ]);
        cmd_login(
            &cli,
            LoginArgs {
                nsec: Some(nsec.clone()),
                nsec_file: None,
            },
        )
        .expect("login");

        let session = load_session(state_dir.path()).expect("session");
        assert_eq!(session.token, "bearer-123");
        assert_eq!(session.npub, expected_npub);
        assert!(session.can_forge_write);
    }

    #[test]
    fn wait_returns_error_when_ci_fails() {
        let state_dir = tempdir().expect("state dir");
        save_session(
            state_dir.path(),
            &Session {
                base_url: "http://placeholder".to_string(),
                token: "token".to_string(),
                npub: "npub1test".to_string(),
                is_admin: false,
                can_forge_write: true,
            },
        )
        .expect("save session");

        let calls = Arc::new(AtomicUsize::new(0));
        let base_url = spawn_test_server({
            let calls = Arc::clone(&calls);
            Router::new()
                .route("/news/api/forge/branch/resolve", get(|| async {
                    Json(serde_json::json!({
                        "branch_id": 7,
                        "repo": "sledtools/pika",
                        "branch_name": "feature/wait",
                        "branch_state": "open"
                    }))
                }))
                .route(
                    "/news/api/forge/branch/7",
                    get(move || {
                        let calls = Arc::clone(&calls);
                        async move {
                            let idx = calls.fetch_add(1, Ordering::SeqCst);
                            let ci_status = if idx == 0 { "running" } else { "failed" };
                            Json(serde_json::json!({
                                "branch": {
                                    "branch_id": 7,
                                    "repo": "sledtools/pika",
                                    "branch_name": "feature/wait",
                                    "title": "wait",
                                    "branch_state": "open",
                                    "updated_at": "2026-03-19T00:00:00Z",
                                    "target_branch": "master",
                                    "head_sha": "deadbeef",
                                    "merge_base_sha": "base",
                                    "merge_commit_sha": null,
                                    "tutorial_status": "ready",
                                    "ci_status": ci_status,
                                    "error_message": null
                                },
                                "ci_runs": [{
                                    "id": 5,
                                    "source_head_sha": "deadbeef",
                                    "status": ci_status,
                                    "lane_count": 1,
                                    "rerun_of_run_id": null,
                                    "created_at": "2026-03-19T00:00:00Z",
                                    "started_at": "2026-03-19T00:00:01Z",
                                    "finished_at": if ci_status == "failed" { serde_json::json!("2026-03-19T00:00:02Z") } else { serde_json::Value::Null },
                                    "lanes": [{
                                        "id": 9,
                                        "lane_id": "check-pika",
                                        "title": "check-pika",
                                        "entrypoint": "just checks::pre-merge-pika",
                                        "status": ci_status,
                                        "pikaci_run_id": null,
                                        "pikaci_target_id": null,
                                        "log_text": "boom",
                                        "retry_count": 0,
                                        "rerun_of_lane_run_id": null,
                                        "created_at": "2026-03-19T00:00:00Z",
                                        "started_at": "2026-03-19T00:00:01Z",
                                        "finished_at": if ci_status == "failed" { serde_json::json!("2026-03-19T00:00:02Z") } else { serde_json::Value::Null }
                                    }]
                                }]
                            }))
                        }
                    }),
                )
        });
        let mut session = load_session(state_dir.path()).expect("session");
        session.base_url = base_url.clone();
        save_session(state_dir.path(), &session).expect("update session");
        let cli = Cli::parse_from([
            "ph",
            "--state-dir",
            state_dir.path().to_str().expect("state dir path"),
            "wait",
            "--poll-secs",
            "0",
            "feature/wait",
        ]);
        let result = cmd_wait(
            &cli,
            match &cli.command {
                PhCommand::Wait { branch_or_id, .. } => branch_or_id.as_deref(),
                _ => unreachable!(),
            },
            0,
        );
        assert!(result.is_err());
    }

    #[test]
    fn authenticated_commands_refuse_cross_host_token_reuse() {
        let state_dir = tempdir().expect("state dir");
        save_session(
            state_dir.path(),
            &Session {
                base_url: "https://git.pikachat.org".to_string(),
                token: "token".to_string(),
                npub: "npub1test".to_string(),
                is_admin: false,
                can_forge_write: true,
            },
        )
        .expect("save session");

        let cli = Cli::parse_from([
            "ph",
            "--state-dir",
            state_dir.path().to_str().expect("state dir path"),
            "--base-url",
            "https://other-host.example",
            "whoami",
        ]);
        let err = cmd_whoami(&cli).expect_err("cross-host token reuse should fail");
        assert!(err.to_string().contains("refusing to reuse its token"));
    }

    #[test]
    fn resolve_branch_ref_accepts_closed_branch_name() {
        let state_dir = tempdir().expect("state dir");
        save_session(
            state_dir.path(),
            &Session {
                base_url: "http://placeholder".to_string(),
                token: "token".to_string(),
                npub: "npub1test".to_string(),
                is_admin: false,
                can_forge_write: true,
            },
        )
        .expect("save session");

        let base_url = spawn_test_server(Router::new().route(
            "/news/api/forge/branch/resolve",
            get(|| async {
                Json(serde_json::json!({
                    "branch_id": 19,
                    "repo": "sledtools/pika",
                    "branch_name": "feature/history",
                    "branch_state": "merged"
                }))
            }),
        ));
        let mut session = load_session(state_dir.path()).expect("session");
        session.base_url = base_url.clone();
        save_session(state_dir.path(), &session).expect("save session");
        let api = ApiClient::new(base_url, Some(session.token)).expect("api");

        let resolved = resolve_branch_ref(&api, Some("feature/history")).expect("resolve branch");

        assert_eq!(
            resolved,
            BranchRef {
                branch_id: 19,
                branch_name: Some("feature/history".to_string()),
            }
        );
    }

    #[test]
    fn merge_and_close_use_authenticated_json_endpoints() {
        let state_dir = tempdir().expect("state dir");
        save_session(
            state_dir.path(),
            &Session {
                base_url: "http://placeholder".to_string(),
                token: "token-123".to_string(),
                npub: "npub1test".to_string(),
                is_admin: false,
                can_forge_write: true,
            },
        )
        .expect("save session");
        let merge_auth = Arc::new(AtomicUsize::new(0));
        let close_auth = Arc::new(AtomicUsize::new(0));
        let base_url = spawn_test_server({
            let merge_auth = Arc::clone(&merge_auth);
            let close_auth = Arc::clone(&close_auth);
            Router::new()
                .route(
                    "/news/api/forge/branch/resolve",
                    get(|| async {
                        Json(serde_json::json!({
                            "branch_id": 11,
                            "repo": "sledtools/pika",
                            "branch_name": "feature/merge",
                            "branch_state": "open"
                        }))
                    }),
                )
                .route(
                    "/news/api/forge/branch/11/merge",
                    post(move |headers: axum::http::HeaderMap| {
                        let merge_auth = Arc::clone(&merge_auth);
                        async move {
                            if headers.get("authorization").and_then(|v| v.to_str().ok())
                                == Some("Bearer token-123")
                            {
                                merge_auth.fetch_add(1, Ordering::SeqCst);
                            }
                            Json(serde_json::json!({
                                "status": "ok",
                                "branch_id": 11,
                                "merge_commit_sha": "abc123"
                            }))
                        }
                    }),
                )
                .route(
                    "/news/api/forge/branch/11/close",
                    post(move |headers: axum::http::HeaderMap| {
                        let close_auth = Arc::clone(&close_auth);
                        async move {
                            if headers.get("authorization").and_then(|v| v.to_str().ok())
                                == Some("Bearer token-123")
                            {
                                close_auth.fetch_add(1, Ordering::SeqCst);
                            }
                            Json(serde_json::json!({
                                "status": "ok",
                                "branch_id": 11,
                                "deleted": true
                            }))
                        }
                    }),
                )
        });
        let mut session = load_session(state_dir.path()).expect("session");
        session.base_url = base_url;
        save_session(state_dir.path(), &session).expect("save session");
        let cli = Cli::parse_from([
            "ph",
            "--state-dir",
            state_dir.path().to_str().expect("state dir path"),
            "merge",
            "feature/merge",
        ]);
        cmd_merge(&cli, Some("feature/merge")).expect("merge");
        let cli = Cli::parse_from([
            "ph",
            "--state-dir",
            state_dir.path().to_str().expect("state dir path"),
            "close",
            "feature/merge",
        ]);
        cmd_close(&cli, Some("feature/merge")).expect("close");
        assert_eq!(merge_auth.load(Ordering::SeqCst), 1);
        assert_eq!(close_auth.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fail_lane_resolves_branch_lane_name_against_latest_run() {
        let state_dir = tempdir().expect("state dir");
        save_session(
            state_dir.path(),
            &Session {
                base_url: "http://placeholder".to_string(),
                token: "token-123".to_string(),
                npub: "npub1test".to_string(),
                is_admin: false,
                can_forge_write: true,
            },
        )
        .expect("save session");
        let fail_auth = Arc::new(AtomicUsize::new(0));
        let base_url = spawn_test_server({
            let fail_auth = Arc::clone(&fail_auth);
            Router::new()
                .route(
                    "/news/api/forge/branch/resolve",
                    get(|| async {
                        Json(serde_json::json!({
                            "branch_id": 7,
                            "repo": "sledtools/pika",
                            "branch_name": "feature/recover",
                            "branch_state": "open"
                        }))
                    }),
                )
                .route(
                    "/news/api/forge/branch/7",
                    get(|| async {
                        Json(serde_json::json!({
                            "branch": {
                                "branch_id": 7,
                                "repo": "sledtools/pika",
                                "branch_name": "feature/recover",
                                "title": "recover",
                                "branch_state": "open",
                                "updated_at": "2026-03-19T00:00:00Z",
                                "target_branch": "master",
                                "head_sha": "deadbeef",
                                "merge_base_sha": "base",
                                "merge_commit_sha": null,
                                "tutorial_status": "ready",
                                "ci_status": "running",
                                "error_message": null
                            },
                            "ci_runs": [
                                {
                                    "id": 5,
                                    "source_head_sha": "deadbeef",
                                    "status": "running",
                                    "lane_count": 1,
                                    "rerun_of_run_id": null,
                                    "created_at": "2026-03-19T00:00:00Z",
                                    "started_at": "2026-03-19T00:00:01Z",
                                    "finished_at": null,
                                    "lanes": [{
                                        "id": 91,
                                        "lane_id": "check-pika",
                                        "title": "check-pika",
                                        "entrypoint": "just checks::pre-merge-pika",
                                        "status": "running",
                                        "pikaci_run_id": null,
                                        "pikaci_target_id": null,
                                        "log_text": null,
                                        "retry_count": 0,
                                        "rerun_of_lane_run_id": null,
                                        "created_at": "2026-03-19T00:00:00Z",
                                        "started_at": "2026-03-19T00:00:01Z",
                                        "finished_at": null
                                    }]
                                },
                                {
                                    "id": 4,
                                    "source_head_sha": "cafebabe",
                                    "status": "failed",
                                    "lane_count": 1,
                                    "rerun_of_run_id": null,
                                    "created_at": "2026-03-18T00:00:00Z",
                                    "started_at": "2026-03-18T00:00:01Z",
                                    "finished_at": "2026-03-18T00:00:10Z",
                                    "lanes": [{
                                        "id": 90,
                                        "lane_id": "check-pika",
                                        "title": "check-pika",
                                        "entrypoint": "just checks::pre-merge-pika",
                                        "status": "failed",
                                        "pikaci_run_id": null,
                                        "pikaci_target_id": null,
                                        "log_text": "boom",
                                        "retry_count": 0,
                                        "rerun_of_lane_run_id": null,
                                        "created_at": "2026-03-18T00:00:00Z",
                                        "started_at": "2026-03-18T00:00:01Z",
                                        "finished_at": "2026-03-18T00:00:10Z"
                                    }]
                                }
                            ]
                        }))
                    }),
                )
                .route(
                    "/news/branch/7/ci/fail/91",
                    post(move |headers: axum::http::HeaderMap| {
                        let fail_auth = Arc::clone(&fail_auth);
                        async move {
                            if headers.get("authorization").and_then(|v| v.to_str().ok())
                                == Some("Bearer token-123")
                            {
                                fail_auth.fetch_add(1, Ordering::SeqCst);
                            }
                            Json(serde_json::json!({
                                "status": "ok",
                                "branch_id": 7,
                                "lane_run_id": 91,
                                "lane_status": "failed"
                            }))
                        }
                    }),
                )
        });
        let mut session = load_session(state_dir.path()).expect("session");
        session.base_url = base_url;
        save_session(state_dir.path(), &session).expect("save session");
        let cli = Cli::parse_from([
            "ph",
            "--state-dir",
            state_dir.path().to_str().expect("state dir path"),
            "fail-lane",
            "feature/recover",
            "--lane",
            "check-pika",
        ]);

        cmd_fail_lane(
            &cli,
            match &cli.command {
                PhCommand::FailLane(args) => args,
                _ => unreachable!(),
            },
        )
        .expect("fail lane");

        assert_eq!(fail_auth.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn requeue_lane_resolves_nightly_lane_name() {
        let state_dir = tempdir().expect("state dir");
        save_session(
            state_dir.path(),
            &Session {
                base_url: "http://placeholder".to_string(),
                token: "token-123".to_string(),
                npub: "npub1test".to_string(),
                is_admin: false,
                can_forge_write: true,
            },
        )
        .expect("save session");
        let requeue_auth = Arc::new(AtomicUsize::new(0));
        let base_url = spawn_test_server({
            let requeue_auth = Arc::clone(&requeue_auth);
            Router::new()
                .route(
                    "/news/api/forge/nightly/12",
                    get(|| async {
                        Json(serde_json::json!({
                            "nightly_run_id": 12,
                            "repo": "sledtools/pika",
                            "scheduled_for": "2026-03-19T00:00:00Z",
                            "created_at": "2026-03-19T00:00:00Z",
                            "source_ref": "refs/heads/master",
                            "source_head_sha": "deadbeef",
                            "status": "running",
                            "summary": null,
                            "rerun_of_run_id": null,
                            "started_at": "2026-03-19T00:00:01Z",
                            "finished_at": null,
                            "lanes": [{
                                "id": 44,
                                "lane_id": "nightly_pika",
                                "title": "nightly-pika",
                                "entrypoint": "just checks::nightly-pika-e2e",
                                "status": "running",
                                "pikaci_run_id": null,
                                "pikaci_target_id": null,
                                "log_text": null,
                                "retry_count": 0,
                                "rerun_of_lane_run_id": null,
                                "created_at": "2026-03-19T00:00:00Z",
                                "started_at": "2026-03-19T00:00:01Z",
                                "finished_at": null
                            }]
                        }))
                    }),
                )
                .route(
                    "/news/nightly/12/requeue/44",
                    post(move |headers: axum::http::HeaderMap| {
                        let requeue_auth = Arc::clone(&requeue_auth);
                        async move {
                            if headers.get("authorization").and_then(|v| v.to_str().ok())
                                == Some("Bearer token-123")
                            {
                                requeue_auth.fetch_add(1, Ordering::SeqCst);
                            }
                            Json(serde_json::json!({
                                "status": "ok",
                                "nightly_run_id": 12,
                                "lane_run_id": 44,
                                "lane_status": "queued"
                            }))
                        }
                    }),
                )
        });
        let mut session = load_session(state_dir.path()).expect("session");
        session.base_url = base_url;
        save_session(state_dir.path(), &session).expect("save session");
        let cli = Cli::parse_from([
            "ph",
            "--state-dir",
            state_dir.path().to_str().expect("state dir path"),
            "requeue-lane",
            "--nightly-run-id",
            "12",
            "--lane",
            "nightly_pika",
        ]);

        cmd_requeue_lane(
            &cli,
            match &cli.command {
                PhCommand::RequeueLane(args) => args,
                _ => unreachable!(),
            },
        )
        .expect("requeue lane");

        assert_eq!(requeue_auth.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn wake_ci_hits_scheduler_wake_endpoint() {
        let state_dir = tempdir().expect("state dir");
        save_session(
            state_dir.path(),
            &Session {
                base_url: "http://placeholder".to_string(),
                token: "token-123".to_string(),
                npub: "npub1test".to_string(),
                is_admin: false,
                can_forge_write: true,
            },
        )
        .expect("save session");
        let wake_auth = Arc::new(AtomicUsize::new(0));
        let base_url = spawn_test_server({
            let wake_auth = Arc::clone(&wake_auth);
            Router::new().route(
                "/news/api/forge/ci/wake",
                post(move |headers: axum::http::HeaderMap| {
                    let wake_auth = Arc::clone(&wake_auth);
                    async move {
                        if headers.get("authorization").and_then(|v| v.to_str().ok())
                            == Some("Bearer token-123")
                        {
                            wake_auth.fetch_add(1, Ordering::SeqCst);
                        }
                        Json(serde_json::json!({
                            "status": "ok",
                            "message": "scheduler wake requested"
                        }))
                    }
                }),
            )
        });
        let mut session = load_session(state_dir.path()).expect("session");
        session.base_url = base_url;
        save_session(state_dir.path(), &session).expect("save session");
        let cli = Cli::parse_from([
            "ph",
            "--state-dir",
            state_dir.path().to_str().expect("state dir path"),
            "wake-ci",
        ]);

        cmd_wake_ci(&cli).expect("wake ci");

        assert_eq!(wake_auth.load(Ordering::SeqCst), 1);
    }

    fn git<P: AsRef<Path>>(cwd: P, args: &[&str]) {
        let output = ProcessCommand::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn spawn_test_server(app: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        listener
            .set_nonblocking(true)
            .expect("set listener nonblocking");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            runtime.block_on(async move {
                axum::serve(
                    tokio::net::TcpListener::from_std(listener).expect("tokio listener"),
                    app,
                )
                .await
                .expect("serve test app");
            });
        });
        std::thread::sleep(Duration::from_millis(50));
        format!("http://{}", addr)
    }
}
