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
    /// Bind address for the hosted web server.
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: String,
    /// Bind port for the hosted web server.
    #[arg(long, default_value_t = 8787)]
    pub port: u16,
}

impl ServeArgs {
    pub fn bind(&self) -> String {
        format!("{}:{}", self.bind, self.port)
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
