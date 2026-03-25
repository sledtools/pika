mod auth;
mod branch_store;
mod ci;
mod ci_manifest;
mod ci_state;
mod cli;
mod config;
mod forge;
mod forge_runtime;
mod forge_service;
mod github;
mod live;
mod local;
mod mirror;
mod model;
mod pikaci_store;
mod poller;
mod render;
mod storage;
mod tutorial;
mod web;
mod worker;

use anyhow::Context;
use clap::Parser;
use cli::{Cli, Commands};
use std::fs;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve(args) => {
            let cwd = cli::current_dir().context("resolve current working directory")?;
            let config_path = args.resolved_config_path(&cwd);
            let db_path = args.resolved_db_path(&cwd);
            if let Some(parent) = db_path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("create sqlite parent directory {}", parent.display())
                })?;
            }

            let config = config::load(&config_path).context("load config file")?;
            let store = storage::Store::open(&db_path).context("initialize sqlite storage")?;
            let max_prs = args.max_prs;
            let bind_addr = args.bind_with_config(&config.bind_address, config.bind_port);

            // Recover artifacts stuck in 'generating' from a previous unclean shutdown.
            match store.recover_stale_generating() {
                Ok(0) => {}
                Ok(n) => eprintln!("recovered {} stale generating artifact(s)", n),
                Err(err) => eprintln!("warning: failed to recover stale generating: {}", err),
            }
            match store.recover_stale_ci_lanes() {
                Ok(0) => {}
                Ok(n) => eprintln!("recovered {} stale ci lane(s)", n),
                Err(err) => eprintln!("warning: failed to recover stale ci lanes: {}", err),
            }

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
