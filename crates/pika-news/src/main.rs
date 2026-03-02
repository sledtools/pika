mod cli;
mod config;
mod github;
mod local;
mod model;
mod poller;
mod render;
mod storage;
mod tutorial;
mod worker;

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
            let worker_result = worker::run_generation_pass(&store, &config)
                .context("run initial hosted generation pass")?;
            println!(
                "serve mode scaffold ready: cli_bind={} config_bind={}:{} db={} repos={} poll_interval_secs={} model={} api_key_env={} prs_seen={} queued={} head_sha_changes={} worker_claimed={} worker_ready={} worker_failed={} worker_retry_scheduled={}",
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
                poll_result.head_sha_changes,
                worker_result.claimed,
                worker_result.ready,
                worker_result.failed,
                worker_result.retry_scheduled
            );
        }
        Commands::Local(args) => {
            local::run(&args)?;
        }
    }

    Ok(())
}
