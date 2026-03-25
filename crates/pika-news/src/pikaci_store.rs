use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use pikaci::{load_logs, load_run_bundle, load_run_record, LogKind, Logs, RunBundle, RunRecord};

use crate::config::{Config, ForgeRepoConfig};
use crate::forge;

#[derive(Clone, Debug)]
pub struct PikaciRunStore {
    state_root: PathBuf,
}

impl PikaciRunStore {
    pub fn from_config(config: &Config) -> Option<Self> {
        config
            .effective_forge_repo()
            .map(|repo| Self::from_forge_repo(&repo))
    }

    pub fn from_forge_repo(repo: &ForgeRepoConfig) -> Self {
        Self {
            state_root: forge::pikaci_state_root(repo),
        }
    }

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub fn load_run(&self, run_id: &str) -> Result<RunRecord> {
        load_run_record(self.state_root(), run_id)
    }

    pub fn load_logs(&self, run_id: &str, job_id: Option<&str>, kind: LogKind) -> Result<Logs> {
        load_logs(self.state_root(), run_id, job_id, kind)
    }

    pub fn load_run_bundle(&self, run_id: &str) -> Result<RunBundle> {
        load_run_bundle(self.state_root(), run_id)
    }
}

pub fn require_pikaci_run_store(store: Option<&PikaciRunStore>) -> Result<&PikaciRunStore> {
    store.ok_or_else(|| anyhow!("forge repo is not configured"))
}
