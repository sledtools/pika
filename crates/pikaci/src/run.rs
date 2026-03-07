use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, anyhow};
use chrono::Utc;
use uuid::Uuid;

use crate::executor::{HostContext, run_job_on_runner};
use crate::model::{JobRecord, JobSpec, RunRecord, RunStatus};
use crate::snapshot::{create_snapshot, git_dirty, git_head, materialize_workspace};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogKind {
    Host,
    Guest,
    Both,
}

#[derive(Clone, Debug)]
pub struct Logs {
    pub host: Option<String>,
    pub guest: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RunOptions {
    pub source_root: PathBuf,
    pub state_root: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct RunMetadata {
    pub rerun_of: Option<String>,
    pub target_id: Option<String>,
    pub target_description: Option<String>,
    pub changed_files: Vec<String>,
    pub filters: Vec<String>,
    pub message: Option<String>,
}

pub fn run_job(job: &JobSpec, options: &RunOptions) -> anyhow::Result<RunRecord> {
    run_jobs(std::slice::from_ref(job), options)
}

pub fn run_jobs(jobs: &[JobSpec], options: &RunOptions) -> anyhow::Result<RunRecord> {
    run_jobs_with_metadata(jobs, options, RunMetadata::default())
}

pub fn run_jobs_with_metadata(
    jobs: &[JobSpec],
    options: &RunOptions,
    metadata: RunMetadata,
) -> anyhow::Result<RunRecord> {
    let prepared = prepare_run(options)?;
    run_host_setup_commands(jobs, &options.source_root, &prepared.run_dir)?;
    let snapshot_dir = prepared.run_dir.join("snapshot");
    let snapshot = create_snapshot(&options.source_root, &snapshot_dir, &prepared.created_at)?;
    let snapshot = SnapshotSource {
        source_root: snapshot.source_root,
        snapshot_dir: PathBuf::from(&snapshot.snapshot_dir),
        snapshot_dir_string: snapshot.snapshot_dir,
        git_head: snapshot.git_head,
        git_dirty: snapshot.git_dirty,
    };
    run_jobs_against_snapshot(jobs, &prepared, &snapshot, metadata)
}

pub fn rerun_jobs_with_metadata(
    jobs: &[JobSpec],
    previous: &RunRecord,
    options: &RunOptions,
    metadata: RunMetadata,
) -> anyhow::Result<RunRecord> {
    if previous.snapshot_dir.is_empty() {
        return Err(anyhow!(
            "run `{}` has no snapshot to rerun",
            previous.run_id
        ));
    }

    let snapshot = SnapshotSource {
        source_root: previous.source_root.clone(),
        snapshot_dir: PathBuf::from(&previous.snapshot_dir),
        snapshot_dir_string: previous.snapshot_dir.clone(),
        git_head: previous.git_head.clone(),
        git_dirty: previous.git_dirty,
    };
    if !snapshot.snapshot_dir.exists() {
        return Err(anyhow!(
            "snapshot for run `{}` no longer exists at {}",
            previous.run_id,
            previous.snapshot_dir
        ));
    }

    let prepared = prepare_run(options)?;
    run_jobs_against_snapshot(jobs, &prepared, &snapshot, metadata)
}

fn run_jobs_against_snapshot(
    jobs: &[JobSpec],
    prepared: &PreparedRun,
    snapshot: &SnapshotSource,
    metadata: RunMetadata,
) -> anyhow::Result<RunRecord> {
    let mut run_record = RunRecord {
        run_id: prepared.run_id.clone(),
        status: RunStatus::Running,
        rerun_of: metadata.rerun_of,
        target_id: metadata.target_id,
        target_description: metadata.target_description,
        source_root: snapshot.source_root.clone(),
        snapshot_dir: snapshot.snapshot_dir_string.clone(),
        git_head: snapshot.git_head.clone(),
        git_dirty: snapshot.git_dirty,
        created_at: prepared.created_at.clone(),
        finished_at: None,
        changed_files: metadata.changed_files,
        filters: metadata.filters,
        message: metadata.message,
        jobs: Vec::new(),
    };
    write_run_record(&prepared.run_dir, &run_record)?;

    let mut run_failed = false;
    for job in jobs {
        let job_record = run_one_job(
            job,
            &snapshot.snapshot_dir,
            &prepared.jobs_dir,
            &prepared.shared_cargo_home_dir,
            &prepared.run_target_dir,
        )?;
        if job_record.status == RunStatus::Failed {
            run_failed = true;
        }
        run_record.jobs.push(job_record);
        run_record.status = if run_failed {
            RunStatus::Failed
        } else {
            RunStatus::Running
        };
        write_run_record(&prepared.run_dir, &run_record)?;
        if run_failed {
            break;
        }
    }
    run_record.status = if run_failed {
        RunStatus::Failed
    } else {
        RunStatus::Passed
    };
    run_record.finished_at = Some(Utc::now().to_rfc3339());
    write_run_record(&prepared.run_dir, &run_record)?;
    Ok(run_record)
}

pub fn record_skipped_run(
    options: &RunOptions,
    metadata: RunMetadata,
) -> anyhow::Result<RunRecord> {
    let run_id = new_run_id();
    let created_at = Utc::now().to_rfc3339();
    let run_dir = options.state_root.join("runs").join(&run_id);
    fs::create_dir_all(&run_dir).with_context(|| format!("create {}", run_dir.display()))?;

    let run_record = RunRecord {
        run_id,
        status: RunStatus::Skipped,
        rerun_of: metadata.rerun_of,
        target_id: metadata.target_id,
        target_description: metadata.target_description,
        source_root: options.source_root.display().to_string(),
        snapshot_dir: String::new(),
        git_head: git_head(&options.source_root),
        git_dirty: git_dirty(&options.source_root),
        created_at: created_at.clone(),
        finished_at: Some(created_at),
        changed_files: metadata.changed_files,
        filters: metadata.filters,
        message: metadata.message,
        jobs: Vec::new(),
    };
    write_run_record(&run_dir, &run_record)?;
    Ok(run_record)
}

fn run_one_job(
    job: &JobSpec,
    snapshot_dir: &Path,
    jobs_dir: &Path,
    shared_cargo_home_dir: &Path,
    shared_target_dir: &Path,
) -> anyhow::Result<JobRecord> {
    let job_dir = jobs_dir.join(job.id);
    let host_log_path = job_dir.join("host.log");
    let guest_log_path = job_dir.join("artifacts/guest.log");
    fs::create_dir_all(&job_dir).with_context(|| format!("create {}", job_dir.display()))?;

    let started_at = Utc::now().to_rfc3339();
    let mut job_record = JobRecord {
        id: job.id.to_string(),
        description: job.description.to_string(),
        status: RunStatus::Running,
        executor: job.runner_kind().as_str().to_string(),
        timeout_secs: job.timeout_secs,
        host_log_path: host_log_path.display().to_string(),
        guest_log_path: guest_log_path.display().to_string(),
        started_at,
        finished_at: None,
        exit_code: None,
        message: None,
    };
    write_job_record(&job_dir, &job_record)?;

    let ctx = HostContext {
        workspace_snapshot_dir: prepare_job_workspace(job, snapshot_dir, &job_dir)?,
        workspace_read_only: !job.writable_workspace,
        job_dir: job_dir.clone(),
        host_log_path,
        guest_log_path,
        shared_cargo_home_dir: shared_cargo_home_dir.to_path_buf(),
        shared_target_dir: shared_target_dir.to_path_buf(),
    };
    let outcome = run_job_on_runner(job, &ctx);

    let finished_at = Utc::now().to_rfc3339();
    match outcome {
        Ok(outcome) => {
            job_record.status = outcome.status;
            job_record.finished_at = Some(finished_at);
            job_record.exit_code = outcome.exit_code;
            job_record.message = Some(outcome.message);
        }
        Err(err) => {
            job_record.status = RunStatus::Failed;
            job_record.finished_at = Some(finished_at);
            job_record.message = Some(format!("{err:#}"));
        }
    }
    write_job_record(&job_dir, &job_record)?;
    Ok(job_record)
}

fn prepare_job_workspace(
    job: &JobSpec,
    snapshot_dir: &Path,
    job_dir: &Path,
) -> anyhow::Result<PathBuf> {
    if !job.writable_workspace {
        return Ok(snapshot_dir.to_path_buf());
    }

    let workspace_dir = job_dir.join("workspace");
    materialize_workspace(snapshot_dir, &workspace_dir)?;
    Ok(workspace_dir)
}

pub fn list_runs(state_root: &Path) -> anyhow::Result<Vec<RunRecord>> {
    let runs_root = state_root.join("runs");
    if !runs_root.exists() {
        return Ok(Vec::new());
    }

    let mut runs = Vec::new();
    for entry in
        fs::read_dir(&runs_root).with_context(|| format!("read {}", runs_root.display()))?
    {
        let entry = entry?;
        let path = entry.path().join("run.json");
        if !path.exists() {
            continue;
        }
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let run: RunRecord =
            serde_json::from_slice(&bytes).with_context(|| format!("decode {}", path.display()))?;
        runs.push(run);
    }
    runs.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(runs)
}

pub fn gc_runs(state_root: &Path, keep_runs: usize) -> anyhow::Result<Vec<String>> {
    let runs_root = state_root.join("runs");
    if !runs_root.exists() {
        return Ok(Vec::new());
    }

    let mut run_dirs = Vec::new();
    for entry in
        fs::read_dir(&runs_root).with_context(|| format!("read {}", runs_root.display()))?
    {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            run_dirs.push(entry.path());
        }
    }
    run_dirs.sort_by(|left, right| {
        right
            .file_name()
            .and_then(|name| name.to_str())
            .cmp(&left.file_name().and_then(|name| name.to_str()))
    });

    let mut removed = Vec::new();
    for run_dir in run_dirs.into_iter().skip(keep_runs) {
        let run_id = run_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("invalid run directory name: {}", run_dir.display()))?
            .to_string();
        fs::remove_dir_all(&run_dir).with_context(|| format!("remove {}", run_dir.display()))?;
        removed.push(run_id);
    }

    Ok(removed)
}

pub fn load_logs(
    state_root: &Path,
    run_id: &str,
    job_id: Option<&str>,
    kind: LogKind,
) -> anyhow::Result<Logs> {
    let run = load_run_record(state_root, run_id)?;
    let job = if let Some(job_id) = job_id {
        run.jobs
            .iter()
            .find(|job| job.id == job_id)
            .ok_or_else(|| anyhow!("job `{job_id}` not found in run `{run_id}`"))?
    } else {
        run.jobs
            .first()
            .ok_or_else(|| anyhow!("run `{run_id}` has no jobs"))?
    };

    let host = if matches!(kind, LogKind::Host | LogKind::Both) {
        Some(
            fs::read_to_string(&job.host_log_path)
                .with_context(|| format!("read {}", job.host_log_path))?,
        )
    } else {
        None
    };
    let guest = if matches!(kind, LogKind::Guest | LogKind::Both) {
        Some(
            fs::read_to_string(&job.guest_log_path)
                .with_context(|| format!("read {}", job.guest_log_path))?,
        )
    } else {
        None
    };
    Ok(Logs { host, guest })
}

pub fn load_run_record(state_root: &Path, run_id: &str) -> anyhow::Result<RunRecord> {
    let path = state_root.join("runs").join(run_id).join("run.json");
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decode {}", path.display()))
}

fn write_run_record(run_dir: &Path, record: &RunRecord) -> anyhow::Result<()> {
    write_json(run_dir.join("run.json"), record)
}

fn write_job_record(job_dir: &Path, record: &JobRecord) -> anyhow::Result<()> {
    write_json(job_dir.join("status.json"), record)
}

fn write_json(path: PathBuf, value: &impl serde::Serialize) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).context("encode json")?;
    fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))
}

fn new_run_id() -> String {
    format!(
        "{}-{}",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        &Uuid::new_v4().simple().to_string()[..8]
    )
}

struct PreparedRun {
    run_id: String,
    created_at: String,
    run_dir: PathBuf,
    jobs_dir: PathBuf,
    shared_cargo_home_dir: PathBuf,
    run_target_dir: PathBuf,
}

struct SnapshotSource {
    source_root: String,
    snapshot_dir: PathBuf,
    snapshot_dir_string: String,
    git_head: Option<String>,
    git_dirty: Option<bool>,
}

fn prepare_run(options: &RunOptions) -> anyhow::Result<PreparedRun> {
    let run_id = new_run_id();
    let created_at = Utc::now().to_rfc3339();
    let run_dir = options.state_root.join("runs").join(&run_id);
    let jobs_dir = run_dir.join("jobs");
    let cache_dir = options.state_root.join("cache");
    let shared_cargo_home_dir = cache_dir.join("cargo-home");
    let run_target_dir = run_dir.join("cargo-target");
    fs::create_dir_all(&jobs_dir).with_context(|| format!("create {}", jobs_dir.display()))?;
    fs::create_dir_all(&shared_cargo_home_dir)
        .with_context(|| format!("create {}", shared_cargo_home_dir.display()))?;
    fs::create_dir_all(&run_target_dir)
        .with_context(|| format!("create {}", run_target_dir.display()))?;
    Ok(PreparedRun {
        run_id,
        created_at,
        run_dir,
        jobs_dir,
        shared_cargo_home_dir,
        run_target_dir,
    })
}

fn run_host_setup_commands(
    jobs: &[JobSpec],
    source_root: &Path,
    run_dir: &Path,
) -> anyhow::Result<()> {
    let mut commands = Vec::new();
    for job in jobs {
        let Some(command) = job.host_setup_command() else {
            continue;
        };
        if !commands.contains(&command) {
            commands.push(command);
        }
    }
    if commands.is_empty() {
        return Ok(());
    }

    let log_path = run_dir.join("host-setup.log");
    for command in commands {
        let output = Command::new("bash")
            .arg("--noprofile")
            .arg("--norc")
            .arg("-lc")
            .arg(command)
            .current_dir(source_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("run host setup `{command}`"))?;

        let mut log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("open {}", log_path.display()))?;
        log_file
            .write_all(format!("[pikaci] host setup: {command}\n").as_bytes())
            .with_context(|| format!("write {}", log_path.display()))?;
        log_file
            .write_all(&output.stdout)
            .with_context(|| format!("write {}", log_path.display()))?;
        log_file
            .write_all(&output.stderr)
            .with_context(|| format!("write {}", log_path.display()))?;

        if !output.status.success() {
            return Err(anyhow!(
                "host setup `{command}` failed with {:?}; see {}",
                output.status.code(),
                log_path.display()
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::gc_runs;

    #[test]
    fn gc_runs_keeps_latest_run_directories() {
        let root = std::env::temp_dir().join(format!("pikaci-gc-test-{}", uuid::Uuid::new_v4()));
        let runs_root = root.join("runs");
        fs::create_dir_all(&runs_root).expect("create runs root");
        for run_id in [
            "20260307T000001Z-aaaa0001",
            "20260307T000002Z-bbbb0002",
            "20260307T000003Z-cccc0003",
        ] {
            fs::create_dir_all(runs_root.join(run_id)).expect("create run dir");
        }

        let removed = gc_runs(&root, 1).expect("gc runs");

        assert_eq!(
            removed,
            vec![
                "20260307T000002Z-bbbb0002".to_string(),
                "20260307T000001Z-aaaa0001".to_string()
            ]
        );
        assert!(runs_root.join("20260307T000003Z-cccc0003").exists());
        assert!(!runs_root.join("20260307T000002Z-bbbb0002").exists());
        assert!(!runs_root.join("20260307T000001Z-aaaa0001").exists());

        let _ = fs::remove_dir_all(&root);
    }
}
