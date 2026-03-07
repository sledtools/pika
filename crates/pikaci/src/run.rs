use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, anyhow};
use chrono::Utc;
use uuid::Uuid;

use crate::executor::{
    HostContext, compiled_guest_command, materialize_vfkit_runner_flake, run_job_on_runner,
};
use crate::model::{
    ExecuteNode, JobRecord, JobSpec, PlanExecutorKind, PlanNodeRecord, PlanScope, PrepareNode,
    RunPlanRecord, RunRecord, RunStatus, RunnerKind,
};
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
    let plan = build_run_plan(jobs, prepared, snapshot, &metadata)?;
    let plan_path = write_run_plan_record(&prepared.run_dir, &plan.record)?;
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
        plan_path: Some(plan_path.display().to_string()),
        changed_files: metadata.changed_files,
        filters: metadata.filters,
        message: metadata.message,
        jobs: Vec::new(),
    };
    write_run_record(&prepared.run_dir, &run_record)?;

    let mut run_failed = false;
    for planned_job in &plan.jobs {
        let job_record = run_one_job(
            &planned_job.job,
            &planned_job.execute_node_id,
            &planned_job.ctx,
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
        plan_path: None,
        changed_files: metadata.changed_files,
        filters: metadata.filters,
        message: metadata.message,
        jobs: Vec::new(),
    };
    write_run_record(&run_dir, &run_record)?;
    Ok(run_record)
}

fn run_one_job(job: &JobSpec, plan_node_id: &str, ctx: &HostContext) -> anyhow::Result<JobRecord> {
    let job_dir = ctx.job_dir.clone();
    let host_log_path = ctx.host_log_path.clone();
    let guest_log_path = ctx.guest_log_path.clone();
    fs::create_dir_all(&job_dir).with_context(|| format!("create {}", job_dir.display()))?;

    let started_at = Utc::now().to_rfc3339();
    let mut job_record = JobRecord {
        id: job.id.to_string(),
        description: job.description.to_string(),
        status: RunStatus::Running,
        executor: job.runner_kind().as_str().to_string(),
        plan_node_id: Some(plan_node_id.to_string()),
        timeout_secs: job.timeout_secs,
        host_log_path: host_log_path.display().to_string(),
        guest_log_path: guest_log_path.display().to_string(),
        started_at,
        finished_at: None,
        exit_code: None,
        message: None,
    };
    write_job_record(&job_dir, &job_record)?;

    let outcome = run_job_on_runner(job, ctx);

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
    if workspace_dir.exists() {
        return Ok(workspace_dir);
    }
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

fn write_run_plan_record(run_dir: &Path, record: &RunPlanRecord) -> anyhow::Result<PathBuf> {
    let path = run_dir.join("plan.json");
    write_json(path.clone(), record)?;
    Ok(path)
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

struct PlannedJob {
    job: JobSpec,
    execute_node_id: String,
    ctx: HostContext,
}

struct RunPlan {
    record: RunPlanRecord,
    jobs: Vec<PlannedJob>,
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

fn build_run_plan(
    jobs: &[JobSpec],
    prepared: &PreparedRun,
    snapshot: &SnapshotSource,
    metadata: &RunMetadata,
) -> anyhow::Result<RunPlan> {
    let mut nodes = Vec::new();
    let mut planned_jobs = Vec::new();

    for job in jobs {
        let job_dir = prepared.jobs_dir.join(job.id);
        let ctx = HostContext {
            workspace_snapshot_dir: prepare_job_workspace(job, &snapshot.snapshot_dir, &job_dir)?,
            workspace_read_only: !job.writable_workspace,
            job_dir: job_dir.clone(),
            host_log_path: job_dir.join("host.log"),
            guest_log_path: job_dir.join("artifacts/guest.log"),
            shared_cargo_home_dir: prepared.shared_cargo_home_dir.clone(),
            shared_target_dir: prepared.run_target_dir.clone(),
        };
        let mut depends_on = Vec::new();
        if job.runner_kind() == RunnerKind::VfkitLocal {
            let prepare_node_id = format!("prepare-{}-runner", job.id);
            let installable = materialize_vfkit_runner_flake(job, &ctx)?;
            nodes.push(PlanNodeRecord::Prepare {
                id: prepare_node_id.clone(),
                description: format!("Build vfkit runner for `{}`", job.id),
                executor: PlanExecutorKind::HostLocal,
                depends_on: Vec::new(),
                prepare: PrepareNode::NixBuild {
                    installable,
                    output_name: "nixosConfigurations.pikaci-wave1.config.microvm.declaredRunner"
                        .to_string(),
                },
            });
            depends_on.push(prepare_node_id);
        }

        let execute_node_id = format!("execute-{}", job.id);
        let (command, run_as_root) = compiled_guest_command(job);
        nodes.push(PlanNodeRecord::Execute {
            id: execute_node_id.clone(),
            description: job.description.to_string(),
            executor: job.runner_kind().into(),
            depends_on,
            execute: ExecuteNode::VmCommand {
                command,
                run_as_root,
                timeout_secs: job.timeout_secs,
                writable_workspace: job.writable_workspace,
            },
        });
        planned_jobs.push(PlannedJob {
            job: job.clone(),
            execute_node_id,
            ctx,
        });
    }

    Ok(RunPlan {
        record: RunPlanRecord {
            schema_version: 1,
            run_id: prepared.run_id.clone(),
            target_id: metadata.target_id.clone(),
            target_description: metadata.target_description.clone(),
            created_at: prepared.created_at.clone(),
            scope: PlanScope::PostHostSetupAndSnapshot,
            preconditions: vec![
                "host_setup_complete".to_string(),
                "workspace_snapshot_created".to_string(),
            ],
            nodes,
        },
        jobs: planned_jobs,
    })
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

    use super::{
        PreparedRun, RunMetadata, SnapshotSource, build_run_plan, gc_runs, write_run_plan_record,
    };
    use crate::model::{
        ExecuteNode, GuestCommand, JobSpec, PlanExecutorKind, PlanNodeRecord, PlanScope,
        PrepareNode, RunPlanRecord,
    };

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

    #[test]
    fn build_run_plan_records_prepare_and_execute_nodes_for_vfkit_jobs() {
        let root = std::env::temp_dir().join(format!("pikaci-plan-test-{}", uuid::Uuid::new_v4()));
        let prepared = sample_prepared_run(&root);
        let metadata = RunMetadata {
            target_id: Some("pre-merge-pika-rust".to_string()),
            target_description: Some("Run pika rust lane".to_string()),
            ..RunMetadata::default()
        };
        let jobs = vec![JobSpec {
            id: "pika-core-lib-tests",
            description: "Run pika_core lib and test targets in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pika_core --lib --tests -- --nocapture",
            },
        }];

        let snapshot = sample_snapshot_source(&prepared);
        let plan = build_run_plan(&jobs, &prepared, &snapshot, &metadata).expect("build plan");

        assert_eq!(plan.record.schema_version, 1);
        assert_eq!(plan.record.scope, PlanScope::PostHostSetupAndSnapshot);
        assert_eq!(
            plan.record.preconditions,
            vec![
                "host_setup_complete".to_string(),
                "workspace_snapshot_created".to_string()
            ]
        );
        assert_eq!(plan.record.nodes.len(), 2);
        match &plan.record.nodes[0] {
            PlanNodeRecord::Prepare {
                id,
                executor,
                prepare,
                ..
            } => {
                assert_eq!(id, "prepare-pika-core-lib-tests-runner");
                assert_eq!(*executor, PlanExecutorKind::HostLocal);
                match prepare {
                    PrepareNode::NixBuild {
                        installable,
                        output_name,
                    } => {
                        assert!(installable.contains(
                            "jobs/pika-core-lib-tests/vm/flake#nixosConfigurations.pikaci-wave1.config.microvm.declaredRunner"
                        ));
                        assert_eq!(
                            output_name,
                            "nixosConfigurations.pikaci-wave1.config.microvm.declaredRunner"
                        );
                    }
                }
            }
            other => panic!("expected prepare node, got {other:?}"),
        }

        match &plan.record.nodes[1] {
            PlanNodeRecord::Execute {
                id,
                executor,
                depends_on,
                execute,
                ..
            } => {
                assert_eq!(id, "execute-pika-core-lib-tests");
                assert_eq!(*executor, PlanExecutorKind::VfkitLocal);
                assert_eq!(
                    depends_on,
                    &vec!["prepare-pika-core-lib-tests-runner".to_string()]
                );
                match execute {
                    ExecuteNode::VmCommand {
                        command,
                        run_as_root,
                        timeout_secs,
                        writable_workspace,
                    } => {
                        assert_eq!(
                            command,
                            "bash --noprofile --norc -lc 'cargo test -p pika_core --lib --tests -- --nocapture'"
                        );
                        assert!(!run_as_root);
                        assert_eq!(*timeout_secs, 1800);
                        assert!(!writable_workspace);
                    }
                }
            }
            other => panic!("expected execute node, got {other:?}"),
        }

        assert_eq!(plan.jobs.len(), 1);
        assert_eq!(plan.jobs[0].execute_node_id, "execute-pika-core-lib-tests");
        assert!(
            prepared
                .run_dir
                .join("jobs/pika-core-lib-tests/vm/flake/flake.nix")
                .exists()
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_run_plan_record_persists_machine_readable_plan() {
        let root =
            std::env::temp_dir().join(format!("pikaci-plan-write-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create root");
        let plan = RunPlanRecord {
            schema_version: 1,
            run_id: "run-1".to_string(),
            target_id: Some("beachhead".to_string()),
            target_description: Some("Run one tiny exact unit test".to_string()),
            created_at: "2026-03-07T00:00:00Z".to_string(),
            scope: PlanScope::PostHostSetupAndSnapshot,
            preconditions: vec![
                "host_setup_complete".to_string(),
                "workspace_snapshot_created".to_string(),
            ],
            nodes: vec![PlanNodeRecord::Execute {
                id: "execute-beachhead".to_string(),
                description: "Run one tiny exact unit test in a vfkit guest".to_string(),
                executor: PlanExecutorKind::VfkitLocal,
                depends_on: Vec::new(),
                execute: ExecuteNode::VmCommand {
                    command: "cargo test -p pika-agent-control-plane tests::command_envelope_round_trips -- --exact --nocapture".to_string(),
                    run_as_root: false,
                    timeout_secs: 1800,
                    writable_workspace: false,
                },
            }],
        };

        let path = write_run_plan_record(&root, &plan).expect("write plan");
        let bytes = fs::read(&path).expect("read plan");
        let decoded: RunPlanRecord = serde_json::from_slice(&bytes).expect("decode plan");

        assert_eq!(decoded.run_id, "run-1");
        assert_eq!(decoded.scope, PlanScope::PostHostSetupAndSnapshot);
        assert_eq!(decoded.nodes.len(), 1);
        assert!(path.ends_with("plan.json"));

        let _ = fs::remove_dir_all(&root);
    }

    fn sample_prepared_run(root: &std::path::Path) -> PreparedRun {
        let run_dir = root.join("runs").join("run-1");
        let jobs_dir = run_dir.join("jobs");
        fs::create_dir_all(&jobs_dir).expect("create jobs dir");
        fs::create_dir_all(root.join("cache").join("cargo-home")).expect("create cargo home");
        fs::create_dir_all(run_dir.join("cargo-target")).expect("create cargo target");
        PreparedRun {
            run_id: "run-1".to_string(),
            created_at: "2026-03-07T00:00:00Z".to_string(),
            run_dir,
            jobs_dir,
            shared_cargo_home_dir: root.join("cache").join("cargo-home"),
            run_target_dir: root.join("runs").join("run-1").join("cargo-target"),
        }
    }

    fn sample_snapshot_source(prepared: &PreparedRun) -> SnapshotSource {
        let snapshot_dir = prepared.run_dir.join("snapshot");
        fs::create_dir_all(&snapshot_dir).expect("create snapshot dir");
        SnapshotSource {
            source_root: "/tmp/source".to_string(),
            snapshot_dir: snapshot_dir.clone(),
            snapshot_dir_string: snapshot_dir.display().to_string(),
            git_head: Some("deadbeef".to_string()),
            git_dirty: Some(false),
        }
    }
}
