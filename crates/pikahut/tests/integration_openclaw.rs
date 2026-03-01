use anyhow::Result;

use pikahut::testing::{
    ArtifactPolicy, Requirement, scenarios, scenarios::OpenclawE2eRequest,
    skip_if_missing_requirements,
};

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_path(name: &str) -> Option<std::path::PathBuf> {
    env_opt(name).map(std::path::PathBuf::from)
}

#[tokio::test]
#[ignore = "heavy integration lane (OpenClaw checkout + network)"]
async fn openclaw_gateway_e2e() -> Result<()> {
    if skip_if_missing_requirements(
        &workspace_root(),
        &[Requirement::OpenclawCheckout, Requirement::PublicNetwork],
    ) {
        return Ok(());
    }

    let mut context = pikahut::testing::TestContext::builder("openclaw-gateway-e2e")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;

    let result = scenarios::run_openclaw_e2e(OpenclawE2eRequest {
        state_dir: env_path("PIKAHUT_OPENCLAW_E2E_STATE_DIR")
            .or_else(|| Some(context.state_dir().to_path_buf())),
        relay_url: env_opt("PIKAHUT_OPENCLAW_E2E_RELAY_URL"),
        openclaw_dir: env_path("PIKAHUT_OPENCLAW_E2E_OPENCLAW_DIR"),
        keep_state: false,
    })
    .await;

    if result.is_ok() {
        context.mark_success();
    } else {
        eprintln!(
            "openclaw e2e failed; preserved artifacts at {}",
            context.state_dir().join("artifacts/openclaw-e2e").display()
        );
    }

    result.map(|_| ())
}
