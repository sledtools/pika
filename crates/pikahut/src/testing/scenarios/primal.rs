use std::collections::BTreeMap;

use anyhow::Result;

use crate::config;
use crate::testing::{ArtifactPolicy, CommandRunner, CommandSpec, TestContext};

use super::artifacts::{self, CommandOutcomeRecord};
use super::types::ScenarioRunOutput;

pub fn run_primal_nostrconnect_smoke() -> Result<ScenarioRunOutput> {
    let root = config::find_workspace_root()?;
    let mut context = TestContext::builder("primal-nostrconnect-smoke")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    let runner = CommandRunner::new(&context);

    let artifact_dir = context.ensure_artifact_subdir("primal-nightly")?;
    let command = runner.run(
        &CommandSpec::new("./tools/primal-ios-interop-nightly")
            .cwd(&root)
            .env(
                "PIKA_PRIMAL_ARTIFACT_DIR",
                artifact_dir.to_string_lossy().to_string(),
            )
            .capture_name("primal-nightly-smoke"),
    )?;

    let mut result = ScenarioRunOutput::completed(context.state_dir().to_path_buf())
        .with_artifact(command.stdout_path.clone())
        .with_artifact(command.stderr_path.clone());
    let summary = artifacts::write_standard_summary(
        &context,
        "primal::nostrconnect_smoke",
        &result,
        vec![CommandOutcomeRecord::from_output(
            "primal-nightly-smoke",
            &command,
        )],
        BTreeMap::new(),
    )?;
    result = result.with_summary(summary);

    context.mark_success();
    Ok(result)
}
