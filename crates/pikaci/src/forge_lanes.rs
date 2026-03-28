use crate::ci_catalog::{
    CI_CATALOG_DEFINITION_PATH, CiCatalog, CiTarget, compiled_ci_catalog, select_branch_targets,
};

pub const FORGE_LANE_DEFINITION_PATH: &str = CI_CATALOG_DEFINITION_PATH;

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
    let catalog = compiled_ci_catalog();
    let branch_lanes = catalog
        .branch_targets
        .into_iter()
        .map(project_target_to_lane)
        .collect();
    let nightly_lanes = catalog
        .nightly_targets
        .into_iter()
        .map(project_target_to_lane)
        .collect();
    ForgeCiManifest {
        nightly_hour_utc: catalog.nightly_hour_utc,
        nightly_minute_utc: catalog.nightly_minute_utc,
        branch_lanes,
        nightly_lanes,
    }
}

pub fn select_branch_lanes(
    manifest: &ForgeCiManifest,
    changed_paths: &[String],
) -> anyhow::Result<Vec<ForgeLane>> {
    let catalog = project_manifest_to_catalog(manifest);
    select_branch_targets(&catalog, changed_paths)
        .map(|targets| targets.into_iter().map(project_target_to_lane).collect())
}

fn project_target_to_lane(target: CiTarget) -> ForgeLane {
    ForgeLane {
        id: target.id,
        title: target.title,
        entrypoint: target.entrypoint,
        command: target.command,
        paths: target.paths,
        concurrency_group: target.concurrency_group,
    }
}

fn project_lane_to_target(lane: &ForgeLane) -> CiTarget {
    CiTarget {
        id: lane.id.clone(),
        title: lane.title.clone(),
        entrypoint: lane.entrypoint.clone(),
        command: lane.command.clone(),
        paths: lane.paths.clone(),
        concurrency_group: lane.concurrency_group.clone(),
    }
}

fn project_manifest_to_catalog(manifest: &ForgeCiManifest) -> CiCatalog {
    CiCatalog {
        nightly_hour_utc: manifest.nightly_hour_utc,
        nightly_minute_utc: manifest.nightly_minute_utc,
        branch_targets: manifest
            .branch_lanes
            .iter()
            .map(project_lane_to_target)
            .collect(),
        nightly_targets: manifest
            .nightly_lanes
            .iter()
            .map(project_lane_to_target)
            .collect(),
    }
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
