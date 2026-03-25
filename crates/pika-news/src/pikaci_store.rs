use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use pikaci::{load_logs, load_run_bundle, load_run_record, LogKind, Logs, RunBundle, RunRecord};

#[cfg(test)]
use pikaci::{JobRecord, PreparedOutputsRecord, RemoteLinuxVmExecutionRecord, RunStatus};
#[cfg(test)]
use std::fs;

use crate::config::{Config, ForgeRepoConfig};

#[derive(Clone, Debug)]
pub struct PikaciRunStore {
    state_root: PathBuf,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub struct TestPikaciJobFixture {
    pub id: String,
    pub description: String,
    pub status: RunStatus,
    pub executor: String,
    pub timeout_secs: u64,
    pub host_log: String,
    pub guest_log: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub message: Option<String>,
    pub pre_execution_prepare_duration_ms: Option<u64>,
    pub remote_linux_vm_execution: Option<RemoteLinuxVmExecutionRecord>,
}

#[cfg(test)]
impl TestPikaciJobFixture {
    pub fn passed_remote_linux(job_id: &str, description: &str) -> Self {
        Self {
            id: job_id.to_string(),
            description: description.to_string(),
            status: RunStatus::Passed,
            executor: "remote_linux_vm".to_string(),
            timeout_secs: 30,
            host_log: "host fixture\n".to_string(),
            guest_log: "guest fixture\n".to_string(),
            started_at: "2026-03-19T00:00:01Z".to_string(),
            finished_at: Some("2026-03-19T00:00:02Z".to_string()),
            exit_code: Some(0),
            message: None,
            pre_execution_prepare_duration_ms: None,
            remote_linux_vm_execution: None,
        }
    }
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub struct TestPikaciRunFixture {
    pub run_id: String,
    pub status: RunStatus,
    pub target_id: Option<String>,
    pub target_description: Option<String>,
    pub source_root: String,
    pub snapshot_dir: String,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub message: Option<String>,
    pub jobs: Vec<TestPikaciJobFixture>,
    pub prepared_outputs: Option<PreparedOutputsRecord>,
}

#[cfg(test)]
impl TestPikaciRunFixture {
    pub fn passed(run_id: &str, target_id: Option<&str>, target_description: Option<&str>) -> Self {
        Self {
            run_id: run_id.to_string(),
            status: RunStatus::Passed,
            target_id: target_id.map(ToOwned::to_owned),
            target_description: target_description.map(ToOwned::to_owned),
            source_root: "/tmp/source".to_string(),
            snapshot_dir: "/tmp/snapshot".to_string(),
            created_at: "2026-03-19T00:00:00Z".to_string(),
            finished_at: Some("2026-03-19T00:00:02Z".to_string()),
            message: None,
            jobs: Vec::new(),
            prepared_outputs: None,
        }
    }
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
    pub fn run_record_path(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("run.json")
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

    #[cfg(test)]
    pub fn write_fixture(&self, fixture: &TestPikaciRunFixture) -> Result<()> {
        let run_dir = self.run_dir(&fixture.run_id);
        fs::create_dir_all(&run_dir)?;

        let prepared_outputs_path = fixture.prepared_outputs.as_ref().map(|prepared_outputs| {
            let path = self.prepared_outputs_path(&fixture.run_id);
            (path, prepared_outputs)
        });
        if let Some((path, prepared_outputs)) = &prepared_outputs_path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, serde_json::to_vec(prepared_outputs)?)?;
        }

        let mut jobs = Vec::with_capacity(fixture.jobs.len());
        for job in &fixture.jobs {
            let host_log_path = self.host_log_path(&fixture.run_id, &job.id);
            let guest_log_path = self.guest_log_path(&fixture.run_id, &job.id);
            if let Some(parent) = host_log_path.parent() {
                fs::create_dir_all(parent)?;
            }
            if let Some(parent) = guest_log_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&host_log_path, &job.host_log)?;
            fs::write(&guest_log_path, &job.guest_log)?;
            jobs.push(JobRecord {
                id: job.id.clone(),
                description: job.description.clone(),
                status: job.status,
                executor: job.executor.clone(),
                plan_node_id: None,
                timeout_secs: job.timeout_secs,
                host_log_path: host_log_path.display().to_string(),
                guest_log_path: guest_log_path.display().to_string(),
                started_at: job.started_at.clone(),
                finished_at: job.finished_at.clone(),
                exit_code: job.exit_code,
                message: job.message.clone(),
                pre_execution_prepare_duration_ms: job.pre_execution_prepare_duration_ms,
                remote_linux_vm_execution: job.remote_linux_vm_execution.clone(),
            });
        }

        let run = RunRecord {
            run_id: fixture.run_id.clone(),
            status: fixture.status,
            rerun_of: None,
            target_id: fixture.target_id.clone(),
            target_description: fixture.target_description.clone(),
            source_root: fixture.source_root.clone(),
            snapshot_dir: fixture.snapshot_dir.clone(),
            git_head: None,
            git_dirty: None,
            created_at: fixture.created_at.clone(),
            finished_at: fixture.finished_at.clone(),
            plan_path: None,
            prepared_outputs_path: prepared_outputs_path
                .as_ref()
                .map(|(path, _)| path.display().to_string()),
            prepared_output_consumer: None,
            prepared_output_mode: None,
            prepared_output_invocation_mode: None,
            prepared_output_invocation_wrapper_program: None,
            prepared_output_launcher_transport_mode: None,
            prepared_output_launcher_transport_program: None,
            prepared_output_launcher_transport_host: None,
            prepared_output_launcher_transport_remote_launcher_program: None,
            prepared_output_launcher_transport_remote_helper_program: None,
            prepared_output_launcher_transport_remote_work_dir: None,
            changed_files: Vec::new(),
            filters: Vec::new(),
            message: fixture.message.clone(),
            prepare_timings: Vec::new(),
            jobs,
        };
        fs::write(
            self.run_record_path(&fixture.run_id),
            serde_json::to_vec(&run)?,
        )?;
        Ok(())
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
