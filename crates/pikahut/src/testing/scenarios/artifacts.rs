use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::testing::{CommandOutput, TestContext};

use super::types::ScenarioRunOutput;

#[derive(Debug, Clone, Serialize)]
pub struct CommandOutcomeRecord {
    pub command: String,
    pub status: i32,
    pub attempts: u32,
    pub stdout_path: String,
    pub stderr_path: String,
}

impl CommandOutcomeRecord {
    pub fn from_output(command: impl Into<String>, output: &CommandOutput) -> Self {
        Self {
            command: command.into(),
            status: output.status.code().unwrap_or(-1),
            attempts: output.attempts,
            stdout_path: output.stdout_path.to_string_lossy().to_string(),
            stderr_path: output.stderr_path.to_string_lossy().to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ScenarioSummaryRecord {
    scenario: String,
    run_name: String,
    run_id: String,
    state_dir: Option<String>,
    skipped: bool,
    skip_reason: Option<String>,
    artifacts: Vec<String>,
    summary_path: Option<String>,
    metadata: BTreeMap<String, String>,
    command_outcomes: Vec<CommandOutcomeRecord>,
}

pub fn write_standard_summary(
    context: &TestContext,
    scenario: &str,
    output: &ScenarioRunOutput,
    command_outcomes: Vec<CommandOutcomeRecord>,
    mut metadata: BTreeMap<String, String>,
) -> Result<PathBuf> {
    metadata.insert(
        "artifact_policy".to_string(),
        format!("{:?}", context.artifact_policy()),
    );
    metadata.insert(
        "workspace_root".to_string(),
        context.workspace_root().to_string_lossy().to_string(),
    );

    let record = ScenarioSummaryRecord {
        scenario: scenario.to_string(),
        run_name: context.run_name().to_string(),
        run_id: context.run_id().to_string(),
        state_dir: output
            .state_dir
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        skipped: output.skipped,
        skip_reason: output.skip_reason.clone(),
        artifacts: output
            .artifacts
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
        summary_path: output
            .summary
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        metadata,
        command_outcomes,
    };

    let summary_json = serde_json::to_vec_pretty(&record)?;
    context.write_artifact("summary/summary.json", summary_json)
}

pub fn write_failure_tail(
    context: &TestContext,
    name: &str,
    log_path: &Path,
    line_count: usize,
) -> Result<PathBuf> {
    let content = std::fs::read_to_string(log_path).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(line_count);
    let tail = lines[start..].join("\n");
    context.write_artifact(format!("failures/{name}.tail.log"), tail)
}
