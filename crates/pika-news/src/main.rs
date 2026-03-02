mod cli;
mod config;
mod local;
mod tutorial;

use anyhow::Context;
use clap::Parser;
use cli::{Cli, Commands};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve(args) => {
            let config = config::load(&args.config).context("load config file")?;
            println!(
                "serve mode scaffold ready: cli_bind={} config_bind={}:{} repos={} poll_interval_secs={} model={} api_key_env={}",
                args.bind(),
                config.bind_address,
                config.bind_port,
                config.repos.len(),
                config.poll_interval_secs,
                config.model,
                config.api_key_env
            );
        }
        Commands::Local(args) => {
            local::run(&args)?;
        }
    }

    Ok(())
}
