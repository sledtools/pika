use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub enum GuestCommand {
    ExactCargoTest {
        package: &'static str,
        test_name: &'static str,
    },
    PackageUnitTests {
        package: &'static str,
    },
    PackageTests {
        package: &'static str,
    },
    FilteredCargoTests {
        package: &'static str,
        filter: &'static str,
    },
    ShellCommand {
        command: &'static str,
    },
    ShellCommandAsRoot {
        command: &'static str,
    },
}

#[derive(Clone, Debug)]
pub struct JobSpec {
    pub id: &'static str,
    pub description: &'static str,
    pub timeout_secs: u64,
    pub writable_workspace: bool,
    pub guest_command: GuestCommand,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Passed,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobOutcome {
    pub status: RunStatus,
    pub exit_code: Option<i32>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobRecord {
    pub id: String,
    pub description: String,
    pub status: RunStatus,
    pub executor: String,
    pub timeout_secs: u64,
    pub host_log_path: String,
    pub guest_log_path: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunRecord {
    pub run_id: String,
    pub status: RunStatus,
    #[serde(default)]
    pub rerun_of: Option<String>,
    #[serde(default)]
    pub target_id: Option<String>,
    #[serde(default)]
    pub target_description: Option<String>,
    pub source_root: String,
    pub snapshot_dir: String,
    pub git_head: Option<String>,
    pub git_dirty: Option<bool>,
    pub created_at: String,
    pub finished_at: Option<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub filters: Vec<String>,
    #[serde(default)]
    pub message: Option<String>,
    pub jobs: Vec<JobRecord>,
}
