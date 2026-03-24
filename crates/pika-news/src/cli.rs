use std::env;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "pika-news", about = "Generate browser-first branch tutorials")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run hosted mode web service for the configured canonical forge repository.
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

    pub fn resolved_config_path(&self, cwd: &Path) -> PathBuf {
        absolutize_path(cwd, &self.config)
    }

    pub fn resolved_db_path(&self, cwd: &Path) -> PathBuf {
        absolutize_path(cwd, &self.db)
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

fn absolutize_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

pub fn current_dir() -> anyhow::Result<PathBuf> {
    env::current_dir().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::ServeArgs;

    #[test]
    fn serve_args_resolve_relative_paths_against_cwd() {
        let args = ServeArgs {
            config: PathBuf::from("pika-news.toml"),
            bind: None,
            port: None,
            db: PathBuf::from(".tmp/pika-news.db"),
            max_prs: 2,
        };
        let cwd = PathBuf::from("/tmp/pika-news-dev");
        assert_eq!(
            args.resolved_config_path(&cwd),
            PathBuf::from("/tmp/pika-news-dev/pika-news.toml")
        );
        assert_eq!(
            args.resolved_db_path(&cwd),
            PathBuf::from("/tmp/pika-news-dev/.tmp/pika-news.db")
        );
    }

    #[test]
    fn serve_args_keep_absolute_paths() {
        let args = ServeArgs {
            config: PathBuf::from("/srv/pika-news.toml"),
            bind: None,
            port: None,
            db: PathBuf::from("/srv/pika-news.db"),
            max_prs: 2,
        };
        let cwd = PathBuf::from("/tmp/pika-news-dev");
        assert_eq!(
            args.resolved_config_path(&cwd),
            PathBuf::from("/srv/pika-news.toml")
        );
        assert_eq!(
            args.resolved_db_path(&cwd),
            PathBuf::from("/srv/pika-news.db")
        );
    }
}
