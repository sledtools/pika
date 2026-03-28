use anyhow::Context;
use rusqlite::{params, OptionalExtension};

use crate::storage::Store;

#[derive(Debug, Clone, serde::Serialize)]
pub struct MirrorSyncRunRecord {
    pub id: i64,
    pub remote_name: String,
    pub trigger_source: String,
    pub status: String,
    pub failure_kind: Option<String>,
    pub local_default_head: Option<String>,
    pub remote_default_head: Option<String>,
    pub lagging_ref_count: Option<i64>,
    pub synced_ref_count: Option<i64>,
    pub error_text: Option<String>,
    pub created_at: String,
    pub finished_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MirrorStatusRecord {
    pub remote_name: String,
    pub last_attempt: Option<MirrorSyncRunRecord>,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub consecutive_failure_count: i64,
    pub current_lagging_ref_count: Option<i64>,
    pub current_failure_kind: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MirrorSyncRunInput {
    pub repo: String,
    pub canonical_git_dir: String,
    pub default_branch: String,
    pub remote_name: String,
    pub trigger_source: String,
    pub status: String,
    pub local_default_head: Option<String>,
    pub remote_default_head: Option<String>,
    pub lagging_ref_count: Option<i64>,
    pub synced_ref_count: Option<i64>,
    pub error_text: Option<String>,
}

impl Store {
    pub fn record_mirror_sync_run(&self, input: &MirrorSyncRunInput) -> anyhow::Result<i64> {
        let repo_id = self.ensure_forge_repo_metadata(
            &input.repo,
            &input.canonical_git_dir,
            &input.default_branch,
            crate::ci::FORGE_LANE_DEFINITION_PATH,
        )?;
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO mirror_sync_runs(
                    repo_id,
                    remote_name,
                    trigger_source,
                    status,
                    local_default_head,
                    remote_default_head,
                    lagging_ref_count,
                    synced_ref_count,
                    error_text
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    repo_id,
                    &input.remote_name,
                    &input.trigger_source,
                    &input.status,
                    &input.local_default_head,
                    &input.remote_default_head,
                    input.lagging_ref_count,
                    input.synced_ref_count,
                    &input.error_text,
                ],
            )
            .context("insert mirror sync run")?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn get_mirror_status(
        &self,
        repo: &str,
        remote_name: &str,
    ) -> anyhow::Result<Option<MirrorStatusRecord>> {
        self.with_connection(|conn| {
            let repo_id: Option<i64> = conn
                .query_row(
                    "SELECT id FROM repos WHERE repo = ?1",
                    params![repo],
                    |row| row.get(0),
                )
                .optional()
                .context("lookup repo for mirror status")?;
            let Some(repo_id) = repo_id else {
                return Ok(None);
            };
            let last_attempt = conn
                .query_row(
                    "SELECT id, remote_name, trigger_source, status, local_default_head, remote_default_head,
                            lagging_ref_count, synced_ref_count, error_text, created_at, finished_at
                     FROM mirror_sync_runs
                     WHERE repo_id = ?1 AND remote_name = ?2
                     ORDER BY id DESC
                     LIMIT 1",
                    params![repo_id, remote_name],
                    map_mirror_sync_row,
                )
                .optional()
                .context("query last mirror attempt")?;
            let last_success_at = conn
                .query_row(
                    "SELECT finished_at
                     FROM mirror_sync_runs
                     WHERE repo_id = ?1 AND remote_name = ?2 AND status = 'success'
                     ORDER BY id DESC
                     LIMIT 1",
                    params![repo_id, remote_name],
                    |row| row.get(0),
                )
                .optional()
                .context("query last successful mirror sync")?;
            let last_failure_at = conn
                .query_row(
                    "SELECT finished_at
                     FROM mirror_sync_runs
                     WHERE repo_id = ?1 AND remote_name = ?2 AND status = 'failed'
                     ORDER BY id DESC
                     LIMIT 1",
                    params![repo_id, remote_name],
                    |row| row.get(0),
                )
                .optional()
                .context("query last failed mirror sync")?;
            let mut recent_statuses = conn
                .prepare(
                    "SELECT status
                     FROM mirror_sync_runs
                     WHERE repo_id = ?1 AND remote_name = ?2
                     ORDER BY id DESC
                     LIMIT 32",
                )
                .context("prepare mirror status streak query")?;
            let statuses = recent_statuses
                .query_map(params![repo_id, remote_name], |row| row.get::<_, String>(0))
                .context("query mirror status streak")?;
            let mut consecutive_failure_count = 0_i64;
            for status in statuses {
                let status = status.context("read mirror streak row")?;
                if status == "failed" {
                    consecutive_failure_count += 1;
                } else {
                    break;
                }
            }
            let current_lagging_ref_count =
                last_attempt.as_ref().and_then(|run| run.lagging_ref_count);
            let current_failure_kind = last_attempt
                .as_ref()
                .and_then(|run| run.failure_kind.clone());
            Ok(Some(MirrorStatusRecord {
                remote_name: remote_name.to_string(),
                last_attempt,
                last_success_at,
                last_failure_at,
                consecutive_failure_count,
                current_lagging_ref_count,
                current_failure_kind,
            }))
        })
    }

    pub fn list_recent_mirror_sync_runs(
        &self,
        repo: &str,
        remote_name: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<MirrorSyncRunRecord>> {
        self.with_connection(|conn| {
            let repo_id: Option<i64> = conn
                .query_row(
                    "SELECT id FROM repos WHERE repo = ?1",
                    params![repo],
                    |row| row.get(0),
                )
                .optional()
                .context("lookup repo for mirror run list")?;
            let Some(repo_id) = repo_id else {
                return Ok(Vec::new());
            };
            let mut stmt = conn
                .prepare(
                    "SELECT id, remote_name, trigger_source, status, local_default_head, remote_default_head,
                            lagging_ref_count, synced_ref_count, error_text, created_at, finished_at
                     FROM mirror_sync_runs
                     WHERE repo_id = ?1 AND remote_name = ?2
                     ORDER BY id DESC
                     LIMIT ?3",
                )
                .context("prepare mirror run list query")?;
            let rows = stmt
                .query_map(params![repo_id, remote_name, limit as i64], map_mirror_sync_row)
                .context("query mirror run rows")?;
            let mut runs = Vec::new();
            for row in rows {
                runs.push(row.context("read mirror run row")?);
            }
            Ok(runs)
        })
    }
}

fn map_mirror_sync_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MirrorSyncRunRecord> {
    let status: String = row.get(3)?;
    let error_text: Option<String> = row.get(8)?;
    Ok(MirrorSyncRunRecord {
        id: row.get(0)?,
        remote_name: row.get(1)?,
        trigger_source: row.get(2)?,
        failure_kind: classify_mirror_failure_kind(&status, error_text.as_deref())
            .map(str::to_string),
        status,
        local_default_head: row.get(4)?,
        remote_default_head: row.get(5)?,
        lagging_ref_count: row.get(6)?,
        synced_ref_count: row.get(7)?,
        error_text,
        created_at: row.get(9)?,
        finished_at: row.get(10)?,
    })
}

fn classify_mirror_failure_kind(status: &str, error_text: Option<&str>) -> Option<&'static str> {
    if status != "failed" {
        return None;
    }
    let lower = error_text.unwrap_or_default().to_ascii_lowercase();
    if lower.contains("already in sync; treating this trigger as obsolete") {
        return Some("obsolete");
    }
    if lower.contains("stale mirror sync still holds repo lock") {
        return Some("stale");
    }
    if lower.contains("mirror sync already running") {
        return Some("busy");
    }
    if lower.contains("timed out after") {
        return Some("timeout");
    }
    let config_markers = [
        "mirror remote",
        "authentication failed",
        "could not read username",
        "repository not found",
        "permission denied",
        "not authorized",
        "access denied",
        "requested url returned error: 403",
        "requested url returned error: 401",
        "no such remote",
    ];
    if config_markers.iter().any(|marker| lower.contains(marker)) {
        Some("config")
    } else {
        Some("transient")
    }
}

#[cfg(test)]
mod tests {
    use super::classify_mirror_failure_kind;

    #[test]
    fn classifies_mirror_failure_kinds_for_runtime_safety_cases() {
        assert_eq!(
            classify_mirror_failure_kind(
                "failed",
                Some("mirror sync already running (pid 42, trigger background, elapsed 9s)")
            ),
            Some("busy")
        );
        assert_eq!(
            classify_mirror_failure_kind(
                "failed",
                Some(
                    "stale mirror sync still holds repo lock (pid 42, trigger background, elapsed 999s)"
                )
            ),
            Some("stale")
        );
        assert_eq!(
            classify_mirror_failure_kind(
                "failed",
                Some("push canonical refs to mirror remote `github` timed out after 120s")
            ),
            Some("timeout")
        );
        assert_eq!(
            classify_mirror_failure_kind(
                "failed",
                Some(
                    "mirror sync already running (pid 42, trigger background, elapsed 3s): mirror is already in sync; treating this trigger as obsolete"
                )
            ),
            Some("obsolete")
        );
    }
}
