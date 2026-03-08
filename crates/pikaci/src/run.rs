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
    PreparedOutputExposureKind, PreparedOutputHandoff, PreparedOutputHandoffProtocol,
    PreparedOutputRemoteExposureRequest, PreparedOutputsRecord, RealizedPreparedOutputRecord,
    RunPlanRecord, RunRecord, RunStatus, RunnerKind, StagedLinuxRustLane,
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
    pub changed_files: Vec<String>,
    pub filters: Vec<String>,
    pub message: Option<String>,
}

const STAGED_LINUX_RUST_SUBPROCESS_MODE_ENV: &str = "PIKACI_PRE_MERGE_PIKA_RUST_SUBPROCESS_FULFILL";
const STAGED_LINUX_RUST_SUBPROCESS_MODE_NAME: &str =
    "pre_merge_pika_rust_subprocess_fulfillment_v1";

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
        changed_files: metadata.changed_files,
        filters: metadata.filters,
        message: metadata.message,
        jobs: Vec::new(),
    };
    write_run_record(&prepared.run_dir, &run_record)?;

    let prepared_node_ids = match run_prepare_nodes(
        &prepared.run_dir,
        &plan.prepares,
        &prepared_outputs_path,
        prepared_output_consumer_kind,
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

struct PreparedOutputConsumerResult {
    kind: PreparedOutputConsumerKind,
    exposures: Vec<PreparedOutputExposure>,
    requested_exposures: Vec<PreparedOutputExposure>,
    consumer_request_path: Option<String>,
}

trait PreparedOutputConsumer {
    fn kind(&self) -> PreparedOutputConsumerKind;

    fn consume(
        &self,
        materialization: &PreparedOutputMaterialization<'_>,
        handoff: &PreparedOutputHandoff,
        run_dir: &Path,
        log_paths: &[PathBuf],
    ) -> anyhow::Result<PreparedOutputConsumerResult>;
}

struct HostLocalSymlinkPreparedOutputConsumer;
struct RemoteExposureRequestPreparedOutputConsumer;
struct FulfillRequestCliPreparedOutputConsumer;

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
    let bytes =
        fs::read(request_path).with_context(|| format!("read {}", request_path.display()))?;
    let request: PreparedOutputRemoteExposureRequest = serde_json::from_slice(&bytes)
        .with_context(|| format!("decode {}", request_path.display()))?;
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

fn resolve_prepared_output_fulfillment_program(
    explicit_program: Option<PathBuf>,
    current_exe: PathBuf,
) -> anyhow::Result<PathBuf> {
    if let Some(path) = explicit_program {
        return Ok(path);
    }
    if current_exe
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "pikaci")
    {
        return Ok(current_exe);
    }
    Err(anyhow!(
        "PIKACI_PREPARED_OUTPUT_CONSUMER=fulfill_request_cli_v1 requires PIKACI_PREPARED_OUTPUT_FULFILL_BINARY when the host executable is not `pikaci`; current executable is {}",
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

fn fulfill_prepared_output_request_via_subprocess(
    request_path: &Path,
    log_paths: &[PathBuf],
) -> anyhow::Result<()> {
    let program = prepared_output_fulfillment_program()?;
    append_log_line_many(
        log_paths,
        &format!(
            "[pikaci] prepared output fulfillment subprocess={} request={}",
            program.display(),
            request_path.display()
        ),
    )?;
    let output = Command::new(&program)
        .arg("fulfill-prepared-output-request")
        .arg(request_path)
        .output()
        .with_context(|| {
            format!(
                "run prepared-output fulfillment subprocess `{}` for {}",
                program.display(),
                request_path.display()
            )
        })?;
    append_command_output_many(log_paths, &output.stdout, &output.stderr)?;
    if !output.status.success() {
        return Err(anyhow!(
            "prepared-output fulfillment subprocess `{}` failed with {:?} for {}; see {}",
            program.display(),
            output.status.code(),
            request_path.display(),
            log_paths
                .first()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<no log>".to_string())
        ));
    }
    Ok(())
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
    ) -> anyhow::Result<PreparedOutputConsumerResult> {
        let mut exposures = Vec::new();
        for exposure in &handoff.exposures {
            match exposure.kind {
                PreparedOutputExposureKind::HostSymlinkMount => {
                    let mount_path = Path::new(&exposure.path);
                    repoint_prepare_mount(mount_path, materialization.realized_path)?;
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
                    )?;
                    exposures.push(exposure.clone());
                }
            }
        }
        Ok(PreparedOutputConsumerResult {
            kind: PreparedOutputConsumerKind::HostLocalSymlinkMountsV1,
            exposures,
            requested_exposures: Vec::new(),
            consumer_request_path: None,
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
    ) -> anyhow::Result<PreparedOutputConsumerResult> {
        let request_path =
            write_prepared_output_remote_exposure_request(materialization, handoff, run_dir)?;
        append_log_line_many(
            log_paths,
            &format!(
                "[pikaci] prepared output consumer={} wrote remote exposure request {}",
                prepared_output_consumer_kind_text(
                    PreparedOutputConsumerKind::RemoteExposureRequestV1
                ),
                request_path.display()
            ),
        )?;
        Ok(PreparedOutputConsumerResult {
            kind: PreparedOutputConsumerKind::RemoteExposureRequestV1,
            exposures: Vec::new(),
            requested_exposures: handoff.exposures.clone(),
            consumer_request_path: Some(request_path.display().to_string()),
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
    ) -> anyhow::Result<PreparedOutputConsumerResult> {
        let request_path =
            write_prepared_output_remote_exposure_request(materialization, handoff, run_dir)?;
        append_log_line_many(
            log_paths,
            &format!(
                "[pikaci] prepared output consumer={} wrote fulfillment request {}",
                prepared_output_consumer_kind_text(PreparedOutputConsumerKind::FulfillRequestCliV1),
                request_path.display()
            ),
        )?;
        fulfill_prepared_output_request_via_subprocess(&request_path, log_paths)?;
        append_log_line_many(
            log_paths,
            &format!(
                "[pikaci] prepared output consumer={} fulfilled request {}",
                prepared_output_consumer_kind_text(PreparedOutputConsumerKind::FulfillRequestCliV1),
                request_path.display()
            ),
        )?;
        Ok(PreparedOutputConsumerResult {
            kind: PreparedOutputConsumerKind::FulfillRequestCliV1,
            exposures: handoff.exposures.clone(),
            requested_exposures: handoff.exposures.clone(),
            consumer_request_path: Some(request_path.display().to_string()),
        })
    }
}

fn consume_prepared_output_handoff(
    consumer: &dyn PreparedOutputConsumer,
    materialization: &PreparedOutputMaterialization<'_>,
    handoff: &PreparedOutputHandoff,
    run_dir: &Path,
    log_paths: &[PathBuf],
) -> anyhow::Result<PreparedOutputConsumerResult> {
    let result = consumer.consume(materialization, handoff, run_dir, log_paths)?;
    debug_assert_eq!(result.kind, consumer.kind());
    Ok(result)
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
                    )
                    .map_err(|err| PrepareFailure {
                        node_id: prepare.node_id.clone(),
                        message: format!("{err:#}"),
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

    use super::{
        FulfillRequestCliPreparedOutputConsumer, HostLocalSymlinkPreparedOutputConsumer,
        PrepareFailure, PreparedOutputMaterialization, PreparedRun,
        RemoteExposureRequestPreparedOutputConsumer, RunMetadata,
        STAGED_LINUX_RUST_SUBPROCESS_MODE_ENV, STAGED_LINUX_RUST_SUBPROCESS_MODE_NAME,
        SnapshotSource, build_run_plan, configured_prepared_output_consumer_kind,
        consume_prepared_output_handoff, fulfill_prepared_output_request, gc_runs,
        mark_prepare_failure, parallel_execute_cap_for_jobs, parse_bool_env_flag,
        ready_execute_job_positions, resolve_prepared_output_fulfillment_program,
        resolve_run_prepared_output_consumer_kind_for_mode, selected_prepared_output_consumer,
        upsert_prepared_output_record, validate_prepared_output_consumer_for_jobs, write_json,
        write_run_plan_record,
    };
    use crate::model::{
        ExecuteNode, GuestCommand, JobSpec, PlanExecutorKind, PlanNodeRecord, PlanScope,
        PrepareNode, PreparedOutputConsumerKind, PreparedOutputExposure,
        PreparedOutputExposureAccess, PreparedOutputExposureKind, PreparedOutputHandoff,
        PreparedOutputHandoffProtocol, PreparedOutputRemoteExposureRequest, PreparedOutputsRecord,
        RealizedPreparedOutputRecord, RunPlanRecord, RunRecord, RunStatus,
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
    fn resolve_prepared_output_fulfillment_program_accepts_pikaci_binary_name() {
        let resolved =
            resolve_prepared_output_fulfillment_program(None, PathBuf::from("/tmp/bin/pikaci"))
                .expect("resolve pikaci binary");
        assert_eq!(resolved, PathBuf::from("/tmp/bin/pikaci"));
    }

    #[test]
    fn resolve_prepared_output_fulfillment_program_rejects_non_pikaci_host_binary() {
        let err = resolve_prepared_output_fulfillment_program(
            None,
            PathBuf::from("/tmp/bin/embedding-runner"),
        )
        .expect_err("reject embedding binary");
        assert!(err.to_string().contains(
            "requires PIKACI_PREPARED_OUTPUT_FULFILL_BINARY when the host executable is not `pikaci`"
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
if [ "${1:-}" != "fulfill-prepared-output-request" ]; then
  echo "unexpected command: ${1:-}" >&2
  exit 17
fi
request_path="$2"
realized_path=$(sed -n 's/.*"realized_path": "\(.*\)",/\1/p' "$request_path" | head -n1)
mount_path=$(sed -n 's/.*"path": "\(.*\)",/\1/p' "$request_path" | head -n1)
mkdir -p "$(dirname "$mount_path")"
ln -sfn "$realized_path" "$mount_path"
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
        )
        .expect("consume handoff");

        assert_eq!(result.kind, PreparedOutputConsumerKind::FulfillRequestCliV1);
        assert_eq!(result.exposures, handoff.exposures);
        assert_eq!(result.requested_exposures, handoff.exposures);
        let request_path = result
            .consumer_request_path
            .as_deref()
            .expect("fulfillment request path");
        assert!(request_path.ends_with(
            "prepared-output-requests/prepare-pika-core-linux-rust-workspace-build.json"
        ));
        assert_eq!(
            fs::read_link(&mount_path).expect("read symlink"),
            realized_path
        );
        let log_body = fs::read_to_string(&log_path).expect("read log");
        assert!(log_body.contains("prepared output fulfillment subprocess="));
        assert!(
            log_body.contains("prepared output consumer=fulfill_request_cli_v1 fulfilled request")
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
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let previous = std::env::var(key).ok();
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
            Self { key, previous }
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
