use anyhow::{Context, anyhow};
use serde::Deserialize;
use std::collections::HashMap;

const FORGE_MANIFEST_TOML: &str = include_str!("../../../ci/forge-lanes.toml");

#[derive(Debug, Deserialize)]
struct ManifestFile {
    version: u32,
    #[serde(default)]
    branch: LaneGroup,
}

#[derive(Debug, Default, Deserialize)]
struct LaneGroup {
    #[serde(default)]
    lanes: Vec<LaneFile>,
}

#[derive(Debug, Deserialize)]
struct LaneFile {
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    staged_linux_target: Option<String>,
}

pub(crate) fn branch_lane_paths_for_staged_target(
    target_id: &str,
) -> anyhow::Result<Option<Vec<String>>> {
    Ok(load_branch_lane_paths()?.remove(target_id))
}

fn load_branch_lane_paths() -> anyhow::Result<HashMap<String, Vec<String>>> {
    let manifest: ManifestFile = toml::from_str(FORGE_MANIFEST_TOML)
        .context("parse embedded forge lane manifest TOML for pikaci")?;
    if manifest.version != 1 {
        return Err(anyhow!(
            "unsupported forge lane manifest version {}",
            manifest.version
        ));
    }

    let mut paths_by_target = HashMap::new();
    for lane in manifest.branch.lanes {
        let Some(target) = lane
            .staged_linux_target
            .as_deref()
            .map(str::trim)
            .filter(|target| !target.is_empty())
        else {
            continue;
        };
        if let Some(previous) = paths_by_target.insert(target.to_string(), lane.paths) {
            return Err(anyhow!(
                "forge lane manifest defines duplicate staged_linux_target `{target}` with {} and {} path entries",
                previous.len(),
                paths_by_target.get(target).map_or(0, std::vec::Vec::len)
            ));
        }
    }

    Ok(paths_by_target)
}

#[cfg(test)]
mod tests {
    use super::branch_lane_paths_for_staged_target;

    #[test]
    fn staged_targets_load_branch_lane_filters_from_manifest() {
        let filters = branch_lane_paths_for_staged_target("pre-merge-pika-rust")
            .expect("load forge lane paths")
            .expect("branch lane paths present");

        assert!(
            filters
                .iter()
                .any(|pattern| pattern == "ci/forge-lanes.toml")
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
