use std::collections::BTreeMap;
use std::path::PathBuf;

/// Typed library request for running a named pikachat scenario.
#[derive(Debug, Clone)]
pub struct ScenarioRequest {
    pub scenario: String,
    pub state_dir: Option<PathBuf>,
    pub relay: Option<String>,
    pub extra_args: Vec<String>,
}

/// Typed library request for full OpenClaw gateway e2e.
#[derive(Debug, Clone)]
pub struct OpenclawE2eRequest {
    pub state_dir: Option<PathBuf>,
    pub relay_url: Option<String>,
    pub openclaw_dir: Option<PathBuf>,
    pub keep_state: bool,
}

/// Typed library request for deterministic CLI smoke coverage.
#[derive(Debug, Clone)]
pub struct CliSmokeRequest {
    pub relay: Option<String>,
    pub with_media: bool,
    pub state_dir: Option<PathBuf>,
}

/// Target platform for local UI E2E flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiPlatform {
    Android,
    Ios,
    Desktop,
}

/// Typed library request for local UI E2E flows.
#[derive(Debug, Clone)]
pub struct UiE2eLocalRequest {
    pub platform: UiPlatform,
    pub state_dir: Option<PathBuf>,
    pub keep: bool,
    pub bot_timeout_sec: Option<u64>,
}

/// Typed library request for rust interop baseline flows.
#[derive(Debug, Clone)]
pub struct InteropRustBaselineRequest {
    pub manual: bool,
    pub keep: bool,
    pub state_dir: Option<PathBuf>,
    pub rust_interop_dir: Option<PathBuf>,
    pub bot_timeout_sec: Option<u64>,
}

/// Structured scenario completion metadata, returned by all scenario helpers.
#[derive(Debug, Clone)]
pub struct ScenarioRunOutput {
    pub state_dir: Option<PathBuf>,
    pub artifacts: Vec<PathBuf>,
    pub skipped: bool,
    pub skip_reason: Option<String>,
    pub summary: Option<PathBuf>,
    pub metadata: BTreeMap<String, String>,
}

impl ScenarioRunOutput {
    pub fn completed(state_dir: PathBuf) -> Self {
        Self {
            state_dir: Some(state_dir),
            artifacts: Vec::new(),
            skipped: false,
            skip_reason: None,
            summary: None,
            metadata: BTreeMap::new(),
        }
    }

    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            state_dir: None,
            artifacts: Vec::new(),
            skipped: true,
            skip_reason: Some(reason.into()),
            summary: None,
            metadata: BTreeMap::new(),
        }
    }

    pub fn with_artifact(mut self, artifact: PathBuf) -> Self {
        self.artifacts.push(artifact);
        self
    }

    pub fn with_summary(mut self, summary: PathBuf) -> Self {
        self.summary = Some(summary);
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}
