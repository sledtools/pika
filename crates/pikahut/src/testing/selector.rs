use std::path::Path;

use super::{Capabilities, Requirement};

pub fn emit_skip(reason: &str) {
    eprintln!("SKIP: {reason}");
    if std::env::var("GITHUB_ACTIONS")
        .ok()
        .map(|v| v == "true")
        .unwrap_or(false)
    {
        eprintln!("::notice title=pikahut integration skipped::{reason}");
    }
}

pub fn skip_if_missing_requirements(workspace_root: &Path, requirements: &[Requirement]) -> bool {
    let caps = Capabilities::probe(workspace_root);
    if let Err(skip) = caps.require_all_or_skip(requirements) {
        emit_skip(&skip.to_string());
        true
    } else {
        false
    }
}

pub fn skip_if_missing_env(required: &[&str], resolver: impl Fn(&str) -> Option<String>) -> bool {
    for name in required {
        if resolver(name).is_none() {
            emit_skip(&format!("required env var missing: {name}"));
            return true;
        }
    }
    false
}
