pub use pikaci::forge_lanes::{
    compiled_forge_ci_manifest, select_branch_lanes, ForgeCiManifest, ForgeLane,
    FORGE_LANE_DEFINITION_PATH,
};

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
