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
    pub concurrency_group: Option<String>,
    pub staged_linux_target: Option<String>,
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
    #[serde(default)]
    entrypoint: String,
    #[serde(default)]
    command: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    concurrency_group: Option<String>,
    #[serde(default)]
    staged_linux_target: Option<String>,
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
    let staged_linux_target = lane
        .staged_linux_target
        .as_ref()
        .map(|target| target.trim())
        .filter(|target| !target.is_empty())
        .map(ToOwned::to_owned);
    if staged_linux_target.is_none() && lane.command.is_empty() {
        return Err(anyhow!(
            "forge lane `{}` command must not be empty",
            lane.id
        ));
    }
    let entrypoint = if lane.entrypoint.trim().is_empty() {
        staged_linux_target
            .as_ref()
            .map(|target| format!("./scripts/pikaci-staged-linux-remote.sh run {target}"))
            .ok_or_else(|| anyhow!("forge lane `{}` entrypoint must not be empty", lane.id))?
    } else {
        lane.entrypoint
    };
    let command = match staged_linux_target.as_ref() {
        Some(target) => vec![
            "./scripts/pikaci-staged-linux-remote.sh".to_string(),
            "run".to_string(),
            target.clone(),
        ],
        None => lane.command,
    };
    let concurrency_group = lane
        .concurrency_group
        .as_ref()
        .map(|group| group.trim())
        .filter(|group| !group.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            staged_linux_target
                .as_ref()
                .map(|target| format!("staged-linux:{target}"))
        });
    Ok(ForgeLane {
        id: lane.id,
        title: lane.title,
        entrypoint,
        command,
        paths: lane.paths,
        concurrency_group,
        staged_linux_target,
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

    fn selected_lane_ids(changed: &[&str]) -> Vec<String> {
        let manifest =
            parse_manifest(include_str!("../../../ci/forge-lanes.toml")).expect("manifest");
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
    fn canonical_manifest_change_selects_branch_lanes() {
        let manifest =
            parse_manifest(include_str!("../../../ci/forge-lanes.toml")).expect("manifest");
        let changed = vec!["ci/forge-lanes.toml".to_string()];
        let selected = select_branch_lanes(&manifest, &changed).expect("select lanes");

        assert!(!selected.is_empty());
        assert_eq!(selected.len(), manifest.branch_lanes.len());
    }

    #[test]
    fn staged_linux_target_synthesizes_remote_command_and_group() {
        let manifest = parse_manifest(
            r#"
version = 1
nightly_schedule_utc = "08:00"

[[branch.lanes]]
id = "linux"
title = "linux"
staged_linux_target = "pre-merge-pika-rust"

[[nightly.lanes]]
id = "nightly"
title = "nightly"
entrypoint = "./nightly.sh"
command = ["./nightly.sh"]
"#,
        )
        .expect("manifest");

        let lane = &manifest.branch_lanes[0];
        assert_eq!(
            lane.command,
            vec![
                "./scripts/pikaci-staged-linux-remote.sh".to_string(),
                "run".to_string(),
                "pre-merge-pika-rust".to_string(),
            ]
        );
        assert_eq!(
            lane.entrypoint,
            "./scripts/pikaci-staged-linux-remote.sh run pre-merge-pika-rust"
        );
        assert_eq!(
            lane.concurrency_group.as_deref(),
            Some("staged-linux:pre-merge-pika-rust")
        );
    }

    #[test]
    fn checked_in_authoritative_linux_lanes_use_staged_remote_commands() {
        let manifest =
            parse_manifest(include_str!("../../../ci/forge-lanes.toml")).expect("manifest");
        for (lane_id, target) in [
            ("pika_rust", "pre-merge-pika-rust"),
            ("pika_followup", "pre-merge-pika-followup"),
            ("notifications", "pre-merge-notifications"),
            ("agent_contracts", "pre-merge-agent-contracts"),
            ("rmp", "pre-merge-rmp"),
            ("pikachat", "pre-merge-pikachat-rust"),
            ("pikachat_typescript", "pre-merge-pikachat-typescript"),
            ("pikachat_openclaw_e2e", "pre-merge-pikachat-openclaw-e2e"),
            ("fixture", "pre-merge-fixture-rust"),
        ] {
            let lane = manifest
                .branch_lanes
                .iter()
                .find(|lane| lane.id == lane_id)
                .expect("lane exists");
            assert_eq!(lane.staged_linux_target.as_deref(), Some(target));
            assert_eq!(
                lane.command,
                vec![
                    "./scripts/pikaci-staged-linux-remote.sh".to_string(),
                    "run".to_string(),
                    target.to_string(),
                ]
            );
        }
    }

    #[test]
    fn forge_web_changes_select_control_plane_lanes() {
        assert_eq!(
            selected_lane_ids(&["crates/pika-news/src/web.rs"]),
            vec!["notifications".to_string(), "agent_contracts".to_string()]
        );
    }

    #[test]
    fn ph_cli_changes_select_control_plane_lanes() {
        assert_eq!(
            selected_lane_ids(&["crates/ph/src/lib.rs"]),
            vec!["notifications".to_string(), "agent_contracts".to_string()]
        );
    }

    #[test]
    fn agents_root_file_selects_agent_workflow_lanes() {
        assert_eq!(
            selected_lane_ids(&["AGENTS.md"]),
            vec!["pika_followup".to_string(), "agent_contracts".to_string()]
        );
    }

    #[test]
    fn agents_skill_selects_agent_workflow_lanes() {
        assert_eq!(
            selected_lane_ids(&[".agents/skills/land/SKILL.md"]),
            vec!["pika_followup".to_string(), "agent_contracts".to_string()]
        );
    }
}
