mod auth;
mod cli;
mod config;
mod github;
mod local;
mod model;
mod poller;
mod render;
mod storage;
mod tutorial;
mod web;
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
            let max_prs = args.max_prs;
            let bind_addr = args.bind_with_config(&config.bind_address, config.bind_port);
            println!(
                "serve: bind={} db={} repos={} poll_interval={}s model={}",
                bind_addr,
                store.db_path().display(),
                config.repos.len(),
                config.poll_interval_secs,
                config.model,
            );

            let runtime = tokio::runtime::Runtime::new().context("create tokio runtime")?;
            runtime
                .block_on(web::serve(store, config, bind_addr, max_prs))
                .context("run hosted web server")?;
        }
        Commands::Local(args) => {
            local::run(&args)?;
        }
    }

    Ok(())
}
