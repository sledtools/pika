pub use pikaci::ci_catalog::CI_CATALOG_DEFINITION_PATH as FORGE_LANE_DEFINITION_PATH;

use pikaci::ci_catalog::{compiled_ci_catalog, select_branch_targets, CiCatalog, CiTarget};

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
    ForgeCiManifest {
        nightly_hour_utc: catalog.nightly_hour_utc,
        nightly_minute_utc: catalog.nightly_minute_utc,
        branch_lanes: catalog
            .branch_targets
            .into_iter()
            .map(project_target_to_lane)
            .collect(),
        nightly_lanes: catalog
            .nightly_targets
            .into_iter()
            .map(project_target_to_lane)
            .collect(),
    }
}

pub fn select_branch_lanes(
    manifest: &ForgeCiManifest,
    changed_paths: &[String],
) -> anyhow::Result<Vec<ForgeLane>> {
    let catalog = CiCatalog {
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
    };
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

#[cfg(test)]
mod tests {
    use super::{compiled_forge_ci_manifest, select_branch_lanes, FORGE_LANE_DEFINITION_PATH};

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
}
