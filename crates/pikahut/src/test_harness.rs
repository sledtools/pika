use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};

use crate::testing::scenarios::{
    self, CliSmokeRequest, InteropRustBaselineRequest, OpenclawE2eRequest, ScenarioRequest,
    UiE2eLocalRequest,
};

#[derive(Debug, Subcommand)]
pub enum TestCommand {
    /// Run a pikachat scenario with optional local relay fixture wiring.
    Scenario(TestScenarioArgs),
    /// Run full OpenClaw gateway integration E2E.
    OpenclawE2e(OpenclawE2eArgs),
    /// Run CLI smoke test (invite + welcome + message, optional media).
    CliSmoke(CliSmokeArgs),
    /// Run deterministic local UI E2E against relay+bot fixture.
    UiE2eLocal(UiE2eLocalArgs),
    /// Run baseline interop against external rust_harness bot.
    InteropRustBaseline(InteropRustBaselineArgs),
}

#[derive(Debug, Args)]
pub struct TestScenarioArgs {
    pub scenario: String,

    #[arg(long)]
    pub state_dir: Option<PathBuf>,

    #[arg(long)]
    pub relay: Option<String>,

    #[arg(last = true)]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct OpenclawE2eArgs {
    #[arg(long)]
    pub state_dir: Option<PathBuf>,

    #[arg(long)]
    pub relay_url: Option<String>,

    #[arg(long)]
    pub openclaw_dir: Option<PathBuf>,

    /// Keep generated state/artifacts on success too.
    #[arg(long, default_value_t = false)]
    pub keep_state: bool,
}

#[derive(Debug, Args)]
pub struct CliSmokeArgs {
    #[arg(long)]
    pub relay: Option<String>,

    #[arg(long, default_value_t = false)]
    pub with_media: bool,

    #[arg(long)]
    pub state_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum UiPlatform {
    Android,
    Ios,
    Desktop,
}

#[derive(Debug, Args)]
pub struct UiE2eLocalArgs {
    #[arg(long, value_enum)]
    pub platform: UiPlatform,

    #[arg(long)]
    pub state_dir: Option<PathBuf>,

    /// Keep state directory after completion (also honored via KEEP=1).
    #[arg(long, default_value_t = false)]
    pub keep: bool,

    #[arg(long)]
    pub bot_timeout_sec: Option<u64>,
}

#[derive(Debug, Args)]
pub struct InteropRustBaselineArgs {
    #[arg(long, default_value_t = false)]
    pub manual: bool,

    #[arg(long, default_value_t = false)]
    pub keep: bool,

    #[arg(long)]
    pub state_dir: Option<PathBuf>,

    #[arg(long)]
    pub rust_interop_dir: Option<PathBuf>,

    #[arg(long)]
    pub bot_timeout_sec: Option<u64>,
}

pub async fn run(command: TestCommand) -> Result<()> {
    match command {
        TestCommand::Scenario(args) => scenarios::run_scenario(ScenarioRequest {
            scenario: args.scenario,
            state_dir: args.state_dir,
            relay: args.relay,
            extra_args: args.extra_args,
        })
        .await
        .map(|_| ()),
        TestCommand::OpenclawE2e(args) => scenarios::run_openclaw_e2e(OpenclawE2eRequest {
            state_dir: args.state_dir,
            relay_url: args.relay_url,
            openclaw_dir: args.openclaw_dir,
            keep_state: args.keep_state,
        })
        .await
        .map(|_| ()),
        TestCommand::CliSmoke(args) => scenarios::run_cli_smoke(CliSmokeRequest {
            relay: args.relay,
            with_media: args.with_media,
            state_dir: args.state_dir,
        })
        .await
        .map(|_| ()),
        TestCommand::UiE2eLocal(args) => scenarios::run_ui_e2e_local(UiE2eLocalRequest {
            platform: match args.platform {
                UiPlatform::Android => scenarios::UiPlatform::Android,
                UiPlatform::Ios => scenarios::UiPlatform::Ios,
                UiPlatform::Desktop => scenarios::UiPlatform::Desktop,
            },
            state_dir: args.state_dir,
            keep: args.keep,
            bot_timeout_sec: args.bot_timeout_sec,
        })
        .await
        .map(|_| ()),
        TestCommand::InteropRustBaseline(args) => {
            scenarios::run_interop_rust_baseline(InteropRustBaselineRequest {
                manual: args.manual,
                keep: args.keep,
                state_dir: args.state_dir,
                rust_interop_dir: args.rust_interop_dir,
                bot_timeout_sec: args.bot_timeout_sec,
            })
            .await
            .map(|_| ())
        }
    }
}
