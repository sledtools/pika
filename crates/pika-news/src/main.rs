mod cli;
mod config;
mod github;
mod local;
mod model;
mod poller;
mod storage;
mod tutorial;

use anyhow::Context;
use clap::Parser;
use cli::{Cli, Commands};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve(args) => {
            let config = config::load(&args.config).context("load config file")?;
            let store = storage::Store::open(&args.db).context("initialize sqlite storage")?;
            let poll_result =
                poller::poll_once(&store, &config).context("run initial poller sync")?;
            println!(
                "serve mode scaffold ready: cli_bind={} config_bind={}:{} db={} repos={} poll_interval_secs={} model={} api_key_env={} prs_seen={} queued={} head_sha_changes={}",
                args.bind(),
                config.bind_address,
                config.bind_port,
                store.db_path().display(),
                config.repos.len(),
                config.poll_interval_secs,
                config.model,
                config.api_key_env,
                poll_result.prs_seen,
                poll_result.queued_regenerations,
                poll_result.head_sha_changes
            );
        }
        Commands::Local(args) => {
            local::run(&args)?;
        }
    }

    Ok(())
}
