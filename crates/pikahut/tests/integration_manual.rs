use anyhow::Result;

use pikahut::testing::{Requirement, skip_if_missing_requirements};

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

#[test]
#[ignore = "manual selector: interop runbook contract"]
fn manual_interop_rust_runbook_contract() -> Result<()> {
    if skip_if_missing_requirements(&workspace_root(), &[Requirement::InteropRustRepo]) {
        return Ok(());
    }

    eprintln!(
        "manual contract: run `cargo test -p pikahut --test integration_manual manual_interop_rust_runbook_contract -- --ignored --nocapture` and follow interop runbook output."
    );
    eprintln!("manual contract: `just interop-rust-manual` remains the DX wrapper.");
    Ok(())
}

#[test]
#[ignore = "manual selector: primal lab runbook contract"]
fn manual_primal_lab_runbook_contract() -> Result<()> {
    if skip_if_missing_requirements(
        &workspace_root(),
        &[
            Requirement::HostMacOs,
            Requirement::Xcode,
            Requirement::PrimalRepo,
        ],
    ) {
        return Ok(());
    }

    eprintln!(
        "manual contract: run `cargo test -p pikahut --test integration_manual manual_primal_lab_runbook_contract -- --ignored --nocapture` before executing primal lab flows."
    );
    eprintln!("manual contract: `just primal-ios-lab` remains the DX wrapper.");
    Ok(())
}
