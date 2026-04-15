use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::util::{
    command_exists, default_code_checkout_dir, env_truthy, env_var_present, non_empty_env_path,
    resolve_openclaw_dir_default,
};

/// Environment capability snapshot used for test gating.
///
/// # Examples
///
/// ```no_run
/// use pikahut::testing::{Capabilities, Requirement};
///
/// let caps = Capabilities::probe(std::path::Path::new("."));
/// match caps.require_or_skip_outcome(Requirement::HostMacOs) {
///     pikahut::testing::RequireOutcome::Proceed => {}
///     pikahut::testing::RequireOutcome::Skip(reason) => eprintln!("SKIP: {reason}"),
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Capabilities {
    pub host_macos: bool,
    pub has_xcode: bool,
    pub has_android_tools: bool,
    pub has_android_avd: bool,
    pub android_avd_name: String,
    pub physical_ios_udid: Option<String>,
    pub has_openclaw_checkout: bool,
    pub has_interop_rust_repo: bool,
    pub has_primal_repo: bool,
    pub has_pika_test_nsec: bool,
    pub has_public_network: bool,
    pub openclaw_dir: PathBuf,
    pub interop_rust_dir: PathBuf,
    pub primal_repo_dir: PathBuf,
}

/// A test requirement that can resolve to either `ok` or explicit skip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requirement {
    HostMacOs,
    Xcode,
    AndroidTools,
    AndroidEmulatorAvd,
    PhysicalIosUdid,
    OpenclawCheckout,
    InteropRustRepo,
    PrimalRepo,
    EnvSecretPikaTestNsec,
    EnvVar { name: &'static str },
    PublicNetwork,
}

/// Decision helper for test callsites that need explicit skip outcomes.
#[derive(Debug, Clone)]
pub enum RequireOutcome {
    Proceed,
    Skip(SkipReason),
}

/// Structured skip reason returned by capability gates.
#[derive(Debug, Clone)]
pub struct SkipReason {
    pub requirement: Requirement,
    pub reason: String,
}

impl SkipReason {
    pub fn new(requirement: Requirement, reason: impl Into<String>) -> Self {
        Self {
            requirement,
            reason: reason.into(),
        }
    }
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.requirement, self.reason)
    }
}

impl Error for SkipReason {}

impl Capabilities {
    pub fn probe(workspace_root: &Path) -> Self {
        let openclaw_dir = resolve_openclaw_dir(workspace_root);
        let interop_rust_dir = resolve_interop_rust_dir();
        let primal_repo_dir = resolve_primal_repo_dir();
        let android_avd_name = std::env::var("PIKA_ANDROID_AVD_NAME")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "pika_api35".to_string());
        let has_android_tools = command_exists("adb") && command_exists("emulator");

        Self {
            host_macos: cfg!(target_os = "macos"),
            has_xcode: command_exists("xcodebuild"),
            has_android_tools,
            has_android_avd: has_android_tools && android_avd_exists(&android_avd_name),
            android_avd_name,
            physical_ios_udid: resolve_physical_ios_udid(),
            has_openclaw_checkout: openclaw_dir.join("package.json").is_file(),
            has_interop_rust_repo: interop_rust_dir.is_dir(),
            has_primal_repo: primal_repo_dir.join(".git").is_dir(),
            has_pika_test_nsec: env_var_present("PIKA_TEST_NSEC"),
            has_public_network: !env_truthy("PIKAHUT_ASSUME_OFFLINE"),
            openclaw_dir,
            interop_rust_dir,
            primal_repo_dir,
        }
    }

    pub fn require_or_skip_outcome(&self, requirement: Requirement) -> RequireOutcome {
        match self.require_or_skip(requirement) {
            Ok(()) => RequireOutcome::Proceed,
            Err(reason) => RequireOutcome::Skip(reason),
        }
    }

    pub fn require_all_or_skip(&self, requirements: &[Requirement]) -> Result<(), SkipReason> {
        for requirement in requirements {
            self.require_or_skip(requirement.clone())?;
        }
        Ok(())
    }

    pub fn require_or_skip(&self, requirement: Requirement) -> Result<(), SkipReason> {
        match requirement {
            Requirement::HostMacOs => {
                if self.host_macos {
                    Ok(())
                } else {
                    Err(SkipReason::new(
                        Requirement::HostMacOs,
                        "requires macOS runner",
                    ))
                }
            }
            Requirement::Xcode => {
                if self.has_xcode {
                    Ok(())
                } else {
                    Err(SkipReason::new(
                        Requirement::Xcode,
                        "xcodebuild not found on PATH",
                    ))
                }
            }
            Requirement::AndroidTools => {
                if self.has_android_tools {
                    Ok(())
                } else {
                    Err(SkipReason::new(
                        Requirement::AndroidTools,
                        "requires adb + emulator tools on PATH",
                    ))
                }
            }
            Requirement::AndroidEmulatorAvd => {
                if self.has_android_avd {
                    Ok(())
                } else {
                    Err(SkipReason::new(
                        Requirement::AndroidEmulatorAvd,
                        format!(
                            "requires Android AVD '{}' (set PIKA_ANDROID_AVD_NAME)",
                            self.android_avd_name
                        ),
                    ))
                }
            }
            Requirement::PhysicalIosUdid => {
                if self.physical_ios_udid.is_some() {
                    Ok(())
                } else {
                    Err(SkipReason::new(
                        Requirement::PhysicalIosUdid,
                        "requires physical iOS UDID (set PIKA_IOS_DEVICE_UDID)",
                    ))
                }
            }
            Requirement::OpenclawCheckout => {
                if self.has_openclaw_checkout {
                    Ok(())
                } else {
                    Err(SkipReason::new(
                        Requirement::OpenclawCheckout,
                        format!(
                            "missing OpenClaw checkout at {} (set OPENCLAW_DIR)",
                            self.openclaw_dir.display()
                        ),
                    ))
                }
            }
            Requirement::InteropRustRepo => {
                if self.has_interop_rust_repo {
                    Ok(())
                } else {
                    Err(SkipReason::new(
                        Requirement::InteropRustRepo,
                        format!(
                            "missing rust interop repo at {} (set PIKACHAT_INTEROP_RUST_DIR)",
                            self.interop_rust_dir.display()
                        ),
                    ))
                }
            }
            Requirement::PrimalRepo => {
                if self.has_primal_repo {
                    Ok(())
                } else {
                    Err(SkipReason::new(
                        Requirement::PrimalRepo,
                        format!(
                            "missing Primal repo at {} (set PIKA_PRIMAL_SRC_DIR)",
                            self.primal_repo_dir.display()
                        ),
                    ))
                }
            }
            Requirement::EnvSecretPikaTestNsec => {
                if self.has_pika_test_nsec {
                    Ok(())
                } else {
                    Err(SkipReason::new(
                        Requirement::EnvSecretPikaTestNsec,
                        "PIKA_TEST_NSEC is not set",
                    ))
                }
            }
            Requirement::EnvVar { name } => {
                if env_var_present(name) {
                    Ok(())
                } else {
                    Err(SkipReason::new(
                        Requirement::EnvVar { name },
                        format!("required env var is missing: {name}"),
                    ))
                }
            }
            Requirement::PublicNetwork => {
                if self.has_public_network {
                    Ok(())
                } else {
                    Err(SkipReason::new(
                        Requirement::PublicNetwork,
                        "network-dependent flow disabled by PIKAHUT_ASSUME_OFFLINE",
                    ))
                }
            }
        }
    }
}

fn android_avd_exists(avd_name: &str) -> bool {
    if !command_exists("emulator") {
        return false;
    }

    Command::new("emulator")
        .arg("-list-avds")
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.trim() == avd_name)
        })
        .unwrap_or(false)
}

fn resolve_physical_ios_udid() -> Option<String> {
    for key in ["PIKA_IOS_DEVICE_UDID", "PIKA_IOS_UDID"] {
        if let Ok(value) = std::env::var(key)
            && !value.trim().is_empty()
        {
            return Some(value.trim().to_string());
        }
    }

    if !cfg!(target_os = "macos") || !command_exists("xcrun") {
        return None;
    }

    let output = Command::new("xcrun")
        .args(["xctrace", "list", "devices"])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.contains("Simulator") {
            continue;
        }
        if let Some(udid) = extract_udid_from_line(line) {
            return Some(udid);
        }
    }

    None
}

fn extract_udid_from_line(line: &str) -> Option<String> {
    let mut candidates = Vec::new();

    for segment in line.split(['(', ')']) {
        let value = segment.trim();
        if value.len() < 24 || !value.contains('-') {
            continue;
        }
        if value.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-') {
            candidates.push(value.to_string());
        }
    }

    candidates.pop()
}

fn resolve_openclaw_dir(workspace_root: &Path) -> PathBuf {
    if let Some(path) = non_empty_env_path("OPENCLAW_DIR") {
        return path;
    }
    resolve_openclaw_dir_default(workspace_root)
}

fn resolve_interop_rust_dir() -> PathBuf {
    if let Some(path) = non_empty_env_path("PIKACHAT_INTEROP_RUST_DIR") {
        return path;
    }
    default_code_checkout_dir("pika-interop-lab-rust")
}

fn resolve_primal_repo_dir() -> PathBuf {
    if let Some(path) = non_empty_env_path("PIKA_PRIMAL_SRC_DIR") {
        return path;
    }
    default_code_checkout_dir("primal-ios-app")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline_caps() -> Capabilities {
        Capabilities {
            host_macos: false,
            has_xcode: false,
            has_android_tools: false,
            has_android_avd: false,
            android_avd_name: "pika_api35".to_string(),
            physical_ios_udid: None,
            has_openclaw_checkout: false,
            has_interop_rust_repo: false,
            has_primal_repo: false,
            has_pika_test_nsec: false,
            has_public_network: true,
            openclaw_dir: PathBuf::from("/tmp/openclaw"),
            interop_rust_dir: PathBuf::from("/tmp/interop"),
            primal_repo_dir: PathBuf::from("/tmp/primal"),
        }
    }

    #[test]
    fn require_outcome_returns_skip() {
        let caps = baseline_caps();
        let outcome = caps.require_or_skip_outcome(Requirement::HostMacOs);
        match outcome {
            RequireOutcome::Proceed => panic!("expected skip"),
            RequireOutcome::Skip(skip) => {
                assert_eq!(skip.requirement, Requirement::HostMacOs);
                assert!(skip.reason.contains("macOS"));
            }
        }
    }

    #[test]
    fn require_all_or_skip_returns_first_failure() {
        let caps = baseline_caps();
        let err = caps
            .require_all_or_skip(&[Requirement::HostMacOs, Requirement::Xcode])
            .unwrap_err();
        assert_eq!(err.requirement, Requirement::HostMacOs);
    }

    #[test]
    fn env_var_requirement_reports_missing_name() {
        let caps = baseline_caps();
        let err = caps
            .require_or_skip(Requirement::EnvVar {
                name: "PIKA_TEST_MISSING_ENV",
            })
            .unwrap_err();
        assert_eq!(
            err.requirement,
            Requirement::EnvVar {
                name: "PIKA_TEST_MISSING_ENV"
            }
        );
        assert!(err.reason.contains("PIKA_TEST_MISSING_ENV"));
    }

    #[test]
    fn extract_udid_from_line_parses_device_line() {
        let line = "Justin's iPhone (17.4) (00008140-001E54E90E6A801C)";
        assert_eq!(
            extract_udid_from_line(line).as_deref(),
            Some("00008140-001E54E90E6A801C")
        );
    }
}
