use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use pikahut::{config, fixture};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "pikahut")]
#[command(about = "Unified test environment runner for Pika")]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start fixture components (foreground by default)
    Up {
        #[arg(long, default_value = "backend")]
        profile: config::ProfileName,

        /// TOML overlay config file
        #[arg(long)]
        config: Option<PathBuf>,

        /// Use a temp dir cleaned up on exit
        #[arg(long)]
        ephemeral: bool,

        /// Run in background (default is foreground)
        #[arg(long)]
        background: bool,

        #[arg(long)]
        relay_port: Option<u16>,

        #[arg(long)]
        moq_port: Option<u16>,

        #[arg(long)]
        server_port: Option<u16>,

        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    /// Stop all fixture components
    Down {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    /// Show status of fixture components
    Status {
        #[arg(long)]
        state_dir: Option<PathBuf>,

        #[arg(long)]
        json: bool,
    },
    /// Stream component logs
    Logs {
        #[arg(long)]
        state_dir: Option<PathBuf>,

        #[arg(long)]
        follow: bool,

        #[arg(long)]
        component: Option<String>,
    },
    /// Print shell export lines for the running fixture
    Env {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    /// Run a command with fixture env vars injected
    Exec {
        #[arg(long)]
        state_dir: Option<PathBuf>,

        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Block until all components are healthy
    Wait {
        #[arg(long)]
        state_dir: Option<PathBuf>,

        #[arg(long, default_value = "60")]
        timeout: u64,
    },
    /// Kill all pikahut processes and remove state directory
    Nuke {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cli = Cli::parse();

    match cli.cmd {
        Command::Up {
            profile,
            config: config_path,
            ephemeral,
            background,
            relay_port,
            moq_port,
            server_port,
            state_dir,
        } => {
            let overlay = match config_path {
                Some(p) => {
                    let raw = std::fs::read_to_string(&p)?;
                    Some(toml::from_str::<config::OverlayConfig>(&raw)?)
                }
                None => None,
            };

            if ephemeral && background {
                anyhow::bail!(
                    "--ephemeral and --background cannot be used together \
                     (temp dir would be deleted when this process exits)"
                );
            }

            let resolved = config::ResolvedConfig::new(
                profile,
                overlay,
                ephemeral,
                relay_port,
                moq_port,
                server_port,
                state_dir,
            )?;

            if background {
                fixture::up_background(&resolved).await
            } else {
                fixture::up_foreground(&resolved).await
            }
        }
        Command::Down { state_dir } => {
            let dir = config::resolve_state_dir(state_dir)?;
            fixture::down(&dir).await
        }
        Command::Status { state_dir, json } => {
            let dir = config::resolve_state_dir(state_dir)?;
            fixture::status(&dir, json).await
        }
        Command::Logs {
            state_dir,
            follow,
            component,
        } => {
            let dir = config::resolve_state_dir(state_dir)?;
            fixture::logs(&dir, follow, component.as_deref()).await
        }
        Command::Env { state_dir } => {
            let dir = config::resolve_state_dir(state_dir)?;
            fixture::print_env(&dir).await
        }
        Command::Exec { state_dir, command } => {
            let dir = config::resolve_state_dir(state_dir)?;
            fixture::exec(&dir, &command).await
        }
        Command::Wait { state_dir, timeout } => {
            let dir = config::resolve_state_dir(state_dir)?;
            fixture::wait(&dir, timeout).await
        }
        Command::Nuke { state_dir } => {
            let dir = config::resolve_state_dir(state_dir)?;
            fixture::nuke(&dir).await
        }
    }
}
