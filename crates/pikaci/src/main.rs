use anyhow::{Context, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use pikaci::{
    GuestCommand, JobSpec, LogKind, RunLifecycleEvent, RunMetadata, RunOptions, RunRecord,
    RunStatus, StagedLinuxRemoteDefaults, StagedLinuxRustLane, StagedLinuxRustTarget,
    fulfill_prepared_output_request, gc_runs, git_changed_files, list_runs, load_logs,
    load_logs_metadata, load_prepared_outputs_record, load_run_record,
    record_skipped_run_with_reporter, rerun_jobs_with_metadata_and_reporter,
    run_jobs_with_metadata_and_reporter, staged_linux_remote_defaults,
};
use std::path::PathBuf;

struct TargetSpec {
    id: &'static str,
    description: &'static str,
    filters: &'static [&'static str],
    jobs: Vec<JobSpec>,
}

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
                print_json(&config)?;
            } else {
                println!("target_id={}", config.target_id);
                println!(
                    "target_description={}",
                    shell_escape(config.target_description)
                );
                println!(
                    "workspace_deps_installable={}",
                    config.workspace_deps_installable
                );
                println!(
                    "workspace_build_installable={}",
                    config.workspace_build_installable
                );
                println!("shadow_recipe={}", shell_escape(config.shadow_recipe));
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

fn print_json(value: &impl serde::Serialize) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut locked = stdout.lock();
    serde_json::to_writer_pretty(&mut locked, value).context("encode json output")?;
    use std::io::Write as _;
    writeln!(&mut locked).context("terminate json output")?;
    Ok(())
}

fn print_json_line(value: &impl serde::Serialize) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut locked = stdout.lock();
    serde_json::to_writer(&mut locked, value).context("encode json line")?;
    use std::io::Write as _;
    writeln!(&mut locked).context("terminate json line")?;
    Ok(())
}

fn print_run_human_summary(run: &RunRecord) {
    if run.jobs.is_empty() {
        let target_id = run.target_id.as_deref().unwrap_or("-");
        println!("{} {} {}", run.run_id, target_id, status_text(run.status));
        if let Some(message) = &run.message {
            eprintln!("{message}");
        }
    } else {
        for job in &run.jobs {
            println!("{} {} {}", run.run_id, job.id, status_text(job.status));
            if let Some(message) = &job.message {
                eprintln!("{message}");
            }
        }
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
    previous: &pikaci::RunRecord,
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

fn staged_linux_target(target_id: &str) -> anyhow::Result<StagedLinuxRustTarget> {
    StagedLinuxRustTarget::from_target_id(target_id)
        .ok_or_else(|| anyhow::anyhow!("unsupported staged Linux Rust target `{target_id}`"))
}

fn staged_linux_target_spec(
    target: StagedLinuxRustTarget,
    filters: &'static [&'static str],
    jobs: Vec<JobSpec>,
) -> TargetSpec {
    let config = target.config();
    TargetSpec {
        id: config.target_id,
        description: config.target_description,
        filters,
        jobs,
    }
}

fn single_job_target_spec(
    id: &'static str,
    description: &'static str,
    filters: &'static [&'static str],
    jobs: Vec<JobSpec>,
) -> anyhow::Result<TargetSpec> {
    let job = jobs
        .into_iter()
        .find(|job| job.id == id)
        .ok_or_else(|| anyhow!("missing single-job target `{id}`"))?;
    Ok(TargetSpec {
        id,
        description,
        filters,
        jobs: vec![job],
    })
}

fn target_spec(name: &str) -> anyhow::Result<TargetSpec> {
    match name {
        "tart-beachhead" => Ok(TargetSpec {
            id: "tart-beachhead",
            description: "Run one tiny iOS unit test in a Tart macOS guest",
            filters: &[],
            jobs: vec![tart_agent_button_job(
                "tart-beachhead",
                "Run one tiny iOS unit test in a Tart macOS guest",
            )],
        }),
        "agent-control-plane-unit" => single_job_target_spec(
            "agent-control-plane-unit",
            "Run all pika-cloud unit tests in a remote Linux VM",
            &[],
            agent_contract_jobs(),
        ),
        "server-agent-api-tests" => single_job_target_spec(
            "server-agent-api-tests",
            "Run pika-server agent_api tests in a remote Linux VM",
            &[],
            agent_contract_jobs(),
        ),
        "core-agent-nip98-test" => single_job_target_spec(
            "core-agent-nip98-test",
            "Run pika_core NIP-98 signing contract test in a remote Linux VM",
            &[],
            agent_contract_jobs(),
        ),
        "agent-contracts-smoke" => Ok(TargetSpec {
            id: "agent-contracts-smoke",
            description: "Run the VM-backed agent contracts smoke lane",
            filters: &[],
            jobs: agent_contract_jobs(),
        }),
        "pre-merge-agent-contracts" => Ok(staged_linux_target_spec(
            StagedLinuxRustTarget::PreMergeAgentContracts,
            &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                ".github/workflows/pre-merge.yml",
                "docs/agent-ci.md",
                "crates/pikaci/**",
                "crates/pika-cloud/**",
                "crates/pika-server/**",
                "crates/pika-media/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-test-utils/**",
                "crates/pika-tls/**",
                "rust/**",
            ],
            agent_contract_jobs(),
        )),
        "pre-merge-pika-rust" => Ok(staged_linux_target_spec(
            StagedLinuxRustTarget::PreMergePikaRust,
            &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                ".github/workflows/pre-merge.yml",
                "crates/pikaci/**",
                "rust/**",
                "crates/pika-media/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-tls/**",
            ],
            pika_rust_jobs(),
        )),
        "pre-merge-notifications" => Ok(staged_linux_target_spec(
            StagedLinuxRustTarget::PreMergeNotifications,
            &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                ".github/workflows/pre-merge.yml",
                "crates/pikaci/**",
                "crates/pika-cloud/**",
                "crates/pika-desktop/**",
                "crates/pika-server/**",
                "crates/pikahut/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-test-utils/**",
                "crates/pika-tls/**",
            ],
            notification_jobs(),
        )),
        "pre-merge-pikachat-rust" => Ok(staged_linux_target_spec(
            StagedLinuxRustTarget::PreMergePikachatRust,
            &[
                "Cargo.toml",
                "Cargo.lock",
                "cmd/pika-relay/**",
                "cli/**",
                "flake.nix",
                "flake.lock",
                "fixtures/**",
                "nix/**",
                "justfile",
                ".github/workflows/pre-merge.yml",
                "pikachat-openclaw/**",
                "crates/pikaci/**",
                "crates/pika-cloud/**",
                "crates/pika-desktop/**",
                "crates/hypernote-protocol/**",
                "crates/pikachat-sidecar/**",
                "crates/pikahut/**",
                "crates/pika-agent-protocol/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-media/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-test-utils/**",
                "crates/pika-tls/**",
                "rust/**",
                "tests/**",
            ],
            pikachat_rust_jobs(),
        )),
        "pre-merge-pikachat-typescript" => Ok(staged_linux_target_spec(
            StagedLinuxRustTarget::PreMergePikachatTypescript,
            &[
                ".github/workflows/pre-merge.yml",
                "ci/forge-lanes.toml",
                "crates/pikaci/**",
                "nix/ci/linux-rust.nix",
                "scripts/forge-github-ci-shim.py",
                "scripts/pikaci-staged-linux-remote.sh",
                "scripts/pikachat-typescript-ci.sh",
                "pikachat-claude/package.json",
                "pikachat-claude/tsconfig.json",
                "pikachat-claude/src/**",
                "pikachat-openclaw/openclaw/extensions/pikachat-openclaw/package.json",
                "pikachat-openclaw/openclaw/extensions/pikachat-openclaw/tsconfig.json",
                "pikachat-openclaw/openclaw/extensions/pikachat-openclaw/index.ts",
                "pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/**",
            ],
            pikachat_typescript_jobs(),
        )),
        "pre-merge-pikachat-openclaw-e2e" => Ok(staged_linux_target_spec(
            StagedLinuxRustTarget::PreMergePikachatOpenclawE2e,
            &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "fixtures/**",
                "nix/**",
                "justfile",
                ".github/workflows/pre-merge.yml",
                "cli/**",
                "crates/pikaci/**",
                "crates/pikahut/**",
                "crates/pikachat-sidecar/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-media/**",
                "crates/pika-tls/**",
                "tests/**",
                "tools/**",
                "pikachat-openclaw/**",
                "scripts/pikaci-staged-linux-remote.sh",
                "scripts/pika-build-prewarm-workspace-deps.sh",
                "scripts/pika-build-run-workspace-deps.sh",
                "scripts/lib/pikaci-tools.sh",
            ],
            pikachat_openclaw_e2e_jobs(),
        )),
        "pre-merge-pika-followup" => Ok(staged_linux_target_spec(
            StagedLinuxRustTarget::PreMergePikaFollowup,
            &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                "just/**",
                ".github/actionlint.yaml",
                ".github/workflows/pre-merge.yml",
                "android/**",
                "docs/**",
                "scripts/**",
                "cli/**",
                "crates/pikaci/**",
                "crates/pika-desktop/**",
                "crates/pika-media/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-tls/**",
                "crates/pikahut/**",
                "crates/pikachat-sidecar/**",
                "rust/**",
                "uniffi-bindgen/**",
            ],
            pika_followup_jobs(),
        )),
        "pika-actionlint" => single_job_target_spec(
            "pika-actionlint",
            "Run the staged Pika actionlint follow-up lane",
            &[],
            pika_followup_jobs(),
        ),
        "pika-doc-contracts" => single_job_target_spec(
            "pika-doc-contracts",
            "Run the staged Pika docs/contracts follow-up lane",
            &[],
            pika_followup_jobs(),
        ),
        "pika-rust-deps-hygiene" => single_job_target_spec(
            "pika-rust-deps-hygiene",
            "Run the staged Pika rust-deps-hygiene follow-up lane",
            &[],
            pika_followup_jobs(),
        ),
        "pika-core-lib-app-flows-tests" => single_job_target_spec(
            "pika-core-lib-app-flows-tests",
            "Run the staged pika_core lib/app_flows lane",
            &[],
            pika_rust_jobs(),
        ),
        "pika-server-package-tests" => single_job_target_spec(
            "pika-server-package-tests",
            "Run the staged notifications pika-server package-tests lane",
            &[],
            notification_jobs(),
        ),
        "pikachat-package-tests" => single_job_target_spec(
            "pikachat-package-tests",
            "Run the staged pikachat package-tests lane",
            &[],
            pikachat_rust_jobs(),
        ),
        "pikahut-clippy" => single_job_target_spec(
            "pikahut-clippy",
            "Run the staged fixture pikahut clippy lane",
            &[],
            fixture_rust_jobs(),
        ),
        "pre-merge-pikachat-apple-followup" => Ok(TargetSpec {
            id: "pre-merge-pikachat-apple-followup",
            description: "Run the Apple-host pikachat follow-up after the staged Linux Rust lane",
            filters: &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                "just/checks.just",
                "crates/pikaci/**",
                "cli/**",
                "crates/pika-cloud/**",
                "crates/pika-agent-protocol/**",
                "crates/pika-desktop/**",
                "crates/hypernote-protocol/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-media/**",
                "crates/pika-relay-profiles/**",
                "crates/pikachat-sidecar/**",
                "crates/pikahut/**",
                "pikachat-openclaw/**",
                "rust/**",
            ],
            jobs: pikachat_apple_followup_jobs(),
        }),
        "pika-desktop-package-tests" => single_job_target_spec(
            "pika-desktop-package-tests",
            "Run pika-desktop package tests in a remote Linux VM",
            &[],
            pikachat_rust_jobs(),
        ),
        "pre-merge-fixture-rust" => Ok(staged_linux_target_spec(
            StagedLinuxRustTarget::PreMergeFixtureRust,
            &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                "just/agent.just",
                "just/checks.just",
                "just/infra.just",
                ".github/workflows/pre-merge.yml",
                "docs/testing/ci-selectors.md",
                "docs/testing/integration-matrix.md",
                "docs/testing/wrapper-deprecation-policy.md",
                "cli/Cargo.toml",
                "crates/pikaci/**",
                "crates/hypernote-protocol/**",
                "crates/pika-cloud/**",
                "crates/pika-desktop/**",
                "crates/pikahut/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-media/**",
                "crates/pikachat-sidecar/Cargo.toml",
                "crates/pika-relay-profiles/**",
                "crates/pika-server/Cargo.toml",
                "crates/pika-tls/**",
                "pikachat-openclaw/scripts/run-openclaw-e2e.sh",
                "pikachat-openclaw/scripts/run-scenario.sh",
                "cmd/pika-relay/**",
                "rust/**",
                "rust/tests/e2e_calls.rs",
                "scripts/agent-chat-demo.sh",
                "scripts/agent-demo.sh",
                "tools/cli-smoke",
                "tools/interop-rust-baseline",
                "tools/primal-ios-interop-nightly",
                "tools/ui-e2e-local",
                "tools/ui-e2e-public",
            ],
            fixture_rust_jobs(),
        )),
        "android-sdk-probe" => Ok(TargetSpec {
            id: "android-sdk-probe",
            description: "Verify Android SDK tooling is available inside a Linux guest",
            filters: &[],
            jobs: vec![JobSpec {
                id: "android-sdk-probe",
                description: "Verify Android tooling and AVD provisioning in a Linux guest",
                timeout_secs: 3600,
                writable_workspace: true,
                guest_command: GuestCommand::ShellCommandAsRoot {
                    command: concat!(
                        "set -euo pipefail; ",
                        "test -n \"${ANDROID_HOME:-}\" || { ",
                        "echo 'error: Android SDK is unavailable in this guest; the current flake does not export androidSdk for aarch64-linux.'; ",
                        "exit 2; ",
                        "}; ",
                        "test -n \"${JAVA_HOME:-}\" || { ",
                        "echo 'error: JAVA_HOME is unavailable in this guest because the Android toolchain bundle is missing.'; ",
                        "exit 2; ",
                        "}; ",
                        "echo ANDROID_HOME=$ANDROID_HOME; ",
                        "echo JAVA_HOME=$JAVA_HOME; ",
                        "command -v java; ",
                        "command -v adb; ",
                        "command -v emulator; ",
                        "command -v avdmanager; ",
                        "command -v gradle; ",
                        "cargo ndk --help >/dev/null; ",
                        "rustc --print target-list | grep -qx aarch64-linux-android; ",
                        "./tools/android-avd-ensure >/dev/null; ",
                        "emulator -list-avds",
                    ),
                },
                staged_linux_rust_lane: None,
            }],
        }),
        "android-nostr-connect-intent-test" => Ok(TargetSpec {
            id: "android-nostr-connect-intent-test",
            description: "Run the smallest Android instrumentation class in a Linux guest",
            filters: &[],
            jobs: vec![JobSpec {
                id: "android-nostr-connect-intent-test",
                description: "Run NostrConnectIntentTest on a headless Android emulator in a Linux guest",
                timeout_secs: 7200,
                writable_workspace: true,
                guest_command: GuestCommand::ShellCommandAsRoot {
                    command: concat!(
                        "set -euo pipefail; ",
                        "test -n \"${ANDROID_HOME:-}\" || { ",
                        "echo 'error: Android SDK is unavailable in this guest; the current flake does not export androidSdk for aarch64-linux.'; ",
                        "exit 2; ",
                        "}; ",
                        "test -n \"${JAVA_HOME:-}\" || { ",
                        "echo 'error: JAVA_HOME is unavailable in this guest because the Android toolchain bundle is missing.'; ",
                        "exit 2; ",
                        "}; ",
                        "export PIKA_RUST_PROFILE=debug; ",
                        "export PIKA_ANDROID_ABI=arm64-v8a; ",
                        "export PIKA_ANDROID_TEST_CLASS=com.pika.app.NostrConnectIntentTest; ",
                        "export PIKA_ANDROID_FORCE_GUI=0; ",
                        "export PIKA_ANDROID_EMULATOR_ARGS='-no-window -no-boot-anim -accel off'; ",
                        "./tools/android-ui-test-ci",
                    ),
                },
                staged_linux_rust_lane: None,
            }],
        }),
        "tart-env-probe" => Ok(TargetSpec {
            id: "tart-env-probe",
            description: "Probe the local Tart macOS guest environment",
            filters: &[],
            jobs: vec![JobSpec {
                id: "tart-env-probe",
                description: "Verify that a Tart macOS guest has the tools needed for iOS/macOS tests",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ShellCommand {
                    command: concat!(
                        "set -euo pipefail; ",
                        "sw_vers; ",
                        "xcodebuild -version; ",
                        "cargo --version; ",
                        "rustc --version; ",
                        "python3 --version; ",
                        "./tools/ios-sim-ensure",
                    ),
                },
                staged_linux_rust_lane: None,
            }],
        }),
        "tart-ios-agent-button-state-test" => Ok(TargetSpec {
            id: "tart-ios-agent-button-state-test",
            description: "Run one iOS unit test in a Tart macOS guest",
            filters: &[],
            jobs: vec![tart_agent_button_job(
                "tart-ios-agent-button-state-test",
                "Run AgentTests.testAgentButtonStateReflectsBusyFlag in a Tart macOS guest",
            )],
        }),
        "tart-ios-unit-tests" => Ok(TargetSpec {
            id: "tart-ios-unit-tests",
            description: "Run deterministic iOS unit-test suites in a Tart macOS guest",
            filters: &[],
            jobs: tart_ios_unit_jobs(),
        }),
        "tart-ios-ui-test" => Ok(TargetSpec {
            id: "tart-ios-ui-test",
            description: "Run the deterministic ios-ui-test lane in a Tart macOS guest",
            filters: &[],
            jobs: vec![tart_ios_ui_test_job(
                "tart-ios-ui-test",
                "Run the deterministic ios-ui-test lane in a Tart macOS guest",
            )],
        }),
        "tart-ios-ui-note-to-self" => Ok(TargetSpec {
            id: "tart-ios-ui-note-to-self",
            description: "Run one deterministic iOS UI test in a Tart macOS guest",
            filters: &[],
            jobs: vec![tart_ios_ui_note_to_self_job(
                "tart-ios-ui-note-to-self",
                "Run the note-to-self UI test in a Tart macOS guest",
            )],
        }),
        "tart-desktop-package-tests" => Ok(TargetSpec {
            id: "tart-desktop-package-tests",
            description: "Run pika-desktop package tests in a Tart macOS guest",
            filters: &[],
            jobs: vec![tart_desktop_package_tests_job(
                "tart-desktop-package-tests",
                "Run pika-desktop package tests in a Tart macOS guest",
            )],
        }),
        "pre-merge-apple-deterministic" => Ok(TargetSpec {
            id: "pre-merge-apple-deterministic",
            description: "Run deterministic Apple-platform tests in a Tart macOS guest",
            filters: &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                ".github/workflows/pre-merge.yml",
                "crates/pikaci/**",
                "crates/pika-desktop/**",
                "ios/**",
                "rust/**",
                "tools/cargo-with-xcode",
                "tools/ios-sim-ensure",
                "tools/xcode-dev-dir",
                "tools/xcode-run",
                "tools/xcodebuild-compact",
            ],
            jobs: {
                let mut jobs = tart_ios_unit_jobs();
                jobs.push(tart_ios_ui_test_job(
                    "tart-ios-ui-test",
                    "Run the deterministic ios-ui-test lane in a Tart macOS guest",
                ));
                jobs.push(tart_desktop_package_tests_job(
                    "tart-desktop-package-tests",
                    "Run pika-desktop package tests in a Tart macOS guest",
                ));
                jobs
            },
        }),
        "pre-merge-rmp" => Ok(staged_linux_target_spec(
            StagedLinuxRustTarget::PreMergeRmp,
            &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                ".github/workflows/pre-merge.yml",
                "crates/pikaci/**",
                "crates/rmp-cli/**",
                "crates/pika-relay-profiles/**",
            ],
            rmp_jobs(),
        )),
        "rmp-init-smoke-ci" => single_job_target_spec(
            "rmp-init-smoke-ci",
            "Run the staged RMP init smoke lane",
            &[],
            rmp_jobs(),
        ),
        other => bail!("unknown job `{other}`"),
    }
}

fn format_status_lines(run: &RunRecord) -> Vec<String> {
    let mut lines = vec![format!(
        "{} {} {}",
        run.run_id,
        run.target_id.as_deref().unwrap_or("-"),
        status_text(run.status)
    )];
    if let Some(plan_path) = &run.plan_path {
        lines.push(format!("plan={plan_path}"));
    }
    if let Some(prepared_outputs_path) = &run.prepared_outputs_path {
        lines.push(format!("prepared_outputs={prepared_outputs_path}"));
    }
    if let Some(prepared_output_consumer) = run.prepared_output_consumer {
        lines.push(format!(
            "prepared_output_consumer={}",
            prepared_output_consumer_label(prepared_output_consumer)
        ));
    }
    if let Some(prepared_output_mode) = &run.prepared_output_mode {
        lines.push(format!("prepared_output_mode={prepared_output_mode}"));
    }
    if let Some(prepared_output_invocation_mode) = run.prepared_output_invocation_mode {
        lines.push(format!(
            "prepared_output_invocation_mode={}",
            prepared_output_invocation_mode_label(prepared_output_invocation_mode)
        ));
    }
    if let Some(prepared_output_invocation_wrapper_program) =
        &run.prepared_output_invocation_wrapper_program
    {
        lines.push(format!(
            "prepared_output_invocation_launcher_program={prepared_output_invocation_wrapper_program}"
        ));
    }
    if let Some(prepared_output_launcher_transport_mode) =
        run.prepared_output_launcher_transport_mode
    {
        lines.push(format!(
            "prepared_output_launcher_transport_mode={}",
            prepared_output_launcher_transport_mode_label(prepared_output_launcher_transport_mode)
        ));
    }
    if let Some(prepared_output_launcher_transport_program) =
        &run.prepared_output_launcher_transport_program
    {
        lines.push(format!(
            "prepared_output_launcher_transport_program={prepared_output_launcher_transport_program}"
        ));
    }
    if let Some(prepared_output_launcher_transport_host) =
        &run.prepared_output_launcher_transport_host
    {
        lines.push(format!(
            "prepared_output_launcher_transport_host={prepared_output_launcher_transport_host}"
        ));
    }
    if let Some(prepared_output_launcher_transport_remote_launcher_program) =
        &run.prepared_output_launcher_transport_remote_launcher_program
    {
        lines.push(format!(
            "prepared_output_launcher_transport_remote_launcher_program={prepared_output_launcher_transport_remote_launcher_program}"
        ));
    }
    if let Some(prepared_output_launcher_transport_remote_helper_program) =
        &run.prepared_output_launcher_transport_remote_helper_program
    {
        lines.push(format!(
            "prepared_output_launcher_transport_remote_helper_program={prepared_output_launcher_transport_remote_helper_program}"
        ));
    }
    if let Some(prepared_output_launcher_transport_remote_work_dir) =
        &run.prepared_output_launcher_transport_remote_work_dir
    {
        lines.push(format!(
            "prepared_output_launcher_transport_remote_work_dir={prepared_output_launcher_transport_remote_work_dir}"
        ));
    }
    if let Some(message) = &run.message {
        lines.push(message.clone());
    }
    if !run.changed_files.is_empty() {
        lines.push(format!("changed_files={}", run.changed_files.len()));
    }
    for job in &run.jobs {
        lines.push(format!("{} {}", job.id, status_text(job.status)));
    }
    lines
}

fn prepared_output_consumer_label(kind: pikaci::PreparedOutputConsumerKind) -> &'static str {
    match kind {
        pikaci::PreparedOutputConsumerKind::HostLocalSymlinkMountsV1 => {
            "host_local_symlink_mounts_v1"
        }
        pikaci::PreparedOutputConsumerKind::RemoteExposureRequestV1 => "remote_exposure_request_v1",
        pikaci::PreparedOutputConsumerKind::FulfillRequestCliV1 => "fulfill_request_cli_v1",
    }
}

fn prepared_output_invocation_mode_label(
    mode: pikaci::PreparedOutputInvocationMode,
) -> &'static str {
    match mode {
        pikaci::PreparedOutputInvocationMode::DirectHelperExecV1 => "direct_helper_exec_v1",
        pikaci::PreparedOutputInvocationMode::ExternalWrapperCommandV1 => {
            "external_wrapper_command_v1"
        }
    }
}

fn prepared_output_launcher_transport_mode_label(
    mode: pikaci::PreparedOutputLauncherTransportMode,
) -> &'static str {
    match mode {
        pikaci::PreparedOutputLauncherTransportMode::DirectLauncherExecV1 => {
            "direct_launcher_exec_v1"
        }
        pikaci::PreparedOutputLauncherTransportMode::CommandTransportV1 => "command_transport_v1",
        pikaci::PreparedOutputLauncherTransportMode::SshLauncherTransportV1 => {
            "ssh_launcher_transport_v1"
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

fn agent_contract_jobs() -> Vec<JobSpec> {
    vec![
        JobSpec {
            id: "agent-control-plane-unit",
            description: "Run all pika-cloud unit tests in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageUnitTests {
                package: "pika-cloud",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::AgentContractsControlPlaneUnit),
        },
        JobSpec {
            id: "server-agent-api-tests",
            description: "Run pika-server agent_api tests in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::FilteredCargoTests {
                package: "pika-server",
                filter: "agent_api::tests",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::AgentContractsServerAgentApi),
        },
        JobSpec {
            id: "core-agent-nip98-test",
            description: "Run pika_core NIP-98 signing contract test in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ExactCargoTest {
                package: "pika_core",
                test_name: "core::agent::tests::run_agent_flow_signs_requests_with_nip98_authorization",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::AgentContractsCoreNip98),
        },
    ]
}

fn pika_rust_jobs() -> Vec<JobSpec> {
    vec![
        JobSpec {
            id: "pika-core-lib-app-flows-tests",
            description: "Run pika_core lib tests and app_flows integration tests in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pika_core --lib --test app_flows -- --nocapture",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikaCoreLibAppFlows),
        },
        JobSpec {
            id: "pika-core-messaging-e2e-tests",
            description: "Run pika_core messaging and group profile integration tests in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pika_core --test e2e_messaging --test e2e_group_profiles -- --nocapture",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikaCoreMessagingE2e),
        },
    ]
}

fn notification_jobs() -> Vec<JobSpec> {
    vec![JobSpec {
        id: "pika-server-package-tests",
        description: "Run pika-server package tests with a pikahut postgres fixture in a remote Linux VM",
        timeout_secs: 1800,
        writable_workspace: false,
        guest_command: GuestCommand::ShellCommand {
            command: concat!(
                "set -euo pipefail; ",
                "SD=$(mktemp -d /tmp/pikahut-notifications.XXXXXX); ",
                "cleanup() { cargo run -q -p pikahut -- down --state-dir \"$SD\" 2>/dev/null || true; rm -rf \"$SD\"; }; ",
                "trap cleanup EXIT; ",
                "cargo run -q -p pikahut -- up --profile postgres --background --state-dir \"$SD\" >/dev/null; ",
                "eval \"$(cargo run -q -p pikahut -- env --state-dir \"$SD\")\"; ",
                "cargo test -p pika-server -- --test-threads=1 --nocapture"
            ),
        },
        staged_linux_rust_lane: Some(StagedLinuxRustLane::NotificationsServerPackageTests),
    }]
}

fn pikachat_rust_jobs() -> Vec<JobSpec> {
    vec![
        JobSpec {
            id: "pikachat-package-tests",
            description: "Run pikachat package tests in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageTests {
                package: "pikachat",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikachatPackageTests),
        },
        JobSpec {
            id: "pikachat-sidecar-package-tests",
            description: "Run pikachat-sidecar package tests with deterministic TTS in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "PIKACHAT_TTS_FIXTURE=1 cargo test -p pikachat-sidecar -- --nocapture",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikachatSidecarPackageTests),
        },
        JobSpec {
            id: "pika-desktop-package-tests",
            description: "Run pika-desktop package tests in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageTests {
                package: "pika-desktop",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikachatDesktopPackageTests),
        },
        JobSpec {
            id: "pikachat-cli-smoke-local",
            description: "Run the pikahut cli_smoke_local deterministic test in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikachatCliSmokeLocal),
        },
        JobSpec {
            id: "pikachat-post-rebase-invalid-event",
            description: "Run the post_rebase_invalid_event_rejection boundary test in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic post_rebase_invalid_event_rejection_boundary -- --ignored --nocapture",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikachatPostRebaseInvalidEvent),
        },
        JobSpec {
            id: "pikachat-post-rebase-logout-session",
            description: "Run the post_rebase_logout_session_convergence boundary test in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic post_rebase_logout_session_convergence_boundary -- --ignored --nocapture",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikachatPostRebaseLogoutSession),
        },
        JobSpec {
            id: "openclaw-invite-and-chat",
            description: "Run the OpenClaw invite-and-chat scenario in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat -- --ignored --nocapture",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::OpenclawInviteAndChat),
        },
        JobSpec {
            id: "openclaw-invite-and-chat-rust-bot",
            description: "Run the OpenClaw invite-and-chat rust bot scenario in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_rust_bot -- --ignored --nocapture",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::OpenclawInviteAndChatRustBot),
        },
        JobSpec {
            id: "openclaw-invite-and-chat-daemon",
            description: "Run the OpenClaw invite-and-chat daemon scenario in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_daemon -- --ignored --nocapture",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::OpenclawInviteAndChatDaemon),
        },
        JobSpec {
            id: "openclaw-audio-echo",
            description: "Run the OpenClaw audio echo scenario in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic openclaw_scenario_audio_echo -- --ignored --nocapture",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::OpenclawAudioEcho),
        },
    ]
}

fn pika_followup_jobs() -> Vec<JobSpec> {
    vec![
        JobSpec {
            id: "pika-android-test-compile",
            description: "Compile Android instrumentation test Kotlin for the Pika app in a remote Linux VM guest",
            timeout_secs: 3600,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cd android && ./gradlew :app:compileDebugAndroidTestKotlin",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikaFollowupAndroidTestCompile),
        },
        JobSpec {
            id: "pikachat-build",
            description: "Build pikachat in a remote Linux VM guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo build -p pikachat",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikaFollowupPikachatBuild),
        },
        JobSpec {
            id: "pika-desktop-check",
            description: "Run desktop-check in a remote Linux VM guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "just desktop-check",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikaFollowupDesktopCheck),
        },
        JobSpec {
            id: "pika-actionlint",
            description: "Run actionlint in a remote Linux VM guest",
            timeout_secs: 600,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "actionlint",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikaFollowupActionlint),
        },
        JobSpec {
            id: "pika-doc-contracts",
            description: "Run docs and justfile contract checks in a remote Linux VM guest",
            timeout_secs: 900,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "npx --yes @justinmoon/agent-tools check-docs && npx --yes @justinmoon/agent-tools check-justfile",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikaFollowupDocContracts),
        },
        JobSpec {
            id: "pika-rust-deps-hygiene",
            description: "Run cargo machete dependency hygiene in a remote Linux VM guest",
            timeout_secs: 900,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "./scripts/check-cargo-machete",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikaFollowupRustDepsHygiene),
        },
    ]
}

fn pikachat_openclaw_e2e_jobs() -> Vec<JobSpec> {
    vec![JobSpec {
        id: "pikachat-openclaw-gateway-e2e",
        description: "Run the heavy OpenClaw gateway end-to-end scenario in a remote Linux VM",
        timeout_secs: 3600,
        writable_workspace: false,
        guest_command: GuestCommand::ShellCommand { command: "ignored" },
        staged_linux_rust_lane: Some(StagedLinuxRustLane::OpenclawGatewayE2e),
    }]
}

fn pikachat_typescript_jobs() -> Vec<JobSpec> {
    vec![JobSpec {
        id: "pikachat-typescript",
        description: "Run the Claude and OpenClaw TypeScript typecheck and unit tests in a remote Linux VM",
        timeout_secs: 1800,
        writable_workspace: false,
        guest_command: GuestCommand::ShellCommand { command: "ignored" },
        staged_linux_rust_lane: Some(StagedLinuxRustLane::PikachatTypescript),
    }]
}

fn fixture_rust_jobs() -> Vec<JobSpec> {
    vec![
        JobSpec {
            id: "pikahut-clippy",
            description: "Run pikahut clippy checks in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand { command: "ignored" },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::FixturePikahutClippy),
        },
        JobSpec {
            id: "fixture-relay-smoke",
            description: "Exercise the relay-profile pikahut smoke flow in a remote Linux VM",
            timeout_secs: 300,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand { command: "ignored" },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::FixtureRelaySmoke),
        },
    ]
}

fn pikachat_apple_followup_jobs() -> Vec<JobSpec> {
    vec![
        JobSpec {
            id: "pikachat-clippy",
            description: "Run pikachat clippy on the Apple host",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::HostShellCommand {
                command: concat!(
                    "./scripts/apple-host-record-phase ",
                    "pikachat-followup.pikachat-clippy rust-live-clippy -- ",
                    "cargo clippy -p pikachat -- -D warnings"
                ),
            },
            staged_linux_rust_lane: None,
        },
        JobSpec {
            id: "pikachat-sidecar-clippy",
            description: "Run pikachat-sidecar clippy on the Apple host",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::HostShellCommand {
                command: concat!(
                    "./scripts/apple-host-record-phase ",
                    "pikachat-followup.pikachat-sidecar-clippy rust-live-clippy -- ",
                    "cargo clippy -p pikachat-sidecar -- -D warnings"
                ),
            },
            staged_linux_rust_lane: None,
        },
        JobSpec {
            id: "pikachat-ui-e2e-local-desktop",
            description: "Run the desktop local UI E2E selector on the Apple host",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::HostShellCommand {
                command: concat!(
                    "./scripts/apple-host-record-phase ",
                    "pikachat-followup.pikachat-ui-e2e-local-desktop rust-prepared -- bash -lc '",
                    "if [[ -n \"${PIKACI_APPLE_RUST_PREPARED_MANIFEST:-}\" ]]; then ",
                    "./scripts/apple-host-run-prepared-entry pikahut-integration-deterministic ui_e2e_local_desktop --ignored --nocapture; ",
                    "else ",
                    "cargo test -p pikahut --test integration_deterministic ui_e2e_local_desktop -- --ignored --nocapture; ",
                    "fi'"
                ),
            },
            staged_linux_rust_lane: None,
        },
        JobSpec {
            id: "pikachat-openclaw-channel-behavior",
            description: "Run the OpenClaw channel-behavior TypeScript test on the Apple host",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::HostShellCommand {
                command: concat!(
                    "./scripts/apple-host-record-phase ",
                    "pikachat-followup.pikachat-openclaw-channel-behavior js-live -- ",
                    "npx --yes tsx --test ",
                    "pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/channel-behavior.test.ts"
                ),
            },
            staged_linux_rust_lane: None,
        },
    ]
}

fn rmp_jobs() -> Vec<JobSpec> {
    vec![JobSpec {
        id: "rmp-init-smoke-ci",
        description: "Run the RMP init smoke checks in a remote Linux VM",
        timeout_secs: 1800,
        writable_workspace: false,
        guest_command: GuestCommand::ShellCommand {
            command: concat!(
                "set -euo pipefail; ",
                "ROOT=$PWD; ",
                "BIN=$ROOT/target/debug/rmp; ",
                "TMP=$(mktemp -d \"${TMPDIR:-/tmp}/rmp-init-smoke-ci.XXXXXX\"); ",
                "TARGET=$TMP/target; ",
                "cargo build -p rmp-cli; ",
                "\"$BIN\" init \"$TMP/rmp-mobile-no-iced\" --yes --org 'com.example' --no-iced --json >/dev/null; ",
                "(cd \"$TMP/rmp-mobile-no-iced\" && CARGO_TARGET_DIR=\"$TARGET\" cargo check >/dev/null); ",
                "\"$BIN\" init \"$TMP/rmp-all\" --yes --org 'com.example' --json >/dev/null; ",
                "(cd \"$TMP/rmp-all\" && CARGO_TARGET_DIR=\"$TARGET\" cargo check >/dev/null); ",
                "\"$BIN\" init \"$TMP/rmp-android\" --yes --org 'com.example' --no-ios --json >/dev/null; ",
                "(cd \"$TMP/rmp-android\" && CARGO_TARGET_DIR=\"$TARGET\" cargo check >/dev/null); ",
                "\"$BIN\" init \"$TMP/rmp-ios\" --yes --org 'com.example' --no-android --json >/dev/null; ",
                "(cd \"$TMP/rmp-ios\" && CARGO_TARGET_DIR=\"$TARGET\" cargo check >/dev/null); ",
                "\"$BIN\" init \"$TMP/rmp-iced\" --yes --org 'com.example' --no-ios --no-android --iced --json >/dev/null; ",
                "(cd \"$TMP/rmp-iced\" && CARGO_TARGET_DIR=\"$TARGET\" cargo check -p rmp-iced_core_desktop_iced >/dev/null); ",
                "echo 'ok: rmp init ci smoke passed'"
            ),
        },
        staged_linux_rust_lane: Some(StagedLinuxRustLane::RmpInitSmokeCi),
    }]
}

fn tart_agent_button_job(id: &'static str, description: &'static str) -> JobSpec {
    JobSpec {
        id,
        description,
        timeout_secs: 7200,
        writable_workspace: true,
        guest_command: GuestCommand::ShellCommand {
            command: concat!(
                "set -euo pipefail; ",
                "local_ws=\"$(mktemp -d /tmp/pikaci-ios-workspace.XXXXXX)\"; ",
                "trap 'rm -rf \"$local_ws\" /tmp/pikaci-ios-build' EXIT; ",
                "ditto . \"$local_ws\"; ",
                "cd \"$local_ws\"; ",
                "rm -rf /tmp/pikaci-ios-build; ",
                "mkdir -p '/Volumes/My Shared Files/artifacts/xcodebuild-logs'; ",
                "./tools/ios-sim-ensure | tee /tmp/pikaci-ios-sim-ensure.log >/dev/null; ",
                "udid=\"$(sed -n 's/^ok: ios simulator ready (udid=\\(.*\\))$/\\1/p' /tmp/pikaci-ios-sim-ensure.log)\"; ",
                "if [ -z \"$udid\" ]; then echo 'error: could not determine simulator udid'; cat /tmp/pikaci-ios-sim-ensure.log; exit 1; fi; ",
                "PIKA_XCODEBUILD_LOG_DIR='/Volumes/My Shared Files/artifacts/xcodebuild-logs' ./tools/xcodebuild-compact ",
                "-project ios/Pika.xcodeproj ",
                "-scheme Pika ",
                "-derivedDataPath /tmp/pikaci-ios-build ",
                "-destination \"id=$udid\" ",
                "test ",
                "-skipMacroValidation ",
                "ARCHS=arm64 ",
                "ONLY_ACTIVE_ARCH=YES ",
                "CODE_SIGNING_ALLOWED=NO ",
                "PIKA_APP_BUNDLE_ID=\"${PIKA_IOS_BUNDLE_ID:-org.pikachat.pika.dev}\" ",
                "PIKA_IOS_URL_SCHEME=\"${PIKA_IOS_URL_SCHEME:-pika}\" ",
                "-only-testing:PikaTests/AgentTests/testAgentButtonStateReflectsBusyFlag ",
                "-skip-testing:PikaUITests",
            ),
        },
        staged_linux_rust_lane: None,
    }
}

fn tart_ios_unit_jobs() -> Vec<JobSpec> {
    vec![
        tart_ios_unit_suite_job(
            "tart-ios-agent-tests",
            "Run AgentTests in a Tart macOS guest",
            "PikaTests/AgentTests",
        ),
        tart_ios_unit_suite_job(
            "tart-ios-app-manager-tests",
            "Run AppManagerTests in a Tart macOS guest",
            "PikaTests/AppManagerTests",
        ),
        tart_ios_unit_suite_job(
            "tart-ios-chat-deeplink-tests",
            "Run ChatDeepLinkTests in a Tart macOS guest",
            "PikaTests/ChatDeepLinkTests",
        ),
        tart_ios_unit_suite_job(
            "tart-ios-keychain-tests",
            "Run KeychainNsecStoreTests in a Tart macOS guest",
            "PikaTests/KeychainNsecStoreTests",
        ),
    ]
}

fn tart_ios_unit_suite_job(
    id: &'static str,
    description: &'static str,
    only_testing: &'static str,
) -> JobSpec {
    let command = format!(
        concat!(
            "set -euo pipefail; ",
            "local_ws=\"$(mktemp -d /tmp/pikaci-ios-workspace.XXXXXX)\"; ",
            "trap 'rm -rf \"$local_ws\" /tmp/pikaci-ios-build' EXIT; ",
            "ditto . \"$local_ws\"; ",
            "cd \"$local_ws\"; ",
            "rm -rf /tmp/pikaci-ios-build; ",
            "mkdir -p '/Volumes/My Shared Files/artifacts/xcodebuild-logs'; ",
            "./tools/ios-sim-ensure | tee /tmp/pikaci-ios-sim-ensure.log >/dev/null; ",
            "cp /tmp/pikaci-ios-sim-ensure.log '/Volumes/My Shared Files/artifacts/ios-sim-ensure.log'; ",
            "\"$DEVELOPER_DIR/usr/bin/xcodebuild\" -showsdks > '/Volumes/My Shared Files/artifacts/xcode-showsdks.txt'; ",
            "\"$DEVELOPER_DIR/usr/bin/simctl\" list runtimes > '/Volumes/My Shared Files/artifacts/simctl-runtimes.txt'; ",
            "\"$DEVELOPER_DIR/usr/bin/simctl\" list devices > '/Volumes/My Shared Files/artifacts/simctl-devices.txt'; ",
            "udid=\"$(sed -n 's/^ok: ios simulator ready (udid=\\(.*\\))$/\\1/p' /tmp/pikaci-ios-sim-ensure.log)\"; ",
            "if [ -z \"$udid\" ]; then echo 'error: could not determine simulator udid'; cat /tmp/pikaci-ios-sim-ensure.log; exit 1; fi; ",
            "PIKA_XCODEBUILD_LOG_DIR='/Volumes/My Shared Files/artifacts/xcodebuild-logs' ./tools/xcodebuild-compact ",
            "-project ios/Pika.xcodeproj ",
            "-scheme Pika ",
            "-derivedDataPath /tmp/pikaci-ios-build ",
            "-destination \"id=$udid\" ",
            "test ",
            "-skipMacroValidation ",
            "ARCHS=arm64 ",
            "ONLY_ACTIVE_ARCH=YES ",
            "CODE_SIGNING_ALLOWED=NO ",
            "PIKA_APP_BUNDLE_ID=\"${{PIKA_IOS_BUNDLE_ID:-org.pikachat.pika.dev}}\" ",
            "PIKA_IOS_URL_SCHEME=\"${{PIKA_IOS_URL_SCHEME:-pika}}\" ",
            "-only-testing:{} ",
            "-skip-testing:PikaUITests"
        ),
        only_testing
    );
    JobSpec {
        id,
        description,
        timeout_secs: 7200,
        writable_workspace: true,
        guest_command: GuestCommand::ShellCommand {
            command: Box::leak(command.into_boxed_str()),
        },
        staged_linux_rust_lane: None,
    }
}

fn tart_ios_ui_test_job(id: &'static str, description: &'static str) -> JobSpec {
    JobSpec {
        id,
        description,
        timeout_secs: 7200,
        writable_workspace: true,
        guest_command: GuestCommand::ShellCommand {
            command: concat!(
                "set -euo pipefail; ",
                "local_ws=\"$(mktemp -d /tmp/pikaci-ios-workspace.XXXXXX)\"; ",
                "trap 'rm -rf \"$local_ws\" /tmp/pikaci-ios-build' EXIT; ",
                "ditto . \"$local_ws\"; ",
                "cd \"$local_ws\"; ",
                "rm -rf /tmp/pikaci-ios-build; ",
                "mkdir -p '/Volumes/My Shared Files/artifacts/xcodebuild-logs'; ",
                "./tools/ios-sim-ensure | tee /tmp/pikaci-ios-sim-ensure.log >/dev/null; ",
                "udid=\"$(sed -n 's/^ok: ios simulator ready (udid=\\(.*\\))$/\\1/p' /tmp/pikaci-ios-sim-ensure.log)\"; ",
                "if [ -z \"$udid\" ]; then echo 'error: could not determine simulator udid'; cat /tmp/pikaci-ios-sim-ensure.log; exit 1; fi; ",
                "PIKA_XCODEBUILD_LOG_DIR='/Volumes/My Shared Files/artifacts/xcodebuild-logs' ./tools/xcodebuild-compact ",
                "-project ios/Pika.xcodeproj ",
                "-scheme Pika ",
                "-derivedDataPath /tmp/pikaci-ios-build ",
                "-destination \"id=$udid\" ",
                "test ",
                "-skipMacroValidation ",
                "ARCHS=arm64 ",
                "ONLY_ACTIVE_ARCH=YES ",
                "CODE_SIGNING_ALLOWED=NO ",
                "PIKA_APP_BUNDLE_ID=\"${PIKA_IOS_BUNDLE_ID:-org.pikachat.pika.dev}\" ",
                "PIKA_IOS_URL_SCHEME=\"${PIKA_IOS_URL_SCHEME:-pika}\" ",
                "-skip-testing:PikaUITests/PikaUITests/testE2E_deployedRustBot_pingPong ",
                "-skip-testing:PikaUITests/PikaUITests/testE2E_hypernoteDetailsAndCodeBlock ",
                "-skip-testing:PikaUITests/PikaUITests/testE2E_multiImageGrid",
            ),
        },
        staged_linux_rust_lane: None,
    }
}

fn tart_ios_ui_note_to_self_job(id: &'static str, description: &'static str) -> JobSpec {
    JobSpec {
        id,
        description,
        timeout_secs: 7200,
        writable_workspace: true,
        guest_command: GuestCommand::ShellCommand {
            command: concat!(
                "set -euo pipefail; ",
                "local_ws=\"$(mktemp -d /tmp/pikaci-ios-workspace.XXXXXX)\"; ",
                "trap 'rm -rf \"$local_ws\" /tmp/pikaci-ios-build' EXIT; ",
                "ditto . \"$local_ws\"; ",
                "cd \"$local_ws\"; ",
                "rm -rf /tmp/pikaci-ios-build; ",
                "mkdir -p '/Volumes/My Shared Files/artifacts/xcodebuild-logs'; ",
                "./tools/ios-sim-ensure | tee /tmp/pikaci-ios-sim-ensure.log >/dev/null; ",
                "udid=\"$(sed -n 's/^ok: ios simulator ready (udid=\\(.*\\))$/\\1/p' /tmp/pikaci-ios-sim-ensure.log)\"; ",
                "if [ -z \"$udid\" ]; then echo 'error: could not determine simulator udid'; cat /tmp/pikaci-ios-sim-ensure.log; exit 1; fi; ",
                "PIKA_XCODEBUILD_LOG_DIR='/Volumes/My Shared Files/artifacts/xcodebuild-logs' ./tools/xcodebuild-compact ",
                "-project ios/Pika.xcodeproj ",
                "-scheme Pika ",
                "-derivedDataPath /tmp/pikaci-ios-build ",
                "-destination \"id=$udid\" ",
                "test ",
                "-skipMacroValidation ",
                "ARCHS=arm64 ",
                "ONLY_ACTIVE_ARCH=YES ",
                "CODE_SIGNING_ALLOWED=NO ",
                "PIKA_APP_BUNDLE_ID=\"${PIKA_IOS_BUNDLE_ID:-org.pikachat.pika.dev}\" ",
                "PIKA_IOS_URL_SCHEME=\"${PIKA_IOS_URL_SCHEME:-pika}\" ",
                "-only-testing:PikaUITests/PikaUITests/testCreateAccount_noteToSelf_sendMessage_and_logout ",
                "-skip-testing:PikaUITests/PikaUITests/testE2E_deployedRustBot_pingPong",
            ),
        },
        staged_linux_rust_lane: None,
    }
}

fn tart_desktop_package_tests_job(id: &'static str, description: &'static str) -> JobSpec {
    JobSpec {
        id,
        description,
        timeout_secs: 7200,
        writable_workspace: false,
        guest_command: GuestCommand::ShellCommand {
            command: concat!(
                "set -euo pipefail; ",
                "sdkroot=\"$(xcrun --sdk macosx --show-sdk-path)\"; ",
                "export LIBRARY_PATH=\"$sdkroot/usr/lib\"; ",
                "export LDFLAGS=\"-isysroot $sdkroot -L$sdkroot/usr/lib\"; ",
                "export CARGO_TARGET_DIR=\"$HOME/pikaci-target/pika-desktop\"; ",
                "mkdir -p \"$CARGO_TARGET_DIR\"; ",
                "./tools/cargo-with-xcode test -p pika-desktop -- --nocapture",
            ),
        },
        staged_linux_rust_lane: None,
    }
}

fn run_target(options: &RunOptions, target: TargetSpec) -> anyhow::Result<pikaci::RunRecord> {
    run_target_with_reporter(options, target, &mut |_| Ok(()))
}

fn run_target_with_reporter(
    options: &RunOptions,
    target: TargetSpec,
    reporter: &mut dyn FnMut(RunLifecycleEvent) -> anyhow::Result<()>,
) -> anyhow::Result<pikaci::RunRecord> {
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
        filters: target
            .filters
            .iter()
            .map(|pattern| (*pattern).to_string())
            .collect(),
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
        .any(|path| matches_any_filter(path, target.filters))
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
    previous: &pikaci::RunRecord,
) -> anyhow::Result<pikaci::RunRecord> {
    rerun_target_with_reporter(options, previous, &mut |_| Ok(()))
}

fn rerun_target_with_reporter(
    options: &RunOptions,
    previous: &pikaci::RunRecord,
    reporter: &mut dyn FnMut(RunLifecycleEvent) -> anyhow::Result<()>,
) -> anyhow::Result<pikaci::RunRecord> {
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

fn rerun_metadata(previous: &pikaci::RunRecord, target: &TargetSpec) -> RunMetadata {
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

fn target_spec_for_rerun(previous: &pikaci::RunRecord) -> anyhow::Result<TargetSpec> {
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

fn status_text(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Passed => "passed",
        RunStatus::Failed => "failed",
        RunStatus::Skipped => "skipped",
    }
}

fn matches_any_filter(path: &str, filters: &[&str]) -> bool {
    filters.iter().any(|pattern| matches_filter(path, pattern))
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
    use pikaci::{
        JobRecord, PreparedOutputConsumerKind, RemoteLinuxVmBackend, RunLifecycleEvent, RunRecord,
        RunStatus, RunnerKind, StagedLinuxRustLane, StagedLinuxRustTarget,
        staged_linux_remote_defaults,
    };

    #[test]
    fn path_filters_match_exact_and_prefix_patterns() {
        assert!(matches_filter("Cargo.toml", "Cargo.toml"));
        assert!(!matches_filter("Cargo.lock", "Cargo.toml"));
        assert!(matches_filter("rust/src/core/agent.rs", "rust/**"));
        assert!(matches_filter(
            ".github/workflows/pre-merge.yml",
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
                Some(std::path::PathBuf::from("/var/lib/pika-news/pikaci"))
            ),
            std::path::PathBuf::from("/var/lib/pika-news/pikaci")
        );
    }

    #[test]
    fn standalone_agent_contract_target_uses_staged_remote_linux_lane() {
        let standalone = target_spec("agent-control-plane-unit").expect("standalone target");
        let pre_merge = target_spec("pre-merge-agent-contracts").expect("pre-merge target");

        assert_eq!(standalone.jobs.len(), 1);
        assert_eq!(standalone.jobs[0].runner_kind(), RunnerKind::RemoteLinuxVm);
        assert_eq!(
            standalone.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::AgentContractsControlPlaneUnit)
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
            target.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::NotificationsServerPackageTests)
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
            target.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::FixturePikahutClippy)
        );
        assert_eq!(target.jobs[0].runner_kind(), RunnerKind::RemoteLinuxVm);
        assert_eq!(
            target.jobs[1].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::FixtureRelaySmoke)
        );
        assert_eq!(target.jobs[1].runner_kind(), RunnerKind::RemoteLinuxVm);
    }

    #[test]
    fn pre_merge_rmp_target_uses_staged_linux_lane() {
        let target = target_spec("pre-merge-rmp").expect("rmp target");
        let config = StagedLinuxRustTarget::PreMergeRmp.config();

        assert_eq!(target.jobs.len(), 1);
        assert_eq!(target.id, config.target_id);
        assert_eq!(target.description, config.target_description);
        assert_eq!(
            target.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::RmpInitSmokeCi)
        );
        assert_eq!(target.jobs[0].runner_kind(), RunnerKind::RemoteLinuxVm);
    }

    #[test]
    fn pre_merge_pikachat_rust_target_uses_staged_linux_lanes() {
        let target = target_spec("pre-merge-pikachat-rust").expect("pikachat target");

        assert_eq!(target.jobs.len(), 10);
        assert_eq!(
            target.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::PikachatPackageTests)
        );
        assert_eq!(
            target.jobs[3].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::PikachatCliSmokeLocal)
        );
        assert_eq!(
            target.jobs[9].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::OpenclawAudioEcho)
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
            target.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::PikachatTypescript)
        );
        assert_eq!(target.jobs[0].runner_kind(), RunnerKind::RemoteLinuxVm);
    }

    #[test]
    fn pre_merge_pikachat_openclaw_e2e_target_uses_staged_linux_runner() {
        let target =
            target_spec("pre-merge-pikachat-openclaw-e2e").expect("pikachat openclaw target");

        assert_eq!(target.jobs.len(), 1);
        assert_eq!(
            target.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::OpenclawGatewayE2e)
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
            target.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::PikaFollowupAndroidTestCompile)
        );
        assert_eq!(
            target.jobs[5].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::PikaFollowupRustDepsHygiene)
        );
        assert!(
            target
                .jobs
                .iter()
                .any(|job| job.id == "pika-rust-deps-hygiene")
        );
    }

    #[test]
    fn standalone_experimental_targets_can_address_single_staged_linux_lanes() {
        let actionlint = target_spec("pika-actionlint").expect("actionlint target");
        assert_eq!(actionlint.jobs.len(), 1);
        assert_eq!(actionlint.jobs[0].id, "pika-actionlint");
        assert_eq!(
            actionlint.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::PikaFollowupActionlint)
        );

        let rmp = target_spec("rmp-init-smoke-ci").expect("rmp target");
        assert_eq!(rmp.jobs.len(), 1);
        assert_eq!(rmp.jobs[0].id, "rmp-init-smoke-ci");
        assert_eq!(
            rmp.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::RmpInitSmokeCi)
        );

        let rust_deps = target_spec("pika-rust-deps-hygiene").expect("rust deps hygiene target");
        assert_eq!(rust_deps.jobs.len(), 1);
        assert_eq!(rust_deps.jobs[0].id, "pika-rust-deps-hygiene");
        assert_eq!(
            rust_deps.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::PikaFollowupRustDepsHygiene)
        );

        let app_flows = target_spec("pika-core-lib-app-flows-tests").expect("app flows target");
        assert_eq!(app_flows.jobs.len(), 1);
        assert_eq!(app_flows.jobs[0].id, "pika-core-lib-app-flows-tests");
        assert_eq!(
            app_flows.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::PikaCoreLibAppFlows)
        );

        let notifications = target_spec("pika-server-package-tests").expect("notifications target");
        assert_eq!(notifications.jobs.len(), 1);
        assert_eq!(notifications.jobs[0].id, "pika-server-package-tests");
        assert_eq!(
            notifications.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::NotificationsServerPackageTests)
        );

        let pikachat = target_spec("pikachat-package-tests").expect("pikachat package target");
        assert_eq!(pikachat.jobs.len(), 1);
        assert_eq!(pikachat.jobs[0].id, "pikachat-package-tests");
        assert_eq!(
            pikachat.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::PikachatPackageTests)
        );

        let fixture = target_spec("pikahut-clippy").expect("fixture target");
        assert_eq!(fixture.jobs.len(), 1);
        assert_eq!(fixture.jobs[0].id, "pikahut-clippy");
        assert_eq!(
            fixture.jobs[0].staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::FixturePikahutClippy)
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
        assert!(target.filters.contains(&"just/checks.just"));
        assert!(target.filters.contains(&"cli/**"));
        assert!(target.filters.contains(&"crates/pikachat-sidecar/**"));
        assert!(target.filters.contains(&"crates/pikahut/**"));
        assert!(target.filters.contains(&"crates/pika-desktop/**"));
        assert!(target.filters.contains(&"crates/pika-marmot-runtime/**"));
        assert!(target.filters.contains(&"crates/pika-media/**"));
        assert!(target.filters.contains(&"crates/pika-relay-profiles/**"));
        assert!(target.filters.contains(&"pikachat-openclaw/**"));
        assert!(target.filters.contains(&"rust/**"));
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
        run.jobs = vec![sample_job_record("agent-control-plane-unit")];

        let target = target_spec_for_rerun(&run).expect("target");
        assert_eq!(target.id, "agent-control-plane-unit");
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
            Some(pikaci::PreparedOutputInvocationMode::DirectHelperExecV1)
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
            Some(pikaci::PreparedOutputInvocationMode::ExternalWrapperCommandV1);
        run.prepared_output_invocation_wrapper_program =
            Some("/tmp/bin/pikaci-wrapper".to_string());

        let target = target_spec_for_rerun(&run).expect("target");
        let metadata = rerun_metadata(&run, &target);

        assert_eq!(
            metadata.prepared_output_invocation_mode,
            Some(pikaci::PreparedOutputInvocationMode::ExternalWrapperCommandV1)
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
            Some(pikaci::PreparedOutputInvocationMode::ExternalWrapperCommandV1);
        run.prepared_output_invocation_wrapper_program =
            Some("/tmp/bin/pikaci-launch-fulfill-prepared-output".to_string());
        run.prepared_output_launcher_transport_mode =
            Some(pikaci::PreparedOutputLauncherTransportMode::CommandTransportV1);
        run.prepared_output_launcher_transport_program = Some("/tmp/bin/fake-ssh".to_string());

        let target = target_spec_for_rerun(&run).expect("target");
        let metadata = rerun_metadata(&run, &target);

        assert_eq!(
            metadata.prepared_output_launcher_transport_mode,
            Some(pikaci::PreparedOutputLauncherTransportMode::CommandTransportV1)
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
            Some(pikaci::PreparedOutputInvocationMode::ExternalWrapperCommandV1);
        run.prepared_output_invocation_wrapper_program =
            Some("/tmp/bin/pikaci-launch-fulfill-prepared-output".to_string());
        run.prepared_output_launcher_transport_mode =
            Some(pikaci::PreparedOutputLauncherTransportMode::SshLauncherTransportV1);
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
            Some(pikaci::PreparedOutputLauncherTransportMode::SshLauncherTransportV1)
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
            Some(pikaci::PreparedOutputInvocationMode::ExternalWrapperCommandV1);
        run.prepared_output_invocation_wrapper_program =
            Some("/tmp/bin/pikaci-launch-fulfill-prepared-output".to_string());
        run.prepared_output_launcher_transport_mode =
            Some(pikaci::PreparedOutputLauncherTransportMode::CommandTransportV1);
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
            Some(pikaci::PreparedOutputInvocationMode::ExternalWrapperCommandV1);
        run.prepared_output_invocation_wrapper_program =
            Some("/tmp/bin/pikaci-launch-fulfill-prepared-output".to_string());
        run.prepared_output_launcher_transport_mode =
            Some(pikaci::PreparedOutputLauncherTransportMode::SshLauncherTransportV1);
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
        let payload = serde_json::to_value(StagedLinuxRustTarget::PreMergePikaRust.config())
            .expect("encode target config");
        assert_eq!(payload["target_id"], "pre-merge-pika-rust");
        assert_eq!(
            payload["workspace_deps_installable"],
            ".#ci.x86_64-linux.workspaceDeps"
        );
        assert_eq!(
            payload["workspace_build_installable"],
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
                pikaci::PreparedOutputInvocationMode::DirectHelperExecV1,
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
