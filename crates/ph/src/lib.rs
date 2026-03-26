mod api;
mod commands;
mod resolve;
mod session;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        PhCommand::Login(args) => commands::cmd_login(&cli, args.clone()),
        PhCommand::Whoami => commands::cmd_whoami(&cli),
        PhCommand::Logout => commands::cmd_logout(&cli),
        PhCommand::Status { branch_or_id } => commands::cmd_status(&cli, branch_or_id.as_deref()),
        PhCommand::Wait {
            branch_or_id,
            poll_secs,
        } => commands::cmd_wait(&cli, branch_or_id.as_deref(), *poll_secs),
        PhCommand::Logs {
            branch_or_id,
            lane,
            lane_run_id,
        } => commands::cmd_logs(&cli, branch_or_id.as_deref(), lane.as_deref(), *lane_run_id),
        PhCommand::Merge {
            branch_or_id,
            force,
        } => commands::cmd_merge(&cli, branch_or_id.as_deref(), *force),
        PhCommand::Close { branch_or_id } => commands::cmd_close(&cli, branch_or_id.as_deref()),
        PhCommand::Url { branch_or_id } => commands::cmd_url(&cli, branch_or_id.as_deref()),
        PhCommand::FailLane(args) => commands::cmd_fail_lane(&cli, args),
        PhCommand::RequeueLane(args) => commands::cmd_requeue_lane(&cli, args),
        PhCommand::RecoverRun(args) => commands::cmd_recover_run(&cli, args),
        PhCommand::WakeCi => commands::cmd_wake_ci(&cli),
    }
}

#[derive(Debug, Parser)]
#[command(name = "ph")]
#[command(version, propagate_version = true)]
#[command(about = "Thin forge control-plane client")]
pub struct Cli {
    #[arg(long, global = true, env = "PH_BASE_URL")]
    base_url: Option<String>,

    #[arg(long, global = true, default_value_os_t = session::default_state_dir())]
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
        #[arg(long)]
        force: bool,
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
