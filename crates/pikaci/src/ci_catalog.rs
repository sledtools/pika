use crate::targets::{CiTrigger, ConcurrencyGroupSpec, TargetSpec, all_target_specs};
use anyhow::Context;
use globset::{Glob, GlobSet, GlobSetBuilder};

pub const CI_CATALOG_DEFINITION_PATH: &str = "crates/pikaci/src/ci_catalog.rs";
pub const LEGACY_FORGE_LANE_DEFINITION_PATH: &str = "crates/pikaci/src/forge_lanes.rs";
pub const TARGET_CATALOG_DEFINITION_PATH: &str = "crates/pikaci/src/targets.rs";

#[derive(Debug, Clone)]
pub struct CiCatalog {
    pub nightly_hour_utc: u32,
    pub nightly_minute_utc: u32,
    pub branch_targets: Vec<CiTarget>,
    pub nightly_targets: Vec<CiTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiTarget {
    pub id: String,
    pub title: String,
    pub entrypoint: String,
    pub command: Vec<String>,
    pub paths: Vec<String>,
    pub concurrency_group: Option<String>,
}

pub fn compiled_ci_catalog() -> CiCatalog {
    let targets = all_target_specs().expect("compile pikaci target catalog");
    let mut branch_targets = Vec::new();
    let mut nightly_targets = Vec::new();
    for target in targets {
        match target.ci_trigger {
            CiTrigger::None => {}
            CiTrigger::Branch { concurrency_group } => {
                branch_targets.push(project_target(
                    &target,
                    source_of_truth_paths(&target.filters),
                    concurrency_group,
                ));
            }
            CiTrigger::Nightly { concurrency_group } => {
                nightly_targets.push(project_target(&target, Vec::new(), concurrency_group));
            }
        }
    }
    CiCatalog {
        nightly_hour_utc: 8,
        nightly_minute_utc: 0,
        branch_targets,
        nightly_targets,
    }
}

pub fn select_branch_targets(
    catalog: &CiCatalog,
    changed_paths: &[String],
) -> anyhow::Result<Vec<CiTarget>> {
    if changed_paths.is_empty() {
        return Ok(catalog.branch_targets.clone());
    }
    let mut selected = Vec::new();
    for target in &catalog.branch_targets {
        if target.paths.is_empty() || matches_any_path(changed_paths, &target.paths)? {
            selected.push(target.clone());
        }
    }
    Ok(selected)
}

fn project_target(
    target: &TargetSpec,
    paths: Vec<String>,
    concurrency_group: Option<ConcurrencyGroupSpec>,
) -> CiTarget {
    CiTarget {
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
    std::iter::once(CI_CATALOG_DEFINITION_PATH.to_string())
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
