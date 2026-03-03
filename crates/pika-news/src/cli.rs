use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "pika-news", about = "Generate browser-first PR tutorials")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run hosted mode web service for configured GitHub repositories.
    Serve(ServeArgs),
    /// Analyze local git changes and open an HTML tutorial.
    Local(LocalArgs),
}

#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Config file path with repo list, model, and polling settings.
    #[arg(long, default_value = "pika-news.toml")]
    pub config: PathBuf,
    /// Override bind address from config file.
    #[arg(long)]
    pub bind: Option<String>,
    /// Override bind port from config file.
    #[arg(long)]
    pub port: Option<u16>,
    /// SQLite database path for hosted mode state.
    #[arg(long, default_value = "pika-news.db")]
    pub db: PathBuf,
    /// Maximum number of PRs to ingest per repo per poll cycle. 0 means unlimited.
    #[arg(long, default_value_t = 0)]
    pub max_prs: usize,
}

impl ServeArgs {
    pub fn bind_with_config(&self, config_bind: &str, config_port: u16) -> String {
        let bind = self.bind.as_deref().unwrap_or(config_bind);
        let port = self.port.unwrap_or(config_port);
        format!("{}:{}", bind, port)
    }
}

#[derive(Debug, Args)]
pub struct LocalArgs {
    /// Override the default base ref (`origin/main` fallback `main`).
    #[arg(long)]
    pub base: Option<String>,
    /// Append staged and unstaged local changes to the generated tutorial input.
    #[arg(long)]
    pub include_uncommitted: bool,
    /// Output HTML file path. Defaults to `./pika-news-local.html`.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Disable opening the generated HTML in a browser.
    #[arg(long)]
    pub no_open: bool,
}
