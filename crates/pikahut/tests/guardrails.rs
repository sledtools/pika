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

fn extract_branch_lane_paths(manifest: &str, lane_id: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut in_branch_lane = false;
    let mut current_lane_id: Option<String> = None;
    let mut collecting_paths = false;

    for line in manifest.lines() {
        let trimmed = line.trim();

        if trimmed == "[[branch.lanes]]" {
            if collecting_paths {
                break;
            }
            in_branch_lane = true;
            current_lane_id = None;
            continue;
        }

        if !in_branch_lane {
            continue;
        }

        if let Some(id) = trimmed
            .strip_prefix("id = \"")
            .and_then(|rest| rest.strip_suffix('"'))
        {
            current_lane_id = Some(id.to_string());
            continue;
        }

        if current_lane_id.as_deref() != Some(lane_id) {
            continue;
        }

        if !collecting_paths {
            if trimmed == "paths = [" {
                collecting_paths = true;
            }
            continue;
        }

        if trimmed == "]" {
            break;
        }

        if let Some(entry) = trimmed
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix("\",").or_else(|| rest.strip_suffix('"')))
        {
            entries.push(entry.to_string());
        }
    }

    entries
}

fn matches_filter(path: &str, pattern: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("/**") {
        path == prefix || path.starts_with(&format!("{prefix}/"))
    } else {
        path == pattern
    }
}

fn filter_covers_path<'a>(filters: impl IntoIterator<Item = &'a String>, path: &str) -> bool {
    filters
        .into_iter()
        .any(|pattern| matches_filter(path, pattern))
}

fn extract_pikaci_target_filters(text: &str, target_name: &str) -> Vec<String> {
    let direct_marker = format!("\"{target_name}\" => Ok(TargetSpec {{");
    let staged_marker = format!("\"{target_name}\" => Ok(staged_linux_target_spec(");
    let mut in_target = false;
    let mut in_filters = false;
    let mut filters = Vec::new();

    for line in text.lines() {
        if !in_target {
            if line.contains(&direct_marker) || line.contains(&staged_marker) {
                in_target = true;
            }
            continue;
        }

        if !in_filters {
            if line.contains("filters: &[") || line.trim() == "&[" {
                in_filters = true;
            }
            continue;
        }

        let trimmed = line.trim();
        if trimmed == "]," {
            break;
        }

        if let Some(filter) = trimmed
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix("\","))
        {
            filters.push(filter.to_string());
        }
    }

    filters
}

fn extract_just_alias_target(text: &str, alias_name: &str) -> Option<String> {
    let prefix = format!("alias {alias_name} := ");
    text.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .map(|value| value.trim().to_string())
    })
}

fn extract_just_recipe_body(text: &str, recipe_name: &str) -> Vec<String> {
    let mut in_recipe = false;
    let mut body = Vec::new();

    for line in text.lines() {
        if !in_recipe {
            if line.starts_with(&format!("{recipe_name}:")) {
                in_recipe = true;
            }
            continue;
        }

        if line.trim().is_empty() {
            body.push(String::new());
            continue;
        }

        if !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }

        body.push(line.trim_start().to_string());
    }

    body
}

fn extract_root_join_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let needle = "root.join(\"";
    let mut start = 0usize;

    while let Some(offset) = text[start..].find(needle) {
        let literal_start = start + offset + needle.len();
        let Some(end_offset) = text[literal_start..].find("\")") else {
            break;
        };
        paths.push(text[literal_start..literal_start + end_offset].to_string());
        start = literal_start + end_offset + 2;
    }

    paths
}

fn extract_rust_function_body(text: &str, function_name: &str) -> String {
    let signature = format!("fn {function_name}(");
    let Some(start) = text.find(&signature) else {
        return String::new();
    };
    let Some(open_offset) = text[start..].find('{') else {
        return String::new();
    };

    let mut depth = 0usize;
    let mut body_start = None;
    for (idx, ch) in text[start + open_offset..].char_indices() {
        match ch {
            '{' => {
                depth += 1;
                if depth == 1 {
                    body_start = Some(start + open_offset + idx + 1);
                }
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(body_start) = body_start {
                        return text[body_start..start + open_offset + idx].to_string();
                    }
                    return String::new();
                }
            }
            _ => {}
        }
    }

    String::new()
}

fn extract_shell_if_then_branch(recipe_body: &[String], condition_fragment: &str) -> Vec<String> {
    let mut in_branch = false;
    let mut branch = Vec::new();

    for line in recipe_body {
        if !in_branch {
            if line.contains(condition_fragment) && line.contains("then") {
                in_branch = true;
            }
            continue;
        }

        if line == "else" || line == "fi" {
            break;
        }

        branch.push(line.clone());
    }

    branch
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
fn primal_ios_interop_remains_manual_only_and_wrapper_backed() -> Result<()> {
    let root = workspace_root();

    let manual_gate = fs::read_to_string(root.join("docs/testing/manual-qa-gate.md"))?;
    assert!(
        manual_gate.contains("just nightly-primal-ios-interop"),
        "manual QA gate must keep the retained Primal interop smoke visible"
    );
    assert!(
        manual_gate.contains("manual-only compatibility coverage"),
        "manual QA gate must explain that Primal interop is retained manual-only coverage"
    );

    let selectors = fs::read_to_string(root.join("docs/testing/ci-selectors.md"))?;
    assert!(
        selectors.contains("## Retained Manual Compatibility Smoke"),
        "ci-selectors.md must keep a retained manual compatibility section"
    );
    assert!(
        selectors.contains("just nightly-primal-ios-interop"),
        "ci-selectors.md must keep nightly-primal-ios-interop in the manual inventory"
    );

    let matrix = fs::read_to_string(root.join("docs/testing/integration-matrix.md"))?;
    assert!(
        matrix.contains("| `just nightly-primal-ios-interop`")
            && matrix.contains("| manual-only |"),
        "integration-matrix.md must classify nightly-primal-ios-interop as manual-only"
    );
    assert!(
        matrix.contains("compatibility-only -> `just nightly-primal-ios-interop`"),
        "integration-matrix.md must keep tools/primal-ios-interop-nightly as a thin wrapper"
    );

    Ok(())
}

#[test]
fn agent_guest_chat_recipes_use_non_reset_wrapper() -> Result<()> {
    let root = workspace_root();
    let agent_just = fs::read_to_string(root.join("just/agent.just"))?;
    let agent_demo = fs::read_to_string(root.join("scripts/agent-demo-incus.sh"))?;
    let ensure_demo = fs::read_to_string(root.join("scripts/demo-agent-incus.sh"))?;

    assert!(
        agent_just.contains("./scripts/agent-demo-incus.sh"),
        "agent-incus-chat must keep using the checked-in Incus chat wrapper"
    );
    assert!(
        agent_just.contains("./scripts/demo-agent-incus.sh"),
        "agent-incus must keep using the checked-in Incus ensure wrapper"
    );
    assert!(
        !agent_demo.contains("--recover-after-attempt"),
        "agent-demo wrapper must not pass removed pikachat agent chat flags"
    );
    assert!(
        agent_demo.contains("set_agent_incus_lane_defaults"),
        "agent-demo wrapper must seed the Incus lane defaults"
    );
    assert!(
        ensure_demo.contains("set_agent_incus_lane_defaults"),
        "demo-agent wrapper must seed the Incus lane defaults"
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
fn pre_merge_pikachat_filter_tracks_checked_in_lane_surface() -> Result<()> {
    let root = workspace_root();
    let manifest = fs::read_to_string(root.join("ci/forge-lanes.toml"))?;
    let pikachat_filter = extract_branch_lane_paths(&manifest, "pikachat");
    let pikachat_filter: HashSet<_> = pikachat_filter.into_iter().collect();

    let justfile = fs::read_to_string(root.join("justfile"))?;
    let alias_target = extract_just_alias_target(&justfile, "pre-merge-pikachat");
    assert_eq!(
        alias_target.as_deref(),
        Some("checks::pre-merge-pikachat"),
        "justfile must keep pre-merge-pikachat routed through checks::pre-merge-pikachat for this workflow guardrail"
    );
    assert!(
        pikachat_filter.contains("just/checks.just"),
        "pikachat forge lane paths must include just/checks.just because pre-merge-pikachat is defined there"
    );

    let pikaci = fs::read_to_string(root.join("crates/pikaci/src/main.rs"))?;
    let rust_lane_filters: HashSet<_> =
        extract_pikaci_target_filters(&pikaci, "pre-merge-pikachat-rust")
            .into_iter()
            .collect();
    let apple_followup_filters: HashSet<_> =
        extract_pikaci_target_filters(&pikaci, "pre-merge-pikachat-apple-followup")
            .into_iter()
            .collect();
    let apple_followup_jobs = extract_rust_function_body(&pikaci, "pikachat_apple_followup_jobs");
    let flake = fs::read_to_string(root.join("flake.nix"))?;
    assert!(
        !rust_lane_filters.is_empty(),
        "pikaci main.rs must keep pre-merge-pikachat-rust filters discoverable"
    );
    assert!(
        !apple_followup_filters.is_empty(),
        "pikaci main.rs must keep pre-merge-pikachat-apple-followup filters discoverable"
    );
    assert!(
        !apple_followup_jobs.is_empty(),
        "pikaci main.rs must keep a checked-in Apple follow-up jobs helper"
    );
    assert!(
        flake.contains("pikachatStagedLinuxRustArgs = commonStagedLinuxRustArgs // {"),
        "flake.nix must keep a dedicated staged source config for the pikachat lane"
    );
    assert!(
        flake.contains("src = ciRustWorkspaceSrc;"),
        "pikachat staged source must use the full Rust workspace snapshot so cli and sidecar members stay buildable"
    );
    assert!(
        flake.contains("cargoLock = ./Cargo.lock;"),
        "pikachat staged source must use the full workspace lockfile"
    );
    assert!(
        flake.contains("./VERSION"),
        "pikachat staged source must include the repo-root VERSION file while staged desktop package tests stay in-lane"
    );

    let missing_from_workflow: Vec<_> = rust_lane_filters
        .iter()
        .filter(|entry| !pikachat_filter.contains(*entry))
        .cloned()
        .collect();
    assert!(
        missing_from_workflow.is_empty(),
        "pikachat forge lane paths must cover the checked-in pre-merge-pikachat-rust dependency surface; missing: {:?}",
        missing_from_workflow
    );

    let missing_followup_from_workflow: Vec<_> = apple_followup_filters
        .iter()
        .filter(|entry| !pikachat_filter.contains(*entry))
        .cloned()
        .collect();
    assert!(
        missing_followup_from_workflow.is_empty(),
        "pikachat forge lane paths must cover the checked-in pre-merge-pikachat-apple-followup dependency surface; missing: {:?}",
        missing_followup_from_workflow
    );
    if apple_followup_jobs.contains("channel-behavior.test.ts") {
        assert!(
            apple_followup_filters.contains("pikachat-openclaw/**"),
            "pre-merge-pikachat-apple-followup filters must include pikachat-openclaw/** while the Apple follow-up runs the channel behavior test"
        );
        assert!(
            pikachat_filter.contains("pikachat-openclaw/**"),
            "pikachat forge lane paths must include pikachat-openclaw/** while the Apple follow-up runs the channel behavior test"
        );
    }

    let cli_manifest = fs::read_to_string(root.join("cli/Cargo.toml"))?;
    for (dependency_name, filter_path) in [
        ("pika-agent-protocol", "crates/pika-agent-protocol/**"),
        (
            "pika-agent-control-plane",
            "crates/pika-agent-control-plane/**",
        ),
        ("pika-marmot-runtime", "crates/pika-marmot-runtime/**"),
        ("pika-relay-profiles", "crates/pika-relay-profiles/**"),
        ("pikachat-sidecar", "crates/pikachat-sidecar/**"),
        ("hypernote-protocol", "crates/hypernote-protocol/**"),
    ] {
        if cli_manifest.contains(dependency_name) {
            assert!(
                rust_lane_filters.contains(filter_path),
                "pre-merge-pikachat-rust filters must include {filter_path} while cli/Cargo.toml keeps the direct {dependency_name} dependency"
            );
            assert!(
                pikachat_filter.contains(filter_path),
                "pikachat forge lane paths must include {filter_path} while cli/Cargo.toml keeps the direct {dependency_name} dependency"
            );
        }
    }
    let sidecar_manifest = fs::read_to_string(root.join("crates/pikachat-sidecar/Cargo.toml"))?;
    if apple_followup_jobs.contains("pikachat-sidecar -- -D warnings")
        && sidecar_manifest.contains("pika-media")
    {
        assert!(
            apple_followup_filters.contains("crates/pika-media/**"),
            "pre-merge-pikachat-apple-followup filters must include crates/pika-media/** while Apple follow-up sidecar clippy keeps pika-media in the build graph"
        );
        assert!(
            pikachat_filter.contains("crates/pika-media/**"),
            "pikachat forge lane paths must include crates/pika-media/** while Apple follow-up sidecar clippy keeps pika-media in the build graph"
        );
    }
    if apple_followup_jobs.contains("ui_e2e_local_desktop") {
        assert!(
            apple_followup_filters.contains("rust/**"),
            "pre-merge-pikachat-apple-followup filters must include rust/** while the Apple follow-up keeps the desktop selector path"
        );
        assert!(
            pikachat_filter.contains("rust/**"),
            "pikachat forge lane paths must include rust/** while the Apple follow-up keeps the desktop selector path"
        );
    }
    if cli_manifest.contains("pika-test-utils") {
        assert!(
            rust_lane_filters.contains("crates/pika-test-utils/**"),
            "pre-merge-pikachat-rust filters must include crates/pika-test-utils/** while cli/Cargo.toml keeps the shared test dependency"
        );
        assert!(
            pikachat_filter.contains("crates/pika-test-utils/**"),
            "pikachat forge lane paths must include crates/pika-test-utils/** while cli/Cargo.toml keeps the shared test dependency"
        );
    }

    let pikachat_jobs = extract_rust_function_body(&pikaci, "pikachat_rust_jobs");
    let lane_keeps_relay_backed_selectors = [
        "cli_smoke_local",
        "openclaw_scenario_invite_and_chat",
        "openclaw_scenario_invite_and_chat_rust_bot",
        "openclaw_scenario_invite_and_chat_daemon",
        "openclaw_scenario_audio_echo",
    ]
    .iter()
    .any(|selector| pikachat_jobs.contains(selector));
    let linux_rust = fs::read_to_string(root.join("nix/ci/linux-rust.nix"))?;
    let deterministic =
        fs::read_to_string(root.join("crates/pikahut/src/testing/scenarios/deterministic.rs"))?;
    let openclaw =
        fs::read_to_string(root.join("crates/pikahut/src/testing/scenarios/openclaw.rs"))?;
    let support = fs::read_to_string(root.join("crates/pikahut/tests/support.rs"))?;
    let integration_deterministic =
        fs::read_to_string(root.join("crates/pikahut/tests/integration_deterministic.rs"))?;
    let config = fs::read_to_string(root.join("crates/pikahut/src/config.rs"))?;
    if lane_keeps_relay_backed_selectors {
        let run_scenario_body = extract_rust_function_body(&deterministic, "run_scenario");
        let plugin_filter = "pikachat-openclaw/**";
        assert!(
            rust_lane_filters.contains("cmd/pika-relay/**"),
            "pre-merge-pikachat-rust filters must include cmd/pika-relay/** while relay-backed deterministic selectors stay in-lane"
        );
        assert!(
            pikachat_filter.contains("cmd/pika-relay/**"),
            "pikachat workflow filter must include cmd/pika-relay/** while relay-backed deterministic selectors stay in-lane"
        );
        assert!(
            rust_lane_filters.contains(plugin_filter),
            "pre-merge-pikachat-rust filters must include {plugin_filter} while the deterministic OpenClaw scenarios stay in-lane"
        );
        assert!(
            pikachat_filter.contains(plugin_filter),
            "pikachat workflow filter must include {plugin_filter} while the deterministic OpenClaw scenarios stay in-lane"
        );
        assert!(
            deterministic.contains("PIKAHUT_TEST_PIKACHAT_BIN"),
            "pikahut deterministic helpers must keep the staged pikachat binary override while relay-backed selectors stay in-lane"
        );
        assert!(
            support.contains("env_path_var(\"PIKAHUT_TEST_PIKACHAT_BIN\")"),
            "pikahut daemon-boundary support must prefer the staged pikachat binary override while that selector stays in-lane"
        );
        assert!(
            run_scenario_body
                .contains("pikachat_spec(&root, &scenario_args, \"pikachat-scenario\")"),
            "relay-backed deterministic scenarios must keep routing through pikachat_spec while staged pikachat selectors stay in-lane"
        );
        assert!(
            openclaw.contains("pikachat-openclaw/openclaw/extensions/pikachat-openclaw"),
            "relay-backed deterministic OpenClaw scenarios must keep resolving the checked-in plugin tree while those selectors stay in-lane"
        );
        assert!(
            openclaw.contains("CommandSpec::new(binary)")
                && openclaw.contains(".capture_name(\"openclaw-invite-and-chat-peer\")"),
            "OpenClaw peer coverage must keep running through a direct pikachat binary spec while staged pikachat selectors stay in-lane"
        );
        assert!(
            deterministic.contains("\"--kp-relay\".into(),\n            relay_url.clone(),\n            \"invite\".into()"),
            "cli_smoke_local must keep forwarding the local kp relay into pikachat invite while relay-backed selectors stay in-lane"
        );
        assert!(
            linux_rust.contains("export PIKAHUT_TEST_PIKACHAT_BIN=\"$root/bin/pikachat\""),
            "staged pikachat wrapper must export PIKAHUT_TEST_PIKACHAT_BIN while relay-backed selectors stay in-lane"
        );
        assert!(
            linux_rust.contains("export PIKA_FIXTURE_RELAY_CMD=\"$root/bin/pika-relay\""),
            "staged pikachat wrapper must export PIKA_FIXTURE_RELAY_CMD while relay-backed selectors stay in-lane"
        );
        assert!(
            linux_rust.contains("export PIKAHUT_TEST_WORKSPACE_ROOT=/workspace/snapshot"),
            "staged pikachat wrapper must export PIKAHUT_TEST_WORKSPACE_ROOT while prepared selector execution still depends on workspace discovery"
        );
        assert!(
            linux_rust.contains("cd \"$PIKAHUT_TEST_WORKSPACE_ROOT\""),
            "staged pikachat wrapper must enter the staged workspace root before running deterministic selectors"
        );
        assert!(
            config.contains(
                "pub const TEST_WORKSPACE_ROOT_ENV: &str = \"PIKAHUT_TEST_WORKSPACE_ROOT\";"
            ),
            "pikahut config must keep the staged workspace-root override env while prepared selector execution depends on it"
        );
        assert!(
            integration_deterministic
                .contains("pikahut::config::find_workspace_root().unwrap_or_else(|_|"),
            "integration_deterministic must resolve workspace root from runtime config before falling back to compile-time paths"
        );
    }

    let lane_keeps_pika_core_regression_boundaries = [
        "post_rebase_invalid_event_rejection_boundary",
        "post_rebase_logout_session_convergence_boundary",
    ]
    .iter()
    .any(|selector| pikachat_jobs.contains(selector));
    if lane_keeps_pika_core_regression_boundaries {
        assert!(
            integration_deterministic.contains("PIKAHUT_TEST_PIKA_CORE_E2E_MESSAGING_BIN"),
            "pikahut deterministic regressions must keep the staged pika_core e2e_messaging override while those boundaries stay in-lane"
        );
        assert!(
            linux_rust.contains(
                "export PIKAHUT_TEST_PIKA_CORE_E2E_MESSAGING_BIN=\"$root/bin/pika-core-e2e-messaging\""
            ),
            "staged pikachat wrapper must export PIKAHUT_TEST_PIKA_CORE_E2E_MESSAGING_BIN while that boundary stays in-lane"
        );
    }

    assert!(
        !pikachat_jobs.contains("openclaw_gateway_e2e"),
        "pre-merge-pikachat-rust must keep OpenClaw coverage on the deterministic selector path, not the heavy integration_openclaw lane"
    );

    Ok(())
}

#[test]
fn pre_merge_agent_contracts_filter_tracks_checked_in_lane_surface() -> Result<()> {
    let root = workspace_root();
    let manifest = fs::read_to_string(root.join("ci/forge-lanes.toml"))?;
    let agent_filter = extract_branch_lane_paths(&manifest, "agent_contracts");
    let agent_filter: HashSet<_> = agent_filter.into_iter().collect();
    let checks = fs::read_to_string(root.join("just/checks.just"))?;
    let justfile = fs::read_to_string(root.join("justfile"))?;
    let alias_target = extract_just_alias_target(&justfile, "pre-merge-agent-contracts");
    assert_eq!(
        alias_target.as_deref(),
        Some("checks::pre-merge-agent-contracts"),
        "justfile must keep pre-merge-agent-contracts routed through checks::pre-merge-agent-contracts for this workflow guardrail"
    );
    assert!(
        agent_filter.contains("just/checks.just"),
        "agent_contracts forge lane paths must include just/checks.just because pre-merge-agent-contracts is defined there"
    );

    let recipe = extract_just_recipe_body(&checks, "pre-merge-agent-contracts");
    assert!(
        !recipe.is_empty(),
        "checks.just must keep a checked-in pre-merge-agent-contracts recipe body"
    );
    if recipe
        .iter()
        .any(|line| line.contains("pikaci-remote-fulfill-pre-merge-agent-contracts"))
    {
        let remote_alias_target =
            extract_just_alias_target(&justfile, "pikaci-remote-fulfill-pre-merge-agent-contracts");
        assert_eq!(
            remote_alias_target.as_deref(),
            Some("infra::pikaci-remote-fulfill-pre-merge-agent-contracts"),
            "justfile must keep the Apple remote agent-contracts helper routed through infra::pikaci-remote-fulfill-pre-merge-agent-contracts for this workflow guardrail"
        );
        assert!(
            agent_filter.contains("just/infra.just"),
            "agent_contracts forge lane paths must include just/infra.just while the lane reaches the Apple remote helper through the infra module"
        );

        let infra = fs::read_to_string(root.join("just/infra.just"))?;
        let remote_recipe =
            extract_just_recipe_body(&infra, "pikaci-remote-fulfill-pre-merge-agent-contracts");
        assert!(
            !remote_recipe.is_empty(),
            "just/infra.just must keep a checked-in pikaci-remote-fulfill-pre-merge-agent-contracts recipe body"
        );
        if remote_recipe
            .iter()
            .any(|line| line.contains("scripts/pikaci-staged-linux-remote.sh"))
        {
            assert!(
                agent_filter.contains("scripts/pikaci-staged-linux-remote.sh"),
                "agent_contracts forge lane paths must include scripts/pikaci-staged-linux-remote.sh while the Apple remote helper runs through that checked-in script"
            );
        }
    }

    let pikaci = fs::read_to_string(root.join("crates/pikaci/src/main.rs"))?;
    let rust_lane_filters = extract_pikaci_target_filters(&pikaci, "pre-merge-agent-contracts");
    assert!(
        !rust_lane_filters.is_empty(),
        "pikaci main.rs must keep pre-merge-agent-contracts filters discoverable"
    );

    let missing_from_workflow: Vec<_> = rust_lane_filters
        .iter()
        .filter(|entry| !agent_filter.contains(entry.as_str()))
        .cloned()
        .collect();
    assert!(
        missing_from_workflow.is_empty(),
        "agent_contracts forge lane paths must cover the checked-in pre-merge-agent-contracts dependency surface; missing: {:?}",
        missing_from_workflow
    );

    let microvm_manifest = fs::read_to_string(root.join("crates/pika-agent-microvm/Cargo.toml"))?;
    let server_manifest = fs::read_to_string(root.join("crates/pika-server/Cargo.toml"))?;
    let staged_agent_tests_use_shared_test_utils =
        microvm_manifest.contains("pika-test-utils") || server_manifest.contains("pika-test-utils");
    if staged_agent_tests_use_shared_test_utils {
        assert!(
            rust_lane_filters
                .iter()
                .any(|entry| entry == "crates/pika-test-utils/**"),
            "pre-merge-agent-contracts pikaci target filters must include crates/pika-test-utils/** while staged agent test crates keep that shared test dependency"
        );
        assert!(
            agent_filter.contains("crates/pika-test-utils/**"),
            "agent_contracts forge lane paths must include crates/pika-test-utils/** while staged agent test crates keep that shared test dependency"
        );
    }

    let selector_refs = parse_cli_selector_refs(&recipe.join("\n"));
    let expected_host_selectors = [
        "integration_deterministic::agent_http_ensure_local",
        "integration_deterministic::agent_http_cli_new_local",
        "integration_deterministic::agent_http_cli_new_idempotent_local",
        "integration_deterministic::agent_http_cli_new_me_recover_local",
        "integration_deterministic::agent_launch_provisioning_boundary",
        "integration_deterministic::agent_launch_provisioning_failure_boundary",
        "integration_deterministic::agent_launch_first_reply_boundary",
    ];
    for selector in expected_host_selectors {
        assert!(
            selector_refs.contains(selector),
            "pre-merge-agent-contracts must keep host-side selector coverage for {selector}"
        );
    }
    assert!(
        agent_filter.contains("crates/pikahut/src/**"),
        "agent_contracts forge lane paths must include crates/pikahut/src/** while the lane keeps host-side agent selectors"
    );
    assert!(
        agent_filter.contains("crates/pikahut/tests/**"),
        "agent_contracts forge lane paths must include crates/pikahut/tests/** while the lane keeps host-side agent selectors"
    );
    assert!(
        agent_filter.contains("crates/pikahut/Cargo.toml"),
        "agent_contracts forge lane paths must include crates/pikahut/Cargo.toml while the lane keeps host-side agent selectors"
    );
    let pikahut_manifest = fs::read_to_string(root.join("crates/pikahut/Cargo.toml"))?;
    if pikahut_manifest.contains("pika-desktop") {
        assert!(
            agent_filter.contains("crates/pika-desktop/**"),
            "agent_contracts workflow filter must include crates/pika-desktop/** while host-side pikahut selectors depend on pika-desktop"
        );
    }
    let integration_deterministic =
        fs::read_to_string(root.join("crates/pikahut/tests/integration_deterministic.rs"))?;
    let selected_agent_http_cli_selectors = [
        "agent_http_cli_new_local",
        "agent_http_cli_new_idempotent_local",
        "agent_http_cli_new_me_recover_local",
    ];
    let lane_keeps_host_side_pikachat_shellouts = selected_agent_http_cli_selectors
        .iter()
        .filter(|selector| {
            selector_refs.contains(&format!("integration_deterministic::{selector}"))
        })
        .map(|selector| extract_rust_function_body(&integration_deterministic, selector))
        .any(|body| {
            body.contains("\"run\",") && body.contains("\"-p\",") && body.contains("\"pikachat\",")
        });
    if lane_keeps_host_side_pikachat_shellouts {
        let cli_manifest = fs::read_to_string(root.join("cli/Cargo.toml"))?;
        for (dependency_name, filter_path) in [
            ("pika-agent-protocol", "crates/pika-agent-protocol/**"),
            ("pikachat-sidecar", "crates/pikachat-sidecar/**"),
            ("hypernote-protocol", "crates/hypernote-protocol/**"),
        ] {
            if cli_manifest.contains(dependency_name) {
                assert!(
                    agent_filter.contains(filter_path),
                    "agent_contracts forge lane paths must include {filter_path} while host-side pikachat selectors depend on {dependency_name}"
                );
            }
        }
    }
    assert!(
        agent_filter.contains("cli/**"),
        "agent_contracts forge lane paths must include cli/** while the lane keeps agent_http_cli_new selectors"
    );

    Ok(())
}

#[test]
fn pre_merge_notifications_filter_tracks_checked_in_lane_surface() -> Result<()> {
    let root = workspace_root();
    let manifest = fs::read_to_string(root.join("ci/forge-lanes.toml"))?;
    let notifications_filter = extract_branch_lane_paths(&manifest, "notifications");
    let notifications_filter: HashSet<_> = notifications_filter.into_iter().collect();
    let checks = fs::read_to_string(root.join("just/checks.just"))?;
    let justfile = fs::read_to_string(root.join("justfile"))?;
    let alias_target = extract_just_alias_target(&justfile, "pre-merge-notifications");
    assert_eq!(
        alias_target.as_deref(),
        Some("checks::pre-merge-notifications"),
        "justfile must keep pre-merge-notifications routed through checks::pre-merge-notifications for this workflow guardrail"
    );
    assert!(
        notifications_filter.contains("just/checks.just"),
        "notifications forge lane paths must include just/checks.just because pre-merge-notifications is defined there"
    );

    let recipe = extract_just_recipe_body(&checks, "pre-merge-notifications");
    assert!(
        !recipe.is_empty(),
        "checks.just must keep a checked-in pre-merge-notifications recipe body"
    );
    if recipe
        .iter()
        .any(|line| line.contains("pikaci-remote-fulfill-pre-merge-notifications"))
    {
        let remote_alias_target =
            extract_just_alias_target(&justfile, "pikaci-remote-fulfill-pre-merge-notifications");
        assert_eq!(
            remote_alias_target.as_deref(),
            Some("infra::pikaci-remote-fulfill-pre-merge-notifications"),
            "justfile must keep the Apple remote notifications helper routed through infra::pikaci-remote-fulfill-pre-merge-notifications for this workflow guardrail"
        );
        assert!(
            notifications_filter.contains("just/infra.just"),
            "notifications forge lane paths must include just/infra.just while the lane reaches the Apple remote helper through the infra module"
        );

        let infra = fs::read_to_string(root.join("just/infra.just"))?;
        let remote_recipe =
            extract_just_recipe_body(&infra, "pikaci-remote-fulfill-pre-merge-notifications");
        assert!(
            !remote_recipe.is_empty(),
            "just/infra.just must keep a checked-in pikaci-remote-fulfill-pre-merge-notifications recipe body"
        );
        if remote_recipe
            .iter()
            .any(|line| line.contains("scripts/pikaci-staged-linux-remote.sh"))
        {
            assert!(
                notifications_filter.contains("scripts/pikaci-staged-linux-remote.sh"),
                "notifications forge lane paths must include scripts/pikaci-staged-linux-remote.sh while the Apple remote helper runs through that checked-in script"
            );
        }
    }

    let pikaci = fs::read_to_string(root.join("crates/pikaci/src/main.rs"))?;
    let rust_lane_filters = extract_pikaci_target_filters(&pikaci, "pre-merge-notifications");
    assert!(
        !rust_lane_filters.is_empty(),
        "pikaci main.rs must keep pre-merge-notifications filters discoverable"
    );

    let missing_from_workflow: Vec<_> = rust_lane_filters
        .iter()
        .filter(|entry| !notifications_filter.contains(entry.as_str()))
        .cloned()
        .collect();
    assert!(
        missing_from_workflow.is_empty(),
        "notifications forge lane paths must cover the checked-in pre-merge-notifications dependency surface; missing: {:?}",
        missing_from_workflow
    );

    let server_manifest = fs::read_to_string(root.join("crates/pika-server/Cargo.toml"))?;
    for (dependency_name, filter_path) in [
        (
            "pika-agent-control-plane",
            "crates/pika-agent-control-plane/**",
        ),
        ("pika-agent-microvm", "crates/pika-agent-microvm/**"),
        ("pika-test-utils", "crates/pika-test-utils/**"),
    ] {
        if server_manifest.contains(dependency_name) {
            assert!(
                rust_lane_filters.iter().any(|entry| entry == filter_path),
                "pre-merge-notifications pikaci target filters must include {filter_path} while pika-server keeps dependency {dependency_name}"
            );
            assert!(
                notifications_filter.contains(filter_path),
                "notifications forge lane paths must include {filter_path} while pika-server keeps dependency {dependency_name}"
            );
        }
    }

    let recipe_text = recipe.join("\n");
    if recipe_text.contains("cargo clippy -p pika-server")
        || recipe_text.contains("cargo test -p pika-server")
    {
        assert!(
            notifications_filter.contains("crates/pika-server/**"),
            "notifications forge lane paths must include crates/pika-server/** while the lane runs pika-server checks"
        );
    }
    if recipe_text.contains("cargo run -q -p pikahut -- up --profile postgres") {
        assert!(
            notifications_filter.contains("crates/pikahut/**"),
            "notifications forge lane paths must include crates/pikahut/** while the lane runs the local pikahut postgres fixture wrapper"
        );
        let pikahut_manifest = fs::read_to_string(root.join("crates/pikahut/Cargo.toml"))?;
        if pikahut_manifest.contains("pika-desktop") {
            assert!(
                notifications_filter.contains("crates/pika-desktop/**"),
                "notifications forge lane paths must include crates/pika-desktop/** while the local pikahut wrapper depends on pika-desktop"
            );
        }
    }

    Ok(())
}

#[test]
fn pre_merge_fixture_filter_tracks_checked_in_lane_surface() -> Result<()> {
    let root = workspace_root();
    let manifest = fs::read_to_string(root.join("ci/forge-lanes.toml"))?;
    let fixture_filter = extract_branch_lane_paths(&manifest, "fixture");
    let fixture_filter: HashSet<_> = fixture_filter.into_iter().collect();
    let checks = fs::read_to_string(root.join("just/checks.just"))?;
    let justfile = fs::read_to_string(root.join("justfile"))?;
    let alias_target = extract_just_alias_target(&justfile, "pre-merge-fixture");
    assert_eq!(
        alias_target.as_deref(),
        Some("checks::pre-merge-fixture"),
        "justfile must keep pre-merge-fixture routed through checks::pre-merge-fixture for this workflow guardrail"
    );
    assert!(
        fixture_filter.contains("just/checks.just"),
        "fixture forge lane paths must include just/checks.just because pre-merge-fixture is defined there"
    );

    let recipe = extract_just_recipe_body(&checks, "pre-merge-fixture");
    assert!(
        !recipe.is_empty(),
        "checks.just must keep a checked-in pre-merge-fixture recipe body"
    );
    let recipe_text = recipe.join("\n");
    assert!(
        recipe_text.contains("nix run .#pikaci -- run pre-merge-fixture-rust"),
        "pre-merge-fixture must keep routing its Rust test segment through pre-merge-fixture-rust so pikaci remains the checked-in authority for that lane"
    );
    assert!(
        !recipe_text.contains("cargo test -p pikahut"),
        "pre-merge-fixture must not bypass pre-merge-fixture-rust with a direct cargo test -p pikahut path"
    );

    let pikaci = fs::read_to_string(root.join("crates/pikaci/src/main.rs"))?;
    let rust_lane_filters = extract_pikaci_target_filters(&pikaci, "pre-merge-fixture-rust");
    assert!(
        !rust_lane_filters.is_empty(),
        "pikaci main.rs must keep pre-merge-fixture-rust filters discoverable"
    );

    let missing_from_workflow: Vec<_> = rust_lane_filters
        .iter()
        .filter(|entry| !fixture_filter.contains(entry.as_str()))
        .cloned()
        .collect();
    assert!(
        missing_from_workflow.is_empty(),
        "fixture forge lane paths must cover the checked-in pre-merge-fixture-rust dependency surface; missing: {:?}",
        missing_from_workflow
    );

    let fixture_jobs = extract_rust_function_body(&pikaci, "fixture_rust_jobs");
    if fixture_jobs.contains("PackageTests { package: \"pikahut\" }") {
        let guardrails = fs::read_to_string(root.join("crates/pikahut/tests/guardrails.rs"))?;
        for path in extract_root_join_paths(&guardrails) {
            assert!(
                filter_covers_path(rust_lane_filters.iter(), &path),
                "pre-merge-fixture-rust filters must include {path} while the lane keeps pikahut package tests and guardrails.rs reads that checked-in path"
            );
            assert!(
                filter_covers_path(fixture_filter.iter(), &path),
                "fixture forge lane paths must include {path} while the lane keeps pikahut package tests and guardrails.rs reads that checked-in path"
            );
        }
    }

    let reduced_workspace_manifest =
        fs::read_to_string(root.join("nix/ci/pika-core-workspace/Cargo.toml"))?;
    let flake = fs::read_to_string(root.join("flake.nix"))?;
    let linux_rust = fs::read_to_string(root.join("nix/ci/linux-rust.nix"))?;
    assert!(
        linux_rust.contains("cargo clippy --locked -j \"$cargoJobs\" -p pikahut -- -D warnings"),
        "staged fixture workspace build must validate pikahut clippy before remote execute so the microVM lane stays offline"
    );
    assert!(
        linux_rust.contains("cp ${pikaRelayPkg}/bin/pika-relay \"$out/bin/pika-relay\""),
        "staged fixture workspace build must install pika-relay for remote fixture smoke so the guest does not fall back to go build"
    );
    assert!(
        linux_rust.contains("export PIKA_FIXTURE_RELAY_CMD=\"$root/bin/pika-relay\""),
        "staged fixture relay smoke wrapper must export the staged pika-relay binary so the guest stays offline"
    );
    assert!(
        linux_rust.contains("missing staged pikahut clippy marker at $marker"),
        "staged fixture clippy wrapper must gate on the staged build marker instead of rerunning cargo clippy online in the guest"
    );

    for (member, required_snippet) in [
        ("cli", "cp -R ${./cli}/. \"$out/cli\""),
        (
            "cli/src/main.rs",
            "cp ${./config/channels.json} \"$out/config/channels.json\"",
        ),
        (
            "tests/support/nostr_connect.rs",
            "cp -R ${./tests/support} \"$out/tests/support\"",
        ),
        (
            "crates/pika-agent-protocol",
            "cp -R ${./crates/pika-agent-protocol} \"$out/crates/pika-agent-protocol\"",
        ),
        (
            "crates/pikachat-sidecar",
            "cp -R ${./crates/pikachat-sidecar} \"$out/crates/pikachat-sidecar\"",
        ),
    ] {
        if reduced_workspace_manifest.contains(&format!("\"{member}\"")) {
            assert!(
                flake.contains(required_snippet),
                "flake.nix must snapshot {member} into ciPikaCoreWorkspaceSrc while pre-merge-fixture-rust still uses the reduced staged workspace"
            );
        }
    }

    Ok(())
}

#[test]
fn pre_merge_fixture_remote_lane_skips_fork_pull_requests() -> Result<()> {
    let root = workspace_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/pre-merge.yml"))?;

    assert!(
        workflow.contains("PIKA_BUILD_SSH_KEY: ${{ secrets.PIKA_BUILD_SSH_KEY }}"),
        "staged Linux branch lanes must keep reading PIKA_BUILD_SSH_KEY from repository secrets"
    );
    assert!(
        workflow.contains(
            "error: set repository secret PIKA_BUILD_SSH_KEY for staged Linux shadow lanes"
        ),
        "staged Linux branch lanes must fail fast with an explicit missing-secret error when remote secrets are unavailable"
    );

    Ok(())
}

#[test]
fn pre_merge_pikachat_apple_split_stays_explicit() -> Result<()> {
    let root = workspace_root();
    let checks = fs::read_to_string(root.join("just/checks.just"))?;
    let pikaci = fs::read_to_string(root.join("crates/pikaci/src/main.rs"))?;

    let pre_merge_recipe = extract_just_recipe_body(&checks, "pre-merge-pikachat");
    assert!(
        !pre_merge_recipe.is_empty(),
        "checks.just must keep a concrete pre-merge-pikachat recipe body"
    );
    let apple_branch = extract_shell_if_then_branch(
        &pre_merge_recipe,
        r#"if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ];"#,
    );
    assert!(
        !apple_branch.is_empty(),
        "pre-merge-pikachat must keep an explicit Apple Silicon branch"
    );
    assert!(
        apple_branch
            .iter()
            .any(|line| line.contains("run pre-merge-pikachat-rust")),
        "Apple Silicon branch must keep the staged Linux Rust segment explicit"
    );
    assert!(
        apple_branch
            .iter()
            .any(|line| line.contains("run pre-merge-pikachat-apple-followup")),
        "Apple Silicon branch must keep the Apple host follow-up explicit"
    );
    assert!(
        !apple_branch
            .iter()
            .any(|line| line.contains("cargo clippy -p pikachat -- -D warnings")),
        "Apple Silicon branch must not keep inline pikachat clippy outside the follow-up helper"
    );
    assert!(
        !apple_branch
            .iter()
            .any(|line| line.contains("cargo clippy -p pikachat-sidecar -- -D warnings")),
        "Apple Silicon branch must not keep inline sidecar clippy outside the follow-up helper"
    );
    assert!(
        !apple_branch
            .iter()
            .any(|line| line.contains("channel-behavior.test.ts")),
        "Apple Silicon branch must not keep inline TypeScript follow-up outside the pikaci target"
    );

    let apple_followup = extract_rust_function_body(&pikaci, "pikachat_apple_followup_jobs");
    assert!(
        !apple_followup.is_empty(),
        "pikaci main.rs must keep a checked-in Apple host follow-up jobs helper"
    );
    assert!(
        apple_followup.contains("cargo clippy -p pikachat -- -D warnings"),
        "Apple host follow-up must keep pikachat clippy"
    );
    assert!(
        apple_followup.contains("cargo clippy -p pikachat-sidecar -- -D warnings"),
        "Apple host follow-up must keep pikachat-sidecar clippy"
    );
    assert!(
        apple_followup.contains("ui_e2e_local_desktop"),
        "Apple host follow-up must keep the desktop selector"
    );
    assert!(
        apple_followup.contains("channel-behavior.test.ts"),
        "Apple host follow-up must keep the TypeScript channel behavior test"
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
