use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use pikaci::{load_logs, load_run_bundle, load_run_record, LogKind, Logs, RunBundle, RunRecord};

use crate::config::{Config, ForgeRepoConfig};

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
            state_root: state_root_for_repo(repo),
        }
    }

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    #[cfg(test)]
    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.state_root.join("runs").join(run_id)
    }

    #[cfg(test)]
    pub fn prepared_outputs_path(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("prepared-outputs.json")
    }

    #[cfg(test)]
    pub fn job_dir(&self, run_id: &str, job_id: &str) -> PathBuf {
        self.run_dir(run_id).join("jobs").join(job_id)
    }

    #[cfg(test)]
    pub fn host_log_path(&self, run_id: &str, job_id: &str) -> PathBuf {
        self.job_dir(run_id, job_id).join("host.log")
    }

    #[cfg(test)]
    pub fn guest_log_path(&self, run_id: &str, job_id: &str) -> PathBuf {
        self.job_dir(run_id, job_id)
            .join("artifacts")
            .join("guest.log")
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

fn state_root_for_repo(repo: &ForgeRepoConfig) -> PathBuf {
    let repo_slug = repo
        .repo
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => ch,
            _ => '-',
        })
        .collect::<String>();
    Path::new(&repo.canonical_git_dir)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("pikaci-state")
        .join(repo_slug)
}
