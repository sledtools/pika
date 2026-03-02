use std::fs;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub repos: Vec<String>,
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
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
    60
}

fn default_model() -> String {
    "claude-sonnet-4-5-20250929".to_string()
}

fn default_api_key_env() -> String {
    "ANTHROPIC_API_KEY".to_string()
}

fn default_bind_address() -> String {
    "127.0.0.1".to_string()
}

fn default_bind_port() -> u16 {
    8787
}
