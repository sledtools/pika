use crate::targets::{CiTrigger, ConcurrencyGroupSpec, TargetSpec, all_target_specs};
use anyhow::Context;
use globset::{Glob, GlobSet, GlobSetBuilder};

pub const FORGE_LANE_DEFINITION_PATH: &str = "crates/pikaci/src/forge_lanes.rs";
pub const TARGET_CATALOG_DEFINITION_PATH: &str = "crates/pikaci/src/targets.rs";

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
}

pub fn compiled_forge_ci_manifest() -> ForgeCiManifest {
    let targets = all_target_specs().expect("compile pikaci target catalog");
    let mut branch_lanes = Vec::new();
    let mut nightly_lanes = Vec::new();
    for target in targets {
        match target.ci_trigger {
            CiTrigger::None => {}
            CiTrigger::Branch { concurrency_group } => {
                branch_lanes.push(project_lane(
                    &target,
                    source_of_truth_paths(&target.filters),
                    concurrency_group,
                ));
            }
            CiTrigger::Nightly { concurrency_group } => {
                nightly_lanes.push(project_lane(&target, Vec::new(), concurrency_group));
            }
        }
    }
    ForgeCiManifest {
        nightly_hour_utc: 8,
        nightly_minute_utc: 0,
        branch_lanes,
        nightly_lanes,
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

fn project_lane(
    target: &TargetSpec,
    paths: Vec<String>,
    concurrency_group: Option<ConcurrencyGroupSpec>,
) -> ForgeLane {
    ForgeLane {
        id: target.id.to_string(),
        title: target.title.to_string(),
        entrypoint: format!("nix run .#pikaci -- run {}", target.id),
        command: vec![
            "nix".to_string(),
            "run".to_string(),
            ".#pikaci".to_string(),
            "--".to_string(),
            "run".to_string(),
            target.id.to_string(),
        ],
        paths,
        concurrency_group: resolve_concurrency_group(target.id, concurrency_group),
    }
}

fn resolve_concurrency_group(
    target_id: &str,
    configured: Option<ConcurrencyGroupSpec>,
) -> Option<String> {
    match configured {
        None => None,
        Some(ConcurrencyGroupSpec::Literal(group)) => Some(group.to_string()),
        Some(ConcurrencyGroupSpec::StagedLinuxTarget) => Some(format!("staged-linux:{target_id}")),
    }
}

fn source_of_truth_paths(paths: &[String]) -> Vec<String> {
    std::iter::once(FORGE_LANE_DEFINITION_PATH.to_string())
        .chain(std::iter::once(TARGET_CATALOG_DEFINITION_PATH.to_string()))
        .chain(paths.iter().cloned())
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
    use super::{FORGE_LANE_DEFINITION_PATH, compiled_forge_ci_manifest, select_branch_lanes};

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
        assert!(ids.contains(&"pre-merge-pika-rust".to_string()));
        assert!(ids.contains(&"pre-merge-pika-followup".to_string()));
        assert!(ids.contains(&"pre-merge-fixture-rust".to_string()));
        assert!(!ids.contains(&"pre-merge-notifications".to_string()));
        assert!(!ids.contains(&"pre-merge-rmp".to_string()));
    }

    #[test]
    fn rust_core_branch_selects_heavier_relevant_lanes() {
        let ids = selected_lane_ids(&["rust/src/lib.rs"]);
        assert!(ids.contains(&"pre-merge-pika-rust".to_string()));
        assert!(ids.contains(&"pre-merge-pika-followup".to_string()));
        assert!(ids.contains(&"pre-merge-pikachat-rust".to_string()));
        assert!(ids.contains(&"apple_desktop_compile".to_string()));
        assert!(ids.contains(&"apple_ios_compile".to_string()));
        assert!(ids.contains(&"pre-merge-fixture-rust".to_string()));
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
                vec!["pre-merge-pikachat-typescript".to_string()],
                "{path} should select only the dedicated TypeScript lane"
            );
        }
    }

    #[test]
    fn staged_linux_lane_projects_pikaci_entrypoint_and_group() {
        let manifest = compiled_forge_ci_manifest();
        let lane = manifest
            .branch_lanes
            .iter()
            .find(|lane| lane.id == "pre-merge-pika-rust")
            .expect("pika rust lane");
        assert_eq!(
            lane.command,
            vec![
                "nix".to_string(),
                "run".to_string(),
                ".#pikaci".to_string(),
                "--".to_string(),
                "run".to_string(),
                "pre-merge-pika-rust".to_string(),
            ]
        );
        assert_eq!(
            lane.concurrency_group.as_deref(),
            Some("staged-linux:pre-merge-pika-rust")
        );
    }

    #[test]
    fn apple_and_nightly_lanes_project_pikaci_targets() {
        let manifest = compiled_forge_ci_manifest();
        let apple_lane = manifest
            .branch_lanes
            .iter()
            .find(|lane| lane.id == "apple_desktop_compile")
            .expect("apple desktop branch lane");
        assert_eq!(
            apple_lane.command,
            vec![
                "nix".to_string(),
                "run".to_string(),
                ".#pikaci".to_string(),
                "--".to_string(),
                "run".to_string(),
                "apple_desktop_compile".to_string(),
            ]
        );
        assert_eq!(
            apple_lane.concurrency_group.as_deref(),
            Some("apple-compile")
        );

        let nightly_lane = manifest
            .nightly_lanes
            .iter()
            .find(|lane| lane.id == "nightly_apple_host_bundle")
            .expect("nightly apple lane");
        assert_eq!(
            nightly_lane.command,
            vec![
                "nix".to_string(),
                "run".to_string(),
                ".#pikaci".to_string(),
                "--".to_string(),
                "run".to_string(),
                "nightly_apple_host_bundle".to_string(),
            ]
        );
        assert_eq!(
            nightly_lane.concurrency_group.as_deref(),
            Some("apple-runtime")
        );
    }
}
