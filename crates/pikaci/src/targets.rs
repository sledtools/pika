use crate::catalog::{PikaStagedLinuxLane, PikaStagedLinuxTarget};
use anyhow::{anyhow, bail};
use jerichoci::{
    GuestCommand, HostProcessRuntimeConfig, IncusRuntimeConfig, JobExecutionConfig,
    JobRuntimeConfig, JobSpec, StagedLinuxCommandConfig, TartRuntimeConfig,
};
use pika_incus_guest_role::IncusGuestRole;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiTrigger {
    None,
    Branch {
        concurrency_group: Option<ConcurrencyGroupSpec>,
    },
    Nightly {
        concurrency_group: Option<ConcurrencyGroupSpec>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConcurrencyGroupSpec {
    Literal(&'static str),
    StagedLinuxTarget,
}

#[derive(Debug, Clone)]
pub struct TargetSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub filters: Vec<String>,
    pub ci_trigger: CiTrigger,
    pub jobs: Vec<JobSpec>,
}

const TART_HOST_SETUP_COMMAND: &str = concat!(
    "set -euo pipefail; ",
    "export DEVELOPER_DIR=\"${JERICHOCI_TART_DEVELOPER_DIR:-${JERICHOCI_TART_XCODE_APP:-/Applications/Xcode-16.4.0.app}/Contents/Developer}\"; ",
    "just ios-xcframework ios-xcodeproj",
);

fn remote_incus_runtime(
    staged_linux_command: Option<StagedLinuxCommandConfig>,
) -> JobRuntimeConfig {
    JobRuntimeConfig::Incus(IncusRuntimeConfig {
        guest_role: IncusGuestRole::JerichoRunner,
        staged_linux_command,
    })
}

fn tart_runtime(mount_host_rust_toolchain: bool) -> JobRuntimeConfig {
    JobRuntimeConfig::Tart(TartRuntimeConfig {
        host_setup_command: Some(TART_HOST_SETUP_COMMAND),
        mount_host_rust_toolchain,
    })
}

fn remote_incus_job_base() -> JobSpec {
    JobSpec {
        id: "",
        description: "",
        timeout_secs: 0,
        writable_workspace: false,
        execution: JobExecutionConfig::REMOTE_SSH_INCUS,
        runtime_config: remote_incus_runtime(None),
        guest_command: GuestCommand::ShellCommand { command: "" },
    }
}

fn host_local_job_base() -> JobSpec {
    JobSpec {
        id: "",
        description: "",
        timeout_secs: 0,
        writable_workspace: false,
        execution: JobExecutionConfig::HOST_LOCAL,
        runtime_config: JobRuntimeConfig::HostProcess(HostProcessRuntimeConfig),
        guest_command: GuestCommand::HostShellCommand { command: "" },
    }
}

fn tart_job_base() -> JobSpec {
    JobSpec {
        id: "",
        description: "",
        timeout_secs: 0,
        writable_workspace: false,
        execution: JobExecutionConfig::LOCAL_TART,
        runtime_config: tart_runtime(false),
        guest_command: GuestCommand::ShellCommand { command: "" },
    }
}

pub fn staged_linux_target(target_id: &str) -> anyhow::Result<PikaStagedLinuxTarget> {
    PikaStagedLinuxTarget::from_target_id(target_id)
        .ok_or_else(|| anyhow!("unsupported staged Linux Rust target `{target_id}`"))
}

fn staged_linux_target_spec(
    target: PikaStagedLinuxTarget,
    title: &'static str,
    filters: &'static [&'static str],
    jobs: Vec<JobSpec>,
) -> TargetSpec {
    let config = target.config();
    TargetSpec {
        id: config.target_id,
        title,
        description: config.target_description,
        filters: static_filters(filters),
        ci_trigger: CiTrigger::Branch {
            concurrency_group: Some(ConcurrencyGroupSpec::StagedLinuxTarget),
        },
        jobs,
    }
}

fn single_job_target_spec(
    id: &'static str,
    title: &'static str,
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
        title,
        description,
        filters: static_filters(filters),
        ci_trigger: CiTrigger::None,
        jobs: vec![job],
    })
}

fn host_shell_job(
    id: &'static str,
    description: &'static str,
    timeout_secs: u64,
    command: &'static str,
) -> JobSpec {
    JobSpec {
        id,
        description,
        timeout_secs,
        writable_workspace: false,
        guest_command: GuestCommand::HostShellCommand { command },
        runtime_config: JobRuntimeConfig::HostProcess(HostProcessRuntimeConfig),
        ..host_local_job_base()
    }
}

fn host_shell_target_spec(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    filters: &'static [&'static str],
    ci_trigger: CiTrigger,
    timeout_secs: u64,
    command: &'static str,
) -> TargetSpec {
    TargetSpec {
        id,
        title,
        description,
        filters: static_filters(filters),
        ci_trigger,
        jobs: vec![host_shell_job(id, description, timeout_secs, command)],
    }
}

fn static_filters(filters: &'static [&'static str]) -> Vec<String> {
    filters
        .iter()
        .map(|pattern| (*pattern).to_string())
        .collect()
}

pub fn all_target_specs() -> anyhow::Result<Vec<TargetSpec>> {
    const ALL_TARGET_IDS: &[&str] = &[
        "tart-beachhead",
        "pika-cloud-unit",
        "server-agent-api-tests",
        "core-agent-nip98-test",
        "agent-contracts-smoke",
        "pre-merge-agent-contracts",
        "pre-merge-pika-rust",
        "pre-merge-notifications",
        "pre-merge-pikachat-rust",
        "pre-merge-pikachat-typescript",
        "pre-merge-pikachat-openclaw-e2e",
        "pre-merge-pika-followup",
        "pika-actionlint",
        "pika-doc-contracts",
        "pika-rust-deps-hygiene",
        "pika-core-lib-app-flows-tests",
        "pika-server-package-tests",
        "pikachat-package-tests",
        "pikahut-clippy",
        "pre-merge-pikachat-apple-followup",
        "apple_host_sanity",
        "apple_desktop_compile",
        "apple_ios_compile",
        "nightly_linux",
        "nightly_pika",
        "nightly_pikachat",
        "nightly_pika_ui_android",
        "nightly_apple_host_bundle",
        "pika-desktop-package-tests",
        "pre-merge-fixture-rust",
        "android-sdk-probe",
        "android-nostr-connect-intent-test",
        "tart-env-probe",
        "tart-ios-agent-button-state-test",
        "tart-ios-unit-tests",
        "tart-ios-ui-test",
        "tart-ios-ui-note-to-self",
        "tart-desktop-package-tests",
        "pre-merge-apple-deterministic",
        "pre-merge-rmp",
        "rmp-init-smoke-ci",
    ];

    ALL_TARGET_IDS.iter().map(|id| target_spec(id)).collect()
}

pub fn target_spec(name: &str) -> anyhow::Result<TargetSpec> {
    match name {
        "tart-beachhead" => Ok(TargetSpec {
            id: "tart-beachhead",
            title: "tart-beachhead",
            description: "Run one tiny iOS unit test in a Tart macOS guest",
            filters: Vec::new(),
            ci_trigger: CiTrigger::None,
            jobs: vec![tart_agent_button_job(
                "tart-beachhead",
                "Run one tiny iOS unit test in a Tart macOS guest",
            )],
        }),
        "pika-cloud-unit" => single_job_target_spec(
            "pika-cloud-unit",
            "pika-cloud-unit",
            "Run all pika-cloud unit tests in a remote Linux VM",
            &[],
            agent_contract_jobs(),
        ),
        "server-agent-api-tests" => single_job_target_spec(
            "server-agent-api-tests",
            "server-agent-api-tests",
            "Run pika-server agent_api tests in a remote Linux VM",
            &[],
            agent_contract_jobs(),
        ),
        "core-agent-nip98-test" => single_job_target_spec(
            "core-agent-nip98-test",
            "core-agent-nip98-test",
            "Run pika_core NIP-98 signing contract test in a remote Linux VM",
            &[],
            agent_contract_jobs(),
        ),
        "agent-contracts-smoke" => Ok(TargetSpec {
            id: "agent-contracts-smoke",
            title: "agent-contracts-smoke",
            description: "Run the VM-backed agent contracts smoke lane",
            filters: Vec::new(),
            ci_trigger: CiTrigger::None,
            jobs: agent_contract_jobs(),
        }),
        "pre-merge-agent-contracts" => Ok(staged_linux_target_spec(
            PikaStagedLinuxTarget::PreMergeAgentContracts,
            "check-agent-contracts",
            &[
                "AGENTS.md",
                ".agents/**",
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                "just/checks.just",
                "just/infra.just",
                "scripts/pikaci-staged-linux-remote.sh",
                "scripts/ci-add-known-host.sh",
                "crates/pika-git/**",
                "crates/pikaci/**",
                "crates/pika-cloud/**",
                "crates/pika-agent-protocol/**",
                "crates/ph/**",
                "crates/pika-server/**",
                "crates/hypernote-protocol/**",
                "crates/pika-media/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-test-utils/**",
                "crates/pika-tls/**",
                "rust/**",
                "docs/agent-ci.md",
                "scripts/pikaci-ci-run.sh",
                "scripts/pika-build-prewarm-workspace-deps.sh",
                "scripts/pika-build-run-workspace-deps.sh",
                "scripts/forge-nightly-pika-ui-android.sh",
                "scripts/lib/pikaci-tools.sh",
            ],
            agent_contract_jobs(),
        )),
        "pre-merge-pika-rust" => Ok(staged_linux_target_spec(
            PikaStagedLinuxTarget::PreMergePikaRust,
            "check-pika-rust",
            &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                ".github/**",
                "rust/**",
                "cli/**",
                "crates/pika-desktop/**",
                "crates/pika-media/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-nse/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-tls/**",
                "uniffi-bindgen/**",
                "android/**",
                "ios/**",
                "docs/**",
                "scripts/forge-nightly-pika-ui-android.sh",
                "scripts/**",
                "tools/**",
            ],
            pika_rust_jobs(),
        )),
        "pre-merge-notifications" => Ok(staged_linux_target_spec(
            PikaStagedLinuxTarget::PreMergeNotifications,
            "check-notifications",
            &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                "just/checks.just",
                "just/infra.just",
                "scripts/pikaci-staged-linux-remote.sh",
                "scripts/ci-add-known-host.sh",
                "crates/pika-git/**",
                "crates/pikaci/**",
                "crates/pika-cloud/**",
                "crates/pika-desktop/**",
                "crates/ph/**",
                "crates/pikahut/**",
                "crates/pika-server/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-test-utils/**",
                "crates/pika-tls/**",
                "scripts/pikaci-ci-run.sh",
                "scripts/pika-build-prewarm-workspace-deps.sh",
                "scripts/pika-build-run-workspace-deps.sh",
                "scripts/forge-nightly-pika-ui-android.sh",
                "scripts/lib/pikaci-tools.sh",
            ],
            notification_jobs(),
        )),
        "pre-merge-pikachat-rust" => Ok(staged_linux_target_spec(
            PikaStagedLinuxTarget::PreMergePikachatRust,
            "check-pikachat",
            &[
                "Cargo.toml",
                "Cargo.lock",
                "cmd/pika-relay/**",
                "flake.nix",
                "flake.lock",
                "fixtures/**",
                "nix/**",
                "justfile",
                "just/checks.just",
                "cli/**",
                "crates/pikaci/**",
                "crates/pika-cloud/**",
                "crates/pika-desktop/**",
                "crates/pika-agent-protocol/**",
                "crates/hypernote-protocol/**",
                "crates/pikahut/**",
                "crates/pikachat-sidecar/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-test-utils/**",
                "crates/pika-relay-profiles/**",
                "pikachat-openclaw/**",
                "crates/pika-media/**",
                "crates/pika-tls/**",
                "rust/**",
                "tests/**",
                "scripts/pikaci-ci-run.sh",
                "scripts/pikaci-staged-linux-remote.sh",
                "scripts/ci-add-known-host.sh",
                "scripts/forge-nightly-pika-ui-android.sh",
                "scripts/pika-build-prewarm-workspace-deps.sh",
                "scripts/pika-build-run-workspace-deps.sh",
                "scripts/lib/pikaci-tools.sh",
            ],
            pikachat_rust_jobs(),
        )),
        "pre-merge-pikachat-typescript" => Ok(staged_linux_target_spec(
            PikaStagedLinuxTarget::PreMergePikachatTypescript,
            "check-pikachat-typescript",
            &[
                "crates/pikaci/**",
                "nix/ci/linux-rust.nix",
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
            PikaStagedLinuxTarget::PreMergePikachatOpenclawE2e,
            "check-pikachat-openclaw-e2e",
            &[
                "Cargo.toml",
                "Cargo.lock",
                "cmd/pika-relay/**",
                "flake.nix",
                "flake.lock",
                "fixtures/**",
                "nix/**",
                "justfile",
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
                "pikachat-openclaw/openclaw/extensions/pikachat-openclaw/**",
                "scripts/pikaci-ci-run.sh",
                "scripts/pikaci-staged-linux-remote.sh",
                "scripts/ci-add-known-host.sh",
                "scripts/forge-nightly-pika-ui-android.sh",
                "scripts/pika-build-prewarm-workspace-deps.sh",
                "scripts/pika-build-run-workspace-deps.sh",
                "scripts/lib/pikaci-tools.sh",
            ],
            pikachat_openclaw_e2e_jobs(),
        )),
        "pre-merge-pika-followup" => Ok(staged_linux_target_spec(
            PikaStagedLinuxTarget::PreMergePikaFollowup,
            "check-pika-followup",
            &[
                "AGENTS.md",
                ".agents/**",
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                ".github/**",
                "rust/**",
                "cli/**",
                "crates/pika-desktop/**",
                "crates/pika-media/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-nse/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-tls/**",
                "uniffi-bindgen/**",
                "android/**",
                "ios/**",
                "docs/**",
                "scripts/forge-nightly-pika-ui-android.sh",
                "scripts/**",
                "tools/**",
            ],
            pika_followup_jobs(),
        )),
        "pika-actionlint" => single_job_target_spec(
            "pika-actionlint",
            "pika-actionlint",
            "Run the staged Pika actionlint follow-up lane",
            &[],
            pika_followup_jobs(),
        ),
        "pika-doc-contracts" => single_job_target_spec(
            "pika-doc-contracts",
            "pika-doc-contracts",
            "Run the staged Pika docs/contracts follow-up lane",
            &[],
            pika_followup_jobs(),
        ),
        "pika-rust-deps-hygiene" => single_job_target_spec(
            "pika-rust-deps-hygiene",
            "pika-rust-deps-hygiene",
            "Run the staged Pika rust-deps-hygiene follow-up lane",
            &[],
            pika_followup_jobs(),
        ),
        "pika-core-lib-app-flows-tests" => single_job_target_spec(
            "pika-core-lib-app-flows-tests",
            "pika-core-lib-app-flows-tests",
            "Run the staged pika_core lib/app_flows lane",
            &[],
            pika_rust_jobs(),
        ),
        "pika-server-package-tests" => single_job_target_spec(
            "pika-server-package-tests",
            "pika-server-package-tests",
            "Run the staged notifications pika-server package-tests lane",
            &[],
            notification_jobs(),
        ),
        "pikachat-package-tests" => single_job_target_spec(
            "pikachat-package-tests",
            "pikachat-package-tests",
            "Run the staged pikachat package-tests lane",
            &[],
            pikachat_rust_jobs(),
        ),
        "pikahut-clippy" => single_job_target_spec(
            "pikahut-clippy",
            "pikahut-clippy",
            "Run the staged fixture pikahut clippy lane",
            &[],
            fixture_rust_jobs(),
        ),
        "pre-merge-pikachat-apple-followup" => Ok(TargetSpec {
            id: "pre-merge-pikachat-apple-followup",
            title: "pre-merge-pikachat-apple-followup",
            description: "Run the Apple-host pikachat follow-up after the staged Linux Rust lane",
            filters: static_filters(&[
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
            ]),
            ci_trigger: CiTrigger::None,
            jobs: pikachat_apple_followup_jobs(),
        }),
        "apple_host_sanity" => Ok(host_shell_target_spec(
            "apple_host_sanity",
            "check-apple-host-sanity",
            "Run the Apple host sanity lane via pikaci",
            &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                "just/checks.just",
                ".github/pikaci-apple.env",
                "crates/pika-cloud/**",
                "crates/pika-desktop/**",
                "crates/hypernote-protocol/**",
                "crates/pika-media/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-tls/**",
                "rust/**",
                "scripts/apple-host-prepare.sh",
                "scripts/lib/release-secrets.sh",
                "scripts/pikaci-apple-remote.sh",
                "tools/cargo-with-xcode",
                "tools/xcode-dev-dir",
                "tools/xcode-run",
            ],
            CiTrigger::Branch {
                concurrency_group: Some(ConcurrencyGroupSpec::Literal("apple-host")),
            },
            3600,
            "./scripts/pikaci-apple-remote.sh run --just-recipe apple-host-sanity",
        )),
        "apple_desktop_compile" => Ok(host_shell_target_spec(
            "apple_desktop_compile",
            "check-apple-desktop-compile",
            "Run the Apple desktop compile lane via pikaci",
            &[
                "ci/apple-host-assets.toml",
                "VERSION",
                "Cargo.toml",
                "Cargo.lock",
                "cli/Cargo.toml",
                "config/**",
                "crates/**/Cargo.toml",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                "just/checks.just",
                ".github/pikaci-apple.env",
                "crates/pika-cloud/**",
                "crates/pika-desktop/**",
                "crates/hypernote-protocol/**",
                "crates/pika-managed-agent-contract/**",
                "crates/pika-media/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-tls/**",
                "rust/**",
                "scripts/apple-host-prepare.sh",
                "scripts/apple-host-cache-key",
                "scripts/apple-host-record-phase",
                "scripts/apple-host-record-prepared-entry",
                "scripts/lib/release-secrets.sh",
                "scripts/pikaci-apple-remote.sh",
                "tools/apple-host-asset",
                "tools/cargo-with-xcode",
                "uniffi-bindgen/Cargo.toml",
                "tools/xcode-dev-dir",
                "tools/xcode-run",
            ],
            CiTrigger::Branch {
                concurrency_group: Some(ConcurrencyGroupSpec::Literal("apple-compile")),
            },
            3600,
            "./scripts/pikaci-apple-remote.sh run --just-recipe apple-host-desktop-compile",
        )),
        "apple_ios_compile" => Ok(host_shell_target_spec(
            "apple_ios_compile",
            "check-apple-ios-compile",
            "Run the Apple iOS compile lane via pikaci",
            &[
                "ci/apple-host-assets.toml",
                "VERSION",
                "Cargo.toml",
                "Cargo.lock",
                "cli/Cargo.toml",
                "config/**",
                "crates/**/Cargo.toml",
                "flake.nix",
                "flake.lock",
                "ios/**",
                "nix/**",
                "justfile",
                "just/checks.just",
                ".github/pikaci-apple.env",
                "crates/hypernote-protocol/**",
                "crates/pika-cloud/**",
                "crates/pika-managed-agent-contract/**",
                "crates/pika-media/**",
                "crates/pika-marmot-runtime/**",
                "crates/pika-nse/**",
                "crates/pika-relay-profiles/**",
                "crates/pika-share/**",
                "crates/pika-tls/**",
                "rust/**",
                "scripts/apple-host-prepare.sh",
                "scripts/apple-host-cache-key",
                "scripts/apple-host-record-phase",
                "scripts/apple-host-record-prepared-entry",
                "scripts/ios-build",
                "scripts/lib/mobile-build.sh",
                "scripts/pikaci-apple-host-bootstrap.sh",
                "scripts/pikaci-apple-remote.sh",
                "tools/apple-host-asset",
                "tools/lib/dotenv.sh",
                "tools/ios-runtime-doctor",
                "tools/ios-sim-ensure",
                "tools/xcode-dev-dir",
                "tools/xcode-run",
                "tools/xcodebuild-compact",
                "uniffi-bindgen/**",
            ],
            CiTrigger::Branch {
                concurrency_group: Some(ConcurrencyGroupSpec::Literal("apple-compile")),
            },
            3600,
            "./scripts/pikaci-apple-remote.sh run --just-recipe apple-host-ios-compile",
        )),
        "nightly_linux" => Ok(host_shell_target_spec(
            "nightly_linux",
            "nightly-linux",
            "Run the nightly Linux lane via pikaci",
            &[],
            CiTrigger::Nightly {
                concurrency_group: None,
            },
            14400,
            "nix develop .#default -c just rmp_tools::rmp-nightly-linux",
        )),
        "nightly_pika" => Ok(host_shell_target_spec(
            "nightly_pika",
            "nightly-pika",
            "Run the nightly Pika lane via pikaci",
            &[],
            CiTrigger::Nightly {
                concurrency_group: None,
            },
            14400,
            "nix develop .#default -c just checks::nightly-pika-e2e",
        )),
        "nightly_pikachat" => Ok(host_shell_target_spec(
            "nightly_pikachat",
            "nightly-pikachat",
            "Run the nightly Pikachat lane via pikaci",
            &[],
            CiTrigger::Nightly {
                concurrency_group: None,
            },
            14400,
            "nix develop .#default -c just checks::nightly-pikachat",
        )),
        "nightly_pika_ui_android" => Ok(host_shell_target_spec(
            "nightly_pika_ui_android",
            "nightly-pika-ui-android",
            "Run the nightly Android UI lane via pikaci",
            &[],
            CiTrigger::Nightly {
                concurrency_group: Some(ConcurrencyGroupSpec::Literal("nightly-android")),
            },
            14400,
            "./scripts/forge-nightly-pika-ui-android.sh",
        )),
        "nightly_apple_host_bundle" => Ok(host_shell_target_spec(
            "nightly_apple_host_bundle",
            "nightly-apple-host-bundle",
            "Run the nightly Apple host bundle lane via pikaci",
            &[],
            CiTrigger::Nightly {
                concurrency_group: Some(ConcurrencyGroupSpec::Literal("apple-runtime")),
            },
            14400,
            "./scripts/pikaci-apple-remote.sh run --just-recipe apple-host-bundle",
        )),
        "pika-desktop-package-tests" => single_job_target_spec(
            "pika-desktop-package-tests",
            "pika-desktop-package-tests",
            "Run pika-desktop package tests in a remote Linux VM",
            &[],
            pikachat_rust_jobs(),
        ),
        "pre-merge-fixture-rust" => Ok(staged_linux_target_spec(
            PikaStagedLinuxTarget::PreMergeFixtureRust,
            "check-fixture",
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
                "docs/testing/ci-selectors.md",
                "docs/testing/integration-matrix.md",
                "docs/testing/wrapper-deprecation-policy.md",
                "cli/Cargo.toml",
                "crates/pikaci/**",
                "crates/hypernote-protocol/**",
                "crates/pika-cloud/**",
                "crates/pika-desktop/**",
                "crates/pikahut/**",
                "scripts/pikaci-ci-run.sh",
                "scripts/pikaci-staged-linux-remote.sh",
                "scripts/ci-add-known-host.sh",
                "scripts/forge-nightly-pika-ui-android.sh",
                "scripts/pika-build-prewarm-workspace-deps.sh",
                "scripts/pika-build-run-workspace-deps.sh",
                "scripts/lib/pikaci-tools.sh",
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
            title: "android-sdk-probe",
            description: "Verify Android SDK tooling is available inside a Linux guest",
            filters: Vec::new(),
            ci_trigger: CiTrigger::None,
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
                runtime_config: remote_incus_runtime(None),
                ..remote_incus_job_base()
            }],
        }),
        "android-nostr-connect-intent-test" => Ok(TargetSpec {
            id: "android-nostr-connect-intent-test",
            title: "android-nostr-connect-intent-test",
            description: "Run the smallest Android instrumentation class in a Linux guest",
            filters: Vec::new(),
            ci_trigger: CiTrigger::None,
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
                runtime_config: remote_incus_runtime(None),
                ..remote_incus_job_base()
            }],
        }),
        "tart-env-probe" => Ok(TargetSpec {
            id: "tart-env-probe",
            title: "tart-env-probe",
            description: "Probe the local Tart macOS guest environment",
            filters: Vec::new(),
            ci_trigger: CiTrigger::None,
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
                runtime_config: tart_runtime(true),
                ..tart_job_base()
            }],
        }),
        "tart-ios-agent-button-state-test" => Ok(TargetSpec {
            id: "tart-ios-agent-button-state-test",
            title: "tart-ios-agent-button-state-test",
            description: "Run one iOS unit test in a Tart macOS guest",
            filters: Vec::new(),
            ci_trigger: CiTrigger::None,
            jobs: vec![tart_agent_button_job(
                "tart-ios-agent-button-state-test",
                "Run AgentTests.testAgentButtonStateReflectsBusyFlag in a Tart macOS guest",
            )],
        }),
        "tart-ios-unit-tests" => Ok(TargetSpec {
            id: "tart-ios-unit-tests",
            title: "tart-ios-unit-tests",
            description: "Run deterministic iOS unit-test suites in a Tart macOS guest",
            filters: Vec::new(),
            ci_trigger: CiTrigger::None,
            jobs: tart_ios_unit_jobs(),
        }),
        "tart-ios-ui-test" => Ok(TargetSpec {
            id: "tart-ios-ui-test",
            title: "tart-ios-ui-test",
            description: "Run the deterministic ios-ui-test lane in a Tart macOS guest",
            filters: Vec::new(),
            ci_trigger: CiTrigger::None,
            jobs: vec![tart_ios_ui_test_job(
                "tart-ios-ui-test",
                "Run the deterministic ios-ui-test lane in a Tart macOS guest",
            )],
        }),
        "tart-ios-ui-note-to-self" => Ok(TargetSpec {
            id: "tart-ios-ui-note-to-self",
            title: "tart-ios-ui-note-to-self",
            description: "Run one deterministic iOS UI test in a Tart macOS guest",
            filters: Vec::new(),
            ci_trigger: CiTrigger::None,
            jobs: vec![tart_ios_ui_note_to_self_job(
                "tart-ios-ui-note-to-self",
                "Run the note-to-self UI test in a Tart macOS guest",
            )],
        }),
        "tart-desktop-package-tests" => Ok(TargetSpec {
            id: "tart-desktop-package-tests",
            title: "tart-desktop-package-tests",
            description: "Run pika-desktop package tests in a Tart macOS guest",
            filters: Vec::new(),
            ci_trigger: CiTrigger::None,
            jobs: vec![tart_desktop_package_tests_job(
                "tart-desktop-package-tests",
                "Run pika-desktop package tests in a Tart macOS guest",
            )],
        }),
        "pre-merge-apple-deterministic" => Ok(TargetSpec {
            id: "pre-merge-apple-deterministic",
            title: "pre-merge-apple-deterministic",
            description: "Run deterministic Apple-platform tests in a Tart macOS guest",
            filters: static_filters(&[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                "crates/pikaci/**",
                "crates/pika-desktop/**",
                "ios/**",
                "rust/**",
                "tools/cargo-with-xcode",
                "tools/ios-sim-ensure",
                "tools/xcode-dev-dir",
                "tools/xcode-run",
                "tools/xcodebuild-compact",
            ]),
            ci_trigger: CiTrigger::None,
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
            PikaStagedLinuxTarget::PreMergeRmp,
            "check-rmp",
            &[
                "Cargo.toml",
                "Cargo.lock",
                "flake.nix",
                "flake.lock",
                "nix/**",
                "justfile",
                "crates/rmp-cli/**",
                "scripts/pikaci-staged-linux-remote.sh",
                "scripts/ci-add-known-host.sh",
                "scripts/forge-nightly-pika-ui-android.sh",
                "scripts/pika-build-prewarm-workspace-deps.sh",
                "scripts/pika-build-run-workspace-deps.sh",
                "scripts/lib/pikaci-tools.sh",
            ],
            rmp_jobs(),
        )),
        "rmp-init-smoke-ci" => single_job_target_spec(
            "rmp-init-smoke-ci",
            "rmp-init-smoke-ci",
            "Run the staged RMP init smoke lane",
            &[],
            rmp_jobs(),
        ),
        other => bail!("unknown job `{other}`"),
    }
}

fn agent_contract_jobs() -> Vec<JobSpec> {
    vec![
        JobSpec {
            id: "pika-cloud-unit",
            description: "Run all pika-cloud unit tests in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageUnitTests {
                package: "pika-cloud",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::AgentContractsPikaCloudUnit.command_config(),
            )),
            ..remote_incus_job_base()
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
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::AgentContractsServerAgentApi.command_config(),
            )),
            ..remote_incus_job_base()
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
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::AgentContractsCoreNip98.command_config(),
            )),
            ..remote_incus_job_base()
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
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikaCoreLibAppFlows.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "pika-core-messaging-e2e-tests",
            description: "Run pika_core messaging and group profile integration tests in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pika_core --test e2e_messaging --test e2e_group_profiles -- --nocapture",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikaCoreMessagingE2e.command_config(),
            )),
            ..remote_incus_job_base()
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
        runtime_config: remote_incus_runtime(Some(
            PikaStagedLinuxLane::NotificationsServerPackageTests.command_config(),
        )),
        ..remote_incus_job_base()
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
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikachatPackageTests.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "pikachat-sidecar-package-tests",
            description: "Run pikachat-sidecar package tests with deterministic TTS in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "PIKACHAT_TTS_FIXTURE=1 cargo test -p pikachat-sidecar -- --nocapture",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikachatSidecarPackageTests.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "pika-desktop-package-tests",
            description: "Run pika-desktop package tests in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageTests {
                package: "pika-desktop",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikachatDesktopPackageTests.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "pikachat-cli-smoke-local",
            description: "Run the pikahut cli_smoke_local deterministic test in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikachatCliSmokeLocal.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "pikachat-post-rebase-invalid-event",
            description: "Run the post_rebase_invalid_event_rejection boundary test in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic post_rebase_invalid_event_rejection_boundary -- --ignored --nocapture",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikachatPostRebaseInvalidEvent.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "pikachat-post-rebase-logout-session",
            description: "Run the post_rebase_logout_session_convergence boundary test in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic post_rebase_logout_session_convergence_boundary -- --ignored --nocapture",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikachatPostRebaseLogoutSession.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "openclaw-invite-and-chat",
            description: "Run the OpenClaw invite-and-chat scenario in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat -- --ignored --nocapture",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::OpenclawInviteAndChat.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "openclaw-invite-and-chat-rust-bot",
            description: "Run the OpenClaw invite-and-chat rust bot scenario in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_rust_bot -- --ignored --nocapture",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::OpenclawInviteAndChatRustBot.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "openclaw-invite-and-chat-daemon",
            description: "Run the OpenClaw invite-and-chat daemon scenario in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_daemon -- --ignored --nocapture",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::OpenclawInviteAndChatDaemon.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "openclaw-audio-echo",
            description: "Run the OpenClaw audio echo scenario in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pikahut --test integration_deterministic openclaw_scenario_audio_echo -- --ignored --nocapture",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::OpenclawAudioEcho.command_config(),
            )),
            ..remote_incus_job_base()
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
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikaFollowupAndroidTestCompile.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "pikachat-build",
            description: "Build pikachat in a remote Linux VM guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo build -p pikachat",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikaFollowupPikachatBuild.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "pika-desktop-check",
            description: "Run desktop-check in a remote Linux VM guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "just desktop-check",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikaFollowupDesktopCheck.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "pika-actionlint",
            description: "Run actionlint in a remote Linux VM guest",
            timeout_secs: 600,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "actionlint",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikaFollowupActionlint.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "pika-doc-contracts",
            description: "Run docs and justfile contract checks in a remote Linux VM guest",
            timeout_secs: 900,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "npx --yes @justinmoon/agent-tools check-docs && npx --yes @justinmoon/agent-tools check-justfile",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikaFollowupDocContracts.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "pika-rust-deps-hygiene",
            description: "Run cargo machete dependency hygiene in a remote Linux VM guest",
            timeout_secs: 900,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "./scripts/check-cargo-machete",
            },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::PikaFollowupRustDepsHygiene.command_config(),
            )),
            ..remote_incus_job_base()
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
        runtime_config: remote_incus_runtime(Some(
            PikaStagedLinuxLane::OpenclawGatewayE2e.command_config(),
        )),
        ..remote_incus_job_base()
    }]
}

fn pikachat_typescript_jobs() -> Vec<JobSpec> {
    vec![JobSpec {
        id: "pikachat-typescript",
        description: "Run the Claude and OpenClaw TypeScript typecheck and unit tests in a remote Linux VM",
        timeout_secs: 1800,
        writable_workspace: false,
        guest_command: GuestCommand::ShellCommand { command: "ignored" },
        runtime_config: remote_incus_runtime(Some(
            PikaStagedLinuxLane::PikachatTypescript.command_config(),
        )),
        ..remote_incus_job_base()
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
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::FixturePikahutClippy.command_config(),
            )),
            ..remote_incus_job_base()
        },
        JobSpec {
            id: "fixture-relay-smoke",
            description: "Exercise the relay-profile pikahut smoke flow in a remote Linux VM",
            timeout_secs: 300,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand { command: "ignored" },
            runtime_config: remote_incus_runtime(Some(
                PikaStagedLinuxLane::FixtureRelaySmoke.command_config(),
            )),
            ..remote_incus_job_base()
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
            runtime_config: JobRuntimeConfig::HostProcess(HostProcessRuntimeConfig),
            ..host_local_job_base()
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
            runtime_config: JobRuntimeConfig::HostProcess(HostProcessRuntimeConfig),
            ..host_local_job_base()
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
            runtime_config: JobRuntimeConfig::HostProcess(HostProcessRuntimeConfig),
            ..host_local_job_base()
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
            runtime_config: JobRuntimeConfig::HostProcess(HostProcessRuntimeConfig),
            ..host_local_job_base()
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
        runtime_config: remote_incus_runtime(Some(
            PikaStagedLinuxLane::RmpInitSmokeCi.command_config(),
        )),
        ..remote_incus_job_base()
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
        runtime_config: tart_runtime(true),
        ..tart_job_base()
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
        runtime_config: tart_runtime(false),
        ..tart_job_base()
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
        runtime_config: tart_runtime(false),
        ..tart_job_base()
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
        runtime_config: tart_runtime(false),
        ..tart_job_base()
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
        runtime_config: tart_runtime(false),
        ..tart_job_base()
    }
}
