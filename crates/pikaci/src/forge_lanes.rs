use crate::catalog::PikaStagedLinuxTarget;
use anyhow::{Context, anyhow};
use globset::{Glob, GlobSet, GlobSetBuilder};

pub const FORGE_LANE_DEFINITION_PATH: &str = "crates/pikaci/src/forge_lanes.rs";

#[derive(Debug, Clone)]
pub struct ForgeCiManifest {
    pub nightly_hour_utc: u32,
    pub nightly_minute_utc: u32,
    pub branch_lanes: Vec<ForgeLane>,
    pub nightly_lanes: Vec<ForgeLane>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeLane {
    pub id: String,
    pub title: String,
    pub entrypoint: String,
    pub command: Vec<String>,
    pub paths: Vec<String>,
    pub concurrency_group: Option<String>,
    pub staged_linux_target: Option<String>,
}

pub fn compiled_forge_ci_manifest() -> ForgeCiManifest {
    ForgeCiManifest {
        nightly_hour_utc: 8,
        nightly_minute_utc: 0,
        branch_lanes: vec![
            staged_linux_lane(
                "pika_rust",
                "check-pika-rust",
                "pre-merge-pika-rust",
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
            ),
            staged_linux_lane(
                "pika_followup",
                "check-pika-followup",
                "pre-merge-pika-followup",
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
            ),
            staged_linux_lane(
                "notifications",
                "check-notifications",
                "pre-merge-notifications",
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
            ),
            staged_linux_lane(
                "agent_contracts",
                "check-agent-contracts",
                "pre-merge-agent-contracts",
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
            ),
            staged_linux_lane(
                "rmp",
                "check-rmp",
                "pre-merge-rmp",
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
            ),
            staged_linux_lane(
                "pikachat",
                "check-pikachat",
                "pre-merge-pikachat-rust",
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
            ),
            staged_linux_lane(
                "pikachat_typescript",
                "check-pikachat-typescript",
                "pre-merge-pikachat-typescript",
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
            ),
            pikaci_target_lane(
                "apple_host_sanity",
                "check-apple-host-sanity",
                "apple_host_sanity",
                Some("apple-host"),
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
            ),
            staged_linux_lane(
                "pikachat_openclaw_e2e",
                "check-pikachat-openclaw-e2e",
                "pre-merge-pikachat-openclaw-e2e",
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
            ),
            staged_linux_lane(
                "fixture",
                "check-fixture",
                "pre-merge-fixture-rust",
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
            ),
        ],
        nightly_lanes: vec![
            pikaci_target_lane("nightly_linux", "nightly-linux", "nightly_linux", None, &[]),
            pikaci_target_lane("nightly_pika", "nightly-pika", "nightly_pika", None, &[]),
            pikaci_target_lane(
                "nightly_pikachat",
                "nightly-pikachat",
                "nightly_pikachat",
                None,
                &[],
            ),
            pikaci_target_lane(
                "nightly_pika_ui_android",
                "nightly-pika-ui-android",
                "nightly_pika_ui_android",
                Some("nightly-android"),
                &[],
            ),
            pikaci_target_lane(
                "nightly_apple_host_bundle",
                "nightly-apple-host-bundle",
                "nightly_apple_host_bundle",
                Some("apple-host"),
                &[],
            ),
        ],
    }
}

pub fn select_branch_lanes(
    manifest: &ForgeCiManifest,
    changed_paths: &[String],
) -> anyhow::Result<Vec<ForgeLane>> {
    if changed_paths.is_empty() {
        return Ok(manifest.branch_lanes.clone());
    }
    let mut selected = Vec::new();
    for lane in &manifest.branch_lanes {
        if lane.paths.is_empty() || matches_any_path(changed_paths, &lane.paths)? {
            selected.push(lane.clone());
        }
    }
    Ok(selected)
}

pub fn branch_lane_paths_for_staged_target(target_id: &str) -> anyhow::Result<Option<Vec<String>>> {
    let manifest = compiled_forge_ci_manifest();
    let mut matches = manifest
        .branch_lanes
        .into_iter()
        .filter(|lane| lane.staged_linux_target.as_deref() == Some(target_id));
    let Some(first) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(anyhow!(
            "compiled forge lane catalog defines duplicate staged_linux_target `{target_id}`"
        ));
    }
    Ok(Some(first.paths))
}

fn staged_linux_lane(
    id: &'static str,
    title: &'static str,
    staged_linux_target: &'static str,
    paths: &'static [&'static str],
) -> ForgeLane {
    let Some(_) = PikaStagedLinuxTarget::from_target_id(staged_linux_target) else {
        panic!(
            "unknown staged Linux target `{staged_linux_target}` in compiled forge lane catalog"
        );
    };
    ForgeLane {
        id: id.to_string(),
        title: title.to_string(),
        entrypoint: format!("./scripts/pikaci-staged-linux-remote.sh run {staged_linux_target}"),
        command: vec![
            "./scripts/pikaci-staged-linux-remote.sh".to_string(),
            "run".to_string(),
            staged_linux_target.to_string(),
        ],
        paths: source_of_truth_paths(paths),
        concurrency_group: Some(format!("staged-linux:{staged_linux_target}")),
        staged_linux_target: Some(staged_linux_target.to_string()),
    }
}

fn pikaci_target_lane(
    id: &'static str,
    title: &'static str,
    target_id: &'static str,
    concurrency_group: Option<&'static str>,
    paths: &'static [&'static str],
) -> ForgeLane {
    ForgeLane {
        id: id.to_string(),
        title: title.to_string(),
        entrypoint: format!("nix run .#pikaci -- run {target_id}"),
        command: vec![
            "nix".to_string(),
            "run".to_string(),
            ".#pikaci".to_string(),
            "--".to_string(),
            "run".to_string(),
            target_id.to_string(),
        ],
        paths: source_of_truth_paths(paths),
        concurrency_group: concurrency_group.map(str::to_string),
        staged_linux_target: None,
    }
}

fn source_of_truth_paths(paths: &'static [&'static str]) -> Vec<String> {
    std::iter::once(FORGE_LANE_DEFINITION_PATH)
        .chain(paths.iter().copied())
        .map(str::to_string)
        .collect()
}

fn matches_any_path(changed_paths: &[String], patterns: &[String]) -> anyhow::Result<bool> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).with_context(|| format!("compile glob `{pattern}`"))?);
    }
    let globset: GlobSet = builder.build().context("build lane globset")?;
    Ok(changed_paths.iter().any(|path| globset.is_match(path)))
}

#[cfg(test)]
mod tests {
    use super::{
        FORGE_LANE_DEFINITION_PATH, branch_lane_paths_for_staged_target,
        compiled_forge_ci_manifest, select_branch_lanes,
    };

    fn selected_lane_ids(changed: &[&str]) -> Vec<String> {
        let manifest = compiled_forge_ci_manifest();
        let changed = changed
            .iter()
            .map(|path| path.to_string())
            .collect::<Vec<_>>();
        select_branch_lanes(&manifest, &changed)
            .expect("select lanes")
            .into_iter()
            .map(|lane| lane.id)
            .collect()
    }

    #[test]
    fn docs_only_branch_selects_expected_lanes() {
        let ids = selected_lane_ids(&["docs/testing/ci-selectors.md"]);
        assert!(ids.contains(&"pika_rust".to_string()));
        assert!(ids.contains(&"pika_followup".to_string()));
        assert!(ids.contains(&"fixture".to_string()));
        assert!(!ids.contains(&"notifications".to_string()));
        assert!(!ids.contains(&"rmp".to_string()));
    }

    #[test]
    fn rust_core_branch_selects_heavier_relevant_lanes() {
        let ids = selected_lane_ids(&["rust/src/lib.rs"]);
        assert!(ids.contains(&"pika_rust".to_string()));
        assert!(ids.contains(&"pika_followup".to_string()));
        assert!(ids.contains(&"pikachat".to_string()));
        assert!(ids.contains(&"apple_host_sanity".to_string()));
        assert!(ids.contains(&"fixture".to_string()));
    }

    #[test]
    fn lane_definition_change_selects_all_branch_lanes() {
        let manifest = compiled_forge_ci_manifest();
        let changed = vec![FORGE_LANE_DEFINITION_PATH.to_string()];
        let selected = select_branch_lanes(&manifest, &changed).expect("select lanes");
        assert!(!selected.is_empty());
        assert_eq!(selected.len(), manifest.branch_lanes.len());
    }

    #[test]
    fn pikachat_claude_paths_select_only_typescript_lane() {
        for path in [
            "pikachat-claude/src/access.test.ts",
            "pikachat-claude/src/channel-runtime.ts",
            "pikachat-claude/package.json",
        ] {
            assert_eq!(
                selected_lane_ids(&[path]),
                vec!["pikachat_typescript".to_string()],
                "{path} should select only the dedicated TypeScript lane"
            );
        }
    }

    #[test]
    fn staged_linux_target_synthesizes_remote_command_and_group() {
        let manifest = compiled_forge_ci_manifest();
        let lane = manifest
            .branch_lanes
            .iter()
            .find(|lane| lane.id == "pika_rust")
            .expect("pika rust lane");
        assert_eq!(
            lane.command,
            vec![
                "./scripts/pikaci-staged-linux-remote.sh".to_string(),
                "run".to_string(),
                "pre-merge-pika-rust".to_string(),
            ]
        );
        assert_eq!(
            lane.concurrency_group.as_deref(),
            Some("staged-linux:pre-merge-pika-rust")
        );
        assert_eq!(
            lane.staged_linux_target.as_deref(),
            Some("pre-merge-pika-rust")
        );
    }

    #[test]
    fn forge_owned_apple_and_nightly_lanes_invoke_pikaci_targets() {
        let manifest = compiled_forge_ci_manifest();
        let apple_lane = manifest
            .branch_lanes
            .iter()
            .find(|lane| lane.id == "apple_host_sanity")
            .expect("apple host branch lane");
        assert_eq!(
            apple_lane.command,
            vec![
                "nix".to_string(),
                "run".to_string(),
                ".#pikaci".to_string(),
                "--".to_string(),
                "run".to_string(),
                "apple_host_sanity".to_string(),
            ]
        );
        assert!(apple_lane.staged_linux_target.is_none());

        let nightly_lane = manifest
            .nightly_lanes
            .iter()
            .find(|lane| lane.id == "nightly_pika")
            .expect("nightly pika lane");
        assert_eq!(
            nightly_lane.command,
            vec![
                "nix".to_string(),
                "run".to_string(),
                ".#pikaci".to_string(),
                "--".to_string(),
                "run".to_string(),
                "nightly_pika".to_string(),
            ]
        );
        assert!(nightly_lane.staged_linux_target.is_none());
    }

    #[test]
    fn staged_targets_load_branch_lane_filters_from_compiled_catalog() {
        let filters = branch_lane_paths_for_staged_target("pre-merge-pika-rust")
            .expect("load forge lane paths")
            .expect("branch lane paths present");
        assert!(
            filters
                .iter()
                .any(|pattern| pattern == FORGE_LANE_DEFINITION_PATH)
        );
        assert!(filters.iter().any(|pattern| pattern == "rust/**"));
        assert!(filters.iter().any(|pattern| pattern == "scripts/**"));
    }

    #[test]
    fn unknown_target_has_no_branch_lane_filters() {
        let filters =
            branch_lane_paths_for_staged_target("not-a-target").expect("load forge lane paths");
        assert!(filters.is_none());
    }
}
