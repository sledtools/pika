use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn clean_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != ':' && ch != '*')
        .to_string()
}

fn indent_width(line: &str) -> usize {
    line.chars().take_while(|ch| *ch == ' ').count()
}

fn extract_paths_filter_entries(workflow: &str, filter_name: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut in_filters = false;
    let mut filters_indent = 0usize;
    let mut current_filter: Option<&str> = None;

    for line in workflow.lines() {
        if !in_filters {
            if line.trim() == "filters: |" {
                in_filters = true;
                filters_indent = indent_width(line);
            }
            continue;
        }

        let indent = indent_width(line);
        let trimmed = line.trim_start();

        if !trimmed.is_empty() && indent <= filters_indent {
            break;
        }
        if trimmed.is_empty() {
            continue;
        }

        if indent == filters_indent + 2 && trimmed.ends_with(':') {
            current_filter = Some(trimmed.trim_end_matches(':'));
            continue;
        }

        if current_filter == Some(filter_name)
            && indent >= filters_indent + 4
            && trimmed.starts_with("- ")
        {
            entries.push(
                trimmed
                    .trim_start_matches("- ")
                    .trim_matches('\'')
                    .trim_matches('"')
                    .to_string(),
            );
        }
    }

    entries
}

fn extract_just_recipe_body(text: &str, recipe_name: &str) -> Option<String> {
    let mut lines = text.lines().peekable();
    let header = format!("{recipe_name}:");

    while let Some(line) = lines.next() {
        if line == header {
            let mut body = String::new();
            while let Some(next) = lines.peek().copied() {
                if !next.is_empty() && !next.starts_with(' ') && !next.starts_with('\t') {
                    break;
                }
                body.push_str(next);
                body.push('\n');
                lines.next();
            }
            return Some(body);
        }
    }

    None
}

fn parse_cli_selector_refs(text: &str) -> HashSet<String> {
    let mut selectors = HashSet::new();
    let tokens: Vec<&str> = text.split_whitespace().collect();
    for idx in 0..tokens.len() {
        if tokens[idx] != "--test" || idx + 2 >= tokens.len() {
            continue;
        }
        let test_target = clean_token(tokens[idx + 1]);
        let selector = clean_token(tokens[idx + 2]);
        if test_target.starts_with("integration_")
            && !selector.is_empty()
            && selector
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            selectors.insert(format!("{test_target}::{selector}"));
        }
    }
    selectors
}

fn parse_docs_selector_refs(text: &str) -> HashSet<String> {
    let mut selectors = HashSet::new();
    for raw in text.split_whitespace() {
        let token = clean_token(raw);
        if token.contains('{') || token.contains('}') {
            continue;
        }
        if let Some((target, selector)) = token.split_once("::")
            && target.starts_with("integration_")
            && !selector.is_empty()
        {
            selectors.insert(format!("{target}::{selector}"));
        }
    }
    selectors
}

fn collect_actual_selectors(root: &Path) -> Result<HashSet<String>> {
    let tests_dir = root.join("crates/pikahut/tests");
    let mut selectors = HashSet::new();
    for entry in fs::read_dir(tests_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !stem.starts_with("integration_") {
            continue;
        }
        let content = fs::read_to_string(&path)?;
        for line in content.lines() {
            let trimmed = line.trim_start();
            let name = if let Some(rest) = trimmed.strip_prefix("fn ") {
                rest.split('(').next().map(str::trim)
            } else if let Some(rest) = trimmed.strip_prefix("async fn ") {
                rest.split('(').next().map(str::trim)
            } else {
                None
            };

            if let Some(name) = name
                && !name.is_empty()
            {
                selectors.insert(format!("{stem}::{name}"));
            }
        }
    }
    Ok(selectors)
}

#[test]
fn ci_just_recipes_use_selector_contracts() -> Result<()> {
    let justfile = fs::read_to_string(workspace_root().join("justfile"))?;

    let required = [
        "cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture",
        "cargo test -p pikahut --test integration_deterministic interop_rust_baseline -- --ignored --nocapture",
        "cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat -- --ignored --nocapture",
        "cargo test -p pikahut --test integration_deterministic post_rebase_invalid_event_rejection_boundary -- --ignored --nocapture",
        "cargo test -p pikahut --test integration_deterministic post_rebase_logout_session_convergence_boundary -- --ignored --nocapture",
        "cargo test -p pikahut --test integration_openclaw openclaw_gateway_e2e -- --ignored --nocapture",
        "cargo test -p pikahut --test integration_deterministic call_over_local_moq_relay_boundary -- --ignored --nocapture",
        "cargo test -p pikahut --test integration_deterministic call_with_pikachat_daemon_boundary -- --ignored --nocapture",
        "cargo test -p pikahut --test integration_primal primal_nostrconnect_smoke -- --ignored --nocapture",
        "cargo test -p pikahut --test integration_manual manual_interop_rust_runbook_contract -- --ignored --nocapture",
        "cargo test -p pikahut --test integration_manual manual_primal_lab_runbook_contract -- --ignored --nocapture",
    ];

    for needle in required {
        assert!(
            justfile.contains(needle),
            "missing required selector contract: {needle}"
        );
    }

    let forbidden = [
        "cargo run -q -p pikahut -- test openclaw-e2e",
        "cargo run -q -p pikahut -- test ui-e2e-local --platform android",
        "cargo run -q -p pikahut -- test ui-e2e-local --platform ios",
        "cargo run -q -p pikahut -- test cli-smoke",
        "cargo run -q -p pikahut -- test interop-rust-baseline --manual",
        "./tools/ui-e2e-public --platform all",
        "cargo test -p pika_core --tests -- --ignored --nocapture",
        "--test integration_public",
    ];

    for needle in forbidden {
        assert!(
            !justfile.contains(needle),
            "forbidden non-selector integration path still present: {needle}"
        );
    }

    Ok(())
}

#[test]
fn selector_references_in_docs_and_lanes_exist() -> Result<()> {
    let root = workspace_root();

    let mut referenced = HashSet::new();
    for path in [
        root.join("docs/testing/ci-selectors.md"),
        root.join("docs/testing/integration-matrix.md"),
        root.join("justfile"),
        root.join(".github/workflows/pre-merge.yml"),
    ] {
        let text = fs::read_to_string(path)?;
        referenced.extend(parse_cli_selector_refs(&text));
        referenced.extend(parse_docs_selector_refs(&text));
    }

    let actual = collect_actual_selectors(&root)?;

    for selector in referenced {
        if selector.contains('*') {
            let prefix = selector.replace('*', "");
            assert!(
                actual
                    .iter()
                    .any(|candidate| candidate.starts_with(&prefix)),
                "selector wildcard reference has no concrete match: {selector}"
            );
        } else {
            assert!(
                actual.contains(&selector),
                "selector reference has no matching test function: {selector}"
            );
        }
    }

    Ok(())
}

#[test]
fn required_lanes_do_not_regress_to_cli_test_harness() -> Result<()> {
    let root = workspace_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/pre-merge.yml"))?;

    let forbidden = [
        "cargo run -q -p pikahut -- test",
        "pikahut test openclaw-e2e",
        "pikahut test ui-e2e-local",
    ];

    for needle in forbidden {
        assert!(
            !workflow.contains(needle),
            "forbidden lane path present in pre-merge workflow: {needle}"
        );
    }

    Ok(())
}

#[test]
fn integration_wrapper_scripts_dispatch_to_selectors() -> Result<()> {
    let root = workspace_root();

    assert!(
        !root.join("tools/ui-e2e-public").exists(),
        "tools/ui-e2e-public must not exist (public-infra tests removed)"
    );

    let ui_local = fs::read_to_string(root.join("tools/ui-e2e-local"))?;
    assert!(ui_local.contains("--test integration_deterministic ui_e2e_local_ios"));
    assert!(ui_local.contains("--test integration_deterministic ui_e2e_local_android"));
    assert!(ui_local.contains("--test integration_deterministic ui_e2e_local_desktop"));

    let cli_smoke = fs::read_to_string(root.join("tools/cli-smoke"))?;
    assert!(cli_smoke.contains("--test integration_deterministic cli_smoke_local"));
    assert!(
        !cli_smoke.contains("cargo run -q -p pikahut -- test cli-smoke"),
        "cli-smoke wrapper must dispatch selectors directly"
    );

    let run_scenario = fs::read_to_string(root.join("pikachat-openclaw/scripts/run-scenario.sh"))?;
    assert!(run_scenario.contains("--test integration_deterministic"));
    assert!(
        !run_scenario.contains("cargo run --manifest-path"),
        "run-scenario wrapper must not own CLI harness orchestration"
    );

    let run_openclaw =
        fs::read_to_string(root.join("pikachat-openclaw/scripts/run-openclaw-e2e.sh"))?;
    assert!(run_openclaw.contains("--test integration_openclaw openclaw_gateway_e2e"));
    assert!(
        !run_openclaw.contains("pikahut -- test openclaw-e2e"),
        "run-openclaw-e2e wrapper must dispatch selector directly"
    );

    let interop = fs::read_to_string(root.join("tools/interop-rust-baseline"))?;
    assert!(interop.contains("--test integration_deterministic interop_rust_baseline"));
    assert!(
        interop.contains("--test integration_manual manual_interop_rust_runbook_contract"),
        "interop wrapper manual mode must dispatch integration_manual selector"
    );
    assert!(
        !interop.contains("cargo run -q -p pikahut -- test interop-rust-baseline"),
        "interop wrapper must not dispatch to CLI harness"
    );

    let primal_nightly = fs::read_to_string(root.join("tools/primal-ios-interop-nightly"))?;
    assert!(primal_nightly.contains("--test integration_primal primal_nostrconnect_smoke"));
    assert!(
        !primal_nightly.contains("xcodebuild"),
        "primal nightly wrapper must remain a selector-only shim"
    );

    Ok(())
}

#[test]
fn migration_docs_do_not_reference_legacy_cli_harness_paths() -> Result<()> {
    let root = workspace_root();
    for path in [
        root.join("docs/testing/integration-matrix.md"),
        root.join("docs/testing/ci-selectors.md"),
        root.join("docs/testing/wrapper-deprecation-policy.md"),
    ] {
        let text = fs::read_to_string(path)?;
        for needle in [
            "cargo run -q -p pikahut -- test ui-e2e-local",
            "cargo run -q -p pikahut -- test interop-rust-baseline",
            "cargo run -q -p pikahut -- test openclaw-e2e",
            "pikahut test openclaw-e2e",
            "pikahut test scenario",
        ] {
            assert!(
                !text.contains(needle),
                "legacy CLI harness path still documented: {needle}"
            );
        }
    }

    Ok(())
}

#[test]
fn policy_docs_call_out_non_owner_surfaces_and_deferred_mismatches() -> Result<()> {
    let root = workspace_root();

    let selectors = fs::read_to_string(root.join("docs/testing/ci-selectors.md"))?;
    for heading in [
        "## Policy Classes",
        "## Compatibility-Only Wrappers",
        "## Convenience / Advisory Surfaces",
        "## Deferred Root CI / `pikaci` Mismatches",
    ] {
        assert!(
            selectors.contains(heading),
            "ci-selectors.md must keep policy-truth heading: {heading}"
        );
    }
    assert!(
        selectors.contains(
            "Most CI-owned lanes invoke `cargo test -p pikahut --test integration_* ...`, and intentionally retained non-selector lanes are called out explicitly below."
        ),
        "ci-selectors.md must document the retained non-selector exception to the selector-first rule"
    );
    assert!(
        selectors.contains(
            "## Direct Selector Recipes (Not Owners By Themselves)\n\n| Recipe | Canonical selectors | Current policy status |\n| --- | --- | --- |"
        ),
        "ci-selectors.md must keep the direct-selector table well-formed and three-column"
    );

    let matrix = fs::read_to_string(root.join("docs/testing/integration-matrix.md"))?;
    for heading in [
        "## Policy Classes",
        "## Non-Owner Entry Points",
        "## Deferred Root CI / `pikaci` Asks",
    ] {
        assert!(
            matrix.contains(heading),
            "integration-matrix.md must keep policy-truth heading: {heading}"
        );
    }
    assert!(
        matrix.contains(
            "Retained non-selector exception: some platform-hosted lanes are intentionally still non-selector today, most notably nightly iOS XCTest coverage via `just ios-ui-test`."
        ),
        "integration-matrix.md must keep the retained non-selector exception explicit"
    );

    Ok(())
}

#[test]
fn pre_merge_pikachat_filter_covers_pikahut_selector_surface() -> Result<()> {
    let root = workspace_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/pre-merge.yml"))?;
    let pikachat_filter = extract_paths_filter_entries(&workflow, "pikachat");

    assert!(
        pikachat_filter
            .iter()
            .any(|entry| entry == "crates/pikahut/**"),
        "pikachat path filter must include crates/pikahut/** because pre-merge-pikachat depends on pikahut selectors"
    );

    let checks = fs::read_to_string(root.join("just/checks.just"))?;
    let pre_merge_recipe = extract_just_recipe_body(&checks, "pre-merge-pikachat")
        .expect("just/checks.just must define pre-merge-pikachat");
    assert!(
        pre_merge_recipe.contains("cargo test -p pikahut --test integration_deterministic"),
        "pre-merge-pikachat must continue to exercise pikahut selectors if the workflow filter tracks crates/pikahut/**"
    );

    Ok(())
}

#[test]
fn no_public_infra_selectors_in_ci_lanes() -> Result<()> {
    let root = workspace_root();

    for path in [
        root.join("justfile"),
        root.join(".github/workflows/pre-merge.yml"),
    ] {
        let text = fs::read_to_string(&path)?;
        assert!(
            !text.contains("integration_public"),
            "public-infra selector reference found in {}: integration_public selectors have been removed",
            path.display()
        );
    }

    assert!(
        !root
            .join("crates/pikahut/tests/integration_public.rs")
            .exists(),
        "integration_public.rs must not exist (public-infra tests removed)"
    );

    assert!(
        !root.join("tools/ui-e2e-public").exists(),
        "tools/ui-e2e-public must not exist (public-infra wrapper removed)"
    );

    Ok(())
}

#[test]
fn call_boundaries_have_single_owner() -> Result<()> {
    let root = workspace_root();
    let pikahut =
        fs::read_to_string(root.join("crates/pikahut/tests/integration_deterministic.rs"))?;
    assert!(
        !pikahut.contains("\"call_over_local_moq_relay\""),
        "call_over_local_moq_relay_boundary must not shell into rust/tests/e2e_calls.rs"
    );
    assert!(
        !pikahut.contains("\"call_with_pikachat_daemon\"") && !pikahut.contains("\"e2e_calls\""),
        "call_with_pikachat_daemon_boundary must not shell into rust/tests/e2e_calls.rs"
    );
    assert!(
        !root.join("rust/tests/e2e_calls.rs").exists(),
        "rust/tests/e2e_calls.rs must be deleted once the daemon boundary moves under pikahut"
    );

    Ok(())
}

#[test]
fn desktop_ui_e2e_has_single_owner() -> Result<()> {
    let root = workspace_root();
    let scenario =
        fs::read_to_string(root.join("crates/pikahut/src/testing/scenarios/deterministic.rs"))?;
    assert!(
        scenario.contains("run_local_ping_pong_with_bot("),
        "desktop UI E2E selector must call the desktop helper directly"
    );
    assert!(
        !scenario.contains("desktop_e2e_local_ping_pong_with_bot"),
        "desktop UI E2E selector must not shell into the legacy ignored desktop test"
    );

    let desktop = fs::read_to_string(root.join("crates/pika-desktop/src/app_manager.rs"))?;
    assert!(
        desktop.contains("pub fn run_local_ping_pong_with_bot("),
        "desktop helper must be exposed for direct selector ownership"
    );
    assert!(
        !desktop.contains("fn desktop_e2e_local_ping_pong_with_bot("),
        "legacy ignored desktop owner must be removed once pikahut owns the seam"
    );

    Ok(())
}

#[test]
fn shared_capable_scenarios_use_tenant_namespace_helpers() -> Result<()> {
    let root = workspace_root();

    let openclaw =
        fs::read_to_string(root.join("crates/pikahut/src/testing/scenarios/openclaw.rs"))?;
    assert!(
        openclaw.contains("TenantNamespace::new("),
        "openclaw scenario must derive tenant namespaces from helper API"
    );
    assert!(
        openclaw.contains(".relay_namespace(") && openclaw.contains(".moq_namespace("),
        "openclaw scenario must use TenantNamespace relay/moq helpers"
    );
    assert!(
        !openclaw.contains("tenant/"),
        "openclaw scenario must not hardcode tenant namespace literals"
    );

    let interop = fs::read_to_string(root.join("crates/pikahut/src/testing/scenarios/interop.rs"))?;
    assert!(
        interop.contains("TenantNamespace::new("),
        "interop scenario must derive tenant namespaces from helper API"
    );
    assert!(
        interop.contains(".relay_namespace(") && interop.contains(".moq_namespace("),
        "interop scenario must use TenantNamespace relay/moq helpers"
    );
    assert!(
        !interop.contains("tenant/"),
        "interop scenario must not hardcode tenant namespace literals"
    );

    Ok(())
}
