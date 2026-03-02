mod cli;
mod config;

use anyhow::Context;
use clap::Parser;
use cli::{Cli, Commands};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve(args) => {
            let config = config::load(&args.config).context("load config file")?;
            println!(
                "serve mode scaffold ready: bind={} repos={} poll_interval_secs={} model={} api_key_env={}",
                args.bind(),
                config.repos.len(),
                config.poll_interval_secs,
                config.model,
                config.api_key_env
            );
        }
        Commands::Local(args) => {
            println!(
                "local mode scaffold ready: base={:?} include_uncommitted={} out={:?} no_open={}",
                args.base, args.include_uncommitted, args.out, args.no_open
            );
        }
    }

    Ok(())
}
