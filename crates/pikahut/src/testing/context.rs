use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;

use crate::config;

/// Artifact retention policy for integration test runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArtifactPolicy {
    /// Remove auto-created state/artifacts after successful completion.
    DeleteOnSuccess,
    /// Preserve state/artifacts when a run fails, but clean successful runs.
    #[default]
    PreserveOnFailure,
    /// Preserve state/artifacts for both success and failure.
    PreserveAlways,
}

/// Builder for [`TestContext`].
///
/// # Examples
///
/// ```no_run
/// use pikahut::testing::{ArtifactPolicy, TestContext};
///
/// let context = TestContext::builder("cli-smoke")
///     .artifact_policy(ArtifactPolicy::PreserveOnFailure)
///     .build()
///     .expect("context");
///
/// assert!(context.state_dir().exists());
/// ```
#[derive(Debug, Clone)]
pub struct TestContextBuilder {
    run_name: String,
    run_id: Option<String>,
    state_dir: Option<PathBuf>,
    artifact_policy: ArtifactPolicy,
}

impl TestContextBuilder {
    pub fn new(run_name: impl Into<String>) -> Self {
        Self {
            run_name: sanitize_id(&run_name.into()),
            run_id: None,
            state_dir: None,
            artifact_policy: ArtifactPolicy::default(),
        }
    }

    pub fn run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(sanitize_id(&run_id.into()));
        self
    }

    pub fn state_dir(mut self, state_dir: impl Into<PathBuf>) -> Self {
        self.state_dir = Some(state_dir.into());
        self
    }

    pub fn artifact_policy(mut self, artifact_policy: ArtifactPolicy) -> Self {
        self.artifact_policy = artifact_policy;
        self
    }

    pub fn build(self) -> Result<TestContext> {
        let workspace_root = config::find_workspace_root()?;
        let run_id = self
            .run_id
            .unwrap_or_else(|| Utc::now().format("%Y%m%dt%H%M%SZ").to_string());

        let (state_dir, auto_state_dir) = match self.state_dir {
            Some(path) => {
                fs::create_dir_all(&path)
                    .with_context(|| format!("create state dir {}", path.display()))?;
                (path, false)
            }
            None => {
                let dir = tempfile::Builder::new()
                    .prefix(&format!("pikahut-{}-", self.run_name))
                    .tempdir()
                    .context("create temporary state directory")?
                    .keep();
                fs::create_dir_all(&dir)
                    .with_context(|| format!("create state dir {}", dir.display()))?;
                (dir, true)
            }
        };

        let artifacts_dir = state_dir
            .join("artifacts")
            .join(format!("{}-{}", self.run_name, run_id));
        fs::create_dir_all(&artifacts_dir)
            .with_context(|| format!("create artifacts dir {}", artifacts_dir.display()))?;

        Ok(TestContext {
            run_name: self.run_name,
            run_id,
            workspace_root,
            state_dir,
            artifacts_dir,
            auto_state_dir,
            artifact_policy: self.artifact_policy,
            success: false,
        })
    }
}

/// Per-test execution context for fixture state and artifact lifecycle.
///
/// # Examples
///
/// ```no_run
/// use pikahut::testing::TestContext;
///
/// let mut context = TestContext::builder("openclaw-e2e").build().expect("context");
/// context
///     .write_artifact("notes/summary.txt", "started")
///     .expect("artifact");
/// context.mark_success();
/// ```
#[derive(Debug)]
pub struct TestContext {
    run_name: String,
    run_id: String,
    workspace_root: PathBuf,
    state_dir: PathBuf,
    artifacts_dir: PathBuf,
    auto_state_dir: bool,
    artifact_policy: ArtifactPolicy,
    success: bool,
}

impl TestContext {
    pub fn builder(run_name: impl Into<String>) -> TestContextBuilder {
        TestContextBuilder::new(run_name)
    }

    pub fn run_name(&self) -> &str {
        &self.run_name
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn artifacts_dir(&self) -> &Path {
        &self.artifacts_dir
    }

    pub fn artifact_policy(&self) -> ArtifactPolicy {
        self.artifact_policy
    }

    pub fn is_auto_state_dir(&self) -> bool {
        self.auto_state_dir
    }

    pub fn mark_success(&mut self) {
        self.success = true;
    }

    pub fn ensure_artifact_subdir(&self, relative: impl AsRef<Path>) -> Result<PathBuf> {
        let dir = self.artifacts_dir.join(relative.as_ref());
        fs::create_dir_all(&dir)
            .with_context(|| format!("create artifact directory {}", dir.display()))?;
        Ok(dir)
    }

    pub fn write_artifact(
        &self,
        relative: impl AsRef<Path>,
        contents: impl AsRef<[u8]>,
    ) -> Result<PathBuf> {
        let full_path = self.artifacts_dir.join(relative.as_ref());
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create artifact parent {}", parent.display()))?;
        }
        fs::write(&full_path, contents)
            .with_context(|| format!("write artifact {}", full_path.display()))?;
        Ok(full_path)
    }

    fn should_preserve(&self) -> bool {
        match self.artifact_policy {
            ArtifactPolicy::DeleteOnSuccess => !self.success,
            ArtifactPolicy::PreserveOnFailure => !self.success,
            ArtifactPolicy::PreserveAlways => true,
        }
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        if !self.auto_state_dir {
            return;
        }

        if self.should_preserve() {
            eprintln!(
                "note: preserving test state dir at {} (policy={:?}, success={})",
                self.state_dir.display(),
                self.artifact_policy,
                self.success
            );
            return;
        }

        let _ = fs::remove_dir_all(&self.state_dir);
    }
}

fn sanitize_id(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }

    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "run".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_state_dir_is_default() {
        let mut context = TestContext::builder("auto-default").build().unwrap();
        assert!(context.is_auto_state_dir());
        let state_dir = context.state_dir().to_path_buf();
        context.mark_success();
        drop(context);
        assert!(!state_dir.exists());
    }

    #[test]
    fn delete_on_success_removes_auto_state_dir() {
        let state_dir = {
            let mut context = TestContext::builder("cleanup-on-success")
                .artifact_policy(ArtifactPolicy::DeleteOnSuccess)
                .build()
                .unwrap();
            let path = context.state_dir().to_path_buf();
            context.mark_success();
            path
        };

        assert!(!state_dir.exists());
    }

    #[test]
    fn preserve_on_failure_keeps_auto_state_dir() {
        let state_dir = {
            let context = TestContext::builder("preserve-on-failure").build().unwrap();
            context.state_dir().to_path_buf()
        };

        assert!(state_dir.exists());
        std::fs::remove_dir_all(&state_dir).unwrap();
    }

    #[test]
    fn explicit_state_dir_is_never_auto_deleted() {
        let explicit = tempfile::tempdir().unwrap();
        let state_dir = explicit.path().join("nested-state");
        {
            let mut context = TestContext::builder("explicit-state")
                .state_dir(&state_dir)
                .artifact_policy(ArtifactPolicy::DeleteOnSuccess)
                .build()
                .unwrap();
            context.mark_success();
        }
        assert!(state_dir.exists());
    }
}
