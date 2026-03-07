use anyhow::{Context, bail};
use clap::{Parser, Subcommand, ValueEnum};
use pikaci::{
    GuestCommand, JobSpec, LogKind, RunMetadata, RunOptions, RunStatus, git_changed_files,
    list_runs, load_logs, load_run_record, record_skipped_run, run_jobs_with_metadata,
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
                guest_command: GuestCommand::ExactCargoTest {
                    package: "pika-agent-control-plane",
                    test_name: "tests::command_envelope_round_trips",
                },
            }],
        }),
        "agent-control-plane-unit" => Ok(TargetSpec {
            id: "agent-control-plane-unit",
            description: "Run all pika-agent-control-plane unit tests in a vfkit guest",
            filters: &[],
            jobs: vec![JobSpec {
                id: "agent-control-plane-unit",
                description: "Run all pika-agent-control-plane unit tests in a vfkit guest",
                timeout_secs: 1800,
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
                guest_command: GuestCommand::PackageTests {
                    package: "pika-agent-microvm",
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
            guest_command: GuestCommand::PackageUnitTests {
                package: "pika-agent-control-plane",
            },
        },
        JobSpec {
            id: "agent-microvm-tests",
            description: "Run pika-agent-microvm tests in a vfkit guest",
            timeout_secs: 1800,
            guest_command: GuestCommand::PackageTests {
                package: "pika-agent-microvm",
            },
        },
        JobSpec {
            id: "server-agent-api-tests",
            description: "Run pika-server agent_api tests in a vfkit guest",
            timeout_secs: 1800,
            guest_command: GuestCommand::FilteredCargoTests {
                package: "pika-server",
                filter: "agent_api::tests",
            },
        },
        JobSpec {
            id: "core-agent-nip98-test",
            description: "Run pika_core NIP-98 signing contract test in a vfkit guest",
            timeout_secs: 1800,
            guest_command: GuestCommand::ExactCargoTest {
                package: "pika_core",
                test_name: "core::agent::tests::run_agent_flow_signs_requests_with_nip98_authorization",
            },
        },
    ]
}

fn run_target(options: &RunOptions, target: TargetSpec) -> anyhow::Result<pikaci::RunRecord> {
    let changed_files = git_changed_files(&options.source_root);
    let metadata = RunMetadata {
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
    use super::{matches_any_filter, matches_filter};

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
}
