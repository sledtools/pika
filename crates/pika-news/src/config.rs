use std::fs;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 60;
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-5-20250929";
pub const DEFAULT_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
pub const DEFAULT_GITHUB_TOKEN_ENV: &str = "GITHUB_TOKEN";
pub const DEFAULT_MERGED_LOOKBACK_HOURS: u64 = 72;
pub const DEFAULT_WORKER_CONCURRENCY: usize = 2;
pub const DEFAULT_RETRY_BACKOFF_SECS: u64 = 120;
pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1";
pub const DEFAULT_BIND_PORT: u16 = 8787;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub repos: Vec<String>,
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
    #[serde(default = "default_github_token_env")]
    pub github_token_env: String,
    #[serde(default = "default_merged_lookback_hours")]
    pub merged_lookback_hours: u64,
    #[serde(default = "default_worker_concurrency")]
    pub worker_concurrency: usize,
    #[serde(default = "default_retry_backoff_secs")]
    pub retry_backoff_secs: u64,
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    #[serde(default = "default_bind_port")]
    pub bind_port: u16,
}

pub fn load(path: &Path) -> anyhow::Result<Config> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("read config file {}", path.display()))?;
    let config: Config =
        toml::from_str(&raw).with_context(|| format!("parse config file {}", path.display()))?;
    Ok(config)
}

fn default_poll_interval_secs() -> u64 {
    DEFAULT_POLL_INTERVAL_SECS
}

fn default_model() -> String {
    DEFAULT_MODEL.to_string()
}

fn default_api_key_env() -> String {
    DEFAULT_API_KEY_ENV.to_string()
}

fn default_bind_address() -> String {
    DEFAULT_BIND_ADDRESS.to_string()
}

fn default_bind_port() -> u16 {
    DEFAULT_BIND_PORT
}

fn default_github_token_env() -> String {
    DEFAULT_GITHUB_TOKEN_ENV.to_string()
}

fn default_merged_lookback_hours() -> u64 {
    DEFAULT_MERGED_LOOKBACK_HOURS
}

fn default_worker_concurrency() -> usize {
    DEFAULT_WORKER_CONCURRENCY
}

fn default_retry_backoff_secs() -> u64 {
    DEFAULT_RETRY_BACKOFF_SECS
}
