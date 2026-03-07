use anyhow::{Context, bail};
use clap::{Parser, Subcommand, ValueEnum};
use pikaci::{
    GuestCommand, JobSpec, LogKind, RunMetadata, RunOptions, RunStatus, gc_runs, git_changed_files,
    list_runs, load_logs, load_run_record, record_skipped_run, rerun_jobs_with_metadata,
    run_jobs_with_metadata,
};

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
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Run {
        #[arg(default_value = "beachhead")]
        job: String,
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
    },
    Status {
        run_id: String,
    },
    Rerun {
        run_id: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum LogKindArg {
    Host,
    Guest,
    Both,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir().context("read current directory")?;
    let options = RunOptions {
        source_root: cwd.clone(),
        state_root: cwd.join(".pikaci"),
    };

    match cli.command {
        Command::Run { job } => {
            let target = target_spec(&job)?;
            let run = run_target(&options, target)?;
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
            if matches!(run.status, RunStatus::Failed) {
                std::process::exit(1);
            }
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
        Command::Logs { run_id, job, kind } => {
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
        Command::Status { run_id } => {
            let run = load_run_record(&options.state_root, &run_id)?;
            println!(
                "{} {} {}",
                run.run_id,
                run.target_id.as_deref().unwrap_or("-"),
                status_text(run.status)
            );
            if let Some(plan_path) = &run.plan_path {
                println!("plan={plan_path}");
            }
            if let Some(message) = &run.message {
                println!("{message}");
            }
            if !run.changed_files.is_empty() {
                println!("changed_files={}", run.changed_files.len());
            }
            for job in &run.jobs {
                println!("{} {}", job.id, status_text(job.status));
            }
        }
        Command::Rerun { run_id } => {
            let previous = load_run_record(&options.state_root, &run_id)?;
            let run = rerun_target(&options, &previous)?;
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
            if matches!(run.status, RunStatus::Failed) {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

fn target_spec(name: &str) -> anyhow::Result<TargetSpec> {
    match name {
        "beachhead" => Ok(TargetSpec {
            id: "beachhead",
            description: "Run one tiny exact unit test in a vfkit guest",
            filters: &[],
            jobs: vec![JobSpec {
                id: "beachhead",
                description: "Run one tiny exact unit test in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ExactCargoTest {
                    package: "pika-agent-control-plane",
                    test_name: "tests::command_envelope_round_trips",
                },
            }],
        }),
        "tart-beachhead" => Ok(TargetSpec {
            id: "tart-beachhead",
            description: "Run one tiny iOS unit test in a Tart macOS guest",
            filters: &[],
            jobs: vec![tart_agent_button_job(
                "tart-beachhead",
                "Run one tiny iOS unit test in a Tart macOS guest",
            )],
        }),
        "agent-control-plane-unit" => Ok(TargetSpec {
            id: "agent-control-plane-unit",
            description: "Run all pika-agent-control-plane unit tests in a vfkit guest",
            filters: &[],
            jobs: vec![JobSpec {
                id: "agent-control-plane-unit",
                description: "Run all pika-agent-control-plane unit tests in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::PackageUnitTests {
                    package: "pika-agent-control-plane",
                },
            }],
        }),
        "agent-microvm-tests" => Ok(TargetSpec {
            id: "agent-microvm-tests",
            description: "Run pika-agent-microvm tests in a vfkit guest",
            filters: &[],
            jobs: vec![JobSpec {
                id: "agent-microvm-tests",
                description: "Run pika-agent-microvm tests in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::PackageTests {
                    package: "pika-agent-microvm",
                },
            }],
        }),
        "agent-http-ensure-local" => Ok(TargetSpec {
            id: "agent-http-ensure-local",
            description: "Run the pikahut agent_http_ensure_local deterministic test in a vfkit guest",
            filters: &[],
            jobs: vec![JobSpec {
                id: "agent-http-ensure-local",
                description: "Run the pikahut agent_http_ensure_local deterministic test in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ShellCommand {
                    command: "cargo test -p pikahut --test integration_deterministic agent_http_ensure_local -- --ignored --nocapture",
                },
            }],
        }),
        "server-agent-api-tests" => Ok(TargetSpec {
            id: "server-agent-api-tests",
            description: "Run pika-server agent_api tests in a vfkit guest",
            filters: &[],
            jobs: vec![JobSpec {
                id: "server-agent-api-tests",
                description: "Run pika-server agent_api tests in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::FilteredCargoTests {
                    package: "pika-server",
                    filter: "agent_api::tests",
                },
            }],
        }),
        "core-agent-nip98-test" => Ok(TargetSpec {
            id: "core-agent-nip98-test",
            description: "Run pika_core NIP-98 signing contract test in a vfkit guest",
            filters: &[],
            jobs: vec![JobSpec {
                id: "core-agent-nip98-test",
                description: "Run pika_core NIP-98 signing contract test in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ExactCargoTest {
                    package: "pika_core",
                    test_name: "core::agent::tests::run_agent_flow_signs_requests_with_nip98_authorization",
                },
            }],
        }),
        "agent-contracts-smoke" => Ok(TargetSpec {
            id: "agent-contracts-smoke",
            description: "Run the VM-backed agent contracts smoke lane",
            filters: &[],
            jobs: agent_contract_jobs(),
        }),
        "pre-merge-agent-contracts" => Ok(TargetSpec {
            id: "pre-merge-agent-contracts",
            description: "Run the VM-backed pre-merge agent contracts lane",
            filters: &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                ".github/workflows/pre-merge.yml",
                "docs/agent-ci.md",
                "crates/pikaci/**",
                "crates/pika-agent-control-plane/**",
                "crates/pika-agent-microvm/**",
                "crates/pika-server/**",
                "crates/pika-media/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-tls/**",
                "rust/**",
            ],
            jobs: agent_contract_jobs(),
        }),
        "pre-merge-pika-rust" => Ok(TargetSpec {
            id: "pre-merge-pika-rust",
            description: "Run the VM-backed Rust tests from the pre-merge pika lane",
            filters: &[
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
            jobs: pika_rust_jobs(),
        }),
        "pre-merge-notifications" => Ok(TargetSpec {
            id: "pre-merge-notifications",
            description: "Run the VM-backed Rust tests from the notifications lane",
            filters: &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                ".github/workflows/pre-merge.yml",
                "crates/pikaci/**",
                "crates/pika-server/**",
                "crates/pikahut/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-tls/**",
            ],
            jobs: notification_jobs(),
        }),
        "pre-merge-pikachat-rust" => Ok(TargetSpec {
            id: "pre-merge-pikachat-rust",
            description: "Run the VM-backed Rust tests from the pikachat lane",
            filters: &[
                "Cargo.toml",
                "Cargo.lock",
                "cli/**",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                ".github/workflows/pre-merge.yml",
                "crates/pikaci/**",
                "crates/pika-desktop/**",
                "crates/pikachat-sidecar/**",
                "crates/pikahut/**",
                "crates/pika-agent-protocol/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-media/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-test-utils/**",
                "crates/pika-tls/**",
                "rust/**",
            ],
            jobs: pikachat_rust_jobs(),
        }),
        "pikachat-ui-e2e-local-desktop" => Ok(TargetSpec {
            id: "pikachat-ui-e2e-local-desktop",
            description: "Run the pikahut ui_e2e_local_desktop test in a vfkit guest",
            filters: &[],
            jobs: vec![JobSpec {
                id: "pikachat-ui-e2e-local-desktop",
                description: "Run the pikahut ui_e2e_local_desktop test in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ShellCommand {
                    command: "cargo test -p pikahut --test integration_deterministic ui_e2e_local_desktop -- --ignored --nocapture",
                },
            }],
        }),
        "pika-desktop-e2e-compile" => Ok(TargetSpec {
            id: "pika-desktop-e2e-compile",
            description: "Compile the pika-desktop E2E test target in a vfkit guest without running it",
            filters: &[],
            jobs: vec![JobSpec {
                id: "pika-desktop-e2e-compile",
                description: "Compile the pika-desktop E2E test target in a vfkit guest without running it",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::ShellCommand {
                    command: "cargo test -p pika-desktop desktop_e2e_local_ping_pong_with_bot --no-run",
                },
            }],
        }),
        "pika-desktop-package-tests" => Ok(TargetSpec {
            id: "pika-desktop-package-tests",
            description: "Run pika-desktop package tests in a vfkit guest",
            filters: &[],
            jobs: vec![JobSpec {
                id: "pika-desktop-package-tests",
                description: "Run pika-desktop package tests in a vfkit guest",
                timeout_secs: 1800,
                writable_workspace: false,
                guest_command: GuestCommand::PackageTests {
                    package: "pika-desktop",
                },
            }],
        }),
        "pre-merge-fixture-rust" => Ok(TargetSpec {
            id: "pre-merge-fixture-rust",
            description: "Run the VM-backed Rust tests from the fixture lane",
            filters: &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                ".github/workflows/pre-merge.yml",
                "crates/pikaci/**",
                "crates/pikahut/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-relay-profiles/**",
                "cmd/pika-relay/**",
            ],
            jobs: fixture_rust_jobs(),
        }),
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
        "pre-merge-rmp" => Ok(TargetSpec {
            id: "pre-merge-rmp",
            description: "Run the VM-backed pre-merge RMP lane",
            filters: &[
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
            jobs: rmp_jobs(),
        }),
        other => bail!("unknown job `{other}`"),
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
            description: "Run all pika-agent-control-plane unit tests in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageUnitTests {
                package: "pika-agent-control-plane",
            },
        },
        JobSpec {
            id: "agent-microvm-tests",
            description: "Run pika-agent-microvm tests in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageTests {
                package: "pika-agent-microvm",
            },
        },
        JobSpec {
            id: "server-agent-api-tests",
            description: "Run pika-server agent_api tests in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::FilteredCargoTests {
                package: "pika-server",
                filter: "agent_api::tests",
            },
        },
        JobSpec {
            id: "core-agent-nip98-test",
            description: "Run pika_core NIP-98 signing contract test in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ExactCargoTest {
                package: "pika_core",
                test_name: "core::agent::tests::run_agent_flow_signs_requests_with_nip98_authorization",
            },
        },
    ]
}

fn pika_rust_jobs() -> Vec<JobSpec> {
    vec![JobSpec {
        id: "pika-core-lib-tests",
        description: "Run pika_core lib and test targets in a vfkit guest",
        timeout_secs: 1800,
        writable_workspace: false,
        guest_command: GuestCommand::ShellCommand {
            command: "cargo test -p pika_core --lib --tests -- --nocapture",
        },
    }]
}

fn notification_jobs() -> Vec<JobSpec> {
    vec![JobSpec {
        id: "pika-server-package-tests",
        description: "Run pika-server package tests with a pikahut postgres fixture in a vfkit guest",
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
    }]
}

fn pikachat_rust_jobs() -> Vec<JobSpec> {
    vec![
        JobSpec {
            id: "pikachat-package-tests",
            description: "Run pikachat package tests in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageTests {
                package: "pikachat",
            },
        },
        JobSpec {
            id: "pikachat-sidecar-package-tests",
            description: "Run pikachat-sidecar package tests with deterministic TTS in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "PIKACHAT_TTS_FIXTURE=1 cargo test -p pikachat-sidecar -- --nocapture",
            },
        },
        JobSpec {
            id: "pika-desktop-package-tests",
            description: "Run pika-desktop package tests in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageTests {
                package: "pika-desktop",
            },
        },
        JobSpec {
            id: "pikachat-cli-smoke-local",
            description: "Run the pikahut cli_smoke_local deterministic test in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture",
            },
        },
        JobSpec {
            id: "pikachat-post-rebase-invalid-event",
            description: "Run the post_rebase_invalid_event_rejection boundary test in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic post_rebase_invalid_event_rejection_boundary -- --ignored --nocapture",
            },
        },
        JobSpec {
            id: "pikachat-post-rebase-logout-session",
            description: "Run the post_rebase_logout_session_convergence boundary test in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic post_rebase_logout_session_convergence_boundary -- --ignored --nocapture",
            },
        },
        JobSpec {
            id: "openclaw-invite-and-chat",
            description: "Run the OpenClaw invite-and-chat scenario in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat -- --ignored --nocapture",
            },
        },
        JobSpec {
            id: "openclaw-invite-and-chat-rust-bot",
            description: "Run the OpenClaw invite-and-chat rust bot scenario in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_rust_bot -- --ignored --nocapture",
            },
        },
        JobSpec {
            id: "openclaw-invite-and-chat-daemon",
            description: "Run the OpenClaw invite-and-chat daemon scenario in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_daemon -- --ignored --nocapture",
            },
        },
        JobSpec {
            id: "openclaw-audio-echo",
            description: "Run the OpenClaw audio echo scenario in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic openclaw_scenario_audio_echo -- --ignored --nocapture",
            },
        },
    ]
}

fn fixture_rust_jobs() -> Vec<JobSpec> {
    vec![JobSpec {
        id: "pikahut-package-tests",
        description: "Run pikahut package tests in a vfkit guest",
        timeout_secs: 1800,
        writable_workspace: false,
        guest_command: GuestCommand::PackageTests { package: "pikahut" },
    }]
}

fn rmp_jobs() -> Vec<JobSpec> {
    vec![JobSpec {
        id: "rmp-init-smoke-ci",
        description: "Run the RMP init smoke checks in a vfkit guest",
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
    }
}

fn run_target(options: &RunOptions, target: TargetSpec) -> anyhow::Result<pikaci::RunRecord> {
    let changed_files = git_changed_files(&options.source_root);
    let metadata = RunMetadata {
        rerun_of: None,
        target_id: Some(target.id.to_string()),
        target_description: Some(target.description.to_string()),
        changed_files: changed_files.clone().unwrap_or_default(),
        filters: target
            .filters
            .iter()
            .map(|pattern| (*pattern).to_string())
            .collect(),
        message: None,
    };

    if target.filters.is_empty() {
        return run_jobs_with_metadata(target.jobs.as_slice(), options, metadata);
    }

    let Some(changed_files) = changed_files else {
        let mut metadata = metadata;
        metadata.message =
            Some("git change detection unavailable; running lane without filtering".to_string());
        return run_jobs_with_metadata(target.jobs.as_slice(), options, metadata);
    };

    if changed_files
        .iter()
        .any(|path| matches_any_filter(path, target.filters))
    {
        return run_jobs_with_metadata(target.jobs.as_slice(), options, metadata);
    }

    let mut metadata = metadata;
    metadata.message = Some(format!(
        "skipped; no changed files matched {} filter(s)",
        target.filters.len()
    ));
    record_skipped_run(options, metadata)
}

fn rerun_target(
    options: &RunOptions,
    previous: &pikaci::RunRecord,
) -> anyhow::Result<pikaci::RunRecord> {
    let target = target_spec_for_rerun(previous)?;
    let metadata = RunMetadata {
        rerun_of: Some(previous.run_id.clone()),
        target_id: previous
            .target_id
            .clone()
            .or_else(|| Some(target.id.to_string())),
        target_description: previous
            .target_description
            .clone()
            .or_else(|| Some(target.description.to_string())),
        changed_files: previous.changed_files.clone(),
        filters: previous.filters.clone(),
        message: Some(format!("rerun of {}", previous.run_id)),
    };

    if previous.status == RunStatus::Skipped {
        return record_skipped_run(options, metadata);
    }

    rerun_jobs_with_metadata(target.jobs.as_slice(), previous, options, metadata)
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
    use super::{matches_any_filter, matches_filter, target_spec_for_rerun};
    use pikaci::{JobRecord, RunRecord, RunStatus};

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
        run.jobs = vec![sample_job_record("beachhead")];

        let target = target_spec_for_rerun(&run).expect("target");
        assert_eq!(target.id, "beachhead");
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
            changed_files: vec![],
            filters: vec![],
            message: None,
            jobs: vec![],
        }
    }

    fn sample_job_record(id: &str) -> JobRecord {
        JobRecord {
            id: id.to_string(),
            description: "job".to_string(),
            status: RunStatus::Passed,
            executor: "vfkit_local".to_string(),
            plan_node_id: None,
            timeout_secs: 1,
            host_log_path: "/tmp/host.log".to_string(),
            guest_log_path: "/tmp/guest.log".to_string(),
            started_at: "2026-03-07T00:00:00Z".to_string(),
            finished_at: Some("2026-03-07T00:00:01Z".to_string()),
            exit_code: Some(0),
            message: Some("ok".to_string()),
        }
    }
}
