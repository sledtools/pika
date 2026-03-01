use anyhow::Result;

use pikahut::testing::scenarios::primal;
use pikahut::testing::{Requirement, skip_if_missing_requirements};

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

#[test]
#[ignore = "nightly macOS primal interop lane"]
fn primal_nostrconnect_smoke() -> Result<()> {
    if skip_if_missing_requirements(
        &workspace_root(),
        &[
            Requirement::HostMacOs,
            Requirement::Xcode,
            Requirement::PrimalRepo,
            Requirement::PublicNetwork,
        ],
    ) {
        return Ok(());
    }

    primal::run_primal_nostrconnect_smoke().map(|_| ())
}
