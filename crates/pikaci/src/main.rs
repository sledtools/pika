mod output;

use anyhow::{Context, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use jerichoci::{
    LogKind, RunLifecycleEvent, RunMetadata, RunOptions, RunRecord, RunStatus,
    StagedLinuxRemoteDefaults, fulfill_prepared_output_request, gc_runs, git_changed_files,
    list_runs, load_logs, load_logs_metadata, load_prepared_outputs_record, load_run_record,
    record_skipped_run_with_reporter, rerun_jobs_with_metadata_and_reporter,
    run_jobs_with_metadata_and_reporter, staged_linux_remote_defaults,
};
use output::{
    format_status_lines, prepared_output_consumer_label, print_json, print_json_line,
    print_run_human_summary, status_text,
};
use pikaci::catalog::PikaStagedLinuxTargetInfoJson;
use pikaci::targets::{TargetSpec, staged_linux_target, target_spec};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "pikaci")]
#[command(about = "Wave 1 local-first CI runner for Pika")]
struct Cli {
    #[arg(long, global = true)]
    state_root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Run {
        job: String,
        #[arg(long, value_enum, default_value = "human")]
        output: RunOutputArg,
    },
    List,
    Gc {
        #[arg(long, default_value_t = 10)]
        keep_runs: usize,
    },
    Logs {
        run_id: String,
        #[arg(long)]
        job: Option<String>,
        #[arg(long, value_enum, default_value = "both")]
        kind: LogKindArg,
        #[arg(long)]
        metadata_json: bool,
    },
    PreparedOutputs {
        run_id: String,
        #[arg(long)]
        json: bool,
    },
    Status {
        run_id: String,
        #[arg(long)]
        json: bool,
    },
    FulfillPreparedOutputRequest {
        request_path: String,
    },
    StagedLinuxRemoteDefaults {
        #[arg(long)]
        json: bool,
    },
    StagedLinuxTargetInfo {
        target: String,
        #[arg(long)]
        json: bool,
    },
    Rerun {
        run_id: String,
        #[arg(long, value_enum, default_value = "human")]
        output: RunOutputArg,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum LogKindArg {
    Host,
    Guest,
    Both,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum RunOutputArg {
    Human,
    Json,
    Jsonl,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    fail_if_removed_backend_selector_is_set()?;
    let cwd = std::env::current_dir().context("read current directory")?;
    let options = RunOptions {
        source_root: cwd.clone(),
        state_root: resolve_state_root(&cwd, cli.state_root),
    };

    match cli.command {
        Command::Run { job, output } => {
            let target = target_spec(&job)?;
            exit_for_run(run_target_with_output(&options, target, output)?)?;
        }
        Command::List => {
            for run in list_runs(&options.state_root)? {
                let target = run
                    .target_id
                    .as_deref()
                    .unwrap_or_else(|| run.jobs.first().map(|job| job.id.as_str()).unwrap_or("-"));
                println!(
                    "{}\t{}\t{}\t{}",
                    run.run_id,
                    status_text(run.status),
                    target,
                    run.created_at
                );
            }
        }
        Command::Gc { keep_runs } => {
            let removed = gc_runs(&options.state_root, keep_runs)?;
            println!("removed_runs={}", removed.len());
            for run_id in removed {
                println!("{run_id}");
            }
        }
        Command::Logs {
            run_id,
            job,
            kind,
            metadata_json,
        } => {
            if metadata_json {
                let metadata = load_logs_metadata(&options.state_root, &run_id, job.as_deref())?;
                print_json(&metadata)?;
            } else {
                let logs = load_logs(
                    &options.state_root,
                    &run_id,
                    job.as_deref(),
                    map_log_kind(kind),
                )?;
                if let Some(host) = logs.host {
                    println!("== host ==\n{host}");
                }
                if let Some(guest) = logs.guest {
                    println!("== guest ==\n{guest}");
                }
            }
        }
        Command::PreparedOutputs { run_id, json } => {
            let record = load_prepared_outputs_record(&options.state_root, &run_id)?
                .ok_or_else(|| anyhow!("prepared outputs not found for run `{run_id}`"))?;
            if json {
                print_json(&record)?;
            } else {
                println!("run_id={run_id}");
                println!("schema_version={}", record.schema_version);
                println!("outputs={}", record.outputs.len());
                for output in record.outputs {
                    let payload_kind = output
                        .payload
                        .as_ref()
                        .map(|payload| payload.kind.as_str())
                        .unwrap_or("-");
                    let entrypoint_count = output
                        .payload
                        .as_ref()
                        .map(|payload| payload.entrypoints.len())
                        .unwrap_or(0);
                    println!(
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        output.node_id,
                        output.output_name,
                        output.installable,
                        prepared_output_consumer_label(output.consumer),
                        output.realized_path,
                        payload_kind,
                        entrypoint_count
                    );
                }
            }
        }
        Command::Status { run_id, json } => {
            let run = load_run_record(&options.state_root, &run_id)?;
            if json {
                print_json(&run)?;
            } else {
                for line in format_status_lines(&run) {
                    println!("{line}");
                }
            }
        }
        Command::FulfillPreparedOutputRequest { request_path } => {
            let request = fulfill_prepared_output_request(std::path::Path::new(&request_path))?;
            println!("request={request_path}");
            println!("output={}", request.output_name);
            println!("requested_exposures={}", request.requested_exposures.len());
        }
        Command::StagedLinuxRemoteDefaults { json } => {
            if json {
                print_json(&staged_linux_remote_defaults_json(
                    staged_linux_remote_defaults(),
                ))?;
            } else {
                print_staged_linux_remote_defaults(staged_linux_remote_defaults());
            }
        }
        Command::StagedLinuxTargetInfo { target, json } => {
            let target = staged_linux_target(&target)?;
            let config = target.config();
            if json {
                print_json(&PikaStagedLinuxTargetInfoJson::from(config))?;
            } else {
                println!("target_id={}", config.target_id);
                println!(
                    "target_description={}",
                    shell_escape(config.target_description)
                );
                for payload_spec in config.payload_specs {
                    println!(
                        "payload_{}_installable={}",
                        payload_spec.role.prepare_node_suffix(),
                        payload_spec.nix_installable
                    );
                }
            }
        }
        Command::Rerun { run_id, output } => {
            let previous = load_run_record(&options.state_root, &run_id)?;
            exit_for_run(rerun_target_with_output(&options, &previous, output)?)?;
        }
    }

    Ok(())
}

fn resolve_state_root(cwd: &std::path::Path, configured: Option<PathBuf>) -> PathBuf {
    configured
        .or_else(|| std::env::var_os("PIKACI_STATE_ROOT").map(PathBuf::from))
        .unwrap_or_else(|| cwd.join(".pikaci"))
}

fn fail_if_removed_backend_selector_is_set() -> anyhow::Result<()> {
    let Ok(value) = std::env::var("PIKACI_REMOTE_LINUX_VM_INCUS_LANES") else {
        return Ok(());
    };
    if value.trim().is_empty() {
        return Ok(());
    }
    bail!(
        "PIKACI_REMOTE_LINUX_VM_INCUS_LANES has been removed; use PIKACI_REMOTE_LINUX_VM_BACKEND=incus|auto"
    );
}

fn shell_escape(value: &str) -> String {
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn print_staged_linux_remote_defaults(defaults: StagedLinuxRemoteDefaults) {
    println!("default_ssh_binary={}", shell_escape(defaults.ssh_binary));
    println!(
        "default_ssh_nix_binary={}",
        shell_escape(defaults.ssh_nix_binary)
    );
    println!("default_ssh_host={}", shell_escape(defaults.ssh_host));
    println!(
        "default_remote_work_dir={}",
        shell_escape(defaults.remote_work_dir)
    );
    println!(
        "default_remote_launcher_binary={}",
        shell_escape(defaults.remote_launcher_binary)
    );
    println!(
        "default_remote_helper_binary={}",
        shell_escape(defaults.remote_helper_binary)
    );
    println!(
        "default_store_uri={}",
        shell_escape(&format!("ssh://{}", defaults.ssh_host))
    );
}

#[derive(serde::Serialize)]
struct StagedLinuxRemoteDefaultsJson<'a> {
    ssh_binary: &'a str,
    ssh_nix_binary: &'a str,
    ssh_host: &'a str,
    remote_work_dir: &'a str,
    remote_launcher_binary: &'a str,
    remote_helper_binary: &'a str,
    store_uri: String,
}

fn staged_linux_remote_defaults_json(
    defaults: StagedLinuxRemoteDefaults,
) -> StagedLinuxRemoteDefaultsJson<'static> {
    StagedLinuxRemoteDefaultsJson {
        ssh_binary: defaults.ssh_binary,
        ssh_nix_binary: defaults.ssh_nix_binary,
        ssh_host: defaults.ssh_host,
        remote_work_dir: defaults.remote_work_dir,
        remote_launcher_binary: defaults.remote_launcher_binary,
        remote_helper_binary: defaults.remote_helper_binary,
        store_uri: format!("ssh://{}", defaults.ssh_host),
    }
}

fn exit_for_run(run: RunRecord) -> anyhow::Result<()> {
    if matches!(run.status, RunStatus::Failed) {
        std::process::exit(1);
    }
    Ok(())
}

fn run_target_with_output(
    options: &RunOptions,
    target: TargetSpec,
    output: RunOutputArg,
) -> anyhow::Result<RunRecord> {
    match output {
        RunOutputArg::Human => {
            let run = run_target(options, target)?;
            print_run_human_summary(&run);
            Ok(run)
        }
        RunOutputArg::Json => {
            let run = run_target(options, target)?;
            print_json(&run)?;
            Ok(run)
        }
        RunOutputArg::Jsonl => {
            let mut reporter = |event: RunLifecycleEvent| print_json_line(&event);
            run_target_with_reporter(options, target, &mut reporter)
        }
    }
}

fn rerun_target_with_output(
    options: &RunOptions,
    previous: &jerichoci::RunRecord,
    output: RunOutputArg,
) -> anyhow::Result<RunRecord> {
    match output {
        RunOutputArg::Human => {
            let run = rerun_target(options, previous)?;
            print_run_human_summary(&run);
            Ok(run)
        }
        RunOutputArg::Json => {
            let run = rerun_target(options, previous)?;
            print_json(&run)?;
            Ok(run)
        }
        RunOutputArg::Jsonl => {
            let mut reporter = |event: RunLifecycleEvent| print_json_line(&event);
            rerun_target_with_reporter(options, previous, &mut reporter)
        }
    }
}

fn map_log_kind(kind: LogKindArg) -> LogKind {
    match kind {
        LogKindArg::Host => LogKind::Host,
        LogKindArg::Guest => LogKind::Guest,
        LogKindArg::Both => LogKind::Both,
    }
}

fn run_target(options: &RunOptions, target: TargetSpec) -> anyhow::Result<jerichoci::RunRecord> {
    run_target_with_reporter(options, target, &mut |_| Ok(()))
}

fn run_target_with_reporter(
    options: &RunOptions,
    target: TargetSpec,
    reporter: &mut dyn FnMut(RunLifecycleEvent) -> anyhow::Result<()>,
) -> anyhow::Result<jerichoci::RunRecord> {
    let changed_files = git_changed_files(&options.source_root);
    let metadata = RunMetadata {
        rerun_of: None,
        target_id: Some(target.id.to_string()),
        target_description: Some(target.description.to_string()),
        prepared_output_mode: None,
        prepared_output_invocation_mode: None,
        prepared_output_invocation_wrapper_program: None,
        prepared_output_launcher_transport_mode: None,
        prepared_output_launcher_transport_program: None,
        prepared_output_launcher_transport_host: None,
        prepared_output_launcher_transport_remote_launcher_program: None,
        prepared_output_launcher_transport_remote_helper_program: None,
        prepared_output_launcher_transport_remote_work_dir: None,
        changed_files: changed_files.clone().unwrap_or_default(),
        filters: target.filters.clone(),
        message: None,
    };

    if target.filters.is_empty() {
        return run_jobs_with_metadata_and_reporter(
            target.jobs.as_slice(),
            options,
            metadata,
            reporter,
        );
    }

    let Some(changed_files) = changed_files else {
        let mut metadata = metadata;
        metadata.message =
            Some("git change detection unavailable; running lane without filtering".to_string());
        return run_jobs_with_metadata_and_reporter(
            target.jobs.as_slice(),
            options,
            metadata,
            reporter,
        );
    };

    if changed_files
        .iter()
        .any(|path| matches_any_filter(path, &target.filters))
    {
        return run_jobs_with_metadata_and_reporter(
            target.jobs.as_slice(),
            options,
            metadata,
            reporter,
        );
    }

    let mut metadata = metadata;
    metadata.message = Some(format!(
        "skipped; no changed files matched {} filter(s)",
        target.filters.len()
    ));
    record_skipped_run_with_reporter(options, metadata, reporter)
}

fn rerun_target(
    options: &RunOptions,
    previous: &jerichoci::RunRecord,
) -> anyhow::Result<jerichoci::RunRecord> {
    rerun_target_with_reporter(options, previous, &mut |_| Ok(()))
}

fn rerun_target_with_reporter(
    options: &RunOptions,
    previous: &jerichoci::RunRecord,
    reporter: &mut dyn FnMut(RunLifecycleEvent) -> anyhow::Result<()>,
) -> anyhow::Result<jerichoci::RunRecord> {
    let target = target_spec_for_rerun(previous)?;
    let metadata = rerun_metadata(previous, &target);

    if previous.status == RunStatus::Skipped {
        return record_skipped_run_with_reporter(options, metadata, reporter);
    }

    rerun_jobs_with_metadata_and_reporter(
        target.jobs.as_slice(),
        previous,
        options,
        metadata,
        reporter,
    )
}

fn rerun_metadata(previous: &jerichoci::RunRecord, target: &TargetSpec) -> RunMetadata {
    RunMetadata {
        rerun_of: Some(previous.run_id.clone()),
        target_id: previous
            .target_id
            .clone()
            .or_else(|| Some(target.id.to_string())),
        target_description: previous
            .target_description
            .clone()
            .or_else(|| Some(target.description.to_string())),
        prepared_output_mode: previous.prepared_output_mode.clone(),
        prepared_output_invocation_mode: previous.prepared_output_invocation_mode,
        prepared_output_invocation_wrapper_program: previous
            .prepared_output_invocation_wrapper_program
            .clone(),
        prepared_output_launcher_transport_mode: previous.prepared_output_launcher_transport_mode,
        prepared_output_launcher_transport_program: previous
            .prepared_output_launcher_transport_program
            .clone(),
        prepared_output_launcher_transport_host: previous
            .prepared_output_launcher_transport_host
            .clone(),
        prepared_output_launcher_transport_remote_launcher_program: previous
            .prepared_output_launcher_transport_remote_launcher_program
            .clone(),
        prepared_output_launcher_transport_remote_helper_program: previous
            .prepared_output_launcher_transport_remote_helper_program
            .clone(),
        prepared_output_launcher_transport_remote_work_dir: previous
            .prepared_output_launcher_transport_remote_work_dir
            .clone(),
        changed_files: previous.changed_files.clone(),
        filters: previous.filters.clone(),
        message: Some(format!("rerun of {}", previous.run_id)),
    }
}

fn target_spec_for_rerun(previous: &jerichoci::RunRecord) -> anyhow::Result<TargetSpec> {
    if let Some(target_id) = previous.target_id.as_deref() {
        return target_spec(target_id);
    }

    if previous.jobs.len() == 1 {
        return target_spec(&previous.jobs[0].id);
    }

    bail!(
        "run `{}` cannot be rerun because it does not record a target id",
        previous.run_id
    )
}

fn matches_any_filter<S: AsRef<str>>(path: &str, filters: &[S]) -> bool {
    filters
        .iter()
        .any(|pattern| matches_filter(path, pattern.as_ref()))
}

fn matches_filter(path: &str, pattern: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("/**") {
        path == prefix || path.starts_with(&format!("{prefix}/"))
    } else {
        path == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Cli, Command, format_status_lines, matches_any_filter, matches_filter, rerun_metadata,
        resolve_state_root, staged_linux_remote_defaults_json, target_spec, target_spec_for_rerun,
    };
    use clap::Parser;
    use jerichoci::{
        JobPlacementKind, JobRecord, JobRuntimeKind, PreparedOutputConsumerKind,
        RemoteLinuxVmBackend, RunLifecycleEvent, RunRecord, RunStatus, RunnerKind,
        staged_linux_remote_defaults,
    };
    use pikaci::catalog::{PikaStagedLinuxLane, PikaStagedLinuxTarget};

    #[test]
    fn path_filters_match_exact_and_prefix_patterns() {
        assert!(matches_filter("Cargo.toml", "Cargo.toml"));
        assert!(!matches_filter("Cargo.lock", "Cargo.toml"));
        assert!(matches_filter("rust/src/core/agent.rs", "rust/**"));
        assert!(matches_filter(
            ".github/workflows/release.yml",
            ".github/**"
        ));
        assert!(!matches_filter("docs/agent-ci.md", "rust/**"));
    }

    #[test]
    fn resolve_state_root_defaults_to_dot_pikaci_under_cwd() {
        let cwd = std::path::Path::new("/tmp/pika");
        assert_eq!(resolve_state_root(cwd, None), cwd.join(".pikaci"));
    }

    #[test]
    fn resolve_state_root_prefers_explicit_override() {
        let cwd = std::path::Path::new("/tmp/pika");
        assert_eq!(
            resolve_state_root(
                cwd,
                Some(std::path::PathBuf::from("/var/lib/pika-git/pikaci"))
            ),
            std::path::PathBuf::from("/var/lib/pika-git/pikaci")
        );
    }

    #[test]
    fn standalone_agent_contract_target_uses_staged_remote_linux_lane() {
        let standalone = target_spec("pika-cloud-unit").expect("standalone target");
        let pre_merge = target_spec("pre-merge-agent-contracts").expect("pre-merge target");

        assert_eq!(standalone.jobs.len(), 1);
        assert_eq!(standalone.jobs[0].runner_kind(), RunnerKind::RemoteLinuxVm);
        assert_eq!(
            standalone.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::AgentContractsPikaCloudUnit.command_config())
        );

        assert_eq!(pre_merge.jobs.len(), 3);
        assert!(
            pre_merge
                .jobs
                .iter()
                .all(|job| job.runner_kind() == RunnerKind::RemoteLinuxVm)
        );
        assert!(
            pre_merge
                .jobs
                .iter()
                .all(|job| job.id != "agent-http-deterministic-tests")
        );
    }

    #[test]
    fn pre_merge_notifications_target_uses_staged_linux_lane() {
        let target = target_spec("pre-merge-notifications").expect("notifications target");

        assert_eq!(target.jobs.len(), 1);
        assert_eq!(
            target.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::NotificationsServerPackageTests.command_config())
        );
        assert_eq!(target.jobs[0].runner_kind(), RunnerKind::RemoteLinuxVm);
        assert_eq!(
            target.jobs[0].remote_linux_vm_backend(),
            Some(RemoteLinuxVmBackend::Incus)
        );
    }

    #[test]
    fn pre_merge_fixture_rust_target_uses_staged_linux_lane() {
        let target = target_spec("pre-merge-fixture-rust").expect("fixture target");

        assert_eq!(target.jobs.len(), 2);
        assert_eq!(
            target.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::FixturePikahutClippy.command_config())
        );
        assert_eq!(target.jobs[0].runner_kind(), RunnerKind::RemoteLinuxVm);
        assert_eq!(
            target.jobs[1].staged_linux_command(),
            Some(PikaStagedLinuxLane::FixtureRelaySmoke.command_config())
        );
        assert_eq!(target.jobs[1].runner_kind(), RunnerKind::RemoteLinuxVm);
    }

    #[test]
    fn pre_merge_rmp_target_uses_staged_linux_lane() {
        let target = target_spec("pre-merge-rmp").expect("rmp target");
        let config = PikaStagedLinuxTarget::PreMergeRmp.config();

        assert_eq!(target.jobs.len(), 1);
        assert_eq!(target.id, config.target_id);
        assert_eq!(target.description, config.target_description);
        assert_eq!(
            target.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::RmpInitSmokeCi.command_config())
        );
        assert_eq!(target.jobs[0].runner_kind(), RunnerKind::RemoteLinuxVm);
    }

    #[test]
    fn pre_merge_pikachat_rust_target_uses_staged_linux_lanes() {
        let target = target_spec("pre-merge-pikachat-rust").expect("pikachat target");

        assert_eq!(target.jobs.len(), 10);
        assert_eq!(
            target.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::PikachatPackageTests.command_config())
        );
        assert_eq!(
            target.jobs[3].staged_linux_command(),
            Some(PikaStagedLinuxLane::PikachatCliSmokeLocal.command_config())
        );
        assert_eq!(
            target.jobs[9].staged_linux_command(),
            Some(PikaStagedLinuxLane::OpenclawAudioEcho.command_config())
        );
        assert!(
            target
                .jobs
                .iter()
                .all(|job| job.runner_kind() == RunnerKind::RemoteLinuxVm)
        );
    }

    #[test]
    fn pre_merge_pikachat_typescript_target_uses_staged_linux_runner() {
        let target =
            target_spec("pre-merge-pikachat-typescript").expect("pikachat typescript target");

        assert_eq!(target.jobs.len(), 1);
        assert_eq!(
            target.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::PikachatTypescript.command_config())
        );
        assert_eq!(target.jobs[0].runner_kind(), RunnerKind::RemoteLinuxVm);
    }

    #[test]
    fn pre_merge_pikachat_openclaw_e2e_target_uses_staged_linux_runner() {
        let target =
            target_spec("pre-merge-pikachat-openclaw-e2e").expect("pikachat openclaw target");

        assert_eq!(target.jobs.len(), 1);
        assert_eq!(
            target.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::OpenclawGatewayE2e.command_config())
        );
        assert_eq!(target.jobs[0].runner_kind(), RunnerKind::RemoteLinuxVm);
    }

    #[test]
    fn pre_merge_pika_followup_target_uses_staged_linux_runner() {
        let target = target_spec("pre-merge-pika-followup").expect("pika followup target");

        assert_eq!(target.jobs.len(), 6);
        assert!(
            target
                .jobs
                .iter()
                .all(|job| job.runner_kind() == RunnerKind::RemoteLinuxVm)
        );
        assert!(!target.jobs[0].writable_workspace);
        assert_eq!(
            target.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::PikaFollowupAndroidTestCompile.command_config())
        );
        assert_eq!(
            target.jobs[5].staged_linux_command(),
            Some(PikaStagedLinuxLane::PikaFollowupRustDepsHygiene.command_config())
        );
        assert!(
            target
                .jobs
                .iter()
                .any(|job| job.id == "pika-rust-deps-hygiene")
        );
    }

    #[test]
    fn staged_pre_merge_target_filters_live_in_target_catalog() {
        let target = target_spec("pre-merge-pika-rust").expect("pika rust target");

        assert!(target.filters.iter().any(|pattern| pattern == "rust/**"));
        assert!(target.filters.iter().any(|pattern| pattern == "scripts/**"));
        assert!(
            !target
                .filters
                .iter()
                .any(|pattern| pattern == "crates/pikaci/src/forge_lanes.rs")
        );
    }

    #[test]
    fn standalone_experimental_targets_can_address_single_staged_linux_lanes() {
        let actionlint = target_spec("pika-actionlint").expect("actionlint target");
        assert_eq!(actionlint.jobs.len(), 1);
        assert_eq!(actionlint.jobs[0].id, "pika-actionlint");
        assert_eq!(
            actionlint.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::PikaFollowupActionlint.command_config())
        );

        let rmp = target_spec("rmp-init-smoke-ci").expect("rmp target");
        assert_eq!(rmp.jobs.len(), 1);
        assert_eq!(rmp.jobs[0].id, "rmp-init-smoke-ci");
        assert_eq!(
            rmp.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::RmpInitSmokeCi.command_config())
        );

        let rust_deps = target_spec("pika-rust-deps-hygiene").expect("rust deps hygiene target");
        assert_eq!(rust_deps.jobs.len(), 1);
        assert_eq!(rust_deps.jobs[0].id, "pika-rust-deps-hygiene");
        assert_eq!(
            rust_deps.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::PikaFollowupRustDepsHygiene.command_config())
        );

        let app_flows = target_spec("pika-core-lib-app-flows-tests").expect("app flows target");
        assert_eq!(app_flows.jobs.len(), 1);
        assert_eq!(app_flows.jobs[0].id, "pika-core-lib-app-flows-tests");
        assert_eq!(
            app_flows.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::PikaCoreLibAppFlows.command_config())
        );

        let notifications = target_spec("pika-server-package-tests").expect("notifications target");
        assert_eq!(notifications.jobs.len(), 1);
        assert_eq!(notifications.jobs[0].id, "pika-server-package-tests");
        assert_eq!(
            notifications.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::NotificationsServerPackageTests.command_config())
        );

        let pikachat = target_spec("pikachat-package-tests").expect("pikachat package target");
        assert_eq!(pikachat.jobs.len(), 1);
        assert_eq!(pikachat.jobs[0].id, "pikachat-package-tests");
        assert_eq!(
            pikachat.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::PikachatPackageTests.command_config())
        );

        let fixture = target_spec("pikahut-clippy").expect("fixture target");
        assert_eq!(fixture.jobs.len(), 1);
        assert_eq!(fixture.jobs[0].id, "pikahut-clippy");
        assert_eq!(
            fixture.jobs[0].staged_linux_command(),
            Some(PikaStagedLinuxLane::FixturePikahutClippy.command_config())
        );
    }

    #[test]
    fn pre_merge_pikachat_apple_followup_target_uses_host_local_jobs() {
        let target =
            target_spec("pre-merge-pikachat-apple-followup").expect("apple follow-up target");

        assert_eq!(target.jobs.len(), 4);
        assert_eq!(
            target.jobs.iter().map(|job| job.id).collect::<Vec<_>>(),
            vec![
                "pikachat-clippy",
                "pikachat-sidecar-clippy",
                "pikachat-ui-e2e-local-desktop",
                "pikachat-openclaw-channel-behavior",
            ]
        );
        assert!(
            target
                .jobs
                .iter()
                .all(|job| job.runner_kind() == RunnerKind::HostLocal)
        );
        assert!(
            target
                .filters
                .iter()
                .any(|pattern| pattern == "just/checks.just")
        );
        assert!(target.filters.iter().any(|pattern| pattern == "cli/**"));
        assert!(
            target
                .filters
                .iter()
                .any(|pattern| pattern == "crates/pikachat-sidecar/**")
        );
        assert!(
            target
                .filters
                .iter()
                .any(|pattern| pattern == "crates/pikahut/**")
        );
        assert!(
            target
                .filters
                .iter()
                .any(|pattern| pattern == "crates/pika-desktop/**")
        );
        assert!(
            target
                .filters
                .iter()
                .any(|pattern| pattern == "crates/pika-marmot-runtime/**")
        );
        assert!(
            target
                .filters
                .iter()
                .any(|pattern| pattern == "crates/pika-media/**")
        );
        assert!(
            target
                .filters
                .iter()
                .any(|pattern| pattern == "crates/pika-relay-profiles/**")
        );
        assert!(
            target
                .filters
                .iter()
                .any(|pattern| pattern == "pikachat-openclaw/**")
        );
        assert!(target.filters.iter().any(|pattern| pattern == "rust/**"));
    }

    #[test]
    fn path_filters_match_any_candidate() {
        assert!(matches_any_filter(
            "crates/pika-server/src/agent_api.rs",
            &["docs/**", "crates/pika-server/**"]
        ));
        assert!(!matches_any_filter(
            "android/app/build.gradle.kts",
            &["docs/**", "crates/pika-server/**"]
        ));
    }

    #[test]
    fn rerun_uses_recorded_target_id_when_available() {
        let run = sample_run_record();
        let target = target_spec_for_rerun(&run).expect("target");
        assert_eq!(target.id, "pre-merge-agent-contracts");
    }

    #[test]
    fn rerun_falls_back_to_single_job_id() {
        let mut run = sample_run_record();
        run.target_id = None;
        run.jobs = vec![sample_job_record("pika-cloud-unit")];

        let target = target_spec_for_rerun(&run).expect("target");
        assert_eq!(target.id, "pika-cloud-unit");
    }

    #[test]
    fn status_lines_include_prepared_output_mode_observability() {
        let run = sample_run_record();
        let lines = format_status_lines(&run);
        assert!(lines.contains(&"prepared_output_consumer=fulfill_request_cli_v1".to_string()));
        assert!(lines.contains(
            &"prepared_output_mode=pre_merge_pika_rust_subprocess_fulfillment_v1".to_string()
        ));
        assert!(
            lines.contains(&"prepared_output_invocation_mode=direct_helper_exec_v1".to_string())
        );
        assert!(
            !lines
                .iter()
                .any(|line| { line.starts_with("prepared_output_invocation_wrapper_program=") })
        );
        assert!(
            !lines
                .iter()
                .any(|line| { line.starts_with("prepared_output_launcher_transport_mode=") })
        );
    }

    #[test]
    fn rerun_metadata_preserves_prepared_output_mode() {
        let mut run = sample_run_record();
        run.target_id = Some("pre-merge-pika-rust".to_string());
        run.target_description = Some("Run staged rust lane".to_string());

        let target = target_spec_for_rerun(&run).expect("target");
        let metadata = rerun_metadata(&run, &target);

        assert_eq!(
            metadata.prepared_output_mode.as_deref(),
            Some("pre_merge_pika_rust_subprocess_fulfillment_v1")
        );
        assert_eq!(
            metadata.prepared_output_invocation_mode,
            Some(jerichoci::PreparedOutputInvocationMode::DirectHelperExecV1)
        );
        assert_eq!(metadata.prepared_output_invocation_wrapper_program, None);
        assert_eq!(metadata.prepared_output_launcher_transport_mode, None);
        assert_eq!(metadata.prepared_output_launcher_transport_program, None);
    }

    #[test]
    fn rerun_metadata_preserves_wrapper_invocation_program() {
        let mut run = sample_run_record();
        run.target_id = Some("pre-merge-pika-rust".to_string());
        run.prepared_output_invocation_mode =
            Some(jerichoci::PreparedOutputInvocationMode::ExternalWrapperCommandV1);
        run.prepared_output_invocation_wrapper_program =
            Some("/tmp/bin/pikaci-wrapper".to_string());

        let target = target_spec_for_rerun(&run).expect("target");
        let metadata = rerun_metadata(&run, &target);

        assert_eq!(
            metadata.prepared_output_invocation_mode,
            Some(jerichoci::PreparedOutputInvocationMode::ExternalWrapperCommandV1)
        );
        assert_eq!(
            metadata
                .prepared_output_invocation_wrapper_program
                .as_deref(),
            Some("/tmp/bin/pikaci-wrapper")
        );
        assert_eq!(metadata.prepared_output_launcher_transport_mode, None);
    }

    #[test]
    fn rerun_metadata_preserves_launcher_transport_program() {
        let mut run = sample_run_record();
        run.target_id = Some("pre-merge-pika-rust".to_string());
        run.prepared_output_invocation_mode =
            Some(jerichoci::PreparedOutputInvocationMode::ExternalWrapperCommandV1);
        run.prepared_output_invocation_wrapper_program =
            Some("/tmp/bin/pikaci-launch-fulfill-prepared-output".to_string());
        run.prepared_output_launcher_transport_mode =
            Some(jerichoci::PreparedOutputLauncherTransportMode::CommandTransportV1);
        run.prepared_output_launcher_transport_program = Some("/tmp/bin/fake-ssh".to_string());

        let target = target_spec_for_rerun(&run).expect("target");
        let metadata = rerun_metadata(&run, &target);

        assert_eq!(
            metadata.prepared_output_launcher_transport_mode,
            Some(jerichoci::PreparedOutputLauncherTransportMode::CommandTransportV1)
        );
        assert_eq!(
            metadata
                .prepared_output_launcher_transport_program
                .as_deref(),
            Some("/tmp/bin/fake-ssh")
        );
    }

    #[test]
    fn rerun_metadata_preserves_ssh_launcher_transport_details() {
        let mut run = sample_run_record();
        run.target_id = Some("pre-merge-pika-rust".to_string());
        run.prepared_output_invocation_mode =
            Some(jerichoci::PreparedOutputInvocationMode::ExternalWrapperCommandV1);
        run.prepared_output_invocation_wrapper_program =
            Some("/tmp/bin/pikaci-launch-fulfill-prepared-output".to_string());
        run.prepared_output_launcher_transport_mode =
            Some(jerichoci::PreparedOutputLauncherTransportMode::SshLauncherTransportV1);
        run.prepared_output_launcher_transport_program = Some("/usr/bin/ssh".to_string());
        run.prepared_output_launcher_transport_host = Some("pika-build".to_string());
        run.prepared_output_launcher_transport_remote_launcher_program =
            Some("/opt/pikaci/bin/pikaci-launch-fulfill-prepared-output".to_string());
        run.prepared_output_launcher_transport_remote_helper_program =
            Some("/opt/pikaci/bin/pikaci-fulfill-prepared-output".to_string());
        run.prepared_output_launcher_transport_remote_work_dir =
            Some("/var/tmp/pikaci-remote".to_string());

        let target = target_spec_for_rerun(&run).expect("target");
        let metadata = rerun_metadata(&run, &target);

        assert_eq!(
            metadata.prepared_output_launcher_transport_mode,
            Some(jerichoci::PreparedOutputLauncherTransportMode::SshLauncherTransportV1)
        );
        assert_eq!(
            metadata
                .prepared_output_launcher_transport_program
                .as_deref(),
            Some("/usr/bin/ssh")
        );
        assert_eq!(
            metadata.prepared_output_launcher_transport_host.as_deref(),
            Some("pika-build")
        );
        assert_eq!(
            metadata
                .prepared_output_launcher_transport_remote_launcher_program
                .as_deref(),
            Some("/opt/pikaci/bin/pikaci-launch-fulfill-prepared-output")
        );
        assert_eq!(
            metadata
                .prepared_output_launcher_transport_remote_helper_program
                .as_deref(),
            Some("/opt/pikaci/bin/pikaci-fulfill-prepared-output")
        );
        assert_eq!(
            metadata
                .prepared_output_launcher_transport_remote_work_dir
                .as_deref(),
            Some("/var/tmp/pikaci-remote")
        );
    }

    #[test]
    fn status_lines_include_launcher_transport_observability() {
        let mut run = sample_run_record();
        run.prepared_output_invocation_mode =
            Some(jerichoci::PreparedOutputInvocationMode::ExternalWrapperCommandV1);
        run.prepared_output_invocation_wrapper_program =
            Some("/tmp/bin/pikaci-launch-fulfill-prepared-output".to_string());
        run.prepared_output_launcher_transport_mode =
            Some(jerichoci::PreparedOutputLauncherTransportMode::CommandTransportV1);
        run.prepared_output_launcher_transport_program = Some("/tmp/bin/fake-ssh".to_string());

        let lines = format_status_lines(&run);
        assert!(
            lines.contains(
                &"prepared_output_launcher_transport_mode=command_transport_v1".to_string()
            )
        );
        assert!(
            lines.contains(
                &"prepared_output_launcher_transport_program=/tmp/bin/fake-ssh".to_string()
            )
        );
    }

    #[test]
    fn status_lines_include_ssh_launcher_transport_details() {
        let mut run = sample_run_record();
        run.prepared_output_invocation_mode =
            Some(jerichoci::PreparedOutputInvocationMode::ExternalWrapperCommandV1);
        run.prepared_output_invocation_wrapper_program =
            Some("/tmp/bin/pikaci-launch-fulfill-prepared-output".to_string());
        run.prepared_output_launcher_transport_mode =
            Some(jerichoci::PreparedOutputLauncherTransportMode::SshLauncherTransportV1);
        run.prepared_output_launcher_transport_program = Some("/usr/bin/ssh".to_string());
        run.prepared_output_launcher_transport_host = Some("pika-build".to_string());
        run.prepared_output_launcher_transport_remote_launcher_program =
            Some("/opt/pikaci/bin/pikaci-launch-fulfill-prepared-output".to_string());
        run.prepared_output_launcher_transport_remote_helper_program =
            Some("/opt/pikaci/bin/pikaci-fulfill-prepared-output".to_string());
        run.prepared_output_launcher_transport_remote_work_dir =
            Some("/var/tmp/pikaci-remote".to_string());

        let lines = format_status_lines(&run);
        assert!(lines.contains(
            &"prepared_output_launcher_transport_mode=ssh_launcher_transport_v1".to_string()
        ));
        assert!(
            lines.contains(&"prepared_output_launcher_transport_program=/usr/bin/ssh".to_string())
        );
        assert!(lines.contains(&"prepared_output_launcher_transport_host=pika-build".to_string()));
        assert!(lines.contains(
            &"prepared_output_launcher_transport_remote_launcher_program=/opt/pikaci/bin/pikaci-launch-fulfill-prepared-output".to_string()
        ));
        assert!(lines.contains(
            &"prepared_output_launcher_transport_remote_helper_program=/opt/pikaci/bin/pikaci-fulfill-prepared-output".to_string()
        ));
        assert!(
            lines.contains(
                &"prepared_output_launcher_transport_remote_work_dir=/var/tmp/pikaci-remote"
                    .to_string()
            )
        );
    }

    #[test]
    fn staged_linux_remote_defaults_json_includes_store_uri() {
        let payload = serde_json::to_value(staged_linux_remote_defaults_json(
            staged_linux_remote_defaults(),
        ))
        .expect("encode defaults json");
        assert_eq!(payload["ssh_host"], "pika-build");
        assert_eq!(payload["store_uri"], "ssh://pika-build");
    }

    #[test]
    fn run_lifecycle_event_serializes_run_started_with_immediate_run_id() {
        let payload = serde_json::to_value(RunLifecycleEvent::RunStarted {
            run_id: "20260319T010203Z-deadbeef".to_string(),
            created_at: "2026-03-19T01:02:03Z".to_string(),
            rerun_of: Some("20260318T235959Z-abcdef12".to_string()),
            target_id: Some("pre-merge-pika-rust".to_string()),
            target_description: Some("Run staged rust lane".to_string()),
        })
        .expect("encode run started event");
        assert_eq!(payload["event"], "run_started");
        assert_eq!(payload["run_id"], "20260319T010203Z-deadbeef");
        assert_eq!(payload["target_id"], "pre-merge-pika-rust");
    }

    #[test]
    fn staged_linux_target_config_serializes_machine_readably() {
        let payload = serde_json::to_value(PikaStagedLinuxTarget::PreMergePikaRust.config())
            .expect("encode target config");
        assert_eq!(payload["target_id"], "pre-merge-pika-rust");
        assert_eq!(
            payload["payload_specs"][0]["nix_installable"],
            ".#ci.x86_64-linux.workspaceDeps"
        );
        assert_eq!(
            payload["payload_specs"][1]["nix_installable"],
            ".#ci.x86_64-linux.workspaceBuild"
        );
    }

    fn sample_run_record() -> RunRecord {
        RunRecord {
            run_id: "run-1".to_string(),
            status: RunStatus::Passed,
            rerun_of: None,
            target_id: Some("pre-merge-agent-contracts".to_string()),
            target_description: Some("test".to_string()),
            source_root: "/tmp/source".to_string(),
            snapshot_dir: "/tmp/snapshot".to_string(),
            git_head: Some("deadbeef".to_string()),
            git_dirty: Some(true),
            created_at: "2026-03-07T00:00:00Z".to_string(),
            finished_at: Some("2026-03-07T00:00:01Z".to_string()),
            plan_path: None,
            prepared_outputs_path: None,
            prepared_output_consumer: Some(PreparedOutputConsumerKind::FulfillRequestCliV1),
            prepared_output_mode: Some("pre_merge_pika_rust_subprocess_fulfillment_v1".to_string()),
            prepared_output_invocation_mode: Some(
                jerichoci::PreparedOutputInvocationMode::DirectHelperExecV1,
            ),
            prepared_output_invocation_wrapper_program: None,
            prepared_output_launcher_transport_mode: None,
            prepared_output_launcher_transport_program: None,
            prepared_output_launcher_transport_host: None,
            prepared_output_launcher_transport_remote_launcher_program: None,
            prepared_output_launcher_transport_remote_helper_program: None,
            prepared_output_launcher_transport_remote_work_dir: None,
            changed_files: vec![],
            filters: vec![],
            message: None,
            prepare_timings: vec![],
            jobs: vec![],
        }
    }

    fn sample_job_record(id: &str) -> JobRecord {
        JobRecord {
            id: id.to_string(),
            description: "job".to_string(),
            status: RunStatus::Passed,
            executor: "remote_linux_vm".to_string(),
            placement: Some(JobPlacementKind::RemoteSsh),
            runtime: Some(JobRuntimeKind::Incus),
            plan_node_id: None,
            timeout_secs: 1,
            host_log_path: "/tmp/host.log".to_string(),
            guest_log_path: "/tmp/guest.log".to_string(),
            started_at: "2026-03-07T00:00:00Z".to_string(),
            finished_at: Some("2026-03-07T00:00:01Z".to_string()),
            exit_code: Some(0),
            message: Some("ok".to_string()),
            pre_execution_prepare_duration_ms: None,
            remote_linux_vm_execution: None,
        }
    }

    #[test]
    fn cli_parses_prepared_outputs_subcommand() {
        let cli = Cli::try_parse_from(["pikaci", "prepared-outputs", "run-123", "--json"])
            .expect("parse prepared outputs cli");

        match cli.command {
            Command::PreparedOutputs { run_id, json } => {
                assert_eq!(run_id, "run-123");
                assert!(json);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
