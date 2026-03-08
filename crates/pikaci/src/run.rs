use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use anyhow::{Context, anyhow};
use chrono::Utc;
use uuid::Uuid;

use crate::executor::{
    HostContext, compiled_guest_command, materialize_vfkit_runner_flake, prepare_vfkit_runner_link,
    run_job_on_runner,
};
use crate::model::{
    ExecuteNode, JobRecord, JobSpec, PlanExecutorKind, PlanNodeRecord, PlanScope, PrepareNode,
    PreparedOutputConsumerKind, PreparedOutputExposure, PreparedOutputExposureAccess,
    PreparedOutputExposureKind, PreparedOutputFulfillmentLaunchRequest,
    PreparedOutputFulfillmentResult, PreparedOutputFulfillmentStatus,
    PreparedOutputFulfillmentTransportPathContract, PreparedOutputFulfillmentTransportRequest,
    PreparedOutputHandoff, PreparedOutputHandoffProtocol, PreparedOutputInvocationMode,
    PreparedOutputLauncherTransportMode, PreparedOutputRemoteExposureRequest,
    PreparedOutputsRecord, RealizedPreparedOutputRecord, RunPlanRecord, RunRecord, RunStatus,
    RunnerKind, StagedLinuxRustLane,
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
    pub prepared_output_mode: Option<String>,
    pub prepared_output_invocation_mode: Option<PreparedOutputInvocationMode>,
    pub prepared_output_invocation_wrapper_program: Option<String>,
    pub prepared_output_launcher_transport_mode: Option<PreparedOutputLauncherTransportMode>,
    pub prepared_output_launcher_transport_program: Option<String>,
    pub prepared_output_launcher_transport_host: Option<String>,
    pub prepared_output_launcher_transport_remote_launcher_program: Option<String>,
    pub prepared_output_launcher_transport_remote_helper_program: Option<String>,
    pub prepared_output_launcher_transport_remote_work_dir: Option<String>,
    pub changed_files: Vec<String>,
    pub filters: Vec<String>,
    pub message: Option<String>,
}

const STAGED_LINUX_RUST_SUBPROCESS_MODE_ENV: &str = "PIKACI_PRE_MERGE_PIKA_RUST_SUBPROCESS_FULFILL";
const STAGED_LINUX_RUST_SUBPROCESS_MODE_NAME: &str =
    "pre_merge_pika_rust_subprocess_fulfillment_v1";
const PREPARED_OUTPUT_FULFILLMENT_HELPER_BASENAME: &str = "pikaci-fulfill-prepared-output";
const PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_BASENAME: &str = "pikaci-launch-fulfill-prepared-output";
const PREPARED_OUTPUT_FULFILLMENT_INVOCATION_ENV: &str =
    "PIKACI_PREPARED_OUTPUT_FULFILL_INVOCATION";
const PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_TRANSPORT_ENV: &str =
    "PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_TRANSPORT";
const PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_BINARY_ENV: &str =
    "PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY";
const PREPARED_OUTPUT_FULFILLMENT_TRANSPORT_BINARY_ENV: &str =
    "PIKACI_PREPARED_OUTPUT_FULFILL_TRANSPORT_BINARY";
const PREPARED_OUTPUT_FULFILLMENT_WRAPPER_BINARY_ENV: &str =
    "PIKACI_PREPARED_OUTPUT_FULFILL_WRAPPER_BINARY";
const PREPARED_OUTPUT_FULFILLMENT_SSH_BINARY_ENV: &str =
    "PIKACI_PREPARED_OUTPUT_FULFILL_SSH_BINARY";
const PREPARED_OUTPUT_FULFILLMENT_SSH_NIX_BINARY_ENV: &str =
    "PIKACI_PREPARED_OUTPUT_FULFILL_SSH_NIX_BINARY";
const PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV: &str = "PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST";
const PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_LAUNCHER_BINARY_ENV: &str =
    "PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_LAUNCHER_BINARY";
const PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_HELPER_BINARY_ENV: &str =
    "PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_HELPER_BINARY";
const PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_WORK_DIR_ENV: &str =
    "PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR";

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
    let prepared_output_consumer_kind = configured_prepared_output_consumer_kind()?;
    let (prepared_output_consumer_kind, prepared_output_mode) =
        resolve_run_prepared_output_consumer_kind(jobs, &metadata, prepared_output_consumer_kind)?;
    let prepared_output_invocation_mode = resolve_run_prepared_output_invocation_mode(
        prepared_output_consumer_kind,
        metadata.prepared_output_invocation_mode,
    )?;
    let prepared_output_invocation_wrapper_program =
        resolve_run_prepared_output_invocation_wrapper_program(
            prepared_output_invocation_mode,
            metadata
                .prepared_output_invocation_wrapper_program
                .as_deref(),
        )?;
    let prepared_output_launcher_transport_mode =
        resolve_run_prepared_output_launcher_transport_mode(
            prepared_output_invocation_mode,
            metadata.prepared_output_launcher_transport_mode,
        )?;
    let prepared_output_launcher_transport_program =
        resolve_run_prepared_output_launcher_transport_program(
            prepared_output_launcher_transport_mode,
            metadata
                .prepared_output_launcher_transport_program
                .as_deref(),
        )?;
    let prepared_output_launcher_transport_host =
        resolve_run_prepared_output_launcher_transport_host(
            prepared_output_launcher_transport_mode,
            metadata.prepared_output_launcher_transport_host.as_deref(),
        )?;
    let prepared_output_launcher_transport_remote_launcher_program =
        resolve_run_prepared_output_launcher_transport_remote_launcher_program(
            prepared_output_launcher_transport_mode,
            metadata
                .prepared_output_launcher_transport_remote_launcher_program
                .as_deref(),
        )?;
    let prepared_output_launcher_transport_remote_helper_program =
        resolve_run_prepared_output_launcher_transport_remote_helper_program(
            prepared_output_launcher_transport_mode,
            metadata
                .prepared_output_launcher_transport_remote_helper_program
                .as_deref(),
        )?;
    let prepared_output_launcher_transport_remote_work_dir =
        resolve_run_prepared_output_launcher_transport_remote_work_dir(
            prepared_output_launcher_transport_mode,
            metadata
                .prepared_output_launcher_transport_remote_work_dir
                .as_deref(),
        )?;
    validate_prepared_output_consumer_for_jobs(prepared_output_consumer_kind, &plan.jobs)?;
    let plan_path = write_run_plan_record(&prepared.run_dir, &plan.record)?;
    let prepared_outputs_path = write_prepared_outputs_record(
        &prepared.run_dir,
        &PreparedOutputsRecord {
            schema_version: 1,
            outputs: Vec::new(),
        },
    )?;
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
        prepared_outputs_path: Some(prepared_outputs_path.display().to_string()),
        prepared_output_consumer: Some(prepared_output_consumer_kind),
        prepared_output_mode: prepared_output_mode.map(str::to_string),
        prepared_output_invocation_mode,
        prepared_output_invocation_wrapper_program: prepared_output_invocation_wrapper_program
            .clone(),
        prepared_output_launcher_transport_mode,
        prepared_output_launcher_transport_program: prepared_output_launcher_transport_program
            .clone(),
        prepared_output_launcher_transport_host: prepared_output_launcher_transport_host.clone(),
        prepared_output_launcher_transport_remote_launcher_program:
            prepared_output_launcher_transport_remote_launcher_program.clone(),
        prepared_output_launcher_transport_remote_helper_program:
            prepared_output_launcher_transport_remote_helper_program.clone(),
        prepared_output_launcher_transport_remote_work_dir:
            prepared_output_launcher_transport_remote_work_dir.clone(),
        changed_files: metadata.changed_files,
        filters: metadata.filters,
        message: metadata.message,
        jobs: Vec::new(),
    };
    write_run_record(&prepared.run_dir, &run_record)?;
    let invocation = PreparedOutputInvocationConfig {
        invocation_mode: prepared_output_invocation_mode,
        launcher_program: prepared_output_invocation_wrapper_program
            .as_deref()
            .map(Path::new),
        launcher_transport_mode: prepared_output_launcher_transport_mode,
        launcher_transport_program: prepared_output_launcher_transport_program
            .as_deref()
            .map(Path::new),
        launcher_transport_host: prepared_output_launcher_transport_host.as_deref(),
        launcher_transport_remote_launcher_program:
            prepared_output_launcher_transport_remote_launcher_program
                .as_deref()
                .map(Path::new),
        launcher_transport_remote_helper_program:
            prepared_output_launcher_transport_remote_helper_program
                .as_deref()
                .map(Path::new),
        launcher_transport_remote_work_dir: prepared_output_launcher_transport_remote_work_dir
            .as_deref()
            .map(Path::new),
    };

    let prepared_node_ids = match run_prepare_nodes(
        &prepared.run_dir,
        &plan.prepares,
        &prepared_outputs_path,
        prepared_output_consumer_kind,
        invocation,
    ) {
        Ok(node_ids) => node_ids,
        Err(failure) => {
            mark_prepare_failure(&mut run_record, &plan, &failure)?;
            run_record.status = RunStatus::Failed;
            run_record.finished_at = Some(Utc::now().to_rfc3339());
            write_run_record(&prepared.run_dir, &run_record)?;
            return Ok(run_record);
        }
    };

    let mut run_failed = false;
    let mut completed_node_ids: HashSet<String> = prepared_node_ids.into_iter().collect();
    let mut pending: Vec<usize> = (0..plan.jobs.len()).collect();
    let mut active: HashMap<usize, PlannedJob> = HashMap::new();
    let (tx, rx) = mpsc::channel::<(usize, anyhow::Result<JobRecord>)>();
    let max_parallel = max_parallel_execute_jobs(&plan.jobs);
    let mut stop_scheduling = false;

    while !pending.is_empty() || !active.is_empty() {
        while !stop_scheduling && active.len() < max_parallel {
            let Some(next_ready_pos) =
                ready_execute_job_positions(&pending, &plan.jobs, &completed_node_ids)
                    .into_iter()
                    .next()
            else {
                break;
            };
            let planned_job_index = pending.remove(next_ready_pos);
            let planned_job = plan.jobs[planned_job_index].clone();
            let running_record = running_job_record(
                &planned_job.job,
                &planned_job.execute_node_id,
                &planned_job.ctx,
            );
            write_job_record(&planned_job.ctx.job_dir, &running_record)?;
            upsert_run_job_record(&mut run_record, running_record);
            write_run_record(&prepared.run_dir, &run_record)?;

            let tx = tx.clone();
            let planned_job_for_thread = planned_job.clone();
            thread::spawn(move || {
                let result = run_one_job(
                    &planned_job_for_thread.job,
                    &planned_job_for_thread.execute_node_id,
                    &planned_job_for_thread.ctx,
                );
                let _ = tx.send((planned_job_index, result));
            });
            active.insert(planned_job_index, planned_job);
        }

        if active.is_empty() {
            if pending.is_empty() || stop_scheduling {
                break;
            }
            return Err(anyhow!(
                "no ready execute nodes for run `{}`; unresolved dependencies in plan",
                prepared.run_id
            ));
        }

        let (planned_job_index, result) = rx
            .recv()
            .map_err(|err| anyhow!("wait for scheduled execute node: {err}"))?;
        let planned_job = active.remove(&planned_job_index).ok_or_else(|| {
            anyhow!("missing active job for scheduler slot `{planned_job_index}`")
        })?;
        let job_record = match result {
            Ok(record) => record,
            Err(err) => failed_job_record(
                &planned_job.job,
                &planned_job.execute_node_id,
                &planned_job.ctx,
                format!("{err:#}"),
                None,
            ),
        };
        if job_record.status == RunStatus::Failed {
            run_failed = true;
            stop_scheduling = true;
        }
        completed_node_ids.insert(planned_job.execute_node_id.clone());
        upsert_run_job_record(&mut run_record, job_record);
        run_record.status = if run_failed {
            RunStatus::Failed
        } else {
            RunStatus::Running
        };
        write_run_record(&prepared.run_dir, &run_record)?;
    }

    if run_failed {
        for index in pending {
            let planned_job = &plan.jobs[index];
            let skipped = skipped_job_record(
                &planned_job.job,
                &planned_job.execute_node_id,
                &planned_job.ctx,
                "not run because an earlier execute node failed".to_string(),
            );
            write_job_record(&planned_job.ctx.job_dir, &skipped)?;
            upsert_run_job_record(&mut run_record, skipped);
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
        prepared_outputs_path: None,
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
        changed_files: metadata.changed_files,
        filters: metadata.filters,
        message: metadata.message,
        jobs: Vec::new(),
    };
    write_run_record(&run_dir, &run_record)?;
    Ok(run_record)
}

fn run_one_job(job: &JobSpec, plan_node_id: &str, ctx: &HostContext) -> anyhow::Result<JobRecord> {
    let mut job_record = running_job_record(job, plan_node_id, ctx);
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
    write_job_record(&ctx.job_dir, &job_record)?;
    Ok(job_record)
}

fn running_job_record(job: &JobSpec, plan_node_id: &str, ctx: &HostContext) -> JobRecord {
    JobRecord {
        id: job.id.to_string(),
        description: job.description.to_string(),
        status: RunStatus::Running,
        executor: job.runner_kind().as_str().to_string(),
        plan_node_id: Some(plan_node_id.to_string()),
        timeout_secs: job.timeout_secs,
        host_log_path: ctx.host_log_path.display().to_string(),
        guest_log_path: ctx.guest_log_path.display().to_string(),
        started_at: Utc::now().to_rfc3339(),
        finished_at: None,
        exit_code: None,
        message: None,
    }
}

fn failed_job_record(
    job: &JobSpec,
    plan_node_id: &str,
    ctx: &HostContext,
    message: String,
    exit_code: Option<i32>,
) -> JobRecord {
    JobRecord {
        status: RunStatus::Failed,
        finished_at: Some(Utc::now().to_rfc3339()),
        exit_code,
        message: Some(message),
        ..running_job_record(job, plan_node_id, ctx)
    }
}

fn skipped_job_record(
    job: &JobSpec,
    plan_node_id: &str,
    ctx: &HostContext,
    message: String,
) -> JobRecord {
    JobRecord {
        status: RunStatus::Skipped,
        finished_at: Some(Utc::now().to_rfc3339()),
        message: Some(message),
        ..running_job_record(job, plan_node_id, ctx)
    }
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

fn write_prepared_outputs_record(
    run_dir: &Path,
    record: &PreparedOutputsRecord,
) -> anyhow::Result<PathBuf> {
    let path = run_dir.join("prepared-outputs.json");
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

#[derive(Clone)]
enum PrepareAction {
    NixBuildOutput {
        installable: String,
        output_name: &'static str,
        handoff: Option<PreparedOutputHandoff>,
        mount_paths: Vec<PathBuf>,
        log_paths: Vec<PathBuf>,
    },
    VfkitRunner {
        installable: String,
        runner_link: PathBuf,
        log_paths: Vec<PathBuf>,
    },
}

#[derive(Clone)]
struct PlannedPrepare {
    node_id: String,
    depends_on: Vec<String>,
    action: PrepareAction,
}

struct PrepareFailure {
    node_id: String,
    message: String,
}

struct PreparedOutputMaterialization<'a> {
    node_id: &'a str,
    installable: &'a str,
    output_name: &'a str,
    protocol: PreparedOutputHandoffProtocol,
    realized_path: &'a Path,
}

#[derive(Debug)]
struct PreparedOutputConsumerResult {
    kind: PreparedOutputConsumerKind,
    exposures: Vec<PreparedOutputExposure>,
    requested_exposures: Vec<PreparedOutputExposure>,
    consumer_request_path: Option<String>,
    consumer_result_path: Option<String>,
    consumer_launch_request_path: Option<String>,
    consumer_transport_request_path: Option<String>,
}

#[derive(Debug)]
struct PreparedOutputConsumerFailure {
    kind: PreparedOutputConsumerKind,
    message: String,
    requested_exposures: Vec<PreparedOutputExposure>,
    consumer_request_path: Option<String>,
    consumer_result_path: Option<String>,
    consumer_launch_request_path: Option<String>,
    consumer_transport_request_path: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
struct PreparedOutputInvocationConfig<'a> {
    invocation_mode: Option<PreparedOutputInvocationMode>,
    launcher_program: Option<&'a Path>,
    launcher_transport_mode: Option<PreparedOutputLauncherTransportMode>,
    launcher_transport_program: Option<&'a Path>,
    launcher_transport_host: Option<&'a str>,
    launcher_transport_remote_launcher_program: Option<&'a Path>,
    launcher_transport_remote_helper_program: Option<&'a Path>,
    launcher_transport_remote_work_dir: Option<&'a Path>,
}

#[derive(Clone, Copy, Debug, Default)]
struct PreparedOutputSubprocessConfig<'a> {
    run_dir: Option<&'a Path>,
    launcher_program: Option<&'a Path>,
    launch_request_path: Option<&'a Path>,
    launcher_transport_mode: Option<PreparedOutputLauncherTransportMode>,
    launcher_transport_program: Option<&'a Path>,
    transport_request_path: Option<&'a Path>,
    launcher_transport_host: Option<&'a str>,
    launcher_transport_remote_launcher_program: Option<&'a Path>,
    launcher_transport_remote_helper_program: Option<&'a Path>,
    launcher_transport_remote_work_dir: Option<&'a Path>,
}

trait PreparedOutputConsumer {
    fn kind(&self) -> PreparedOutputConsumerKind;

    fn consume(
        &self,
        materialization: &PreparedOutputMaterialization<'_>,
        handoff: &PreparedOutputHandoff,
        run_dir: &Path,
        log_paths: &[PathBuf],
        invocation: PreparedOutputInvocationConfig<'_>,
    ) -> Result<PreparedOutputConsumerResult, Box<PreparedOutputConsumerFailure>>;
}

trait PreparedOutputFulfillmentInvoker {
    fn mode(&self) -> PreparedOutputInvocationMode;

    fn invoke(
        &self,
        helper_program: &Path,
        subprocess: PreparedOutputSubprocessConfig<'_>,
        request_path: &Path,
        result_path: &Path,
        log_paths: &[PathBuf],
    ) -> anyhow::Result<std::process::Output>;
}

trait PreparedOutputFulfillmentLauncherTransport {
    fn mode(&self) -> PreparedOutputLauncherTransportMode;

    fn invoke(
        &self,
        launcher_program: &Path,
        transport_program: Option<&Path>,
        launch_request_path: &Path,
        transport_request_path: Option<&Path>,
    ) -> anyhow::Result<std::process::Output>;
}

struct HostLocalSymlinkPreparedOutputConsumer;
struct RemoteExposureRequestPreparedOutputConsumer;
struct FulfillRequestCliPreparedOutputConsumer;
struct DirectHelperExecPreparedOutputFulfillmentInvoker;
struct ExternalWrapperPreparedOutputFulfillmentInvoker;
struct DirectLauncherExecPreparedOutputFulfillmentTransport;
struct CommandTransportPreparedOutputFulfillmentTransport;
struct SshLauncherTransportPreparedOutputFulfillmentTransport;

#[derive(Clone)]
struct PlannedJob {
    job: JobSpec,
    execute_node_id: String,
    depends_on: Vec<String>,
    ctx: HostContext,
}

struct RunPlan {
    record: RunPlanRecord,
    prepares: Vec<PlannedPrepare>,
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
    let mut prepare_nodes = HashMap::new();
    let mut planned_prepares = Vec::new();
    let mut execute_nodes = Vec::new();
    let mut planned_jobs = Vec::new();

    for job in jobs {
        let job_dir = prepared.jobs_dir.join(job.id);
        fs::create_dir_all(&job_dir).with_context(|| format!("create {}", job_dir.display()))?;
        let ctx = HostContext {
            workspace_snapshot_dir: prepare_job_workspace(job, &snapshot.snapshot_dir, &job_dir)?,
            workspace_read_only: !job.writable_workspace,
            job_dir: job_dir.clone(),
            host_log_path: job_dir.join("host.log"),
            guest_log_path: job_dir.join("artifacts/guest.log"),
            shared_cargo_home_dir: prepared.shared_cargo_home_dir.clone(),
            shared_target_dir: prepared.run_target_dir.clone(),
            staged_linux_rust_workspace_deps_dir: job
                .staged_linux_rust_lane()
                .map(|_| job_dir.join("staged-linux-rust").join("workspace-deps")),
            staged_linux_rust_workspace_build_dir: job
                .staged_linux_rust_lane()
                .map(|_| job_dir.join("staged-linux-rust").join("workspace-build")),
        };
        ensure_log_file(&ctx.host_log_path)?;
        ensure_log_file(&ctx.guest_log_path)?;

        let mut depends_on = Vec::new();
        if let Some(lane) = job.staged_linux_rust_lane() {
            let prefix = lane.shared_prepare_node_prefix();
            let deps_node_id = format!("prepare-{prefix}-workspace-deps");
            let build_node_id = format!("prepare-{prefix}-workspace-build");
            let workspace_deps_installable =
                staged_linux_rust_installable(&snapshot.snapshot_dir, lane, true);
            let workspace_build_installable =
                staged_linux_rust_installable(&snapshot.snapshot_dir, lane, false);

            prepare_nodes
                .entry(deps_node_id.clone())
                .or_insert_with(|| {
                    planned_prepares.push(PlannedPrepare {
                        node_id: deps_node_id.clone(),
                        depends_on: Vec::new(),
                        action: PrepareAction::NixBuildOutput {
                            installable: workspace_deps_installable.clone(),
                            output_name: lane.workspace_deps_output_name(),
                            handoff: Some(PreparedOutputHandoff {
                                protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
                                exposures: Vec::new(),
                            }),
                            mount_paths: Vec::new(),
                            log_paths: Vec::new(),
                        },
                    });
                    PlanNodeRecord::Prepare {
                        id: deps_node_id.clone(),
                        description: format!(
                            "Build staged Linux Rust dependencies for {}",
                            lane.shared_prepare_description()
                        ),
                        executor: PlanExecutorKind::HostLocal,
                        depends_on: Vec::new(),
                        prepare: PrepareNode::NixBuild {
                            installable: workspace_deps_installable.clone(),
                            output_name: lane.workspace_deps_output_name().to_string(),
                            handoff: Some(PreparedOutputHandoff {
                                protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
                                exposures: Vec::new(),
                            }),
                        },
                    }
                });
            prepare_nodes
                .entry(build_node_id.clone())
                .or_insert_with(|| {
                    planned_prepares.push(PlannedPrepare {
                        node_id: build_node_id.clone(),
                        depends_on: vec![deps_node_id.clone()],
                        action: PrepareAction::NixBuildOutput {
                            installable: workspace_build_installable.clone(),
                            output_name: lane.workspace_build_output_name(),
                            handoff: Some(PreparedOutputHandoff {
                                protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
                                exposures: Vec::new(),
                            }),
                            mount_paths: Vec::new(),
                            log_paths: Vec::new(),
                        },
                    });
                    PlanNodeRecord::Prepare {
                        id: build_node_id.clone(),
                        description: format!(
                            "Build staged Linux Rust test artifacts for {}",
                            lane.shared_prepare_description()
                        ),
                        executor: PlanExecutorKind::HostLocal,
                        depends_on: vec![deps_node_id.clone()],
                        prepare: PrepareNode::NixBuild {
                            installable: workspace_build_installable.clone(),
                            output_name: lane.workspace_build_output_name().to_string(),
                            handoff: Some(PreparedOutputHandoff {
                                protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
                                exposures: Vec::new(),
                            }),
                        },
                    }
                });

            add_staged_mount_consumer(
                &mut planned_prepares,
                &mut prepare_nodes,
                &deps_node_id,
                ctx.staged_linux_rust_workspace_deps_dir
                    .as_ref()
                    .ok_or_else(|| anyhow!("missing staged Linux Rust workspaceDeps mount path"))?,
                &ctx.host_log_path,
            );
            add_staged_mount_consumer(
                &mut planned_prepares,
                &mut prepare_nodes,
                &build_node_id,
                ctx.staged_linux_rust_workspace_build_dir
                    .as_ref()
                    .ok_or_else(|| {
                        anyhow!("missing staged Linux Rust workspaceBuild mount path")
                    })?,
                &ctx.host_log_path,
            );
            depends_on.push(deps_node_id);
            depends_on.push(build_node_id);
        }
        if job.runner_kind() == RunnerKind::VfkitLocal {
            let prepare_node_id = format!("prepare-{}-runner", job.id);
            let installable = materialize_vfkit_runner_flake(job, &ctx)?;
            planned_prepares.push(PlannedPrepare {
                node_id: prepare_node_id.clone(),
                depends_on: Vec::new(),
                action: PrepareAction::VfkitRunner {
                    installable: installable.clone(),
                    runner_link: ctx.job_dir.join("vm").join("runner"),
                    log_paths: vec![ctx.host_log_path.clone()],
                },
            });
            prepare_nodes.insert(
                prepare_node_id.clone(),
                PlanNodeRecord::Prepare {
                    id: prepare_node_id.clone(),
                    description: format!("Build vfkit runner for `{}`", job.id),
                    executor: PlanExecutorKind::HostLocal,
                    depends_on: Vec::new(),
                    prepare: PrepareNode::NixBuild {
                        installable,
                        output_name:
                            "nixosConfigurations.pikaci-wave1.config.microvm.declaredRunner"
                                .to_string(),
                        handoff: None,
                    },
                },
            );
            depends_on.push(prepare_node_id);
        }

        let execute_node_id = format!("execute-{}", job.id);
        let (command, run_as_root) = compiled_guest_command(job);
        execute_nodes.push(PlanNodeRecord::Execute {
            id: execute_node_id.clone(),
            description: job.description.to_string(),
            executor: job.runner_kind().into(),
            depends_on: depends_on.clone(),
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
            depends_on,
            ctx,
        });
    }

    let mut nodes: Vec<PlanNodeRecord> = planned_prepares
        .iter()
        .map(|prepare| {
            prepare_nodes
                .get(&prepare.node_id)
                .cloned()
                .expect("missing prepare node record")
        })
        .collect();
    nodes.extend(execute_nodes);
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
        prepares: planned_prepares,
        jobs: planned_jobs,
    })
}

fn staged_linux_rust_installable(
    snapshot_dir: &Path,
    lane: StagedLinuxRustLane,
    deps_only: bool,
) -> String {
    let output_name = if deps_only {
        lane.workspace_deps_output_name()
    } else {
        lane.workspace_build_output_name()
    };
    format!("path:{}#{output_name}", snapshot_dir.display())
}

fn add_staged_mount_consumer(
    prepares: &mut [PlannedPrepare],
    prepare_nodes: &mut HashMap<String, PlanNodeRecord>,
    node_id: &str,
    mount_path: &Path,
    log_path: &Path,
) {
    let Some(prepare) = prepares
        .iter_mut()
        .find(|prepare| prepare.node_id == node_id)
    else {
        return;
    };
    let PrepareAction::NixBuildOutput {
        handoff,
        mount_paths,
        log_paths,
        ..
    } = &mut prepare.action
    else {
        return;
    };
    if !mount_paths.iter().any(|path| path == mount_path) {
        mount_paths.push(mount_path.to_path_buf());
    }
    if !log_paths.iter().any(|path| path == log_path) {
        log_paths.push(log_path.to_path_buf());
    }
    if let Some(handoff) = handoff {
        add_handoff_exposure(handoff, mount_path);
    }
    if let Some(PlanNodeRecord::Prepare {
        prepare:
            PrepareNode::NixBuild {
                handoff: Some(handoff),
                ..
            },
        ..
    }) = prepare_nodes.get_mut(node_id)
    {
        add_handoff_exposure(handoff, mount_path);
    }
}

fn add_handoff_exposure(handoff: &mut PreparedOutputHandoff, mount_path: &Path) {
    let exposure = PreparedOutputExposure {
        kind: PreparedOutputExposureKind::HostSymlinkMount,
        path: mount_path.display().to_string(),
        access: PreparedOutputExposureAccess::ReadOnly,
    };
    if !handoff
        .exposures
        .iter()
        .any(|existing| existing == &exposure)
    {
        handoff.exposures.push(exposure);
    }
}

fn repoint_prepare_mount(mount_path: &Path, output_path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = mount_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    if mount_path.exists() || mount_path.is_symlink() {
        fs::remove_file(mount_path)
            .or_else(|_| fs::remove_dir_all(mount_path))
            .with_context(|| format!("remove {}", mount_path.display()))?;
    }
    unix_fs::symlink(output_path, mount_path).with_context(|| {
        format!(
            "symlink {} -> {}",
            mount_path.display(),
            output_path.display()
        )
    })
}

pub fn fulfill_prepared_output_request(
    request_path: &Path,
) -> anyhow::Result<PreparedOutputRemoteExposureRequest> {
    let request = load_prepared_output_request(request_path)?;
    if request.schema_version != 1 {
        return Err(anyhow!(
            "prepared output request `{}` uses unsupported schema_version {}; expected 1",
            request_path.display(),
            request.schema_version
        ));
    }
    if request.protocol != PreparedOutputHandoffProtocol::NixStorePathV1 {
        return Err(anyhow!(
            "prepared output request `{}` uses unsupported protocol {:?}",
            request_path.display(),
            request.protocol
        ));
    }
    let realized_path = Path::new(&request.realized_path);
    let nix_store_root = Path::new("/nix/store");
    if !realized_path.is_absolute() || !realized_path.starts_with(nix_store_root) {
        return Err(anyhow!(
            "prepared output request `{}` points at non-Nix-store realized path {}",
            request_path.display(),
            realized_path.display()
        ));
    }
    if !realized_path.exists() {
        return Err(anyhow!(
            "prepared output request `{}` points at missing realized path {}",
            request_path.display(),
            realized_path.display()
        ));
    }
    for exposure in &request.requested_exposures {
        match exposure.kind {
            PreparedOutputExposureKind::HostSymlinkMount => {
                repoint_prepare_mount(Path::new(&exposure.path), realized_path)?;
            }
        }
    }
    Ok(request)
}

pub fn fulfill_prepared_output_request_result(
    request_path: &Path,
) -> PreparedOutputFulfillmentResult {
    match fulfill_prepared_output_request(request_path) {
        Ok(request) => PreparedOutputFulfillmentResult {
            schema_version: 1,
            request_path: request_path.display().to_string(),
            node_id: Some(request.node_id),
            output_name: Some(request.output_name),
            realized_path: Some(request.realized_path),
            status: PreparedOutputFulfillmentStatus::Succeeded,
            fulfilled_exposures_count: request.requested_exposures.len(),
            fulfilled_exposures: request.requested_exposures,
            error: None,
        },
        Err(err) => {
            let request = load_prepared_output_request(request_path).ok();
            PreparedOutputFulfillmentResult {
                schema_version: 1,
                request_path: request_path.display().to_string(),
                node_id: request.as_ref().map(|request| request.node_id.clone()),
                output_name: request.as_ref().map(|request| request.output_name.clone()),
                realized_path: request
                    .as_ref()
                    .map(|request| request.realized_path.clone()),
                status: PreparedOutputFulfillmentStatus::Failed,
                fulfilled_exposures_count: 0,
                fulfilled_exposures: Vec::new(),
                error: Some(format!("{err:#}")),
            }
        }
    }
}

fn write_prepared_output_remote_exposure_request(
    materialization: &PreparedOutputMaterialization<'_>,
    handoff: &PreparedOutputHandoff,
    run_dir: &Path,
) -> anyhow::Result<PathBuf> {
    let requests_dir = run_dir.join("prepared-output-requests");
    fs::create_dir_all(&requests_dir)
        .with_context(|| format!("create {}", requests_dir.display()))?;
    let request_path = requests_dir.join(format!("{}.json", materialization.node_id));
    write_json(
        request_path.clone(),
        &PreparedOutputRemoteExposureRequest {
            schema_version: 1,
            node_id: materialization.node_id.to_string(),
            installable: materialization.installable.to_string(),
            output_name: materialization.output_name.to_string(),
            protocol: materialization.protocol,
            realized_path: materialization.realized_path.display().to_string(),
            requested_exposures: handoff.exposures.clone(),
        },
    )?;
    Ok(request_path)
}

fn load_prepared_output_request(
    request_path: &Path,
) -> anyhow::Result<PreparedOutputRemoteExposureRequest> {
    let bytes =
        fs::read(request_path).with_context(|| format!("read {}", request_path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decode {}", request_path.display()))
}

pub fn write_prepared_output_fulfillment_result(
    result_path: &Path,
    result: &PreparedOutputFulfillmentResult,
) -> anyhow::Result<()> {
    if let Some(parent) = result_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    write_json(result_path.to_path_buf(), result)
}

fn load_prepared_output_fulfillment_result(
    result_path: &Path,
) -> anyhow::Result<PreparedOutputFulfillmentResult> {
    let bytes = fs::read(result_path).with_context(|| format!("read {}", result_path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decode {}", result_path.display()))
}

pub fn write_prepared_output_fulfillment_launch_request(
    request_path: &Path,
    request: &PreparedOutputFulfillmentLaunchRequest,
) -> anyhow::Result<()> {
    if let Some(parent) = request_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    write_json(request_path.to_path_buf(), request)
}

pub fn load_prepared_output_fulfillment_launch_request(
    request_path: &Path,
) -> anyhow::Result<PreparedOutputFulfillmentLaunchRequest> {
    let bytes =
        fs::read(request_path).with_context(|| format!("read {}", request_path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decode {}", request_path.display()))
}

pub fn write_prepared_output_fulfillment_transport_request(
    request_path: &Path,
    request: &PreparedOutputFulfillmentTransportRequest,
) -> anyhow::Result<()> {
    if let Some(parent) = request_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    write_json(request_path.to_path_buf(), request)
}

pub fn load_prepared_output_fulfillment_transport_request(
    request_path: &Path,
) -> anyhow::Result<PreparedOutputFulfillmentTransportRequest> {
    let bytes =
        fs::read(request_path).with_context(|| format!("read {}", request_path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decode {}", request_path.display()))
}

fn prepared_output_fulfillment_helper_file_name(current_exe: &Path) -> String {
    match current_exe.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if !ext.is_empty() => {
            format!("{PREPARED_OUTPUT_FULFILLMENT_HELPER_BASENAME}.{ext}")
        }
        _ => PREPARED_OUTPUT_FULFILLMENT_HELPER_BASENAME.to_string(),
    }
}

fn resolve_prepared_output_fulfillment_program(
    explicit_program: Option<PathBuf>,
    current_exe: PathBuf,
) -> anyhow::Result<PathBuf> {
    if let Some(path) = explicit_program {
        return Ok(path);
    }
    let current_stem = current_exe.file_stem().and_then(|name| name.to_str());
    if current_stem.is_some_and(|name| name == PREPARED_OUTPUT_FULFILLMENT_HELPER_BASENAME) {
        return Ok(current_exe);
    }
    if current_stem.is_some_and(|name| name == "pikaci") {
        return Ok(
            current_exe.with_file_name(prepared_output_fulfillment_helper_file_name(&current_exe))
        );
    }
    Err(anyhow!(
        "PIKACI_PREPARED_OUTPUT_CONSUMER=fulfill_request_cli_v1 requires PIKACI_PREPARED_OUTPUT_FULFILL_BINARY when the host executable is neither `pikaci` nor `{PREPARED_OUTPUT_FULFILLMENT_HELPER_BASENAME}`; current executable is {}",
        current_exe.display()
    ))
}

fn prepared_output_fulfillment_program() -> anyhow::Result<PathBuf> {
    let explicit_program = std::env::var("PIKACI_PREPARED_OUTPUT_FULFILL_BINARY")
        .ok()
        .map(PathBuf::from);
    let current_exe = std::env::current_exe()
        .context("resolve host executable for prepared-output fulfillment")?;
    resolve_prepared_output_fulfillment_program(explicit_program, current_exe)
}

fn resolve_prepared_output_fulfillment_launcher_file_name(current_exe: &Path) -> String {
    match current_exe
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some(extension) if !extension.is_empty() => {
            format!("{PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_BASENAME}.{extension}")
        }
        _ => PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_BASENAME.to_string(),
    }
}

fn prepared_output_fulfillment_launcher_program() -> anyhow::Result<PathBuf> {
    if let Ok(explicit_program) = std::env::var(PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_BINARY_ENV) {
        return Ok(PathBuf::from(explicit_program));
    }
    if let Ok(explicit_program) = std::env::var(PREPARED_OUTPUT_FULFILLMENT_WRAPPER_BINARY_ENV) {
        return Ok(PathBuf::from(explicit_program));
    }
    let current_exe = std::env::current_exe()
        .context("resolve host executable for prepared-output fulfillment launcher")?;
    let current_stem = current_exe.file_stem().and_then(|name| name.to_str());
    if current_stem.is_some_and(|name| name == PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_BASENAME) {
        return Ok(current_exe);
    }
    if current_stem.is_some_and(|name| name == "pikaci")
        || current_stem.is_some_and(|name| name == PREPARED_OUTPUT_FULFILLMENT_HELPER_BASENAME)
    {
        return Ok(current_exe.with_file_name(
            resolve_prepared_output_fulfillment_launcher_file_name(&current_exe),
        ));
    }
    Err(anyhow!(
        "{PREPARED_OUTPUT_FULFILLMENT_INVOCATION_ENV}=external_wrapper_command_v1 requires {PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_BINARY_ENV} when the host executable is neither `pikaci`, `{PREPARED_OUTPUT_FULFILLMENT_HELPER_BASENAME}`, nor `{PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_BASENAME}`; current executable is {}",
        current_exe.display()
    ))
}

fn configured_prepared_output_invocation_mode() -> anyhow::Result<PreparedOutputInvocationMode> {
    match std::env::var(PREPARED_OUTPUT_FULFILLMENT_INVOCATION_ENV)
        .ok()
        .as_deref()
    {
        None => Ok(PreparedOutputInvocationMode::DirectHelperExecV1),
        Some("direct_helper_exec_v1") => Ok(PreparedOutputInvocationMode::DirectHelperExecV1),
        Some("external_wrapper_command_v1") => {
            Ok(PreparedOutputInvocationMode::ExternalWrapperCommandV1)
        }
        Some(value) => Err(anyhow!(
            "unsupported {PREPARED_OUTPUT_FULFILLMENT_INVOCATION_ENV} `{value}`; expected `direct_helper_exec_v1` or `external_wrapper_command_v1`"
        )),
    }
}

fn configured_prepared_output_launcher_transport_mode()
-> anyhow::Result<PreparedOutputLauncherTransportMode> {
    match std::env::var(PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_TRANSPORT_ENV)
        .ok()
        .as_deref()
    {
        None => Ok(PreparedOutputLauncherTransportMode::DirectLauncherExecV1),
        Some("direct_launcher_exec_v1") => {
            Ok(PreparedOutputLauncherTransportMode::DirectLauncherExecV1)
        }
        Some("command_transport_v1") => Ok(PreparedOutputLauncherTransportMode::CommandTransportV1),
        Some("ssh_launcher_transport_v1") => {
            Ok(PreparedOutputLauncherTransportMode::SshLauncherTransportV1)
        }
        Some(value) => Err(anyhow!(
            "unsupported {PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_TRANSPORT_ENV} `{value}`; expected `direct_launcher_exec_v1`, `command_transport_v1`, or `ssh_launcher_transport_v1`"
        )),
    }
}

fn resolve_run_prepared_output_invocation_wrapper_program(
    invocation_mode: Option<PreparedOutputInvocationMode>,
    recorded_wrapper_program: Option<&str>,
) -> anyhow::Result<Option<String>> {
    match invocation_mode {
        Some(PreparedOutputInvocationMode::ExternalWrapperCommandV1) => {
            if let Some(recorded_wrapper_program) = recorded_wrapper_program {
                return Ok(Some(recorded_wrapper_program.to_string()));
            }
            Ok(Some(
                prepared_output_fulfillment_launcher_program()?
                    .display()
                    .to_string(),
            ))
        }
        _ => Ok(None),
    }
}

fn resolve_run_prepared_output_launcher_transport_mode(
    invocation_mode: Option<PreparedOutputInvocationMode>,
    recorded_transport_mode: Option<PreparedOutputLauncherTransportMode>,
) -> anyhow::Result<Option<PreparedOutputLauncherTransportMode>> {
    if invocation_mode != Some(PreparedOutputInvocationMode::ExternalWrapperCommandV1) {
        return Ok(None);
    }
    if let Some(recorded_transport_mode) = recorded_transport_mode {
        return Ok(Some(recorded_transport_mode));
    }
    Ok(Some(configured_prepared_output_launcher_transport_mode()?))
}

fn resolve_run_prepared_output_launcher_transport_program(
    transport_mode: Option<PreparedOutputLauncherTransportMode>,
    recorded_transport_program: Option<&str>,
) -> anyhow::Result<Option<String>> {
    match transport_mode {
        Some(PreparedOutputLauncherTransportMode::CommandTransportV1) => {
            if let Some(recorded_transport_program) = recorded_transport_program {
                return Ok(Some(recorded_transport_program.to_string()));
            }
            Ok(Some(
                std::env::var(PREPARED_OUTPUT_FULFILLMENT_TRANSPORT_BINARY_ENV)
                    .map_err(|_| {
                        anyhow!(
                            "{PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_TRANSPORT_ENV}=command_transport_v1 requires {PREPARED_OUTPUT_FULFILLMENT_TRANSPORT_BINARY_ENV}"
                        )
                    })?,
            ))
        }
        Some(PreparedOutputLauncherTransportMode::SshLauncherTransportV1) => Ok(Some(
            recorded_transport_program
                .map(str::to_string)
                .unwrap_or_else(|| {
                    std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_BINARY_ENV)
                        .unwrap_or_else(|_| "ssh".to_string())
                }),
        )),
        _ => Ok(None),
    }
}

fn resolve_run_prepared_output_launcher_transport_host(
    transport_mode: Option<PreparedOutputLauncherTransportMode>,
    recorded_transport_host: Option<&str>,
) -> anyhow::Result<Option<String>> {
    match transport_mode {
        Some(PreparedOutputLauncherTransportMode::SshLauncherTransportV1) => Ok(Some(
            recorded_transport_host
                .map(str::to_string)
                .unwrap_or_else(|| {
                    std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV)
                        .unwrap_or_else(|_| "pika-build".to_string())
                }),
        )),
        _ => Ok(None),
    }
}

fn resolve_run_prepared_output_launcher_transport_remote_launcher_program(
    transport_mode: Option<PreparedOutputLauncherTransportMode>,
    recorded_remote_launcher_program: Option<&str>,
) -> anyhow::Result<Option<String>> {
    match transport_mode {
        Some(PreparedOutputLauncherTransportMode::SshLauncherTransportV1) => {
            if let Some(recorded_remote_launcher_program) = recorded_remote_launcher_program {
                return Ok(Some(recorded_remote_launcher_program.to_string()));
            }
            Ok(Some(
                std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_LAUNCHER_BINARY_ENV)
                    .map_err(|_| {
                        anyhow!(
                            "{PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_TRANSPORT_ENV}=ssh_launcher_transport_v1 requires {PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_LAUNCHER_BINARY_ENV}"
                        )
                    })?,
            ))
        }
        _ => Ok(None),
    }
}

fn resolve_run_prepared_output_launcher_transport_remote_helper_program(
    transport_mode: Option<PreparedOutputLauncherTransportMode>,
    recorded_remote_helper_program: Option<&str>,
) -> anyhow::Result<Option<String>> {
    match transport_mode {
        Some(PreparedOutputLauncherTransportMode::SshLauncherTransportV1) => {
            if let Some(recorded_remote_helper_program) = recorded_remote_helper_program {
                return Ok(Some(recorded_remote_helper_program.to_string()));
            }
            Ok(Some(
                std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_HELPER_BINARY_ENV)
                    .map_err(|_| {
                        anyhow!(
                            "{PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_TRANSPORT_ENV}=ssh_launcher_transport_v1 requires {PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_HELPER_BINARY_ENV}"
                        )
                    })?,
            ))
        }
        _ => Ok(None),
    }
}

fn resolve_run_prepared_output_launcher_transport_remote_work_dir(
    transport_mode: Option<PreparedOutputLauncherTransportMode>,
    recorded_remote_work_dir: Option<&str>,
) -> anyhow::Result<Option<String>> {
    match transport_mode {
        Some(PreparedOutputLauncherTransportMode::SshLauncherTransportV1) => Ok(Some(
            recorded_remote_work_dir
                .map(str::to_string)
                .unwrap_or_else(|| {
                    std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_WORK_DIR_ENV)
                        .unwrap_or_else(|_| "/tmp/pikaci-prepared-output".to_string())
                }),
        )),
        _ => Ok(None),
    }
}

fn selected_prepared_output_fulfillment_invoker(
    mode: PreparedOutputInvocationMode,
) -> Box<dyn PreparedOutputFulfillmentInvoker> {
    match mode {
        PreparedOutputInvocationMode::DirectHelperExecV1 => {
            Box::new(DirectHelperExecPreparedOutputFulfillmentInvoker)
        }
        PreparedOutputInvocationMode::ExternalWrapperCommandV1 => {
            Box::new(ExternalWrapperPreparedOutputFulfillmentInvoker)
        }
    }
}

fn prepared_output_invocation_mode_text(mode: PreparedOutputInvocationMode) -> &'static str {
    match mode {
        PreparedOutputInvocationMode::DirectHelperExecV1 => "direct_helper_exec_v1",
        PreparedOutputInvocationMode::ExternalWrapperCommandV1 => "external_wrapper_command_v1",
    }
}

fn selected_prepared_output_fulfillment_launcher_transport(
    mode: PreparedOutputLauncherTransportMode,
) -> Box<dyn PreparedOutputFulfillmentLauncherTransport> {
    match mode {
        PreparedOutputLauncherTransportMode::DirectLauncherExecV1 => {
            Box::new(DirectLauncherExecPreparedOutputFulfillmentTransport)
        }
        PreparedOutputLauncherTransportMode::CommandTransportV1 => {
            Box::new(CommandTransportPreparedOutputFulfillmentTransport)
        }
        PreparedOutputLauncherTransportMode::SshLauncherTransportV1 => {
            Box::new(SshLauncherTransportPreparedOutputFulfillmentTransport)
        }
    }
}

fn prepared_output_launcher_transport_mode_text(
    mode: PreparedOutputLauncherTransportMode,
) -> &'static str {
    match mode {
        PreparedOutputLauncherTransportMode::DirectLauncherExecV1 => "direct_launcher_exec_v1",
        PreparedOutputLauncherTransportMode::CommandTransportV1 => "command_transport_v1",
        PreparedOutputLauncherTransportMode::SshLauncherTransportV1 => "ssh_launcher_transport_v1",
    }
}

impl PreparedOutputFulfillmentInvoker for DirectHelperExecPreparedOutputFulfillmentInvoker {
    fn mode(&self) -> PreparedOutputInvocationMode {
        PreparedOutputInvocationMode::DirectHelperExecV1
    }

    fn invoke(
        &self,
        helper_program: &Path,
        _subprocess: PreparedOutputSubprocessConfig<'_>,
        request_path: &Path,
        result_path: &Path,
        _log_paths: &[PathBuf],
    ) -> anyhow::Result<std::process::Output> {
        Command::new(helper_program)
            .arg("--result-path")
            .arg(result_path)
            .arg(request_path)
            .output()
            .with_context(|| {
                format!(
                    "run prepared-output fulfillment helper `{}` for {}",
                    helper_program.display(),
                    request_path.display()
                )
            })
    }
}

impl PreparedOutputFulfillmentInvoker for ExternalWrapperPreparedOutputFulfillmentInvoker {
    fn mode(&self) -> PreparedOutputInvocationMode {
        PreparedOutputInvocationMode::ExternalWrapperCommandV1
    }

    fn invoke(
        &self,
        helper_program: &Path,
        subprocess: PreparedOutputSubprocessConfig<'_>,
        _request_path: &Path,
        _result_path: &Path,
        _log_paths: &[PathBuf],
    ) -> anyhow::Result<std::process::Output> {
        let launcher_program = subprocess.launcher_program.ok_or_else(|| {
            anyhow!(
                "missing launcher program for {}",
                prepared_output_invocation_mode_text(
                    PreparedOutputInvocationMode::ExternalWrapperCommandV1
                )
            )
        })?;
        let launch_request_path = subprocess.launch_request_path.ok_or_else(|| {
            anyhow!(
                "missing launch request path for {}",
                prepared_output_invocation_mode_text(
                    PreparedOutputInvocationMode::ExternalWrapperCommandV1
                )
            )
        })?;
        let launcher_transport_mode = subprocess
            .launcher_transport_mode
            .unwrap_or(PreparedOutputLauncherTransportMode::DirectLauncherExecV1);
        let launcher_transport =
            selected_prepared_output_fulfillment_launcher_transport(launcher_transport_mode);
        launcher_transport
            .invoke(
                launcher_program,
                subprocess.launcher_transport_program,
                launch_request_path,
                subprocess.transport_request_path,
            )
            .with_context(|| {
                format!(
                    "run prepared-output fulfillment launcher `{}` with helper `{}` via transport {}",
                    launcher_program.display(),
                    helper_program.display(),
                    prepared_output_launcher_transport_mode_text(launcher_transport.mode())
                )
            })
    }
}

impl PreparedOutputFulfillmentLauncherTransport
    for DirectLauncherExecPreparedOutputFulfillmentTransport
{
    fn mode(&self) -> PreparedOutputLauncherTransportMode {
        PreparedOutputLauncherTransportMode::DirectLauncherExecV1
    }

    fn invoke(
        &self,
        launcher_program: &Path,
        _transport_program: Option<&Path>,
        launch_request_path: &Path,
        _transport_request_path: Option<&Path>,
    ) -> anyhow::Result<std::process::Output> {
        Command::new(launcher_program)
            .arg(launch_request_path)
            .output()
            .with_context(|| {
                format!(
                    "run prepared-output fulfillment launcher `{}` with {}",
                    launcher_program.display(),
                    launch_request_path.display()
                )
            })
    }
}

impl PreparedOutputFulfillmentLauncherTransport
    for CommandTransportPreparedOutputFulfillmentTransport
{
    fn mode(&self) -> PreparedOutputLauncherTransportMode {
        PreparedOutputLauncherTransportMode::CommandTransportV1
    }

    fn invoke(
        &self,
        launcher_program: &Path,
        transport_program: Option<&Path>,
        _launch_request_path: &Path,
        transport_request_path: Option<&Path>,
    ) -> anyhow::Result<std::process::Output> {
        let transport_program = transport_program.ok_or_else(|| {
            anyhow!(
                "missing transport program for {}",
                prepared_output_launcher_transport_mode_text(
                    PreparedOutputLauncherTransportMode::CommandTransportV1
                )
            )
        })?;
        let transport_request_path = transport_request_path.ok_or_else(|| {
            anyhow!(
                "missing transport request path for {}",
                prepared_output_launcher_transport_mode_text(
                    PreparedOutputLauncherTransportMode::CommandTransportV1
                )
            )
        })?;
        Command::new(transport_program)
            .arg(transport_request_path)
            .output()
            .with_context(|| {
                format!(
                    "run prepared-output fulfillment launcher transport `{}` for launcher `{}` via {}",
                    transport_program.display(),
                    launcher_program.display(),
                    transport_request_path.display()
                )
            })
    }
}

fn ssh_transport_required_string(
    request: &PreparedOutputFulfillmentTransportRequest,
    value: Option<&String>,
    label: &str,
) -> anyhow::Result<String> {
    value.cloned().ok_or_else(|| {
        anyhow!(
            "transport request for node {:?} is missing {label}",
            request.node_id
        )
    })
}

fn translate_prepared_output_remote_path(
    local_run_dir: &Path,
    remote_work_dir: &Path,
    local_path: &Path,
) -> anyhow::Result<PathBuf> {
    let relative = local_path.strip_prefix(local_run_dir).with_context(|| {
        format!(
            "translate {} into remote work dir {}",
            local_path.display(),
            remote_work_dir.display()
        )
    })?;
    Ok(remote_work_dir.join(relative))
}

fn write_remote_file_via_ssh(
    ssh_program: &Path,
    host: &str,
    remote_path: &Path,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let remote_parent = remote_path
        .parent()
        .ok_or_else(|| anyhow!("remote path {} has no parent", remote_path.display()))?;
    let mkdir_status = Command::new(ssh_program)
        .arg(host)
        .arg("mkdir")
        .arg("-p")
        .arg(remote_parent)
        .status()
        .with_context(|| {
            format!(
                "create remote directory {} via {} on {}",
                remote_parent.display(),
                ssh_program.display(),
                host
            )
        })?;
    if !mkdir_status.success() {
        return Err(anyhow!(
            "remote mkdir for {} on {} failed with {:?}",
            remote_parent.display(),
            host,
            mkdir_status.code()
        ));
    }

    let mut child = Command::new(ssh_program)
        .arg(host)
        .arg("tee")
        .arg(remote_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .with_context(|| {
            format!(
                "open remote writer for {} via {} on {}",
                remote_path.display(),
                ssh_program.display(),
                host
            )
        })?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| anyhow!("missing stdin for remote tee {}", remote_path.display()))?
        .write_all(bytes)
        .with_context(|| format!("stream {}", remote_path.display()))?;
    let status = child.wait().with_context(|| {
        format!(
            "wait for remote writer {} via {} on {}",
            remote_path.display(),
            ssh_program.display(),
            host
        )
    })?;
    if !status.success() {
        return Err(anyhow!(
            "remote writer for {} on {} failed with {:?}",
            remote_path.display(),
            host,
            status.code()
        ));
    }
    Ok(())
}

fn read_remote_file_via_ssh(
    ssh_program: &Path,
    host: &str,
    remote_path: &Path,
) -> anyhow::Result<Vec<u8>> {
    let output = Command::new(ssh_program)
        .arg(host)
        .arg("cat")
        .arg(remote_path)
        .output()
        .with_context(|| {
            format!(
                "read remote file {} via {} on {}",
                remote_path.display(),
                ssh_program.display(),
                host
            )
        })?;
    if !output.status.success() {
        return Err(anyhow!(
            "remote read for {} on {} failed with {:?}",
            remote_path.display(),
            host,
            output.status.code()
        ));
    }
    Ok(output.stdout)
}

fn validate_remote_fulfillment_result(
    result: &PreparedOutputFulfillmentResult,
    expected_request_path: &Path,
    expected_node_id: &str,
    expected_output_name: &str,
    expected_realized_path: &str,
    expected_exposures: &[PreparedOutputExposure],
) -> anyhow::Result<()> {
    if result.schema_version != 1 {
        return Err(anyhow!(
            "remote helper wrote unsupported schema_version={}",
            result.schema_version
        ));
    }
    if result.status != PreparedOutputFulfillmentStatus::Succeeded {
        return Err(anyhow!(
            "remote helper reported {:?} for {}; {}",
            result.status,
            expected_request_path.display(),
            result
                .error
                .clone()
                .unwrap_or_else(|| "no remote helper error detail".to_string())
        ));
    }
    if result.request_path != expected_request_path.display().to_string() {
        return Err(anyhow!(
            "remote helper reported request_path={} but expected {}",
            result.request_path,
            expected_request_path.display()
        ));
    }
    if result.node_id.as_deref() != Some(expected_node_id) {
        return Err(anyhow!(
            "remote helper reported node_id={:?} but expected {}",
            result.node_id,
            expected_node_id
        ));
    }
    if result.output_name.as_deref() != Some(expected_output_name) {
        return Err(anyhow!(
            "remote helper reported output_name={:?} but expected {}",
            result.output_name,
            expected_output_name
        ));
    }
    if result.realized_path.as_deref() != Some(expected_realized_path) {
        return Err(anyhow!(
            "remote helper reported realized_path={:?} but expected {}",
            result.realized_path,
            expected_realized_path
        ));
    }
    if result.fulfilled_exposures_count != result.fulfilled_exposures.len() {
        return Err(anyhow!(
            "remote helper reported fulfilled_exposures_count={} but returned {} exposure record(s)",
            result.fulfilled_exposures_count,
            result.fulfilled_exposures.len()
        ));
    }
    if result.fulfilled_exposures != expected_exposures {
        return Err(anyhow!(
            "remote helper reported fulfilled exposures that do not match the translated remote request"
        ));
    }
    Ok(())
}

impl PreparedOutputFulfillmentLauncherTransport
    for SshLauncherTransportPreparedOutputFulfillmentTransport
{
    fn mode(&self) -> PreparedOutputLauncherTransportMode {
        PreparedOutputLauncherTransportMode::SshLauncherTransportV1
    }

    fn invoke(
        &self,
        _launcher_program: &Path,
        transport_program: Option<&Path>,
        launch_request_path: &Path,
        transport_request_path: Option<&Path>,
    ) -> anyhow::Result<std::process::Output> {
        let ssh_program = transport_program
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("ssh"));
        let transport_request_path = transport_request_path.ok_or_else(|| {
            anyhow!(
                "missing transport request path for {}",
                prepared_output_launcher_transport_mode_text(
                    PreparedOutputLauncherTransportMode::SshLauncherTransportV1
                )
            )
        })?;
        let transport_request =
            load_prepared_output_fulfillment_transport_request(transport_request_path)?;
        if transport_request.path_contract
            != PreparedOutputFulfillmentTransportPathContract::SshRemoteWorkDirTranslationV1
        {
            return Err(anyhow!(
                "ssh transport requires path_contract=ssh_remote_work_dir_translation_v1 in {}",
                transport_request_path.display()
            ));
        }
        let host = ssh_transport_required_string(
            &transport_request,
            transport_request.remote_host.as_ref(),
            "remote_host",
        )?;
        let local_run_dir = PathBuf::from(ssh_transport_required_string(
            &transport_request,
            transport_request.local_run_dir.as_ref(),
            "local_run_dir",
        )?);
        let remote_work_dir = PathBuf::from(ssh_transport_required_string(
            &transport_request,
            transport_request.remote_work_dir.as_ref(),
            "remote_work_dir",
        )?);
        let remote_launcher_program = PathBuf::from(ssh_transport_required_string(
            &transport_request,
            transport_request.remote_launcher_program.as_ref(),
            "remote_launcher_program",
        )?);
        let remote_helper_program = PathBuf::from(ssh_transport_required_string(
            &transport_request,
            transport_request.remote_helper_program.as_ref(),
            "remote_helper_program",
        )?);

        let launch_request = load_prepared_output_fulfillment_launch_request(launch_request_path)?;
        let helper_request_path = PathBuf::from(&launch_request.helper_request_path);
        let helper_result_path = PathBuf::from(&launch_request.helper_result_path);
        let helper_request = load_prepared_output_request(&helper_request_path)?;
        let remote_launcher_request_path = if let Some(remote_launcher_request_path) =
            transport_request.remote_launcher_request_path.as_deref()
        {
            PathBuf::from(remote_launcher_request_path)
        } else {
            translate_prepared_output_remote_path(
                &local_run_dir,
                &remote_work_dir,
                launch_request_path,
            )?
        };
        let remote_helper_request_path = if let Some(remote_helper_request_path) =
            transport_request.remote_helper_request_path.as_deref()
        {
            PathBuf::from(remote_helper_request_path)
        } else {
            translate_prepared_output_remote_path(
                &local_run_dir,
                &remote_work_dir,
                &helper_request_path,
            )?
        };
        let remote_helper_result_path = if let Some(remote_helper_result_path) =
            transport_request.remote_helper_result_path.as_deref()
        {
            PathBuf::from(remote_helper_result_path)
        } else {
            translate_prepared_output_remote_path(
                &local_run_dir,
                &remote_work_dir,
                &helper_result_path,
            )?
        };

        let translated_exposures = helper_request
            .requested_exposures
            .iter()
            .map(|exposure| {
                Ok(PreparedOutputExposure {
                    kind: exposure.kind,
                    path: translate_prepared_output_remote_path(
                        &local_run_dir,
                        &remote_work_dir,
                        Path::new(&exposure.path),
                    )?
                    .display()
                    .to_string(),
                    access: exposure.access,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let remote_helper_request = PreparedOutputRemoteExposureRequest {
            schema_version: helper_request.schema_version,
            node_id: helper_request.node_id.clone(),
            installable: helper_request.installable.clone(),
            output_name: helper_request.output_name.clone(),
            protocol: helper_request.protocol,
            realized_path: helper_request.realized_path.clone(),
            requested_exposures: translated_exposures.clone(),
        };
        let remote_launch_request = PreparedOutputFulfillmentLaunchRequest {
            schema_version: launch_request.schema_version,
            helper_program: remote_helper_program.display().to_string(),
            helper_request_path: remote_helper_request_path.display().to_string(),
            helper_result_path: remote_helper_result_path.display().to_string(),
            node_id: launch_request.node_id.clone(),
            output_name: launch_request.output_name.clone(),
        };

        let nix_program = std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_NIX_BINARY_ENV)
            .unwrap_or_else(|_| "nix".to_string());
        let nix_copy_status = Command::new(&nix_program)
            .arg("copy")
            .arg("--to")
            .arg(format!("ssh://{host}"))
            .arg(&helper_request.realized_path)
            .status()
            .with_context(|| {
                format!(
                    "copy {} to remote host {} via {}",
                    helper_request.realized_path, host, nix_program
                )
            })?;
        if !nix_copy_status.success() {
            return Err(anyhow!(
                "nix copy of {} to {} failed with {:?}",
                helper_request.realized_path,
                host,
                nix_copy_status.code()
            ));
        }

        let remote_helper_request_bytes = serde_json::to_vec_pretty(&remote_helper_request)
            .context("encode remote helper request")?;
        let remote_launch_request_bytes = serde_json::to_vec_pretty(&remote_launch_request)
            .context("encode remote launch request")?;
        write_remote_file_via_ssh(
            &ssh_program,
            &host,
            &remote_helper_request_path,
            &remote_helper_request_bytes,
        )?;
        write_remote_file_via_ssh(
            &ssh_program,
            &host,
            &remote_launcher_request_path,
            &remote_launch_request_bytes,
        )?;

        let launch_output = Command::new(&ssh_program)
            .arg(&host)
            .arg(&remote_launcher_program)
            .arg(&remote_launcher_request_path)
            .output()
            .with_context(|| {
                format!(
                    "launch remote fulfillment helper on {} via {} with {}",
                    host,
                    ssh_program.display(),
                    remote_launcher_request_path.display()
                )
            })?;

        let remote_result_bytes =
            read_remote_file_via_ssh(&ssh_program, &host, &remote_helper_result_path)?;
        let remote_result: PreparedOutputFulfillmentResult =
            serde_json::from_slice(&remote_result_bytes).with_context(|| {
                format!(
                    "decode remote helper result from {} on {}",
                    remote_helper_result_path.display(),
                    host
                )
            })?;
        validate_remote_fulfillment_result(
            &remote_result,
            &remote_helper_request_path,
            &helper_request.node_id,
            &helper_request.output_name,
            &helper_request.realized_path,
            &translated_exposures,
        )?;

        let local_result =
            fulfill_prepared_output_request_result(Path::new(&launch_request.helper_request_path));
        write_prepared_output_fulfillment_result(
            Path::new(&launch_request.helper_result_path),
            &local_result,
        )?;

        Ok(launch_output)
    }
}

fn fulfill_prepared_output_request_via_subprocess(
    invocation_mode: PreparedOutputInvocationMode,
    subprocess: PreparedOutputSubprocessConfig<'_>,
    request_path: &Path,
    result_path: &Path,
    log_paths: &[PathBuf],
) -> anyhow::Result<PreparedOutputFulfillmentResult> {
    let request = load_prepared_output_request(request_path).with_context(|| {
        format!(
            "load prepared-output fulfillment request before invoking helper {}",
            request_path.display()
        )
    })?;
    let program = prepared_output_fulfillment_program()?;
    let invoker = selected_prepared_output_fulfillment_invoker(invocation_mode);
    let launcher_transport_mode = subprocess
        .launcher_transport_mode
        .or(match invocation_mode {
            PreparedOutputInvocationMode::ExternalWrapperCommandV1 => {
                Some(PreparedOutputLauncherTransportMode::DirectLauncherExecV1)
            }
            PreparedOutputInvocationMode::DirectHelperExecV1 => None,
        });
    if let Some(launch_request_path) = subprocess.launch_request_path {
        write_prepared_output_fulfillment_launch_request(
            launch_request_path,
            &PreparedOutputFulfillmentLaunchRequest {
                schema_version: 1,
                helper_program: program.display().to_string(),
                helper_request_path: request_path.display().to_string(),
                helper_result_path: result_path.display().to_string(),
                node_id: Some(request.node_id.clone()),
                output_name: Some(request.output_name.clone()),
            },
        )?;
    }
    if let (Some(transport_request_path), Some(launcher_program), Some(launcher_transport_mode)) = (
        subprocess.transport_request_path,
        subprocess.launcher_program,
        launcher_transport_mode,
    ) {
        let (remote_launcher_request_path, remote_helper_request_path, remote_helper_result_path) =
            if launcher_transport_mode
                == PreparedOutputLauncherTransportMode::SshLauncherTransportV1
            {
                let local_run_dir = subprocess.run_dir.ok_or_else(|| {
                    anyhow!("missing run_dir for ssh prepared-output launcher transport")
                })?;
                let remote_work_dir =
                    subprocess
                        .launcher_transport_remote_work_dir
                        .ok_or_else(|| {
                            anyhow!(
                                "missing remote work dir for ssh prepared-output launcher transport"
                            )
                        })?;
                (
                    Some(
                        translate_prepared_output_remote_path(
                            local_run_dir,
                            remote_work_dir,
                            subprocess.launch_request_path.ok_or_else(|| {
                                anyhow!("missing launch request path for ssh transport request")
                            })?,
                        )?
                        .display()
                        .to_string(),
                    ),
                    Some(
                        translate_prepared_output_remote_path(
                            local_run_dir,
                            remote_work_dir,
                            request_path,
                        )?
                        .display()
                        .to_string(),
                    ),
                    Some(
                        translate_prepared_output_remote_path(
                            local_run_dir,
                            remote_work_dir,
                            result_path,
                        )?
                        .display()
                        .to_string(),
                    ),
                )
            } else {
                (None, None, None)
            };
        let path_contract = match launcher_transport_mode {
            PreparedOutputLauncherTransportMode::DirectLauncherExecV1
            | PreparedOutputLauncherTransportMode::CommandTransportV1 => {
                PreparedOutputFulfillmentTransportPathContract::SameHostAbsolutePathsV1
            }
            PreparedOutputLauncherTransportMode::SshLauncherTransportV1 => {
                PreparedOutputFulfillmentTransportPathContract::SshRemoteWorkDirTranslationV1
            }
        };
        write_prepared_output_fulfillment_transport_request(
            transport_request_path,
            &PreparedOutputFulfillmentTransportRequest {
                schema_version: 1,
                path_contract,
                launcher_program: launcher_program.display().to_string(),
                launcher_request_path: subprocess
                    .launch_request_path
                    .ok_or_else(|| anyhow!("missing launch request path for transport request"))?
                    .display()
                    .to_string(),
                local_run_dir: subprocess.run_dir.map(|path| path.display().to_string()),
                remote_host: subprocess.launcher_transport_host.map(str::to_string),
                remote_work_dir: subprocess
                    .launcher_transport_remote_work_dir
                    .map(|path| path.display().to_string()),
                remote_launcher_program: subprocess
                    .launcher_transport_remote_launcher_program
                    .map(|path| path.display().to_string()),
                remote_helper_program: subprocess
                    .launcher_transport_remote_helper_program
                    .map(|path| path.display().to_string()),
                remote_launcher_request_path: remote_launcher_request_path.clone(),
                remote_helper_request_path: remote_helper_request_path.clone(),
                remote_helper_result_path: remote_helper_result_path.clone(),
                node_id: Some(request.node_id.clone()),
                output_name: Some(request.output_name.clone()),
            },
        )?;
        append_log_line_many(
            log_paths,
            &format!(
                "[pikaci] prepared output fulfillment launcher_transport_mode={} transport={} transport_host={} remote_launcher={} remote_helper={} remote_work_dir={} remote_launch_request={} remote_helper_request={} remote_helper_result={} transport_request={}",
                prepared_output_launcher_transport_mode_text(launcher_transport_mode),
                subprocess
                    .launcher_transport_program
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string()),
                subprocess.launcher_transport_host.unwrap_or("-"),
                subprocess
                    .launcher_transport_remote_launcher_program
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string()),
                subprocess
                    .launcher_transport_remote_helper_program
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string()),
                subprocess
                    .launcher_transport_remote_work_dir
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string()),
                remote_launcher_request_path.as_deref().unwrap_or("-"),
                remote_helper_request_path.as_deref().unwrap_or("-"),
                remote_helper_result_path.as_deref().unwrap_or("-"),
                transport_request_path.display()
            ),
        )?;
    }
    append_log_line_many(
        log_paths,
        &format!(
            "[pikaci] prepared output fulfillment invocation_mode={} launcher={} launcher_transport_mode={} launcher_transport={} launcher_transport_host={} remote_launcher={} remote_helper={} remote_work_dir={} launch_request={} transport_request={} helper={} request={} result={}",
            prepared_output_invocation_mode_text(invoker.mode()),
            subprocess
                .launcher_program
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            launcher_transport_mode
                .map(prepared_output_launcher_transport_mode_text)
                .unwrap_or("-"),
            subprocess
                .launcher_transport_program
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            subprocess.launcher_transport_host.unwrap_or("-"),
            subprocess
                .launcher_transport_remote_launcher_program
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            subprocess
                .launcher_transport_remote_helper_program
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            subprocess
                .launcher_transport_remote_work_dir
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            subprocess
                .launch_request_path
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            subprocess
                .transport_request_path
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            program.display(),
            request_path.display(),
            result_path.display()
        ),
    )?;
    let output = invoker.invoke(
        &program,
        PreparedOutputSubprocessConfig {
            run_dir: subprocess.run_dir,
            launcher_program: subprocess.launcher_program,
            launch_request_path: subprocess.launch_request_path,
            launcher_transport_mode,
            launcher_transport_program: subprocess.launcher_transport_program,
            transport_request_path: subprocess.transport_request_path,
            launcher_transport_host: subprocess.launcher_transport_host,
            launcher_transport_remote_launcher_program: subprocess
                .launcher_transport_remote_launcher_program,
            launcher_transport_remote_helper_program: subprocess
                .launcher_transport_remote_helper_program,
            launcher_transport_remote_work_dir: subprocess.launcher_transport_remote_work_dir,
        },
        request_path,
        result_path,
        log_paths,
    )?;
    append_command_output_many(log_paths, &output.stdout, &output.stderr)?;
    let result = load_prepared_output_fulfillment_result(result_path).with_context(|| {
        format!(
            "load prepared-output fulfillment result for request {}",
            request_path.display()
        )
    })?;
    append_log_line_many(
        log_paths,
        &format!(
            "[pikaci] prepared output fulfillment result status={:?} exposures={} result={}",
            result.status,
            result.fulfilled_exposures_count,
            result_path.display()
        ),
    )?;
    if result.status != PreparedOutputFulfillmentStatus::Succeeded {
        return Err(anyhow!(
            "prepared-output fulfillment helper `{}` reported {:?} for {}; {}",
            program.display(),
            result.status,
            request_path.display(),
            result
                .error
                .clone()
                .unwrap_or_else(|| "no helper error detail".to_string())
        ));
    }
    if !output.status.success() {
        return Err(anyhow!(
            "prepared-output fulfillment helper `{}` failed with {:?} for {}; {}; see {}",
            program.display(),
            output.status.code(),
            request_path.display(),
            result
                .error
                .clone()
                .unwrap_or_else(|| "no helper error detail".to_string()),
            log_paths
                .first()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<no log>".to_string())
        ));
    }
    if result.schema_version != 1 {
        return Err(anyhow!(
            "prepared-output fulfillment helper `{}` wrote unsupported schema_version={} for {}",
            program.display(),
            result.schema_version,
            request_path.display()
        ));
    }
    if result.request_path != request_path.display().to_string() {
        return Err(anyhow!(
            "prepared-output fulfillment helper `{}` reported request_path={} but expected {}",
            program.display(),
            result.request_path,
            request_path.display()
        ));
    }
    if result.fulfilled_exposures_count != result.fulfilled_exposures.len() {
        return Err(anyhow!(
            "prepared-output fulfillment helper `{}` reported fulfilled_exposures_count={} but returned {} exposure record(s) for {}",
            program.display(),
            result.fulfilled_exposures_count,
            result.fulfilled_exposures.len(),
            request_path.display()
        ));
    }
    if result.fulfilled_exposures != request.requested_exposures {
        return Err(anyhow!(
            "prepared-output fulfillment helper `{}` reported fulfilled exposures that do not match the request for {}",
            program.display(),
            request_path.display()
        ));
    }
    Ok(result)
}

impl PreparedOutputConsumer for HostLocalSymlinkPreparedOutputConsumer {
    fn kind(&self) -> PreparedOutputConsumerKind {
        PreparedOutputConsumerKind::HostLocalSymlinkMountsV1
    }

    fn consume(
        &self,
        materialization: &PreparedOutputMaterialization<'_>,
        handoff: &PreparedOutputHandoff,
        _run_dir: &Path,
        log_paths: &[PathBuf],
        _invocation: PreparedOutputInvocationConfig<'_>,
    ) -> Result<PreparedOutputConsumerResult, Box<PreparedOutputConsumerFailure>> {
        let mut exposures = Vec::new();
        for exposure in &handoff.exposures {
            match exposure.kind {
                PreparedOutputExposureKind::HostSymlinkMount => {
                    let mount_path = Path::new(&exposure.path);
                    repoint_prepare_mount(mount_path, materialization.realized_path).map_err(
                        |err| {
                            Box::new(PreparedOutputConsumerFailure {
                                kind: PreparedOutputConsumerKind::HostLocalSymlinkMountsV1,
                                message: format!("{err:#}"),
                                requested_exposures: Vec::new(),
                                consumer_request_path: None,
                                consumer_result_path: None,
                                consumer_launch_request_path: None,
                                consumer_transport_request_path: None,
                            })
                        },
                    )?;
                    append_log_line_many(
                        log_paths,
                        &format!(
                            "[pikaci] prepared output consumer={} exposed {} -> {}",
                            prepared_output_consumer_kind_text(
                                PreparedOutputConsumerKind::HostLocalSymlinkMountsV1
                            ),
                            mount_path.display(),
                            materialization.realized_path.display()
                        ),
                    )
                    .map_err(|err| {
                        Box::new(PreparedOutputConsumerFailure {
                            kind: PreparedOutputConsumerKind::HostLocalSymlinkMountsV1,
                            message: format!("{err:#}"),
                            requested_exposures: Vec::new(),
                            consumer_request_path: None,
                            consumer_result_path: None,
                            consumer_launch_request_path: None,
                            consumer_transport_request_path: None,
                        })
                    })?;
                    exposures.push(exposure.clone());
                }
            }
        }
        Ok(PreparedOutputConsumerResult {
            kind: PreparedOutputConsumerKind::HostLocalSymlinkMountsV1,
            exposures,
            requested_exposures: Vec::new(),
            consumer_request_path: None,
            consumer_result_path: None,
            consumer_launch_request_path: None,
            consumer_transport_request_path: None,
        })
    }
}

impl PreparedOutputConsumer for RemoteExposureRequestPreparedOutputConsumer {
    fn kind(&self) -> PreparedOutputConsumerKind {
        PreparedOutputConsumerKind::RemoteExposureRequestV1
    }

    fn consume(
        &self,
        materialization: &PreparedOutputMaterialization<'_>,
        handoff: &PreparedOutputHandoff,
        run_dir: &Path,
        log_paths: &[PathBuf],
        _invocation: PreparedOutputInvocationConfig<'_>,
    ) -> Result<PreparedOutputConsumerResult, Box<PreparedOutputConsumerFailure>> {
        let request_path =
            write_prepared_output_remote_exposure_request(materialization, handoff, run_dir)
                .map_err(|err| {
                    Box::new(PreparedOutputConsumerFailure {
                        kind: PreparedOutputConsumerKind::RemoteExposureRequestV1,
                        message: format!("{err:#}"),
                        requested_exposures: handoff.exposures.clone(),
                        consumer_request_path: None,
                        consumer_result_path: None,
                        consumer_launch_request_path: None,
                        consumer_transport_request_path: None,
                    })
                })?;
        append_log_line_many(
            log_paths,
            &format!(
                "[pikaci] prepared output consumer={} wrote remote exposure request {}",
                prepared_output_consumer_kind_text(
                    PreparedOutputConsumerKind::RemoteExposureRequestV1
                ),
                request_path.display()
            ),
        )
        .map_err(|err| {
            Box::new(PreparedOutputConsumerFailure {
                kind: PreparedOutputConsumerKind::RemoteExposureRequestV1,
                message: format!("{err:#}"),
                requested_exposures: handoff.exposures.clone(),
                consumer_request_path: Some(request_path.display().to_string()),
                consumer_result_path: None,
                consumer_launch_request_path: None,
                consumer_transport_request_path: None,
            })
        })?;
        Ok(PreparedOutputConsumerResult {
            kind: PreparedOutputConsumerKind::RemoteExposureRequestV1,
            exposures: Vec::new(),
            requested_exposures: handoff.exposures.clone(),
            consumer_request_path: Some(request_path.display().to_string()),
            consumer_result_path: None,
            consumer_launch_request_path: None,
            consumer_transport_request_path: None,
        })
    }
}

impl PreparedOutputConsumer for FulfillRequestCliPreparedOutputConsumer {
    fn kind(&self) -> PreparedOutputConsumerKind {
        PreparedOutputConsumerKind::FulfillRequestCliV1
    }

    fn consume(
        &self,
        materialization: &PreparedOutputMaterialization<'_>,
        handoff: &PreparedOutputHandoff,
        run_dir: &Path,
        log_paths: &[PathBuf],
        invocation: PreparedOutputInvocationConfig<'_>,
    ) -> Result<PreparedOutputConsumerResult, Box<PreparedOutputConsumerFailure>> {
        let request_path =
            write_prepared_output_remote_exposure_request(materialization, handoff, run_dir)
                .map_err(|err| {
                    Box::new(PreparedOutputConsumerFailure {
                        kind: PreparedOutputConsumerKind::FulfillRequestCliV1,
                        message: format!("{err:#}"),
                        requested_exposures: handoff.exposures.clone(),
                        consumer_request_path: None,
                        consumer_result_path: None,
                        consumer_launch_request_path: None,
                        consumer_transport_request_path: None,
                    })
                })?;
        let results_dir = run_dir.join("prepared-output-results");
        fs::create_dir_all(&results_dir)
            .with_context(|| format!("create {}", results_dir.display()))
            .map_err(|err| {
                Box::new(PreparedOutputConsumerFailure {
                    kind: PreparedOutputConsumerKind::FulfillRequestCliV1,
                    message: format!("{err:#}"),
                    requested_exposures: handoff.exposures.clone(),
                    consumer_request_path: Some(request_path.display().to_string()),
                    consumer_result_path: None,
                    consumer_launch_request_path: None,
                    consumer_transport_request_path: None,
                })
            })?;
        let result_path = results_dir.join(format!("{}.json", materialization.node_id));
        let launch_request_path = (invocation.invocation_mode
            == Some(PreparedOutputInvocationMode::ExternalWrapperCommandV1))
        .then(|| {
            run_dir
                .join("prepared-output-launch-requests")
                .join(format!("{}.json", materialization.node_id))
        });
        let transport_request_path = matches!(
            invocation.launcher_transport_mode,
            Some(PreparedOutputLauncherTransportMode::CommandTransportV1)
                | Some(PreparedOutputLauncherTransportMode::SshLauncherTransportV1)
        )
        .then(|| {
            run_dir
                .join("prepared-output-transport-requests")
                .join(format!("{}.json", materialization.node_id))
        });
        append_log_line_many(
            log_paths,
            &format!(
                "[pikaci] prepared output consumer={} wrote fulfillment request {}",
                prepared_output_consumer_kind_text(PreparedOutputConsumerKind::FulfillRequestCliV1),
                request_path.display()
            ),
        )
        .map_err(|err| {
            Box::new(PreparedOutputConsumerFailure {
                kind: PreparedOutputConsumerKind::FulfillRequestCliV1,
                message: format!("{err:#}"),
                requested_exposures: handoff.exposures.clone(),
                consumer_request_path: Some(request_path.display().to_string()),
                consumer_result_path: Some(result_path.display().to_string()),
                consumer_launch_request_path: launch_request_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                consumer_transport_request_path: transport_request_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
            })
        })?;
        let invocation_mode = invocation
            .invocation_mode
            .unwrap_or(PreparedOutputInvocationMode::DirectHelperExecV1);
        let fulfillment_result = fulfill_prepared_output_request_via_subprocess(
            invocation_mode,
            PreparedOutputSubprocessConfig {
                run_dir: Some(run_dir),
                launcher_program: invocation.launcher_program,
                launch_request_path: launch_request_path.as_deref(),
                launcher_transport_mode: invocation.launcher_transport_mode,
                launcher_transport_program: invocation.launcher_transport_program,
                transport_request_path: transport_request_path.as_deref(),
                launcher_transport_host: invocation.launcher_transport_host,
                launcher_transport_remote_launcher_program: invocation
                    .launcher_transport_remote_launcher_program,
                launcher_transport_remote_helper_program: invocation
                    .launcher_transport_remote_helper_program,
                launcher_transport_remote_work_dir: invocation.launcher_transport_remote_work_dir,
            },
            &request_path,
            &result_path,
            log_paths,
        )
        .map_err(|err| {
            Box::new(PreparedOutputConsumerFailure {
                kind: PreparedOutputConsumerKind::FulfillRequestCliV1,
                message: format!("{err:#}"),
                requested_exposures: handoff.exposures.clone(),
                consumer_request_path: Some(request_path.display().to_string()),
                consumer_result_path: Some(result_path.display().to_string()),
                consumer_launch_request_path: launch_request_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                consumer_transport_request_path: transport_request_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
            })
        })?;
        if fulfillment_result.node_id.as_deref() != Some(materialization.node_id) {
            return Err(Box::new(PreparedOutputConsumerFailure {
                kind: PreparedOutputConsumerKind::FulfillRequestCliV1,
                message: format!(
                    "prepared-output fulfillment helper reported node_id={:?} but expected {}",
                    fulfillment_result.node_id, materialization.node_id
                ),
                requested_exposures: handoff.exposures.clone(),
                consumer_request_path: Some(request_path.display().to_string()),
                consumer_result_path: Some(result_path.display().to_string()),
                consumer_launch_request_path: launch_request_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                consumer_transport_request_path: transport_request_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
            }));
        }
        if fulfillment_result.output_name.as_deref() != Some(materialization.output_name) {
            return Err(Box::new(PreparedOutputConsumerFailure {
                kind: PreparedOutputConsumerKind::FulfillRequestCliV1,
                message: format!(
                    "prepared-output fulfillment helper reported output_name={:?} but expected {}",
                    fulfillment_result.output_name, materialization.output_name
                ),
                requested_exposures: handoff.exposures.clone(),
                consumer_request_path: Some(request_path.display().to_string()),
                consumer_result_path: Some(result_path.display().to_string()),
                consumer_launch_request_path: launch_request_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                consumer_transport_request_path: transport_request_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
            }));
        }
        let expected_realized_path = materialization.realized_path.display().to_string();
        if fulfillment_result.realized_path.as_deref() != Some(expected_realized_path.as_str()) {
            return Err(Box::new(PreparedOutputConsumerFailure {
                kind: PreparedOutputConsumerKind::FulfillRequestCliV1,
                message: format!(
                    "prepared-output fulfillment helper reported realized_path={:?} but expected {}",
                    fulfillment_result.realized_path, expected_realized_path
                ),
                requested_exposures: handoff.exposures.clone(),
                consumer_request_path: Some(request_path.display().to_string()),
                consumer_result_path: Some(result_path.display().to_string()),
                consumer_launch_request_path: launch_request_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                consumer_transport_request_path: transport_request_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
            }));
        }
        append_log_line_many(
            log_paths,
            &format!(
                "[pikaci] prepared output consumer={} fulfilled request {} via {} exposure(s)",
                prepared_output_consumer_kind_text(PreparedOutputConsumerKind::FulfillRequestCliV1),
                request_path.display(),
                fulfillment_result.fulfilled_exposures_count
            ),
        )
        .map_err(|err| {
            Box::new(PreparedOutputConsumerFailure {
                kind: PreparedOutputConsumerKind::FulfillRequestCliV1,
                message: format!("{err:#}"),
                requested_exposures: handoff.exposures.clone(),
                consumer_request_path: Some(request_path.display().to_string()),
                consumer_result_path: Some(result_path.display().to_string()),
                consumer_launch_request_path: launch_request_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                consumer_transport_request_path: transport_request_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
            })
        })?;
        Ok(PreparedOutputConsumerResult {
            kind: PreparedOutputConsumerKind::FulfillRequestCliV1,
            exposures: fulfillment_result.fulfilled_exposures,
            requested_exposures: handoff.exposures.clone(),
            consumer_request_path: Some(request_path.display().to_string()),
            consumer_result_path: Some(result_path.display().to_string()),
            consumer_launch_request_path: launch_request_path
                .as_ref()
                .map(|path| path.display().to_string()),
            consumer_transport_request_path: transport_request_path
                .as_ref()
                .map(|path| path.display().to_string()),
        })
    }
}

fn consume_prepared_output_handoff(
    consumer: &dyn PreparedOutputConsumer,
    materialization: &PreparedOutputMaterialization<'_>,
    handoff: &PreparedOutputHandoff,
    run_dir: &Path,
    log_paths: &[PathBuf],
    invocation: PreparedOutputInvocationConfig<'_>,
) -> Result<PreparedOutputConsumerResult, Box<PreparedOutputConsumerFailure>> {
    let result = consumer.consume(materialization, handoff, run_dir, log_paths, invocation)?;
    debug_assert_eq!(result.kind, consumer.kind());
    Ok(result)
}

fn record_failed_prepared_output_handoff(
    prepared_outputs_path: &Path,
    materialization: &PreparedOutputMaterialization<'_>,
    handoff: &PreparedOutputHandoff,
    failure: &PreparedOutputConsumerFailure,
) -> anyhow::Result<()> {
    upsert_prepared_output_record(
        prepared_outputs_path,
        RealizedPreparedOutputRecord {
            node_id: materialization.node_id.to_string(),
            installable: materialization.installable.to_string(),
            output_name: materialization.output_name.to_string(),
            protocol: handoff.protocol,
            consumer: failure.kind,
            realized_path: materialization.realized_path.display().to_string(),
            consumer_request_path: failure.consumer_request_path.clone(),
            consumer_result_path: failure.consumer_result_path.clone(),
            consumer_launch_request_path: failure.consumer_launch_request_path.clone(),
            consumer_transport_request_path: failure.consumer_transport_request_path.clone(),
            exposures: Vec::new(),
            requested_exposures: failure.requested_exposures.clone(),
        },
    )
}

fn prepared_output_consumer_kind_text(kind: PreparedOutputConsumerKind) -> &'static str {
    match kind {
        PreparedOutputConsumerKind::HostLocalSymlinkMountsV1 => "host_local_symlink_mounts_v1",
        PreparedOutputConsumerKind::RemoteExposureRequestV1 => "remote_exposure_request_v1",
        PreparedOutputConsumerKind::FulfillRequestCliV1 => "fulfill_request_cli_v1",
    }
}

fn realize_nix_build_output(
    installable: &str,
    output_name: &str,
    log_paths: &[PathBuf],
) -> anyhow::Result<PathBuf> {
    append_log_line_many(
        log_paths,
        &format!(
            "[pikaci] prepare {output_name}: nix build --accept-flake-config --no-link --print-out-paths {installable}"
        ),
    )?;
    let output = Command::new("nix")
        .arg("build")
        .arg("--accept-flake-config")
        .arg("--no-link")
        .arg("--print-out-paths")
        .arg(installable)
        .output()
        .with_context(|| format!("run staged prepare `{output_name}`"))?;
    append_command_output_many(log_paths, &output.stdout, &output.stderr)?;
    if !output.status.success() {
        return Err(anyhow!(
            "staged prepare `{output_name}` failed with {:?}; see {}",
            output.status.code(),
            log_paths
                .first()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<no log>".to_string())
        ));
    }

    let stdout = String::from_utf8(output.stdout).context("decode nix build output")?;
    let path = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| anyhow!("nix build for `{output_name}` did not return an output path"))?;
    Ok(PathBuf::from(path.trim()))
}

fn upsert_prepared_output_record(
    prepared_outputs_path: &Path,
    record: RealizedPreparedOutputRecord,
) -> anyhow::Result<()> {
    let existing = if prepared_outputs_path.exists() {
        let bytes = fs::read(prepared_outputs_path)
            .with_context(|| format!("read {}", prepared_outputs_path.display()))?;
        serde_json::from_slice::<PreparedOutputsRecord>(&bytes)
            .with_context(|| format!("decode {}", prepared_outputs_path.display()))?
    } else {
        PreparedOutputsRecord {
            schema_version: 1,
            outputs: Vec::new(),
        }
    };
    let mut updated = existing;
    if let Some(existing) = updated
        .outputs
        .iter_mut()
        .find(|output| output.node_id == record.node_id)
    {
        *existing = record;
    } else {
        updated.outputs.push(record);
    }
    write_json(prepared_outputs_path.to_path_buf(), &updated)
}

fn parse_bool_env_flag(name: &str) -> anyhow::Result<bool> {
    match std::env::var(name).ok().as_deref() {
        None => Ok(false),
        Some("1" | "true" | "TRUE" | "yes" | "YES") => Ok(true),
        Some("0" | "false" | "FALSE" | "no" | "NO") => Ok(false),
        Some(value) => Err(anyhow!(
            "unsupported {name} value `{value}`; expected 1/true/yes or 0/false/no"
        )),
    }
}

fn resolve_run_prepared_output_consumer_kind(
    jobs: &[JobSpec],
    metadata: &RunMetadata,
    configured_kind: PreparedOutputConsumerKind,
) -> anyhow::Result<(PreparedOutputConsumerKind, Option<&'static str>)> {
    resolve_run_prepared_output_consumer_kind_for_mode(
        metadata.prepared_output_mode.as_deref(),
        parse_bool_env_flag(STAGED_LINUX_RUST_SUBPROCESS_MODE_ENV)?,
        jobs,
        metadata,
        configured_kind,
    )
}

fn resolve_run_prepared_output_consumer_kind_for_mode(
    recorded_mode: Option<&str>,
    subprocess_mode_enabled: bool,
    jobs: &[JobSpec],
    metadata: &RunMetadata,
    configured_kind: PreparedOutputConsumerKind,
) -> anyhow::Result<(PreparedOutputConsumerKind, Option<&'static str>)> {
    let requested_mode = match recorded_mode {
        Some(STAGED_LINUX_RUST_SUBPROCESS_MODE_NAME) => {
            Some(STAGED_LINUX_RUST_SUBPROCESS_MODE_NAME)
        }
        Some(value) => {
            return Err(anyhow!(
                "unsupported recorded prepared_output_mode `{value}`"
            ));
        }
        None if subprocess_mode_enabled => Some(STAGED_LINUX_RUST_SUBPROCESS_MODE_NAME),
        None => None,
    };
    if requested_mode.is_none() {
        return Ok((configured_kind, None));
    }
    if configured_kind != PreparedOutputConsumerKind::HostLocalSymlinkMountsV1 {
        return Err(anyhow!(
            "{STAGED_LINUX_RUST_SUBPROCESS_MODE_ENV} cannot be combined with PIKACI_PREPARED_OUTPUT_CONSUMER"
        ));
    }
    if metadata.target_id.as_deref() != Some("pre-merge-pika-rust") {
        return Err(anyhow!(
            "{STAGED_LINUX_RUST_SUBPROCESS_MODE_ENV} only supports target `pre-merge-pika-rust`"
        ));
    }
    if jobs.is_empty()
        || jobs
            .iter()
            .any(|job| job.staged_linux_rust_lane().is_none())
    {
        return Err(anyhow!(
            "{STAGED_LINUX_RUST_SUBPROCESS_MODE_ENV} requires staged Linux Rust jobs"
        ));
    }
    Ok((
        PreparedOutputConsumerKind::FulfillRequestCliV1,
        requested_mode,
    ))
}

fn configured_prepared_output_consumer_kind() -> anyhow::Result<PreparedOutputConsumerKind> {
    match std::env::var("PIKACI_PREPARED_OUTPUT_CONSUMER")
        .ok()
        .as_deref()
    {
        None => Ok(PreparedOutputConsumerKind::HostLocalSymlinkMountsV1),
        Some("remote_request_v1") => Ok(PreparedOutputConsumerKind::RemoteExposureRequestV1),
        Some("fulfill_request_cli_v1") => Ok(PreparedOutputConsumerKind::FulfillRequestCliV1),
        Some(value) => Err(anyhow!(
            "unsupported PIKACI_PREPARED_OUTPUT_CONSUMER `{value}`; expected `remote_request_v1` or `fulfill_request_cli_v1`"
        )),
    }
}

fn resolve_run_prepared_output_invocation_mode(
    consumer_kind: PreparedOutputConsumerKind,
    recorded_mode: Option<PreparedOutputInvocationMode>,
) -> anyhow::Result<Option<PreparedOutputInvocationMode>> {
    if consumer_kind != PreparedOutputConsumerKind::FulfillRequestCliV1 {
        return Ok(None);
    }
    if let Some(recorded_mode) = recorded_mode {
        return Ok(Some(recorded_mode));
    }
    Ok(Some(configured_prepared_output_invocation_mode()?))
}

fn selected_prepared_output_consumer(
    kind: PreparedOutputConsumerKind,
) -> Box<dyn PreparedOutputConsumer> {
    match kind {
        PreparedOutputConsumerKind::HostLocalSymlinkMountsV1 => {
            Box::new(HostLocalSymlinkPreparedOutputConsumer)
        }
        PreparedOutputConsumerKind::RemoteExposureRequestV1 => {
            Box::new(RemoteExposureRequestPreparedOutputConsumer)
        }
        PreparedOutputConsumerKind::FulfillRequestCliV1 => {
            Box::new(FulfillRequestCliPreparedOutputConsumer)
        }
    }
}

fn validate_prepared_output_consumer_for_jobs(
    kind: PreparedOutputConsumerKind,
    jobs: &[PlannedJob],
) -> anyhow::Result<()> {
    if kind == PreparedOutputConsumerKind::RemoteExposureRequestV1
        && jobs.iter().any(|planned_job| {
            planned_job.job.runner_kind() == RunnerKind::VfkitLocal
                && planned_job.job.staged_linux_rust_lane().is_some()
        })
    {
        return Err(anyhow!(
            "PIKACI_PREPARED_OUTPUT_CONSUMER=remote_request_v1 is prototype-only; staged Linux Rust vfkit jobs still require local prepared-output mounts"
        ));
    }
    Ok(())
}

fn run_prepare_nodes(
    run_dir: &Path,
    prepares: &[PlannedPrepare],
    prepared_outputs_path: &Path,
    consumer_kind: PreparedOutputConsumerKind,
    invocation: PreparedOutputInvocationConfig<'_>,
) -> Result<Vec<String>, PrepareFailure> {
    let prepared_output_consumer = selected_prepared_output_consumer(consumer_kind);
    let mut completed = HashSet::new();
    let mut completed_order = Vec::new();
    let mut pending: Vec<_> = prepares.iter().collect();

    while !pending.is_empty() {
        let Some(next_ready_pos) = pending
            .iter()
            .position(|prepare| prepare.depends_on.iter().all(|dep| completed.contains(dep)))
        else {
            return Err(PrepareFailure {
                node_id: "prepare-scheduler".to_string(),
                message: "no ready prepare nodes; unresolved dependencies in run plan".to_string(),
            });
        };
        let prepare = pending.remove(next_ready_pos);
        match &prepare.action {
            PrepareAction::NixBuildOutput {
                installable,
                output_name,
                handoff,
                mount_paths,
                log_paths,
            } => {
                let output_path = realize_nix_build_output(installable, output_name, log_paths)
                    .map_err(|err| PrepareFailure {
                        node_id: prepare.node_id.clone(),
                        message: format!("{err:#}"),
                    })?;
                append_log_line_many(
                    log_paths,
                    &format!(
                        "[pikaci] staged Linux Rust output ready: {} -> {}",
                        output_name,
                        output_path.display()
                    ),
                )
                .map_err(|err| PrepareFailure {
                    node_id: prepare.node_id.clone(),
                    message: format!("{err:#}"),
                })?;
                if let Some(handoff) = handoff {
                    let materialization = PreparedOutputMaterialization {
                        node_id: &prepare.node_id,
                        installable,
                        output_name,
                        protocol: handoff.protocol,
                        realized_path: &output_path,
                    };
                    let consumer_result = consume_prepared_output_handoff(
                        prepared_output_consumer.as_ref(),
                        &materialization,
                        handoff,
                        run_dir,
                        log_paths,
                        invocation,
                    )
                    .map_err(|err| {
                        let mut message = err.message.clone();
                        if (err.consumer_request_path.is_some()
                            || err.consumer_result_path.is_some())
                            && let Err(record_err) = record_failed_prepared_output_handoff(
                                prepared_outputs_path,
                                &materialization,
                                handoff,
                                &err,
                            )
                        {
                            message = format!(
                                "{message}; also failed to persist prepared-output failure state: {record_err:#}"
                            );
                        }
                        PrepareFailure {
                            node_id: prepare.node_id.clone(),
                            message,
                        }
                    })?;
                    upsert_prepared_output_record(
                        prepared_outputs_path,
                        RealizedPreparedOutputRecord {
                            node_id: prepare.node_id.clone(),
                            installable: installable.clone(),
                            output_name: (*output_name).to_string(),
                            protocol: handoff.protocol,
                            consumer: consumer_result.kind,
                            realized_path: output_path.display().to_string(),
                            consumer_request_path: consumer_result.consumer_request_path,
                            consumer_result_path: consumer_result.consumer_result_path,
                            consumer_launch_request_path: consumer_result
                                .consumer_launch_request_path,
                            consumer_transport_request_path: consumer_result
                                .consumer_transport_request_path,
                            exposures: consumer_result.exposures,
                            requested_exposures: consumer_result.requested_exposures,
                        },
                    )
                    .map_err(|err| PrepareFailure {
                        node_id: prepare.node_id.clone(),
                        message: format!("{err:#}"),
                    })?;
                    append_log_line_many(
                        log_paths,
                        &format!(
                            "[pikaci] prepared output handoff recorded via {}: {} ({})",
                            prepared_output_consumer_kind_text(consumer_result.kind),
                            output_name,
                            prepared_outputs_path.display()
                        ),
                    )
                    .map_err(|err| PrepareFailure {
                        node_id: prepare.node_id.clone(),
                        message: format!("{err:#}"),
                    })?;
                } else {
                    for mount_path in mount_paths {
                        repoint_prepare_mount(mount_path, &output_path).map_err(|err| {
                            PrepareFailure {
                                node_id: prepare.node_id.clone(),
                                message: format!("{err:#}"),
                            }
                        })?;
                    }
                }
            }
            PrepareAction::VfkitRunner {
                installable,
                runner_link,
                log_paths,
            } => {
                let log_path = log_paths.first().ok_or_else(|| PrepareFailure {
                    node_id: prepare.node_id.clone(),
                    message: "missing vfkit runner prepare log path".to_string(),
                })?;
                prepare_vfkit_runner_link(installable, runner_link, log_path).map_err(|err| {
                    PrepareFailure {
                        node_id: prepare.node_id.clone(),
                        message: format!("{err:#}"),
                    }
                })?;
            }
        }
        completed.insert(prepare.node_id.clone());
        completed_order.push(prepare.node_id.clone());
    }

    Ok(completed_order)
}

fn mark_prepare_failure(
    run_record: &mut RunRecord,
    plan: &RunPlan,
    failure: &PrepareFailure,
) -> anyhow::Result<()> {
    for planned_job in &plan.jobs {
        let directly_blocked = planned_job
            .depends_on
            .iter()
            .any(|dep| dep == &failure.node_id);
        let (status_message, record) = if directly_blocked {
            let message = format!(
                "prepare node `{}` failed before execute: {}",
                failure.node_id, failure.message
            );
            (
                message.clone(),
                failed_job_record(
                    &planned_job.job,
                    &planned_job.execute_node_id,
                    &planned_job.ctx,
                    message,
                    None,
                ),
            )
        } else {
            let message = format!(
                "not run because prepare phase stopped after `{}` failed: {}",
                failure.node_id, failure.message
            );
            (
                message.clone(),
                skipped_job_record(
                    &planned_job.job,
                    &planned_job.execute_node_id,
                    &planned_job.ctx,
                    message,
                ),
            )
        };
        append_log_line(
            &planned_job.ctx.host_log_path,
            &format!("[pikaci] {status_message}"),
        )?;
        write_job_record(&planned_job.ctx.job_dir, &record)?;
        upsert_run_job_record(run_record, record);
    }
    Ok(())
}

fn upsert_run_job_record(run_record: &mut RunRecord, record: JobRecord) {
    if let Some(existing) = run_record.jobs.iter_mut().find(|job| job.id == record.id) {
        *existing = record;
    } else {
        run_record.jobs.push(record);
    }
}

fn max_parallel_execute_jobs(jobs: &[PlannedJob]) -> usize {
    let configured_cap = std::env::var("PIKACI_MAX_CONCURRENT_EXECUTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(2);
    parallel_execute_cap_for_jobs(jobs, configured_cap)
}

fn parallel_execute_cap_for_jobs(jobs: &[PlannedJob], configured_cap: usize) -> usize {
    if !jobs
        .iter()
        .all(|planned_job| planned_job.job.supports_parallel_execute())
    {
        return 1;
    }
    configured_cap
}

fn ready_execute_job_positions(
    pending: &[usize],
    jobs: &[PlannedJob],
    completed_node_ids: &HashSet<String>,
) -> Vec<usize> {
    pending
        .iter()
        .enumerate()
        .filter_map(|(position, index)| {
            jobs[*index]
                .depends_on
                .iter()
                .all(|dep| completed_node_ids.contains(dep))
                .then_some(position)
        })
        .collect()
}

fn ensure_log_file(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let _ = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    Ok(())
}

fn append_log_line(path: &Path, line: &str) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    writeln!(file, "{line}").with_context(|| format!("write {}", path.display()))
}

fn append_log_line_many(paths: &[PathBuf], line: &str) -> anyhow::Result<()> {
    for path in paths {
        append_log_line(path, line)?;
    }
    Ok(())
}

fn append_command_output(log_path: &Path, stdout: &[u8], stderr: &[u8]) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("open {}", log_path.display()))?;
    file.write_all(stdout)
        .with_context(|| format!("write stdout to {}", log_path.display()))?;
    file.write_all(stderr)
        .with_context(|| format!("write stderr to {}", log_path.display()))
}

fn append_command_output_many(
    log_paths: &[PathBuf],
    stdout: &[u8],
    stderr: &[u8],
) -> anyhow::Result<()> {
    for log_path in log_paths {
        append_command_output(log_path, stdout, stderr)?;
    }
    Ok(())
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
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use super::{
        FulfillRequestCliPreparedOutputConsumer, HostLocalSymlinkPreparedOutputConsumer,
        PREPARED_OUTPUT_FULFILLMENT_HELPER_BASENAME, PREPARED_OUTPUT_FULFILLMENT_INVOCATION_ENV,
        PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_BINARY_ENV,
        PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_TRANSPORT_ENV,
        PREPARED_OUTPUT_FULFILLMENT_SSH_BINARY_ENV, PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV,
        PREPARED_OUTPUT_FULFILLMENT_SSH_NIX_BINARY_ENV,
        PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_HELPER_BINARY_ENV,
        PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_LAUNCHER_BINARY_ENV,
        PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_WORK_DIR_ENV,
        PREPARED_OUTPUT_FULFILLMENT_TRANSPORT_BINARY_ENV,
        PREPARED_OUTPUT_FULFILLMENT_WRAPPER_BINARY_ENV, PrepareFailure,
        PreparedOutputConsumerFailure, PreparedOutputInvocationConfig,
        PreparedOutputMaterialization, PreparedRun, RemoteExposureRequestPreparedOutputConsumer,
        RunMetadata, STAGED_LINUX_RUST_SUBPROCESS_MODE_ENV, STAGED_LINUX_RUST_SUBPROCESS_MODE_NAME,
        SnapshotSource, build_run_plan, configured_prepared_output_consumer_kind,
        configured_prepared_output_invocation_mode,
        configured_prepared_output_launcher_transport_mode, consume_prepared_output_handoff,
        fulfill_prepared_output_request, fulfill_prepared_output_request_result, gc_runs,
        load_prepared_output_fulfillment_launch_request, load_prepared_output_fulfillment_result,
        load_prepared_output_fulfillment_transport_request, mark_prepare_failure,
        parallel_execute_cap_for_jobs, parse_bool_env_flag,
        prepared_output_fulfillment_launcher_program, ready_execute_job_positions,
        record_failed_prepared_output_handoff, resolve_prepared_output_fulfillment_program,
        resolve_run_prepared_output_consumer_kind_for_mode,
        resolve_run_prepared_output_invocation_mode,
        resolve_run_prepared_output_invocation_wrapper_program,
        resolve_run_prepared_output_launcher_transport_host,
        resolve_run_prepared_output_launcher_transport_mode,
        resolve_run_prepared_output_launcher_transport_program,
        resolve_run_prepared_output_launcher_transport_remote_helper_program,
        resolve_run_prepared_output_launcher_transport_remote_launcher_program,
        resolve_run_prepared_output_launcher_transport_remote_work_dir,
        selected_prepared_output_consumer, upsert_prepared_output_record,
        validate_prepared_output_consumer_for_jobs, write_json,
        write_prepared_output_fulfillment_result, write_run_plan_record,
    };
    use crate::model::{
        ExecuteNode, GuestCommand, JobSpec, PlanExecutorKind, PlanNodeRecord, PlanScope,
        PrepareNode, PreparedOutputConsumerKind, PreparedOutputExposure,
        PreparedOutputExposureAccess, PreparedOutputExposureKind, PreparedOutputFulfillmentResult,
        PreparedOutputFulfillmentStatus, PreparedOutputFulfillmentTransportPathContract,
        PreparedOutputHandoff, PreparedOutputHandoffProtocol, PreparedOutputInvocationMode,
        PreparedOutputLauncherTransportMode, PreparedOutputRemoteExposureRequest,
        PreparedOutputsRecord, RealizedPreparedOutputRecord, RunPlanRecord, RunRecord, RunStatus,
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
    fn build_run_plan_shares_prepare_nodes_across_staged_linux_rust_jobs() {
        let root = std::env::temp_dir().join(format!("pikaci-plan-test-{}", uuid::Uuid::new_v4()));
        let prepared = sample_prepared_run(&root);
        let metadata = RunMetadata {
            target_id: Some("pre-merge-pika-rust".to_string()),
            target_description: Some("Run pika rust lane".to_string()),
            ..RunMetadata::default()
        };
        let jobs = vec![
            JobSpec {
                id: "pika-core-lib-app-flows-tests",
                description: "Run pika_core lib tests and app_flows integration tests in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ShellCommand {
                    command: "cargo test -p pika_core --lib --test app_flows -- --nocapture",
                },
            },
            JobSpec {
                id: "pika-core-messaging-e2e-tests",
                description: "Run pika_core messaging and group profile integration tests in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ShellCommand {
                    command: "cargo test -p pika_core --test e2e_messaging --test e2e_group_profiles -- --nocapture",
                },
            },
        ];

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
        assert_eq!(plan.record.nodes.len(), 6);
        match &plan.record.nodes[0] {
            PlanNodeRecord::Prepare {
                id,
                executor,
                prepare,
                ..
            } => {
                assert_eq!(id, "prepare-pika-core-linux-rust-workspace-deps");
                assert_eq!(*executor, PlanExecutorKind::HostLocal);
                match prepare {
                    PrepareNode::NixBuild {
                        installable,
                        output_name,
                        handoff,
                    } => {
                        assert!(
                            installable
                                .contains("runs/run-1/snapshot#ci.aarch64-linux.workspaceDeps")
                        );
                        assert_eq!(output_name, "ci.aarch64-linux.workspaceDeps");
                        let handoff = handoff.as_ref().expect("workspace deps handoff");
                        assert_eq!(
                            handoff.protocol,
                            PreparedOutputHandoffProtocol::NixStorePathV1
                        );
                        assert_eq!(handoff.exposures.len(), 2);
                        assert!(handoff.exposures.iter().any(|exposure| {
                            exposure.path.ends_with(
                                "jobs/pika-core-lib-app-flows-tests/staged-linux-rust/workspace-deps"
                            )
                        }));
                        assert!(handoff.exposures.iter().any(|exposure| {
                            exposure.path.ends_with(
                                "jobs/pika-core-messaging-e2e-tests/staged-linux-rust/workspace-deps"
                            )
                        }));
                    }
                }
            }
            other => panic!("expected prepare node, got {other:?}"),
        }

        match &plan.record.nodes[1] {
            PlanNodeRecord::Prepare {
                id,
                executor,
                depends_on,
                prepare,
                ..
            } => {
                assert_eq!(id, "prepare-pika-core-linux-rust-workspace-build");
                assert_eq!(*executor, PlanExecutorKind::HostLocal);
                assert_eq!(
                    depends_on,
                    &vec!["prepare-pika-core-linux-rust-workspace-deps".to_string()]
                );
                match prepare {
                    PrepareNode::NixBuild {
                        installable,
                        output_name,
                        handoff,
                    } => {
                        assert!(
                            installable
                                .contains("runs/run-1/snapshot#ci.aarch64-linux.workspaceBuild")
                        );
                        assert_eq!(output_name, "ci.aarch64-linux.workspaceBuild");
                        let handoff = handoff.as_ref().expect("workspace build handoff");
                        assert_eq!(
                            handoff.protocol,
                            PreparedOutputHandoffProtocol::NixStorePathV1
                        );
                        assert_eq!(handoff.exposures.len(), 2);
                        assert!(handoff.exposures.iter().all(|exposure| {
                            exposure.kind == PreparedOutputExposureKind::HostSymlinkMount
                                && exposure.access == PreparedOutputExposureAccess::ReadOnly
                        }));
                    }
                }
            }
            other => panic!("expected staged build prepare node, got {other:?}"),
        }

        match &plan.record.nodes[2] {
            PlanNodeRecord::Prepare {
                id,
                executor,
                prepare,
                ..
            } => {
                assert_eq!(id, "prepare-pika-core-lib-app-flows-tests-runner");
                assert_eq!(*executor, PlanExecutorKind::HostLocal);
                match prepare {
                    PrepareNode::NixBuild {
                        installable,
                        output_name,
                        handoff,
                    } => {
                        assert!(installable.contains(
                            "jobs/pika-core-lib-app-flows-tests/vm/flake#nixosConfigurations.pikaci-wave1.config.microvm.declaredRunner"
                        ));
                        assert_eq!(
                            output_name,
                            "nixosConfigurations.pikaci-wave1.config.microvm.declaredRunner"
                        );
                        assert!(handoff.is_none());
                    }
                }
            }
            other => panic!("expected runner prepare node, got {other:?}"),
        }

        match &plan.record.nodes[3] {
            PlanNodeRecord::Prepare {
                id,
                executor,
                prepare,
                ..
            } => {
                assert_eq!(id, "prepare-pika-core-messaging-e2e-tests-runner");
                assert_eq!(*executor, PlanExecutorKind::HostLocal);
                match prepare {
                    PrepareNode::NixBuild {
                        installable,
                        output_name,
                        handoff,
                    } => {
                        assert!(installable.contains(
                            "jobs/pika-core-messaging-e2e-tests/vm/flake#nixosConfigurations.pikaci-wave1.config.microvm.declaredRunner"
                        ));
                        assert_eq!(
                            output_name,
                            "nixosConfigurations.pikaci-wave1.config.microvm.declaredRunner"
                        );
                        assert!(handoff.is_none());
                    }
                }
            }
            other => panic!("expected runner prepare node, got {other:?}"),
        }

        match &plan.record.nodes[4] {
            PlanNodeRecord::Execute {
                id,
                executor,
                depends_on,
                execute,
                ..
            } => {
                assert_eq!(id, "execute-pika-core-lib-app-flows-tests");
                assert_eq!(*executor, PlanExecutorKind::VfkitLocal);
                assert_eq!(
                    depends_on,
                    &vec![
                        "prepare-pika-core-linux-rust-workspace-deps".to_string(),
                        "prepare-pika-core-linux-rust-workspace-build".to_string(),
                        "prepare-pika-core-lib-app-flows-tests-runner".to_string()
                    ]
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
                            "/staged/linux-rust/workspace-build/bin/run-pika-core-lib-app-flows-tests"
                        );
                        assert!(!run_as_root);
                        assert_eq!(*timeout_secs, 1800);
                        assert!(!writable_workspace);
                    }
                }
            }
            other => panic!("expected execute node, got {other:?}"),
        }

        match &plan.record.nodes[5] {
            PlanNodeRecord::Execute {
                id,
                depends_on,
                execute,
                ..
            } => {
                assert_eq!(id, "execute-pika-core-messaging-e2e-tests");
                assert_eq!(
                    depends_on,
                    &vec![
                        "prepare-pika-core-linux-rust-workspace-deps".to_string(),
                        "prepare-pika-core-linux-rust-workspace-build".to_string(),
                        "prepare-pika-core-messaging-e2e-tests-runner".to_string()
                    ]
                );
                match execute {
                    ExecuteNode::VmCommand { command, .. } => {
                        assert_eq!(
                            command,
                            "/staged/linux-rust/workspace-build/bin/run-pika-core-messaging-e2e-tests"
                        );
                    }
                }
            }
            other => panic!("expected execute node, got {other:?}"),
        }

        assert_eq!(plan.jobs.len(), 2);
        assert!(
            prepared
                .run_dir
                .join("jobs/pika-core-lib-app-flows-tests/vm/flake/flake.nix")
                .exists()
        );
        assert!(
            prepared
                .run_dir
                .join("jobs/pika-core-messaging-e2e-tests/vm/flake/flake.nix")
                .exists()
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ready_execute_positions_require_shared_and_job_specific_prepare_nodes() {
        let root = std::env::temp_dir().join(format!("pikaci-ready-test-{}", uuid::Uuid::new_v4()));
        let prepared = sample_prepared_run(&root);
        let snapshot = sample_snapshot_source(&prepared);
        let jobs = vec![
            JobSpec {
                id: "pika-core-lib-app-flows-tests",
                description: "Run pika_core lib tests and app_flows integration tests in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ShellCommand {
                    command: "cargo test -p pika_core --lib --test app_flows -- --nocapture",
                },
            },
            JobSpec {
                id: "pika-core-messaging-e2e-tests",
                description: "Run pika_core messaging and group profile integration tests in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ShellCommand {
                    command: "cargo test -p pika_core --test e2e_messaging --test e2e_group_profiles -- --nocapture",
                },
            },
        ];

        let plan = build_run_plan(&jobs, &prepared, &snapshot, &RunMetadata::default())
            .expect("build plan");
        let pending = vec![0usize, 1usize];
        let mut completed = std::collections::HashSet::from([
            "prepare-pika-core-linux-rust-workspace-deps".to_string(),
            "prepare-pika-core-linux-rust-workspace-build".to_string(),
        ]);

        assert_eq!(
            ready_execute_job_positions(&pending, &plan.jobs, &completed),
            Vec::<usize>::new()
        );

        completed.insert("prepare-pika-core-lib-app-flows-tests-runner".to_string());
        assert_eq!(
            ready_execute_job_positions(&pending, &plan.jobs, &completed),
            vec![0]
        );

        completed.insert("prepare-pika-core-messaging-e2e-tests-runner".to_string());
        assert_eq!(
            ready_execute_job_positions(&pending, &plan.jobs, &completed),
            vec![0, 1]
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

    #[test]
    fn upsert_prepared_output_record_persists_machine_readable_handoff_state() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-prepared-output-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("create root");
        let path = root.join("prepared-outputs.json");

        upsert_prepared_output_record(
            &path,
            RealizedPreparedOutputRecord {
                node_id: "prepare-pika-core-linux-rust-workspace-build".to_string(),
                installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild".to_string(),
                output_name: "ci.aarch64-linux.workspaceBuild".to_string(),
                protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
                consumer: PreparedOutputConsumerKind::HostLocalSymlinkMountsV1,
                realized_path: "/nix/store/workspace-build".to_string(),
                consumer_request_path: None,
                consumer_result_path: None,
                consumer_launch_request_path: None,
                consumer_transport_request_path: None,
                exposures: vec![PreparedOutputExposure {
                    kind: PreparedOutputExposureKind::HostSymlinkMount,
                    path: "/tmp/run/jobs/pika-core-lib-app-flows-tests/staged-linux-rust/workspace-build"
                        .to_string(),
                    access: PreparedOutputExposureAccess::ReadOnly,
                }],
                requested_exposures: Vec::new(),
            },
        )
        .expect("write prepared output record");

        let bytes = fs::read(&path).expect("read prepared outputs");
        let decoded: PreparedOutputsRecord =
            serde_json::from_slice(&bytes).expect("decode prepared outputs");

        assert_eq!(decoded.schema_version, 1);
        assert_eq!(decoded.outputs.len(), 1);
        assert_eq!(
            decoded.outputs[0].protocol,
            PreparedOutputHandoffProtocol::NixStorePathV1
        );
        assert_eq!(
            decoded.outputs[0].consumer,
            PreparedOutputConsumerKind::HostLocalSymlinkMountsV1
        );
        assert_eq!(
            decoded.outputs[0].exposures[0].path,
            "/tmp/run/jobs/pika-core-lib-app-flows-tests/staged-linux-rust/workspace-build"
        );
        assert!(decoded.outputs[0].requested_exposures.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn record_failed_prepared_output_handoff_persists_helper_result_paths() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-prepared-output-failure-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("create root");
        let prepared_outputs_path = root.join("prepared-outputs.json");
        let realized_path = first_test_nix_store_path();
        let materialization = PreparedOutputMaterialization {
            node_id: "prepare-pika-core-linux-rust-workspace-build",
            installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild",
            output_name: "ci.aarch64-linux.workspaceBuild",
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            realized_path: &realized_path,
        };
        let handoff = PreparedOutputHandoff {
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            exposures: vec![PreparedOutputExposure {
                kind: PreparedOutputExposureKind::HostSymlinkMount,
                path: root
                    .join("jobs/job-1/staged-linux-rust/workspace-build")
                    .display()
                    .to_string(),
                access: PreparedOutputExposureAccess::ReadOnly,
            }],
        };
        let failure = PreparedOutputConsumerFailure {
            kind: PreparedOutputConsumerKind::FulfillRequestCliV1,
            message: "helper failed".to_string(),
            requested_exposures: handoff.exposures.clone(),
            consumer_request_path: Some(
                root.join("prepared-output-requests/request.json")
                    .display()
                    .to_string(),
            ),
            consumer_result_path: Some(
                root.join("prepared-output-results/result.json")
                    .display()
                    .to_string(),
            ),
            consumer_launch_request_path: Some(
                root.join("prepared-output-launch-requests/request.json")
                    .display()
                    .to_string(),
            ),
            consumer_transport_request_path: None,
        };

        record_failed_prepared_output_handoff(
            &prepared_outputs_path,
            &materialization,
            &handoff,
            &failure,
        )
        .expect("record failed handoff");

        let bytes = fs::read(&prepared_outputs_path).expect("read prepared outputs");
        let decoded: PreparedOutputsRecord =
            serde_json::from_slice(&bytes).expect("decode prepared outputs");
        assert_eq!(decoded.outputs.len(), 1);
        assert_eq!(
            decoded.outputs[0].consumer,
            PreparedOutputConsumerKind::FulfillRequestCliV1
        );
        assert!(decoded.outputs[0].exposures.is_empty());
        assert_eq!(decoded.outputs[0].requested_exposures, handoff.exposures);
        assert!(
            decoded.outputs[0]
                .consumer_request_path
                .as_deref()
                .unwrap_or_default()
                .ends_with("prepared-output-requests/request.json")
        );
        assert!(
            decoded.outputs[0]
                .consumer_result_path
                .as_deref()
                .unwrap_or_default()
                .ends_with("prepared-output-results/result.json")
        );
        assert!(
            decoded.outputs[0]
                .consumer_launch_request_path
                .as_deref()
                .unwrap_or_default()
                .ends_with("prepared-output-launch-requests/request.json")
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn local_prepared_output_consumer_exposes_symlink_mounts() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-prepared-output-consumer-test-{}",
            uuid::Uuid::new_v4()
        ));
        let realized_path = root.join("nix-store-output");
        let mount_path = root.join("jobs/job-1/staged-linux-rust/workspace-build");
        let log_path = root.join("job.log");
        fs::create_dir_all(&realized_path).expect("create realized path");
        fs::write(realized_path.join("marker"), "ok").expect("write marker");

        let handoff = PreparedOutputHandoff {
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            exposures: vec![PreparedOutputExposure {
                kind: PreparedOutputExposureKind::HostSymlinkMount,
                path: mount_path.display().to_string(),
                access: PreparedOutputExposureAccess::ReadOnly,
            }],
        };
        let consumer = HostLocalSymlinkPreparedOutputConsumer;
        let materialization = PreparedOutputMaterialization {
            node_id: "prepare-pika-core-linux-rust-workspace-build",
            installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild",
            output_name: "ci.aarch64-linux.workspaceBuild",
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            realized_path: &realized_path,
        };

        let result = consume_prepared_output_handoff(
            &consumer,
            &materialization,
            &handoff,
            &root,
            std::slice::from_ref(&log_path),
            PreparedOutputInvocationConfig::default(),
        )
        .expect("consume handoff");

        assert_eq!(
            result.kind,
            PreparedOutputConsumerKind::HostLocalSymlinkMountsV1
        );
        assert_eq!(result.exposures, handoff.exposures);
        assert!(result.requested_exposures.is_empty());
        assert!(result.consumer_request_path.is_none());
        assert_eq!(
            fs::read_link(&mount_path).expect("read symlink"),
            realized_path
        );
        assert!(
            fs::read_to_string(&log_path)
                .expect("read log")
                .contains("prepared output consumer=host_local_symlink_mounts_v1 exposed")
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn remote_prepared_output_consumer_writes_machine_readable_request() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-remote-prepared-output-consumer-test-{}",
            uuid::Uuid::new_v4()
        ));
        let realized_path = root.join("nix-store-output");
        let mount_path = root.join("jobs/job-1/staged-linux-rust/workspace-build");
        let log_path = root.join("job.log");
        fs::create_dir_all(&realized_path).expect("create realized path");
        let handoff = PreparedOutputHandoff {
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            exposures: vec![PreparedOutputExposure {
                kind: PreparedOutputExposureKind::HostSymlinkMount,
                path: mount_path.display().to_string(),
                access: PreparedOutputExposureAccess::ReadOnly,
            }],
        };
        let materialization = PreparedOutputMaterialization {
            node_id: "prepare-pika-core-linux-rust-workspace-build",
            installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild",
            output_name: "ci.aarch64-linux.workspaceBuild",
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            realized_path: &realized_path,
        };
        let consumer = RemoteExposureRequestPreparedOutputConsumer;

        let result = consume_prepared_output_handoff(
            &consumer,
            &materialization,
            &handoff,
            &root,
            std::slice::from_ref(&log_path),
            PreparedOutputInvocationConfig::default(),
        )
        .expect("consume handoff");

        assert_eq!(
            result.kind,
            PreparedOutputConsumerKind::RemoteExposureRequestV1
        );
        assert!(result.exposures.is_empty());
        assert_eq!(result.requested_exposures, handoff.exposures);
        let request_path = result
            .consumer_request_path
            .as_deref()
            .expect("remote request path");
        assert!(request_path.ends_with(
            "prepared-output-requests/prepare-pika-core-linux-rust-workspace-build.json"
        ));
        assert!(!mount_path.exists());
        let request_body = fs::read_to_string(request_path).expect("read request");
        assert!(request_body.contains("\"schema_version\": 1"));
        assert!(request_body.contains("\"output_name\": \"ci.aarch64-linux.workspaceBuild\""));
        assert!(request_body.contains("\"requested_exposures\""));
        assert!(request_body.contains(&mount_path.display().to_string()));
        assert!(fs::read_to_string(&log_path).expect("read log").contains(
            "prepared output consumer=remote_exposure_request_v1 wrote remote exposure request"
        ));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn selected_prepared_output_consumer_defaults_local_and_can_switch_to_remote_request() {
        let _guard = EnvVarGuard::set("PIKACI_PREPARED_OUTPUT_CONSUMER", None);
        let kind = configured_prepared_output_consumer_kind().expect("default consumer kind");
        assert_eq!(
            selected_prepared_output_consumer(kind).kind(),
            PreparedOutputConsumerKind::HostLocalSymlinkMountsV1
        );

        let _guard = EnvVarGuard::set("PIKACI_PREPARED_OUTPUT_CONSUMER", Some("remote_request_v1"));
        let kind = configured_prepared_output_consumer_kind().expect("remote consumer kind");
        assert_eq!(
            selected_prepared_output_consumer(kind).kind(),
            PreparedOutputConsumerKind::RemoteExposureRequestV1
        );

        let _guard = EnvVarGuard::set(
            "PIKACI_PREPARED_OUTPUT_CONSUMER",
            Some("fulfill_request_cli_v1"),
        );
        let kind = configured_prepared_output_consumer_kind().expect("cli fulfill consumer kind");
        assert_eq!(
            selected_prepared_output_consumer(kind).kind(),
            PreparedOutputConsumerKind::FulfillRequestCliV1
        );
    }

    #[test]
    fn configured_prepared_output_consumer_kind_rejects_invalid_values() {
        let _guard = EnvVarGuard::set("PIKACI_PREPARED_OUTPUT_CONSUMER", Some("typo"));
        let err = configured_prepared_output_consumer_kind().expect_err("invalid consumer");
        assert!(
            err.to_string()
                .contains("unsupported PIKACI_PREPARED_OUTPUT_CONSUMER `typo`")
        );
    }

    #[test]
    fn configured_prepared_output_invocation_mode_defaults_and_switches_to_wrapper() {
        let _guard = EnvVarGuard::set(PREPARED_OUTPUT_FULFILLMENT_INVOCATION_ENV, None);
        assert_eq!(
            configured_prepared_output_invocation_mode().expect("default invocation mode"),
            PreparedOutputInvocationMode::DirectHelperExecV1
        );

        let _guard = EnvVarGuard::set(
            PREPARED_OUTPUT_FULFILLMENT_INVOCATION_ENV,
            Some("external_wrapper_command_v1"),
        );
        assert_eq!(
            configured_prepared_output_invocation_mode().expect("wrapper invocation mode"),
            PreparedOutputInvocationMode::ExternalWrapperCommandV1
        );
    }

    #[test]
    fn configured_prepared_output_launcher_transport_mode_defaults_and_switches_modes() {
        let _guard = EnvVarGuard::set(PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_TRANSPORT_ENV, None);
        assert_eq!(
            configured_prepared_output_launcher_transport_mode()
                .expect("default launcher transport mode"),
            PreparedOutputLauncherTransportMode::DirectLauncherExecV1
        );

        let _guard = EnvVarGuard::set(
            PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_TRANSPORT_ENV,
            Some("command_transport_v1"),
        );
        assert_eq!(
            configured_prepared_output_launcher_transport_mode().expect("command transport mode"),
            PreparedOutputLauncherTransportMode::CommandTransportV1
        );

        let _guard = EnvVarGuard::set(
            PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_TRANSPORT_ENV,
            Some("ssh_launcher_transport_v1"),
        );
        assert_eq!(
            configured_prepared_output_launcher_transport_mode().expect("ssh transport mode"),
            PreparedOutputLauncherTransportMode::SshLauncherTransportV1
        );
    }

    #[test]
    fn resolve_run_prepared_output_invocation_mode_uses_recorded_mode_for_reruns() {
        assert_eq!(
            resolve_run_prepared_output_invocation_mode(
                PreparedOutputConsumerKind::FulfillRequestCliV1,
                Some(PreparedOutputInvocationMode::ExternalWrapperCommandV1)
            )
            .expect("resolve recorded invocation mode"),
            Some(PreparedOutputInvocationMode::ExternalWrapperCommandV1)
        );
        assert_eq!(
            resolve_run_prepared_output_invocation_mode(
                PreparedOutputConsumerKind::HostLocalSymlinkMountsV1,
                Some(PreparedOutputInvocationMode::ExternalWrapperCommandV1)
            )
            .expect("ignore invocation mode for non-helper consumer"),
            None
        );
    }

    #[test]
    fn resolve_run_prepared_output_launcher_transport_mode_uses_recorded_mode_for_reruns() {
        assert_eq!(
            resolve_run_prepared_output_launcher_transport_mode(
                Some(PreparedOutputInvocationMode::ExternalWrapperCommandV1),
                Some(PreparedOutputLauncherTransportMode::CommandTransportV1),
            )
            .expect("resolve recorded launcher transport mode"),
            Some(PreparedOutputLauncherTransportMode::CommandTransportV1)
        );
        assert_eq!(
            resolve_run_prepared_output_launcher_transport_mode(
                Some(PreparedOutputInvocationMode::DirectHelperExecV1),
                Some(PreparedOutputLauncherTransportMode::CommandTransportV1),
            )
            .expect("ignore transport mode for direct helper exec"),
            None
        );
    }

    #[test]
    fn prepared_output_fulfillment_launcher_program_requires_explicit_binary() {
        let _launcher_guard =
            EnvVarGuard::set(PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_BINARY_ENV, None);
        let _wrapper_guard = EnvVarGuard::set(PREPARED_OUTPUT_FULFILLMENT_WRAPPER_BINARY_ENV, None);
        let err = prepared_output_fulfillment_launcher_program()
            .expect_err("launcher binary should be required");
        assert!(err.to_string().contains(
            "PIKACI_PREPARED_OUTPUT_FULFILL_INVOCATION=external_wrapper_command_v1 requires PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY"
        ));
    }

    #[test]
    fn resolve_run_prepared_output_launcher_transport_program_uses_recorded_path_for_reruns() {
        assert_eq!(
            resolve_run_prepared_output_launcher_transport_program(
                Some(PreparedOutputLauncherTransportMode::CommandTransportV1),
                Some("/tmp/bin/fake-ssh"),
            )
            .expect("use recorded transport path"),
            Some("/tmp/bin/fake-ssh".to_string())
        );
        assert_eq!(
            resolve_run_prepared_output_launcher_transport_program(
                Some(PreparedOutputLauncherTransportMode::DirectLauncherExecV1),
                Some("/tmp/bin/fake-ssh"),
            )
            .expect("ignore transport path in direct launcher mode"),
            None
        );
        assert_eq!(
            resolve_run_prepared_output_launcher_transport_program(
                Some(PreparedOutputLauncherTransportMode::SshLauncherTransportV1),
                Some("/usr/bin/ssh"),
            )
            .expect("use recorded transport path for ssh mode"),
            Some("/usr/bin/ssh".to_string())
        );
    }

    #[test]
    fn resolve_run_prepared_output_launcher_transport_program_requires_explicit_binary() {
        let _guard = EnvVarGuard::set(PREPARED_OUTPUT_FULFILLMENT_TRANSPORT_BINARY_ENV, None);
        let err = resolve_run_prepared_output_launcher_transport_program(
            Some(PreparedOutputLauncherTransportMode::CommandTransportV1),
            None,
        )
        .expect_err("transport binary should be required");
        assert!(err.to_string().contains(
            "PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_TRANSPORT=command_transport_v1 requires PIKACI_PREPARED_OUTPUT_FULFILL_TRANSPORT_BINARY"
        ));
    }

    #[test]
    fn resolve_run_prepared_output_launcher_transport_ssh_details_use_recorded_or_env() {
        let _ssh_guard = EnvVarGuard::set(
            PREPARED_OUTPUT_FULFILLMENT_SSH_BINARY_ENV,
            Some("/usr/bin/ssh"),
        );
        let _host_guard =
            EnvVarGuard::set(PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV, Some("pika-build"));
        let _launcher_guard = EnvVarGuard::set(
            PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_LAUNCHER_BINARY_ENV,
            Some("/opt/pikaci/bin/pikaci-launch-fulfill-prepared-output"),
        );
        let _helper_guard = EnvVarGuard::set(
            PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_HELPER_BINARY_ENV,
            Some("/opt/pikaci/bin/pikaci-fulfill-prepared-output"),
        );
        let _work_dir_guard = EnvVarGuard::set(
            PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_WORK_DIR_ENV,
            Some("/var/tmp/pikaci-remote"),
        );

        assert_eq!(
            resolve_run_prepared_output_launcher_transport_program(
                Some(PreparedOutputLauncherTransportMode::SshLauncherTransportV1),
                None,
            )
            .expect("ssh transport program"),
            Some("/usr/bin/ssh".to_string())
        );
        assert_eq!(
            resolve_run_prepared_output_launcher_transport_host(
                Some(PreparedOutputLauncherTransportMode::SshLauncherTransportV1),
                None,
            )
            .expect("ssh transport host"),
            Some("pika-build".to_string())
        );
        assert_eq!(
            resolve_run_prepared_output_launcher_transport_remote_launcher_program(
                Some(PreparedOutputLauncherTransportMode::SshLauncherTransportV1),
                None,
            )
            .expect("ssh remote launcher"),
            Some("/opt/pikaci/bin/pikaci-launch-fulfill-prepared-output".to_string())
        );
        assert_eq!(
            resolve_run_prepared_output_launcher_transport_remote_helper_program(
                Some(PreparedOutputLauncherTransportMode::SshLauncherTransportV1),
                None,
            )
            .expect("ssh remote helper"),
            Some("/opt/pikaci/bin/pikaci-fulfill-prepared-output".to_string())
        );
        assert_eq!(
            resolve_run_prepared_output_launcher_transport_remote_work_dir(
                Some(PreparedOutputLauncherTransportMode::SshLauncherTransportV1),
                None,
            )
            .expect("ssh remote work dir"),
            Some("/var/tmp/pikaci-remote".to_string())
        );
    }

    #[test]
    fn parse_bool_env_flag_accepts_expected_values() {
        let _guard = EnvVarGuard::set(STAGED_LINUX_RUST_SUBPROCESS_MODE_ENV, Some("true"));
        assert!(
            parse_bool_env_flag(STAGED_LINUX_RUST_SUBPROCESS_MODE_ENV).expect("parse true flag")
        );

        let _guard = EnvVarGuard::set(STAGED_LINUX_RUST_SUBPROCESS_MODE_ENV, Some("0"));
        assert!(
            !parse_bool_env_flag(STAGED_LINUX_RUST_SUBPROCESS_MODE_ENV).expect("parse false flag")
        );
    }

    #[test]
    fn resolve_run_prepared_output_consumer_kind_enables_staged_subprocess_mode() {
        let jobs = vec![JobSpec {
            id: "pika-core-lib-app-flows-tests",
            description: "Run pika_core lib tests and app_flows integration tests in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pika_core --lib --test app_flows -- --nocapture",
            },
        }];
        let metadata = RunMetadata {
            target_id: Some("pre-merge-pika-rust".to_string()),
            ..RunMetadata::default()
        };

        let (kind, mode) = resolve_run_prepared_output_consumer_kind_for_mode(
            None,
            true,
            &jobs,
            &metadata,
            PreparedOutputConsumerKind::HostLocalSymlinkMountsV1,
        )
        .expect("resolve staged subprocess mode");

        assert_eq!(kind, PreparedOutputConsumerKind::FulfillRequestCliV1);
        assert_eq!(mode, Some(STAGED_LINUX_RUST_SUBPROCESS_MODE_NAME));
    }

    #[test]
    fn resolve_run_prepared_output_consumer_kind_rejects_non_pre_merge_target() {
        let jobs = vec![JobSpec {
            id: "pika-core-lib-app-flows-tests",
            description: "Run pika_core lib tests and app_flows integration tests in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pika_core --lib --test app_flows -- --nocapture",
            },
        }];
        let metadata = RunMetadata {
            target_id: Some("beachhead".to_string()),
            ..RunMetadata::default()
        };

        let err = resolve_run_prepared_output_consumer_kind_for_mode(
            None,
            true,
            &jobs,
            &metadata,
            PreparedOutputConsumerKind::HostLocalSymlinkMountsV1,
        )
        .expect_err("reject non pre-merge target");

        assert!(
            err.to_string()
                .contains("only supports target `pre-merge-pika-rust`")
        );
    }

    #[test]
    fn resolve_run_prepared_output_consumer_kind_rejects_low_level_consumer_conflict() {
        let jobs = vec![JobSpec {
            id: "pika-core-lib-app-flows-tests",
            description: "Run pika_core lib tests and app_flows integration tests in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pika_core --lib --test app_flows -- --nocapture",
            },
        }];
        let metadata = RunMetadata {
            target_id: Some("pre-merge-pika-rust".to_string()),
            ..RunMetadata::default()
        };

        let err = resolve_run_prepared_output_consumer_kind_for_mode(
            None,
            true,
            &jobs,
            &metadata,
            PreparedOutputConsumerKind::FulfillRequestCliV1,
        )
        .expect_err("reject low-level consumer conflict");

        assert!(
            err.to_string()
                .contains("cannot be combined with PIKACI_PREPARED_OUTPUT_CONSUMER")
        );
    }

    #[test]
    fn resolve_run_prepared_output_consumer_kind_uses_recorded_mode_for_reruns() {
        let jobs = vec![JobSpec {
            id: "pika-core-lib-app-flows-tests",
            description: "Run pika_core lib tests and app_flows integration tests in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pika_core --lib --test app_flows -- --nocapture",
            },
        }];
        let metadata = RunMetadata {
            target_id: Some("pre-merge-pika-rust".to_string()),
            prepared_output_mode: Some(STAGED_LINUX_RUST_SUBPROCESS_MODE_NAME.to_string()),
            ..RunMetadata::default()
        };

        let (kind, mode) = resolve_run_prepared_output_consumer_kind_for_mode(
            metadata.prepared_output_mode.as_deref(),
            false,
            &jobs,
            &metadata,
            PreparedOutputConsumerKind::HostLocalSymlinkMountsV1,
        )
        .expect("resolve recorded rerun mode");

        assert_eq!(kind, PreparedOutputConsumerKind::FulfillRequestCliV1);
        assert_eq!(mode, Some(STAGED_LINUX_RUST_SUBPROCESS_MODE_NAME));
    }

    #[test]
    fn resolve_run_prepared_output_invocation_wrapper_program_uses_recorded_path_for_reruns() {
        assert_eq!(
            resolve_run_prepared_output_invocation_wrapper_program(
                Some(PreparedOutputInvocationMode::ExternalWrapperCommandV1),
                Some("/tmp/bin/wrapper"),
            )
            .expect("use recorded wrapper path"),
            Some("/tmp/bin/wrapper".to_string())
        );
        assert_eq!(
            resolve_run_prepared_output_invocation_wrapper_program(
                Some(PreparedOutputInvocationMode::DirectHelperExecV1),
                Some("/tmp/bin/wrapper"),
            )
            .expect("ignore wrapper path in direct mode"),
            None
        );
    }

    #[test]
    fn resolve_prepared_output_fulfillment_program_prefers_helper_sibling_for_pikaci_binary() {
        let resolved =
            resolve_prepared_output_fulfillment_program(None, PathBuf::from("/tmp/bin/pikaci"))
                .expect("resolve helper sibling");
        assert_eq!(
            resolved,
            PathBuf::from("/tmp/bin").join(PREPARED_OUTPUT_FULFILLMENT_HELPER_BASENAME)
        );
    }

    #[test]
    fn resolve_prepared_output_fulfillment_program_accepts_helper_binary_name() {
        let resolved = resolve_prepared_output_fulfillment_program(
            None,
            PathBuf::from("/tmp/bin").join(PREPARED_OUTPUT_FULFILLMENT_HELPER_BASENAME),
        )
        .expect("resolve helper binary");
        assert_eq!(
            resolved,
            PathBuf::from("/tmp/bin").join(PREPARED_OUTPUT_FULFILLMENT_HELPER_BASENAME)
        );
    }

    #[test]
    fn resolve_prepared_output_fulfillment_program_rejects_non_helper_host_binary() {
        let err = resolve_prepared_output_fulfillment_program(
            None,
            PathBuf::from("/tmp/bin/embedding-runner"),
        )
        .expect_err("reject embedding binary");
        assert!(err.to_string().contains(
            "requires PIKACI_PREPARED_OUTPUT_FULFILL_BINARY when the host executable is neither `pikaci` nor `pikaci-fulfill-prepared-output`"
        ));
    }

    #[test]
    fn remote_request_consumer_is_rejected_for_real_staged_vfkit_jobs() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-remote-consumer-guard-test-{}",
            uuid::Uuid::new_v4()
        ));
        let prepared = sample_prepared_run(&root);
        let snapshot = sample_snapshot_source(&prepared);
        let jobs = vec![JobSpec {
            id: "pika-core-lib-app-flows-tests",
            description: "Run pika_core lib tests and app_flows integration tests in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pika_core --lib --test app_flows -- --nocapture",
            },
        }];

        let plan = build_run_plan(&jobs, &prepared, &snapshot, &RunMetadata::default())
            .expect("build plan");
        let err = validate_prepared_output_consumer_for_jobs(
            PreparedOutputConsumerKind::RemoteExposureRequestV1,
            &plan.jobs,
        )
        .expect_err("remote request guard");
        assert!(
            err.to_string()
                .contains("PIKACI_PREPARED_OUTPUT_CONSUMER=remote_request_v1 is prototype-only")
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fulfill_request_cli_consumer_uses_subprocess_boundary() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-fulfill-request-cli-consumer-test-{}",
            uuid::Uuid::new_v4()
        ));
        let realized_path = first_test_nix_store_path();
        let mount_path = root.join("jobs/job-1/staged-linux-rust/workspace-build");
        let log_path = root.join("job.log");
        let helper_path = root.join("fulfill-helper.sh");
        fs::create_dir_all(root.join("jobs/job-1/staged-linux-rust")).expect("create mount root");
        fs::write(
            &helper_path,
            r#"#!/bin/sh
set -eu
if [ "$#" -ne 3 ] || [ "$1" != "--result-path" ]; then
  echo "unexpected args: $*" >&2
  exit 17
fi
result_path="$2"
request_path="$3"
realized_path=$(sed -n 's/.*"realized_path": "\(.*\)",/\1/p' "$request_path" | head -n1)
mount_path=$(sed -n 's/.*"path": "\(.*\)",/\1/p' "$request_path" | head -n1)
mkdir -p "$(dirname "$mount_path")"
ln -sfn "$realized_path" "$mount_path"
mkdir -p "$(dirname "$result_path")"
cat >"$result_path" <<EOF
{"schema_version":1,"request_path":"$request_path","node_id":"prepare-pika-core-linux-rust-workspace-build","output_name":"ci.aarch64-linux.workspaceBuild","realized_path":"$realized_path","status":"succeeded","fulfilled_exposures_count":1,"fulfilled_exposures":[{"kind":"host_symlink_mount","path":"$mount_path","access":"read_only"}],"error":null}
EOF
printf '{"schema_version":1,"request_path":"%s","node_id":"prepare-pika-core-linux-rust-workspace-build","output_name":"ci.aarch64-linux.workspaceBuild","realized_path":"%s","status":"succeeded","fulfilled_exposures_count":1,"fulfilled_exposures":[{"kind":"host_symlink_mount","path":"%s","access":"read_only"}],"error":null}\n' "$request_path" "$realized_path" "$mount_path"
"#,
        )
        .expect("write helper");
        let mut permissions = fs::metadata(&helper_path)
            .expect("helper metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&helper_path, permissions).expect("set helper executable");
        let handoff = PreparedOutputHandoff {
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            exposures: vec![PreparedOutputExposure {
                kind: PreparedOutputExposureKind::HostSymlinkMount,
                path: mount_path.display().to_string(),
                access: PreparedOutputExposureAccess::ReadOnly,
            }],
        };
        let materialization = PreparedOutputMaterialization {
            node_id: "prepare-pika-core-linux-rust-workspace-build",
            installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild",
            output_name: "ci.aarch64-linux.workspaceBuild",
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            realized_path: &realized_path,
        };
        let _helper_guard = EnvVarGuard::set(
            "PIKACI_PREPARED_OUTPUT_FULFILL_BINARY",
            Some(helper_path.to_str().expect("helper path utf8")),
        );
        let consumer = FulfillRequestCliPreparedOutputConsumer;

        let result = consume_prepared_output_handoff(
            &consumer,
            &materialization,
            &handoff,
            &root,
            std::slice::from_ref(&log_path),
            PreparedOutputInvocationConfig {
                invocation_mode: Some(PreparedOutputInvocationMode::DirectHelperExecV1),
                ..PreparedOutputInvocationConfig::default()
            },
        )
        .expect("consume handoff");

        assert_eq!(result.kind, PreparedOutputConsumerKind::FulfillRequestCliV1);
        assert_eq!(result.exposures, handoff.exposures);
        assert_eq!(result.requested_exposures, handoff.exposures);
        let request_path = result
            .consumer_request_path
            .as_deref()
            .expect("fulfillment request path");
        let result_path = result
            .consumer_result_path
            .as_deref()
            .expect("fulfillment result path");
        assert!(request_path.ends_with(
            "prepared-output-requests/prepare-pika-core-linux-rust-workspace-build.json"
        ));
        assert!(result_path.ends_with(
            "prepared-output-results/prepare-pika-core-linux-rust-workspace-build.json"
        ));
        assert_eq!(
            fs::read_link(&mount_path).expect("read symlink"),
            realized_path
        );
        let helper_result =
            load_prepared_output_fulfillment_result(Path::new(result_path)).expect("load result");
        assert_eq!(
            helper_result.status,
            PreparedOutputFulfillmentStatus::Succeeded
        );
        assert_eq!(helper_result.fulfilled_exposures_count, 1);
        assert_eq!(result.exposures, helper_result.fulfilled_exposures);
        let log_body = fs::read_to_string(&log_path).expect("read log");
        assert!(
            log_body.contains("prepared output fulfillment invocation_mode=direct_helper_exec_v1")
        );
        assert!(log_body.contains("prepared output fulfillment result status=Succeeded"));
        assert!(
            log_body.contains("prepared output consumer=fulfill_request_cli_v1 fulfilled request")
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fulfill_request_cli_consumer_can_invoke_helper_via_external_wrapper() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-fulfill-request-cli-wrapper-test-{}",
            uuid::Uuid::new_v4()
        ));
        let realized_path = first_test_nix_store_path();
        let mount_path = root.join("jobs/job-1/staged-linux-rust/workspace-build");
        let log_path = root.join("job.log");
        let helper_path = root.join("fulfill-helper.sh");
        let launcher_path = root.join("launch-fulfill.sh");
        let launcher_log_path = root.join("launcher.log");
        fs::create_dir_all(root.join("jobs/job-1/staged-linux-rust")).expect("create mount root");
        fs::write(
            &helper_path,
            r#"#!/bin/sh
set -eu
if [ "$#" -ne 3 ] || [ "$1" != "--result-path" ]; then
  echo "unexpected helper args: $*" >&2
  exit 17
fi
result_path="$2"
request_path="$3"
realized_path=$(sed -n 's/.*"realized_path": "\(.*\)",/\1/p' "$request_path" | head -n1)
mount_path=$(sed -n 's/.*"path": "\(.*\)",/\1/p' "$request_path" | head -n1)
mkdir -p "$(dirname "$mount_path")"
ln -sfn "$realized_path" "$mount_path"
mkdir -p "$(dirname "$result_path")"
cat >"$result_path" <<EOF
{"schema_version":1,"request_path":"$request_path","node_id":"prepare-pika-core-linux-rust-workspace-build","output_name":"ci.aarch64-linux.workspaceBuild","realized_path":"$realized_path","status":"succeeded","fulfilled_exposures_count":1,"fulfilled_exposures":[{"kind":"host_symlink_mount","path":"$mount_path","access":"read_only"}],"error":null}
EOF
"#,
        )
        .expect("write helper");
        fs::write(
            &launcher_path,
            format!(
                "#!/bin/sh\nset -eu\nif [ \"$#\" -ne 1 ]; then\n  echo \"unexpected launcher args: $*\" >&2\n  exit 23\nfi\nlaunch_request=\"$1\"\necho \"$launch_request\" > \"{}\"\nhelper=$(sed -n 's/.*\"helper_program\": \"\\(.*\\)\",/\\1/p' \"$launch_request\" | head -n1)\nhelper_request=$(sed -n 's/.*\"helper_request_path\": \"\\(.*\\)\",/\\1/p' \"$launch_request\" | head -n1)\nhelper_result=$(sed -n 's/.*\"helper_result_path\": \"\\(.*\\)\",/\\1/p' \"$launch_request\" | head -n1)\nexec \"$helper\" --result-path \"$helper_result\" \"$helper_request\"\n",
                launcher_log_path.display()
            ),
        )
        .expect("write launcher");
        for path in [&helper_path, &launcher_path] {
            let mut permissions = fs::metadata(path).expect("script metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).expect("set script executable");
        }
        let handoff = PreparedOutputHandoff {
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            exposures: vec![PreparedOutputExposure {
                kind: PreparedOutputExposureKind::HostSymlinkMount,
                path: mount_path.display().to_string(),
                access: PreparedOutputExposureAccess::ReadOnly,
            }],
        };
        let materialization = PreparedOutputMaterialization {
            node_id: "prepare-pika-core-linux-rust-workspace-build",
            installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild",
            output_name: "ci.aarch64-linux.workspaceBuild",
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            realized_path: &realized_path,
        };
        let _helper_guard = EnvVarGuard::set(
            "PIKACI_PREPARED_OUTPUT_FULFILL_BINARY",
            Some(helper_path.to_str().expect("helper path utf8")),
        );
        let _launcher_guard = EnvVarGuard::set(
            PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_BINARY_ENV,
            Some(launcher_path.to_str().expect("launcher path utf8")),
        );
        let consumer = FulfillRequestCliPreparedOutputConsumer;

        let result = consume_prepared_output_handoff(
            &consumer,
            &materialization,
            &handoff,
            &root,
            std::slice::from_ref(&log_path),
            PreparedOutputInvocationConfig {
                invocation_mode: Some(PreparedOutputInvocationMode::ExternalWrapperCommandV1),
                launcher_program: Some(&launcher_path),
                ..PreparedOutputInvocationConfig::default()
            },
        )
        .expect("consume handoff via wrapper");

        assert_eq!(result.kind, PreparedOutputConsumerKind::FulfillRequestCliV1);
        assert_eq!(result.exposures, handoff.exposures);
        assert_eq!(
            fs::read_link(&mount_path).expect("read symlink"),
            realized_path
        );
        let launch_request_path = result
            .consumer_launch_request_path
            .as_deref()
            .expect("launch request path");
        let launcher_request =
            load_prepared_output_fulfillment_launch_request(Path::new(launch_request_path))
                .expect("load launcher request");
        assert_eq!(
            launcher_request.helper_program,
            helper_path.display().to_string()
        );
        let launcher_log = fs::read_to_string(&launcher_log_path).expect("read launcher log");
        assert_eq!(launcher_log.trim(), launch_request_path);
        let log_body = fs::read_to_string(&log_path).expect("read log");
        assert!(
            log_body.contains(
                "prepared output fulfillment invocation_mode=external_wrapper_command_v1"
            )
        );
        assert!(log_body.contains("launch_request="));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fulfill_request_cli_consumer_can_invoke_helper_via_command_transport() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-fulfill-request-cli-transport-test-{}",
            uuid::Uuid::new_v4()
        ));
        let realized_path = first_test_nix_store_path();
        let mount_path = root.join("jobs/job-1/staged-linux-rust/workspace-build");
        let log_path = root.join("job.log");
        let helper_path = root.join("fulfill-helper.sh");
        let launcher_path = root.join("launch-fulfill.sh");
        let transport_path = root.join("transport-launcher.sh");
        let launcher_log_path = root.join("launcher.log");
        let transport_log_path = root.join("transport.log");
        fs::create_dir_all(root.join("jobs/job-1/staged-linux-rust")).expect("create mount root");
        fs::write(
            &helper_path,
            r#"#!/bin/sh
set -eu
if [ "$#" -ne 3 ] || [ "$1" != "--result-path" ]; then
  echo "unexpected helper args: $*" >&2
  exit 17
fi
result_path="$2"
request_path="$3"
realized_path=$(sed -n 's/.*"realized_path": "\(.*\)",/\1/p' "$request_path" | head -n1)
mount_path=$(sed -n 's/.*"path": "\(.*\)",/\1/p' "$request_path" | head -n1)
mkdir -p "$(dirname "$mount_path")"
ln -sfn "$realized_path" "$mount_path"
mkdir -p "$(dirname "$result_path")"
cat >"$result_path" <<EOF
{"schema_version":1,"request_path":"$request_path","node_id":"prepare-pika-core-linux-rust-workspace-build","output_name":"ci.aarch64-linux.workspaceBuild","realized_path":"$realized_path","status":"succeeded","fulfilled_exposures_count":1,"fulfilled_exposures":[{"kind":"host_symlink_mount","path":"$mount_path","access":"read_only"}],"error":null}
EOF
"#,
        )
        .expect("write helper");
        fs::write(
            &launcher_path,
            format!(
                "#!/bin/sh\nset -eu\nif [ \"$#\" -ne 1 ]; then\n  echo \"unexpected launcher args: $*\" >&2\n  exit 23\nfi\nlaunch_request=\"$1\"\necho \"$launch_request\" > \"{}\"\nhelper=$(sed -n 's/.*\"helper_program\": \"\\(.*\\)\",/\\1/p' \"$launch_request\" | head -n1)\nhelper_request=$(sed -n 's/.*\"helper_request_path\": \"\\(.*\\)\",/\\1/p' \"$launch_request\" | head -n1)\nhelper_result=$(sed -n 's/.*\"helper_result_path\": \"\\(.*\\)\",/\\1/p' \"$launch_request\" | head -n1)\nexec \"$helper\" --result-path \"$helper_result\" \"$helper_request\"\n",
                launcher_log_path.display()
            ),
        )
        .expect("write launcher");
        fs::write(
            &transport_path,
            format!(
                "#!/bin/sh\nset -eu\nif [ \"$#\" -ne 1 ]; then\n  echo \"unexpected transport args: $*\" >&2\n  exit 29\nfi\ntransport_request=\"$1\"\necho \"$transport_request\" > \"{}\"\nlauncher=$(sed -n 's/.*\"launcher_program\": \"\\(.*\\)\",/\\1/p' \"$transport_request\" | head -n1)\nlauncher_request=$(sed -n 's/.*\"launcher_request_path\": \"\\(.*\\)\",/\\1/p' \"$transport_request\" | head -n1)\nexec \"$launcher\" \"$launcher_request\"\n",
                transport_log_path.display()
            ),
        )
        .expect("write transport");
        for path in [&helper_path, &launcher_path, &transport_path] {
            let mut permissions = fs::metadata(path).expect("script metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).expect("set script executable");
        }
        let handoff = PreparedOutputHandoff {
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            exposures: vec![PreparedOutputExposure {
                kind: PreparedOutputExposureKind::HostSymlinkMount,
                path: mount_path.display().to_string(),
                access: PreparedOutputExposureAccess::ReadOnly,
            }],
        };
        let materialization = PreparedOutputMaterialization {
            node_id: "prepare-pika-core-linux-rust-workspace-build",
            installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild",
            output_name: "ci.aarch64-linux.workspaceBuild",
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            realized_path: &realized_path,
        };
        let _helper_guard = EnvVarGuard::set(
            "PIKACI_PREPARED_OUTPUT_FULFILL_BINARY",
            Some(helper_path.to_str().expect("helper path utf8")),
        );
        let _launcher_guard = EnvVarGuard::set(
            PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_BINARY_ENV,
            Some(launcher_path.to_str().expect("launcher path utf8")),
        );
        let _transport_guard = EnvVarGuard::set(
            PREPARED_OUTPUT_FULFILLMENT_TRANSPORT_BINARY_ENV,
            Some(transport_path.to_str().expect("transport path utf8")),
        );
        let consumer = FulfillRequestCliPreparedOutputConsumer;

        let result = consume_prepared_output_handoff(
            &consumer,
            &materialization,
            &handoff,
            &root,
            std::slice::from_ref(&log_path),
            PreparedOutputInvocationConfig {
                invocation_mode: Some(PreparedOutputInvocationMode::ExternalWrapperCommandV1),
                launcher_program: Some(&launcher_path),
                launcher_transport_mode: Some(
                    PreparedOutputLauncherTransportMode::CommandTransportV1,
                ),
                launcher_transport_program: Some(&transport_path),
                ..PreparedOutputInvocationConfig::default()
            },
        )
        .expect("consume handoff via command transport");

        assert_eq!(result.kind, PreparedOutputConsumerKind::FulfillRequestCliV1);
        assert_eq!(result.exposures, handoff.exposures);
        assert_eq!(
            fs::read_link(&mount_path).expect("read symlink"),
            realized_path
        );
        let launch_request_path = result
            .consumer_launch_request_path
            .as_deref()
            .expect("launch request path");
        let transport_request_path = result
            .consumer_transport_request_path
            .as_deref()
            .expect("transport request path");
        let launcher_request =
            load_prepared_output_fulfillment_launch_request(Path::new(launch_request_path))
                .expect("load launcher request");
        let transport_request =
            load_prepared_output_fulfillment_transport_request(Path::new(transport_request_path))
                .expect("load transport request");
        assert_eq!(
            launcher_request.helper_program,
            helper_path.display().to_string()
        );
        assert_eq!(
            transport_request.launcher_program,
            launcher_path.display().to_string()
        );
        assert_eq!(
            transport_request.path_contract,
            PreparedOutputFulfillmentTransportPathContract::SameHostAbsolutePathsV1
        );
        assert_eq!(transport_request.launcher_request_path, launch_request_path);
        let launcher_log = fs::read_to_string(&launcher_log_path).expect("read launcher log");
        assert_eq!(launcher_log.trim(), launch_request_path);
        let transport_log = fs::read_to_string(&transport_log_path).expect("read transport log");
        assert_eq!(transport_log.trim(), transport_request_path);
        let log_body = fs::read_to_string(&log_path).expect("read log");
        assert!(log_body.contains("launcher_transport_mode=command_transport_v1"));
        assert!(log_body.contains("transport_request="));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fulfill_request_cli_consumer_can_invoke_helper_via_ssh_launcher_transport() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-fulfill-request-cli-ssh-transport-test-{}",
            uuid::Uuid::new_v4()
        ));
        let realized_path = first_test_nix_store_path();
        let mount_path = root.join("jobs/job-1/staged-linux-rust/workspace-build");
        let remote_work_dir = root.join("remote-work");
        let remote_mount_path =
            remote_work_dir.join("jobs/job-1/staged-linux-rust/workspace-build");
        let log_path = root.join("job.log");
        let helper_path = root.join("remote-bin/pikaci-fulfill-prepared-output");
        let launcher_path = root.join("remote-bin/pikaci-launch-fulfill-prepared-output");
        let ssh_path = root.join("fake-ssh.sh");
        let nix_path = root.join("fake-nix.sh");
        let ssh_log_path = root.join("ssh.log");
        let nix_log_path = root.join("nix.log");
        let launcher_log_path = root.join("remote-launcher.log");
        fs::create_dir_all(root.join("jobs/job-1/staged-linux-rust")).expect("create mount root");
        fs::create_dir_all(root.join("remote-bin")).expect("create remote bin root");
        fs::write(
            &helper_path,
            r#"#!/bin/sh
set -eu
if [ "$#" -ne 3 ] || [ "$1" != "--result-path" ]; then
  echo "unexpected helper args: $*" >&2
  exit 17
fi
result_path="$2"
request_path="$3"
realized_path=$(sed -n 's/.*"realized_path": "\(.*\)",/\1/p' "$request_path" | head -n1)
mount_path=$(sed -n 's/.*"path": "\(.*\)",/\1/p' "$request_path" | head -n1)
mkdir -p "$(dirname "$mount_path")"
ln -sfn "$realized_path" "$mount_path"
mkdir -p "$(dirname "$result_path")"
cat >"$result_path" <<EOF
{"schema_version":1,"request_path":"$request_path","node_id":"prepare-pika-core-linux-rust-workspace-build","output_name":"ci.aarch64-linux.workspaceBuild","realized_path":"$realized_path","status":"succeeded","fulfilled_exposures_count":1,"fulfilled_exposures":[{"kind":"host_symlink_mount","path":"$mount_path","access":"read_only"}],"error":null}
EOF
"#,
        )
        .expect("write remote helper");
        fs::write(
            &launcher_path,
            format!(
                "#!/bin/sh\nset -eu\nif [ \"$#\" -ne 1 ]; then\n  echo \"unexpected launcher args: $*\" >&2\n  exit 23\nfi\nlaunch_request=\"$1\"\necho \"$launch_request\" > \"{}\"\nhelper=$(sed -n 's/.*\"helper_program\": \"\\(.*\\)\",/\\1/p' \"$launch_request\" | head -n1)\nhelper_request=$(sed -n 's/.*\"helper_request_path\": \"\\(.*\\)\",/\\1/p' \"$launch_request\" | head -n1)\nhelper_result=$(sed -n 's/.*\"helper_result_path\": \"\\(.*\\)\",/\\1/p' \"$launch_request\" | head -n1)\nexec \"$helper\" --result-path \"$helper_result\" \"$helper_request\"\n",
                launcher_log_path.display()
            ),
        )
        .expect("write remote launcher");
        fs::write(
            &ssh_path,
            format!(
                "#!/bin/sh\nset -eu\nhost=\"$1\"\nshift\necho \"$host $*\" >> \"{}\"\ncmd=\"$1\"\nshift\ncase \"$cmd\" in\n  mkdir)\n    exec mkdir \"$@\"\n    ;;\n  tee)\n    path=\"$1\"\n    mkdir -p \"$(dirname \"$path\")\"\n    cat > \"$path\"\n    ;;\n  cat)\n    exec cat \"$1\"\n    ;;\n  *)\n    exec \"$cmd\" \"$@\"\n    ;;\nesac\n",
                ssh_log_path.display()
            ),
        )
        .expect("write fake ssh");
        fs::write(
            &nix_path,
            format!(
                "#!/bin/sh\nset -eu\necho \"$*\" >> \"{}\"\nif [ \"$#\" -ne 4 ] || [ \"$1\" != \"copy\" ] || [ \"$2\" != \"--to\" ]; then\n  echo \"unexpected nix args: $*\" >&2\n  exit 31\nfi\n",
                nix_log_path.display()
            ),
        )
        .expect("write fake nix");
        for path in [&helper_path, &launcher_path, &ssh_path, &nix_path] {
            let mut permissions = fs::metadata(path).expect("script metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).expect("set script executable");
        }
        let handoff = PreparedOutputHandoff {
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            exposures: vec![PreparedOutputExposure {
                kind: PreparedOutputExposureKind::HostSymlinkMount,
                path: mount_path.display().to_string(),
                access: PreparedOutputExposureAccess::ReadOnly,
            }],
        };
        let materialization = PreparedOutputMaterialization {
            node_id: "prepare-pika-core-linux-rust-workspace-build",
            installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild",
            output_name: "ci.aarch64-linux.workspaceBuild",
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            realized_path: &realized_path,
        };
        let _helper_guard = EnvVarGuard::set(
            "PIKACI_PREPARED_OUTPUT_FULFILL_BINARY",
            Some(helper_path.to_str().expect("helper path utf8")),
        );
        let _launcher_guard = EnvVarGuard::set(
            PREPARED_OUTPUT_FULFILLMENT_LAUNCHER_BINARY_ENV,
            Some(launcher_path.to_str().expect("launcher path utf8")),
        );
        let _ssh_guard = EnvVarGuard::set(
            PREPARED_OUTPUT_FULFILLMENT_SSH_BINARY_ENV,
            Some(ssh_path.to_str().expect("ssh path utf8")),
        );
        let _nix_guard = EnvVarGuard::set(
            PREPARED_OUTPUT_FULFILLMENT_SSH_NIX_BINARY_ENV,
            Some(nix_path.to_str().expect("nix path utf8")),
        );
        let consumer = FulfillRequestCliPreparedOutputConsumer;

        let result = consume_prepared_output_handoff(
            &consumer,
            &materialization,
            &handoff,
            &root,
            std::slice::from_ref(&log_path),
            PreparedOutputInvocationConfig {
                invocation_mode: Some(PreparedOutputInvocationMode::ExternalWrapperCommandV1),
                launcher_program: Some(&launcher_path),
                launcher_transport_mode: Some(
                    PreparedOutputLauncherTransportMode::SshLauncherTransportV1,
                ),
                launcher_transport_program: Some(&ssh_path),
                launcher_transport_host: Some("pika-build"),
                launcher_transport_remote_launcher_program: Some(&launcher_path),
                launcher_transport_remote_helper_program: Some(&helper_path),
                launcher_transport_remote_work_dir: Some(&remote_work_dir),
            },
        )
        .expect("consume handoff via ssh transport");

        assert_eq!(result.kind, PreparedOutputConsumerKind::FulfillRequestCliV1);
        assert_eq!(result.exposures, handoff.exposures);
        assert_eq!(
            fs::read_link(&mount_path).expect("read local symlink"),
            realized_path
        );
        assert_eq!(
            fs::read_link(&remote_mount_path).expect("read remote symlink"),
            realized_path
        );
        let transport_request_path = result
            .consumer_transport_request_path
            .as_deref()
            .expect("transport request path");
        let transport_request =
            load_prepared_output_fulfillment_transport_request(Path::new(transport_request_path))
                .expect("load transport request");
        assert_eq!(
            transport_request.path_contract,
            PreparedOutputFulfillmentTransportPathContract::SshRemoteWorkDirTranslationV1
        );
        assert_eq!(transport_request.remote_host.as_deref(), Some("pika-build"));
        assert_eq!(
            transport_request.remote_launcher_program.as_deref(),
            Some(launcher_path.to_str().expect("launcher path utf8"))
        );
        assert_eq!(
            transport_request.remote_helper_program.as_deref(),
            Some(helper_path.to_str().expect("helper path utf8"))
        );
        assert_eq!(
            transport_request.remote_work_dir.as_deref(),
            Some(remote_work_dir.to_str().expect("remote work dir utf8"))
        );
        assert_eq!(
            transport_request
                .remote_launcher_request_path
                .as_deref()
                .expect("remote launch request path"),
            remote_work_dir
                .join("prepared-output-launch-requests/prepare-pika-core-linux-rust-workspace-build.json")
                .to_str()
                .expect("remote launch request utf8")
        );
        assert_eq!(
            transport_request
                .remote_helper_request_path
                .as_deref()
                .expect("remote helper request path"),
            remote_work_dir
                .join("prepared-output-requests/prepare-pika-core-linux-rust-workspace-build.json")
                .to_str()
                .expect("remote helper request utf8")
        );
        assert_eq!(
            transport_request
                .remote_helper_result_path
                .as_deref()
                .expect("remote helper result path"),
            remote_work_dir
                .join("prepared-output-results/prepare-pika-core-linux-rust-workspace-build.json")
                .to_str()
                .expect("remote helper result utf8")
        );
        let nix_log = fs::read_to_string(&nix_log_path).expect("read nix log");
        assert!(nix_log.contains(&format!(
            "copy --to ssh://pika-build {}",
            realized_path.display()
        )));
        let ssh_log = fs::read_to_string(&ssh_log_path).expect("read ssh log");
        assert!(ssh_log.contains("pika-build tee "));
        assert!(ssh_log.contains(launcher_path.to_str().expect("launcher path utf8")));
        let launcher_log = fs::read_to_string(&launcher_log_path).expect("read launcher log");
        assert_eq!(
            launcher_log.trim(),
            transport_request
                .remote_launcher_request_path
                .as_deref()
                .expect("remote launch request path")
        );
        let log_body = fs::read_to_string(&log_path).expect("read log");
        assert!(log_body.contains("launcher_transport_mode=ssh_launcher_transport_v1"));
        assert!(log_body.contains("transport_host=pika-build"));
        assert!(log_body.contains("remote_helper_result="));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_prepared_output_fulfillment_transport_request_defaults_path_contract_for_v1() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-load-transport-request-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("create root");
        let request_path = root.join("transport-request.json");
        fs::write(
            &request_path,
            r#"{"schema_version":1,"launcher_program":"/tmp/bin/pikaci-launch-fulfill-prepared-output","launcher_request_path":"/tmp/run/prepared-output-launch-requests/request.json","node_id":"prepare-pika-core-linux-rust-workspace-build","output_name":"ci.aarch64-linux.workspaceBuild"}"#,
        )
        .expect("write legacy transport request");

        let request = load_prepared_output_fulfillment_transport_request(&request_path)
            .expect("load legacy transport request");

        assert_eq!(request.schema_version, 1);
        assert_eq!(
            request.path_contract,
            PreparedOutputFulfillmentTransportPathContract::SameHostAbsolutePathsV1
        );
        assert_eq!(
            request.launcher_program,
            "/tmp/bin/pikaci-launch-fulfill-prepared-output"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fulfill_request_cli_consumer_rejects_failed_helper_result_with_zero_exit() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-fulfill-request-cli-failed-result-test-{}",
            uuid::Uuid::new_v4()
        ));
        let realized_path = first_test_nix_store_path();
        let mount_path = root.join("jobs/job-1/staged-linux-rust/workspace-build");
        let log_path = root.join("job.log");
        let helper_path = root.join("pikaci-fulfill-prepared-output");
        let canned_result_path = root.join("failed-result.json");
        fs::create_dir_all(root.join("jobs/job-1/staged-linux-rust")).expect("create mount root");
        write_prepared_output_fulfillment_result(
            &canned_result_path,
            &PreparedOutputFulfillmentResult {
                schema_version: 1,
                request_path: "/tmp/request.json".to_string(),
                node_id: Some("prepare-pika-core-linux-rust-workspace-build".to_string()),
                output_name: Some("ci.aarch64-linux.workspaceBuild".to_string()),
                realized_path: Some(realized_path.display().to_string()),
                status: PreparedOutputFulfillmentStatus::Failed,
                fulfilled_exposures_count: 0,
                fulfilled_exposures: Vec::new(),
                error: Some("helper reported failure".to_string()),
            },
        )
        .expect("write canned result");
        fs::write(
            &helper_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" != \"--result-path\" ]; then exit 91; fi\ncp \"{}\" \"$2\"\nexit 0\n",
                canned_result_path.display()
            ),
        )
        .expect("write helper");
        let mut permissions = fs::metadata(&helper_path)
            .expect("helper metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&helper_path, permissions).expect("set helper executable");
        let handoff = PreparedOutputHandoff {
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            exposures: vec![PreparedOutputExposure {
                kind: PreparedOutputExposureKind::HostSymlinkMount,
                path: mount_path.display().to_string(),
                access: PreparedOutputExposureAccess::ReadOnly,
            }],
        };
        let materialization = PreparedOutputMaterialization {
            node_id: "prepare-pika-core-linux-rust-workspace-build",
            installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild",
            output_name: "ci.aarch64-linux.workspaceBuild",
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            realized_path: &realized_path,
        };
        let _helper_guard = EnvVarGuard::set(
            "PIKACI_PREPARED_OUTPUT_FULFILL_BINARY",
            Some(helper_path.to_str().expect("helper path utf8")),
        );
        let consumer = FulfillRequestCliPreparedOutputConsumer;

        let err = consume_prepared_output_handoff(
            &consumer,
            &materialization,
            &handoff,
            &root,
            std::slice::from_ref(&log_path),
            PreparedOutputInvocationConfig {
                invocation_mode: Some(PreparedOutputInvocationMode::DirectHelperExecV1),
                ..PreparedOutputInvocationConfig::default()
            },
        )
        .expect_err("helper result should fail");

        assert_eq!(err.kind, PreparedOutputConsumerKind::FulfillRequestCliV1);
        assert_eq!(err.requested_exposures, handoff.exposures);
        assert!(
            err.consumer_result_path
                .as_deref()
                .unwrap_or_default()
                .ends_with(
                    "prepared-output-results/prepare-pika-core-linux-rust-workspace-build.json"
                )
        );
        assert!(err.message.contains("reported Failed"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fulfill_request_cli_consumer_rejects_mismatched_success_result_details() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-fulfill-request-cli-mismatched-success-test-{}",
            uuid::Uuid::new_v4()
        ));
        let realized_path = first_test_nix_store_path();
        let mount_path = root.join("jobs/job-1/staged-linux-rust/workspace-build");
        let log_path = root.join("job.log");
        let helper_path = root.join("pikaci-fulfill-prepared-output");
        let canned_result_path = root.join("mismatched-success.json");
        fs::create_dir_all(root.join("jobs/job-1/staged-linux-rust")).expect("create mount root");
        write_prepared_output_fulfillment_result(
            &canned_result_path,
            &PreparedOutputFulfillmentResult {
                schema_version: 1,
                request_path: "/tmp/wrong-request.json".to_string(),
                node_id: Some("prepare-pika-core-linux-rust-workspace-build".to_string()),
                output_name: Some("ci.aarch64-linux.workspaceBuild".to_string()),
                realized_path: Some(realized_path.display().to_string()),
                status: PreparedOutputFulfillmentStatus::Succeeded,
                fulfilled_exposures_count: 0,
                fulfilled_exposures: Vec::new(),
                error: None,
            },
        )
        .expect("write canned result");
        fs::write(
            &helper_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" != \"--result-path\" ]; then exit 91; fi\ncp \"{}\" \"$2\"\nexit 0\n",
                canned_result_path.display()
            ),
        )
        .expect("write helper");
        let mut permissions = fs::metadata(&helper_path)
            .expect("helper metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&helper_path, permissions).expect("set helper executable");
        let handoff = PreparedOutputHandoff {
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            exposures: vec![PreparedOutputExposure {
                kind: PreparedOutputExposureKind::HostSymlinkMount,
                path: mount_path.display().to_string(),
                access: PreparedOutputExposureAccess::ReadOnly,
            }],
        };
        let materialization = PreparedOutputMaterialization {
            node_id: "prepare-pika-core-linux-rust-workspace-build",
            installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild",
            output_name: "ci.aarch64-linux.workspaceBuild",
            protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
            realized_path: &realized_path,
        };
        let _helper_guard = EnvVarGuard::set(
            "PIKACI_PREPARED_OUTPUT_FULFILL_BINARY",
            Some(helper_path.to_str().expect("helper path utf8")),
        );
        let consumer = FulfillRequestCliPreparedOutputConsumer;

        let err = consume_prepared_output_handoff(
            &consumer,
            &materialization,
            &handoff,
            &root,
            std::slice::from_ref(&log_path),
            PreparedOutputInvocationConfig {
                invocation_mode: Some(PreparedOutputInvocationMode::DirectHelperExecV1),
                ..PreparedOutputInvocationConfig::default()
            },
        )
        .expect_err("mismatched success result should fail");

        assert!(err.message.contains("reported request_path="));
        assert!(
            err.consumer_result_path
                .as_deref()
                .unwrap_or_default()
                .ends_with(
                    "prepared-output-results/prepare-pika-core-linux-rust-workspace-build.json"
                )
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fulfill_prepared_output_request_replays_requested_mounts() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-fulfill-request-test-{}",
            uuid::Uuid::new_v4()
        ));
        let realized_path = first_test_nix_store_path();
        let mount_path = root.join("jobs/job-1/staged-linux-rust/workspace-build");
        fs::create_dir_all(root.join("jobs/job-1/staged-linux-rust")).expect("create mount root");

        let request_path = root.join("request.json");
        write_json(
            request_path.clone(),
            &PreparedOutputRemoteExposureRequest {
                schema_version: 1,
                node_id: "prepare-pika-core-linux-rust-workspace-build".to_string(),
                installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild".to_string(),
                output_name: "ci.aarch64-linux.workspaceBuild".to_string(),
                protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
                realized_path: realized_path.display().to_string(),
                requested_exposures: vec![PreparedOutputExposure {
                    kind: PreparedOutputExposureKind::HostSymlinkMount,
                    path: mount_path.display().to_string(),
                    access: PreparedOutputExposureAccess::ReadOnly,
                }],
            },
        )
        .expect("write request");

        let fulfilled =
            fulfill_prepared_output_request(&request_path).expect("fulfill prepared output");

        assert_eq!(fulfilled.output_name, "ci.aarch64-linux.workspaceBuild");
        assert_eq!(fulfilled.requested_exposures.len(), 1);
        assert_eq!(
            fs::read_link(&mount_path).expect("read symlink"),
            realized_path
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fulfill_prepared_output_request_requires_realized_path() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-fulfill-request-missing-path-test-{}",
            uuid::Uuid::new_v4()
        ));
        let request_path = root.join("request.json");
        fs::create_dir_all(&root).expect("create root");
        write_json(
            request_path.clone(),
            &PreparedOutputRemoteExposureRequest {
                schema_version: 1,
                node_id: "prepare-pika-core-linux-rust-workspace-build".to_string(),
                installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild".to_string(),
                output_name: "ci.aarch64-linux.workspaceBuild".to_string(),
                protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
                realized_path: Path::new("/nix/store/missing-output").display().to_string(),
                requested_exposures: Vec::new(),
            },
        )
        .expect("write request");

        let err =
            fulfill_prepared_output_request(&request_path).expect_err("missing realized path");
        assert!(err.to_string().contains("points at missing realized path"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fulfill_prepared_output_request_rejects_non_nix_store_paths() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-fulfill-request-non-store-path-test-{}",
            uuid::Uuid::new_v4()
        ));
        let realized_path = root.join("not-nix-store/output");
        let request_path = root.join("request.json");
        fs::create_dir_all(&realized_path).expect("create realized path");
        write_json(
            request_path.clone(),
            &PreparedOutputRemoteExposureRequest {
                schema_version: 1,
                node_id: "prepare-pika-core-linux-rust-workspace-build".to_string(),
                installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild".to_string(),
                output_name: "ci.aarch64-linux.workspaceBuild".to_string(),
                protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
                realized_path: realized_path.display().to_string(),
                requested_exposures: Vec::new(),
            },
        )
        .expect("write request");

        let err = fulfill_prepared_output_request(&request_path).expect_err("non nix store path");
        assert!(err.to_string().contains("non-Nix-store realized path"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fulfill_prepared_output_request_rejects_unknown_schema_versions() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-fulfill-request-schema-version-test-{}",
            uuid::Uuid::new_v4()
        ));
        let realized_path = first_test_nix_store_path();
        let request_path = root.join("request.json");
        fs::create_dir_all(&root).expect("create root");
        write_json(
            request_path.clone(),
            &PreparedOutputRemoteExposureRequest {
                schema_version: 99,
                node_id: "prepare-pika-core-linux-rust-workspace-build".to_string(),
                installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild".to_string(),
                output_name: "ci.aarch64-linux.workspaceBuild".to_string(),
                protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
                realized_path: realized_path.display().to_string(),
                requested_exposures: Vec::new(),
            },
        )
        .expect("write request");

        let err =
            fulfill_prepared_output_request(&request_path).expect_err("unknown schema version");
        assert!(err.to_string().contains("unsupported schema_version"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fulfill_prepared_output_request_result_reports_success_machine_readably() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-fulfill-request-result-success-test-{}",
            uuid::Uuid::new_v4()
        ));
        let realized_path = first_test_nix_store_path();
        let mount_path = root.join("jobs/job-1/staged-linux-rust/workspace-build");
        fs::create_dir_all(root.join("jobs/job-1/staged-linux-rust")).expect("create mount root");

        let request_path = root.join("request.json");
        write_json(
            request_path.clone(),
            &PreparedOutputRemoteExposureRequest {
                schema_version: 1,
                node_id: "prepare-pika-core-linux-rust-workspace-build".to_string(),
                installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild".to_string(),
                output_name: "ci.aarch64-linux.workspaceBuild".to_string(),
                protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
                realized_path: realized_path.display().to_string(),
                requested_exposures: vec![PreparedOutputExposure {
                    kind: PreparedOutputExposureKind::HostSymlinkMount,
                    path: mount_path.display().to_string(),
                    access: PreparedOutputExposureAccess::ReadOnly,
                }],
            },
        )
        .expect("write request");

        let result = fulfill_prepared_output_request_result(&request_path);
        let realized_path_string = realized_path.display().to_string();

        assert_eq!(result.status, PreparedOutputFulfillmentStatus::Succeeded);
        assert_eq!(result.fulfilled_exposures_count, 1);
        assert_eq!(
            result.realized_path.as_deref(),
            Some(realized_path_string.as_str())
        );
        assert_eq!(
            fs::read_link(&mount_path).expect("read symlink"),
            realized_path
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fulfill_prepared_output_request_result_reports_failure_machine_readably() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-fulfill-request-result-failure-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("create root");
        let request_path = root.join("request.json");
        write_json(
            request_path.clone(),
            &PreparedOutputRemoteExposureRequest {
                schema_version: 1,
                node_id: "prepare-pika-core-linux-rust-workspace-build".to_string(),
                installable: "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild".to_string(),
                output_name: "ci.aarch64-linux.workspaceBuild".to_string(),
                protocol: PreparedOutputHandoffProtocol::NixStorePathV1,
                realized_path: Path::new("/nix/store/missing-output").display().to_string(),
                requested_exposures: Vec::new(),
            },
        )
        .expect("write request");

        let result = fulfill_prepared_output_request_result(&request_path);

        assert_eq!(result.status, PreparedOutputFulfillmentStatus::Failed);
        assert_eq!(result.fulfilled_exposures_count, 0);
        assert!(
            result
                .error
                .as_deref()
                .expect("failure error")
                .contains("points at missing realized path")
        );

        let _ = fs::remove_dir_all(&root);
    }

    fn first_test_nix_store_path() -> PathBuf {
        fs::read_dir("/nix/store")
            .expect("read /nix/store")
            .find_map(|entry| {
                let path = entry.ok()?.path();
                path.exists().then_some(path)
            })
            .expect("find existing /nix/store path for tests")
    }

    #[test]
    fn max_parallel_execute_jobs_is_narrowed_to_staged_wrapper_lane() {
        let root =
            std::env::temp_dir().join(format!("pikaci-parallel-test-{}", uuid::Uuid::new_v4()));
        let prepared = sample_prepared_run(&root);
        let snapshot = sample_snapshot_source(&prepared);
        let staged_jobs = vec![
            JobSpec {
                id: "pika-core-lib-app-flows-tests",
                description: "Run pika_core lib tests and app_flows integration tests in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ShellCommand {
                    command: "cargo test -p pika_core --lib --test app_flows -- --nocapture",
                },
            },
            JobSpec {
                id: "pika-core-messaging-e2e-tests",
                description: "Run pika_core messaging and group profile integration tests in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ShellCommand {
                    command: "cargo test -p pika_core --test e2e_messaging --test e2e_group_profiles -- --nocapture",
                },
            },
        ];
        let mixed_jobs = vec![
            staged_jobs[0].clone(),
            JobSpec {
                id: "agent-control-plane-unit",
                description: "Run all pika-agent-control-plane unit tests in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::PackageUnitTests {
                    package: "pika-agent-control-plane",
                },
            },
        ];

        let staged_plan =
            build_run_plan(&staged_jobs, &prepared, &snapshot, &RunMetadata::default())
                .expect("build staged plan");
        let mixed_plan = build_run_plan(&mixed_jobs, &prepared, &snapshot, &RunMetadata::default())
            .expect("build mixed plan");

        assert_eq!(parallel_execute_cap_for_jobs(&staged_plan.jobs, 2), 2);
        assert_eq!(parallel_execute_cap_for_jobs(&mixed_plan.jobs, 2), 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn mark_prepare_failure_records_sibling_jobs_as_skipped_when_prepare_phase_aborts() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-prepare-failure-test-{}",
            uuid::Uuid::new_v4()
        ));
        let prepared = sample_prepared_run(&root);
        let snapshot = sample_snapshot_source(&prepared);
        let jobs = vec![
            JobSpec {
                id: "pika-core-lib-app-flows-tests",
                description: "Run pika_core lib tests and app_flows integration tests in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ShellCommand {
                    command: "cargo test -p pika_core --lib --test app_flows -- --nocapture",
                },
            },
            JobSpec {
                id: "pika-core-messaging-e2e-tests",
                description: "Run pika_core messaging and group profile integration tests in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ShellCommand {
                    command: "cargo test -p pika_core --test e2e_messaging --test e2e_group_profiles -- --nocapture",
                },
            },
        ];
        let plan = build_run_plan(&jobs, &prepared, &snapshot, &RunMetadata::default())
            .expect("build plan");
        let mut run_record = RunRecord {
            run_id: "run-1".to_string(),
            status: RunStatus::Running,
            rerun_of: None,
            target_id: Some("pre-merge-pika-rust".to_string()),
            target_description: Some("Run pika rust lane".to_string()),
            source_root: snapshot.source_root.clone(),
            snapshot_dir: snapshot.snapshot_dir_string.clone(),
            git_head: snapshot.git_head.clone(),
            git_dirty: snapshot.git_dirty,
            created_at: prepared.created_at.clone(),
            finished_at: None,
            plan_path: Some(prepared.run_dir.join("plan.json").display().to_string()),
            prepared_outputs_path: Some(
                prepared
                    .run_dir
                    .join("prepared-outputs.json")
                    .display()
                    .to_string(),
            ),
            prepared_output_consumer: Some(PreparedOutputConsumerKind::FulfillRequestCliV1),
            prepared_output_mode: Some(STAGED_LINUX_RUST_SUBPROCESS_MODE_NAME.to_string()),
            prepared_output_invocation_mode: Some(PreparedOutputInvocationMode::DirectHelperExecV1),
            prepared_output_invocation_wrapper_program: None,
            prepared_output_launcher_transport_mode: None,
            prepared_output_launcher_transport_program: None,
            prepared_output_launcher_transport_host: None,
            prepared_output_launcher_transport_remote_launcher_program: None,
            prepared_output_launcher_transport_remote_helper_program: None,
            prepared_output_launcher_transport_remote_work_dir: None,
            changed_files: Vec::new(),
            filters: Vec::new(),
            message: None,
            jobs: Vec::new(),
        };

        mark_prepare_failure(
            &mut run_record,
            &plan,
            &PrepareFailure {
                node_id: "prepare-pika-core-lib-app-flows-tests-runner".to_string(),
                message: "runner build failed".to_string(),
            },
        )
        .expect("record prepare failure");

        assert_eq!(run_record.jobs.len(), 2);
        let app_flows = run_record
            .jobs
            .iter()
            .find(|job| job.id == "pika-core-lib-app-flows-tests")
            .expect("app flows job record");
        assert_eq!(app_flows.status, RunStatus::Failed);
        assert!(
            app_flows
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("prepare node `prepare-pika-core-lib-app-flows-tests-runner` failed")
        );
        let messaging = run_record
            .jobs
            .iter()
            .find(|job| job.id == "pika-core-messaging-e2e-tests")
            .expect("messaging job record");
        assert_eq!(messaging.status, RunStatus::Skipped);
        assert!(messaging.message.as_deref().unwrap_or_default().contains(
            "prepare phase stopped after `prepare-pika-core-lib-app-flows-tests-runner` failed"
        ));

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

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<String>,
        _lock: Option<MutexGuard<'static, ()>>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            static ENV_VAR_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
            let lock = (key == "PIKACI_PREPARED_OUTPUT_FULFILL_BINARY").then(|| {
                ENV_VAR_LOCK
                    .get_or_init(|| Mutex::new(()))
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
            });
            let previous = std::env::var(key).ok();
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
            Self {
                key,
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }
}
