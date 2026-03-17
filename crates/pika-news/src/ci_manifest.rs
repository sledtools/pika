use anyhow::{anyhow, Context};
use chrono::{NaiveTime, Timelike};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

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
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    version: u32,
    nightly_schedule_utc: String,
    #[serde(default)]
    branch: LaneGroup,
    #[serde(default)]
    nightly: LaneGroup,
}

#[derive(Debug, Default, Deserialize)]
struct LaneGroup {
    #[serde(default)]
    lanes: Vec<LaneFile>,
}

#[derive(Debug, Deserialize)]
struct LaneFile {
    id: String,
    title: String,
    entrypoint: String,
    command: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
}

pub fn parse_manifest(raw: &str) -> anyhow::Result<ForgeCiManifest> {
    let manifest: ManifestFile = toml::from_str(raw).context("parse forge lane manifest TOML")?;
    if manifest.version != 1 {
        return Err(anyhow!(
            "unsupported forge lane manifest version {}",
            manifest.version
        ));
    }
    let nightly_time = NaiveTime::parse_from_str(&manifest.nightly_schedule_utc, "%H:%M")
        .with_context(|| {
            format!(
                "parse nightly_schedule_utc `{}`",
                manifest.nightly_schedule_utc
            )
        })?;
    Ok(ForgeCiManifest {
        nightly_hour_utc: nightly_time.hour(),
        nightly_minute_utc: nightly_time.minute(),
        branch_lanes: manifest
            .branch
            .lanes
            .into_iter()
            .map(map_lane)
            .collect::<anyhow::Result<Vec<_>>>()?,
        nightly_lanes: manifest
            .nightly
            .lanes
            .into_iter()
            .map(map_lane)
            .collect::<anyhow::Result<Vec<_>>>()?,
    })
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

fn map_lane(lane: LaneFile) -> anyhow::Result<ForgeLane> {
    if lane.id.trim().is_empty() {
        return Err(anyhow!("forge lane id must not be empty"));
    }
    if lane.command.is_empty() {
        return Err(anyhow!(
            "forge lane `{}` command must not be empty",
            lane.id
        ));
    }
    Ok(ForgeLane {
        id: lane.id,
        title: lane.title,
        entrypoint: lane.entrypoint,
        command: lane.command,
        paths: lane.paths,
    })
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
    use super::{parse_manifest, select_branch_lanes};

    #[test]
    fn docs_only_branch_selects_expected_lanes() {
        let manifest =
            parse_manifest(include_str!("../../../ci/forge-lanes.toml")).expect("manifest");
        let changed = vec!["docs/testing/ci-selectors.md".to_string()];
        let selected = select_branch_lanes(&manifest, &changed).expect("select lanes");
        let ids = selected.into_iter().map(|lane| lane.id).collect::<Vec<_>>();

        assert!(ids.contains(&"pika".to_string()));
        assert!(ids.contains(&"fixture".to_string()));
        assert!(!ids.contains(&"notifications".to_string()));
        assert!(!ids.contains(&"rmp".to_string()));
    }

    #[test]
    fn rust_core_branch_selects_heavier_relevant_lanes() {
        let manifest =
            parse_manifest(include_str!("../../../ci/forge-lanes.toml")).expect("manifest");
        let changed = vec!["rust/src/lib.rs".to_string()];
        let selected = select_branch_lanes(&manifest, &changed).expect("select lanes");
        let ids = selected.into_iter().map(|lane| lane.id).collect::<Vec<_>>();

        assert!(ids.contains(&"pika".to_string()));
        assert!(ids.contains(&"pikachat".to_string()));
        assert!(ids.contains(&"apple_host_sanity".to_string()));
        assert!(ids.contains(&"fixture".to_string()));
    }
}
