use std::fs;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub repos: Vec<String>,
    pub poll_interval_secs: u64,
    pub model: String,
    pub api_key_env: String,
}

pub fn load(path: &Path) -> anyhow::Result<Config> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("read config file {}", path.display()))?;
    let config: Config =
        toml::from_str(&raw).with_context(|| format!("parse config file {}", path.display()))?;
    Ok(config)
}
