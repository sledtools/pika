use std::fs;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 60;
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-5-20250929";
pub const DEFAULT_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
pub const DEFAULT_GITHUB_TOKEN_ENV: &str = "GITHUB_TOKEN";
pub const DEFAULT_WORKER_CONCURRENCY: usize = 2;
pub const DEFAULT_RETRY_BACKOFF_SECS: u64 = 120;
pub const DEFAULT_WEBHOOK_SECRET_ENV: &str = "PIKA_GIT_WEBHOOK_SECRET";
pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1";
pub const DEFAULT_BIND_PORT: u16 = 8787;
pub const DEFAULT_FORGE_REPO: &str = "sledtools/pika";
pub const DEFAULT_DEFAULT_BRANCH: &str = "master";
pub const DEFAULT_MIRROR_POLL_INTERVAL_SECS: u64 = 300;
pub const DEFAULT_MIRROR_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Config {
    #[cfg(test)]
    #[serde(default)]
    pub repos: Vec<String>,
    #[serde(default)]
    pub forge_repo: Option<ForgeRepoConfig>,
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
    #[serde(default = "default_github_token_env")]
    pub github_token_env: String,
    #[cfg(test)]
    #[serde(default)]
    pub merged_lookback_hours: u64,
    #[serde(default = "default_worker_concurrency")]
    pub worker_concurrency: usize,
    #[serde(default = "default_retry_backoff_secs")]
    pub retry_backoff_secs: u64,
    #[serde(default = "default_webhook_secret_env")]
    pub webhook_secret_env: String,
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    #[serde(default = "default_bind_port")]
    pub bind_port: u16,
    #[cfg(test)]
    #[serde(default)]
    pub allowed_npubs: Vec<String>,
    #[serde(default)]
    pub bootstrap_admin_npubs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ForgeRepoConfig {
    #[serde(default = "default_forge_repo")]
    pub repo: String,
    pub canonical_git_dir: String,
    #[serde(default = "default_default_branch")]
    pub default_branch: String,
    #[serde(default)]
    pub ci_concurrency: Option<usize>,
    #[serde(default)]
    pub mirror_remote: Option<String>,
    #[serde(default)]
    pub mirror_poll_interval_secs: Option<u64>,
    #[serde(default)]
    pub mirror_timeout_secs: Option<u64>,
    #[serde(default = "default_ci_command")]
    pub ci_command: Vec<String>,
    #[serde(default)]
    pub hook_url: Option<String>,
}

impl Config {
    pub fn effective_forge_repo(&self) -> Option<ForgeRepoConfig> {
        self.forge_repo.clone().map(|mut forge| {
            if forge.repo.trim().is_empty() {
                forge.repo = DEFAULT_FORGE_REPO.to_string();
            }
            if forge.default_branch.trim().is_empty() {
                forge.default_branch = DEFAULT_DEFAULT_BRANCH.to_string();
            }
            if forge.ci_concurrency == Some(0) {
                forge.ci_concurrency = None;
            }
            if forge.ci_command.is_empty() {
                forge.ci_command = default_ci_command();
            }
            if forge.mirror_remote.is_some() && forge.mirror_poll_interval_secs.is_none() {
                forge.mirror_poll_interval_secs = Some(DEFAULT_MIRROR_POLL_INTERVAL_SECS);
            }
            if forge.mirror_remote.is_some() {
                if forge.mirror_timeout_secs == Some(0) {
                    forge.mirror_timeout_secs = None;
                }
                if forge.mirror_timeout_secs.is_none() {
                    forge.mirror_timeout_secs = Some(DEFAULT_MIRROR_TIMEOUT_SECS);
                }
            }
            if forge.hook_url.is_none() {
                forge.hook_url = Some(format!("http://127.0.0.1:{}/git/webhook", self.bind_port));
            }
            forge
        })
    }
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

fn default_webhook_secret_env() -> String {
    DEFAULT_WEBHOOK_SECRET_ENV.to_string()
}

fn default_bind_address() -> String {
    DEFAULT_BIND_ADDRESS.to_string()
}

fn default_bind_port() -> u16 {
    DEFAULT_BIND_PORT
}

fn default_forge_repo() -> String {
    DEFAULT_FORGE_REPO.to_string()
}

fn default_default_branch() -> String {
    DEFAULT_DEFAULT_BRANCH.to_string()
}

fn default_ci_command() -> Vec<String> {
    vec!["just".to_string(), "pre-merge".to_string()]
}

fn default_github_token_env() -> String {
    DEFAULT_GITHUB_TOKEN_ENV.to_string()
}

fn default_worker_concurrency() -> usize {
    DEFAULT_WORKER_CONCURRENCY
}

fn default_retry_backoff_secs() -> u64 {
    DEFAULT_RETRY_BACKOFF_SECS
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn parses_repo_config_contract() {
        let raw = r#"
poll_interval_secs = 30
model = "claude-sonnet-4-5-20250929"
api_key_env = "ANTHROPIC_API_KEY"
github_token_env = "GITHUB_TOKEN"
webhook_secret_env = "MY_WEBHOOK_SECRET"
worker_concurrency = 3
retry_backoff_secs = 90
bind_address = "0.0.0.0"
bind_port = 8080
[forge_repo]
canonical_git_dir = "/srv/pika.git"
default_branch = "master"
mirror_remote = "github"
mirror_poll_interval_secs = 300
mirror_timeout_secs = 120
ci_command = ["just", "pre-merge"]
"#;

        let parsed: Config = toml::from_str(raw).expect("parse config TOML");
        assert_eq!(parsed.poll_interval_secs, 30);
        assert_eq!(parsed.model, "claude-sonnet-4-5-20250929");
        assert_eq!(parsed.api_key_env, "ANTHROPIC_API_KEY");
        assert_eq!(parsed.github_token_env, "GITHUB_TOKEN");
        assert_eq!(parsed.webhook_secret_env, "MY_WEBHOOK_SECRET");
        assert_eq!(parsed.worker_concurrency, 3);
        assert_eq!(parsed.retry_backoff_secs, 90);
        assert_eq!(parsed.bind_address, "0.0.0.0");
        assert_eq!(parsed.bind_port, 8080);
        let forge = parsed.forge_repo.expect("forge repo configured");
        assert_eq!(forge.repo, super::DEFAULT_FORGE_REPO);
        assert_eq!(forge.canonical_git_dir, "/srv/pika.git");
        assert_eq!(forge.default_branch, "master");
        assert_eq!(forge.mirror_remote.as_deref(), Some("github"));
        assert_eq!(forge.mirror_poll_interval_secs, Some(300));
        assert_eq!(forge.mirror_timeout_secs, Some(120));
        assert_eq!(forge.ci_command, vec!["just", "pre-merge"]);
    }

    #[test]
    fn webhook_secret_env_defaults() {
        let raw = "";
        let parsed: Config = toml::from_str(raw).expect("parse minimal config");
        assert_eq!(parsed.webhook_secret_env, super::DEFAULT_WEBHOOK_SECRET_ENV);
    }

    #[test]
    fn forge_repo_defaults_hook_url_and_ci_command() {
        let raw = r#"
bind_port = 9999
[forge_repo]
canonical_git_dir = "/srv/test.git"
"#;
        let parsed: Config = toml::from_str(raw).expect("parse minimal config");
        let forge = parsed.effective_forge_repo().expect("forge repo");
        assert_eq!(forge.repo, super::DEFAULT_FORGE_REPO);
        assert_eq!(forge.default_branch, super::DEFAULT_DEFAULT_BRANCH);
        assert_eq!(forge.ci_command, vec!["just", "pre-merge"]);
        assert_eq!(forge.ci_concurrency, None);
        assert_eq!(forge.mirror_poll_interval_secs, None);
        assert_eq!(forge.mirror_timeout_secs, None);
        assert_eq!(
            forge.hook_url.as_deref(),
            Some("http://127.0.0.1:9999/git/webhook")
        );
    }

    #[test]
    fn mirror_remote_defaults_poll_interval_and_timeout() {
        let raw = r#"
[forge_repo]
canonical_git_dir = "/srv/test.git"
mirror_remote = "github"
"#;
        let parsed: Config = toml::from_str(raw).expect("parse config");
        let forge = parsed.effective_forge_repo().expect("forge repo");
        assert_eq!(forge.mirror_poll_interval_secs, Some(300));
        assert_eq!(forge.mirror_timeout_secs, Some(120));
    }

    #[test]
    fn explicit_zero_mirror_timeout_uses_default() {
        let raw = r#"
[forge_repo]
canonical_git_dir = "/srv/test.git"
mirror_remote = "github"
mirror_timeout_secs = 0
"#;
        let parsed: Config = toml::from_str(raw).expect("parse config");
        let forge = parsed.effective_forge_repo().expect("forge repo");
        assert_eq!(forge.mirror_timeout_secs, Some(120));
    }

    #[test]
    fn bootstrap_admins_default_to_empty() {
        let raw = "";
        let parsed: Config = toml::from_str(raw).expect("parse minimal config");
        assert!(parsed.bootstrap_admin_npubs.is_empty());
    }

    #[test]
    fn parses_explicit_bootstrap_admins() {
        let raw = r#"
bootstrap_admin_npubs = ["npub1admin"]
"#;
        let parsed: Config = toml::from_str(raw).expect("parse config");
        assert_eq!(parsed.bootstrap_admin_npubs, vec!["npub1admin".to_string()]);
    }
}
