use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use anyhow::{bail, Context};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::ci_manifest::ForgeLane;
use crate::ci_state::{
    classify_ci_failure, configured_target_key_for_lane, next_target_cooloff_until,
    CiLaneExecutionReason, CiLaneFailureKind, CiLaneStatus, CiTargetHealthSnapshot,
    CiTargetHealthState, CI_TARGET_HEALTH_INFRA_FAILURE_THRESHOLD,
};
use crate::storage::Store;

pub const CI_LANE_LEASE_LOST: &str = "ci lane lease lost";

#[derive(Debug, Clone)]
pub struct BranchUpsertInput {
    pub repo: String,
    pub canonical_git_dir: String,
    pub default_branch: String,
    pub ci_entrypoint: String,
    pub branch_name: String,
    pub title: String,
    pub head_sha: String,
    pub merge_base_sha: String,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct BranchUpsertOutcome {
    pub branch_id: i64,
    pub inserted: bool,
    pub head_changed: bool,
    pub queued_generation: bool,
    pub queued_ci: bool,
}

#[derive(Debug, Clone)]
pub struct BranchGenerationJob {
    pub artifact_id: i64,
    pub branch_id: i64,
    pub repo: String,
    pub branch_name: String,
    pub title: String,
    pub head_sha: String,
    pub merge_base_sha: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BranchFeedItem {
    pub branch_id: i64,
    pub repo: String,
    pub branch_name: String,
    pub title: String,
    pub state: String,
    pub updated_at: String,
    pub head_sha: String,
    pub tutorial_status: String,
    pub ci_status: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BranchDetailRecord {
    pub branch_id: i64,
    pub current_artifact_id: Option<i64>,
    pub repo: String,
    pub branch_name: String,
    pub title: String,
    pub branch_state: String,
    pub updated_at: String,
    pub target_branch: String,
    pub head_sha: String,
    pub merge_base_sha: String,
    pub merge_commit_sha: Option<String>,
    pub tutorial_status: String,
    pub tutorial_json: Option<String>,
    pub unified_diff: Option<String>,
    pub claude_session_id: Option<String>,
    pub error_message: Option<String>,
    pub ci_status: String,
}

#[derive(Debug, Clone)]
pub struct BranchLookupRecord {
    pub branch_id: i64,
    pub repo: String,
    pub branch_name: String,
    pub branch_state: String,
}

#[derive(Debug, Clone)]
pub struct BranchCiRunRecord {
    pub id: i64,
    pub source_head_sha: String,
    pub status: String,
    pub lane_count: usize,
    pub rerun_of_run_id: Option<i64>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub lanes: Vec<BranchCiLaneRecord>,
}

#[derive(Debug, Clone)]
pub struct BranchCiLaneRecord {
    pub id: i64,
    pub lane_id: String,
    pub title: String,
    pub entrypoint: String,
    pub status: String,
    pub execution_reason: CiLaneExecutionReason,
    pub failure_kind: Option<CiLaneFailureKind>,
    pub pikaci_run_id: Option<String>,
    pub pikaci_target_id: Option<String>,
    pub ci_target_key: Option<String>,
    pub target_health: Option<CiTargetHealthSnapshot>,
    pub log_text: Option<String>,
    pub retry_count: i64,
    pub rerun_of_lane_run_id: Option<i64>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub last_heartbeat_at: Option<String>,
    pub lease_expires_at: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PendingBranchCiLaneJob {
    pub lane_run_id: i64,
    pub claim_token: i64,
    pub suite_id: i64,
    pub branch_id: i64,
    pub source_head_sha: String,
    pub lane_id: String,
    pub title: String,
    pub entrypoint: String,
    pub command: Vec<String>,
    pub concurrency_group: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NightlyFeedItem {
    pub nightly_run_id: i64,
    pub repo: String,
    pub source_head_sha: String,
    pub status: String,
    pub summary: Option<String>,
    pub scheduled_for: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct NightlyRunRecord {
    pub nightly_run_id: i64,
    pub repo: String,
    pub source_ref: String,
    pub source_head_sha: String,
    pub status: String,
    pub summary: Option<String>,
    pub scheduled_for: String,
    pub rerun_of_run_id: Option<i64>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub lanes: Vec<NightlyLaneRecord>,
}

#[derive(Debug, Clone)]
pub struct NightlyLaneRecord {
    pub id: i64,
    pub lane_id: String,
    pub title: String,
    pub entrypoint: String,
    pub status: String,
    pub execution_reason: CiLaneExecutionReason,
    pub failure_kind: Option<CiLaneFailureKind>,
    pub pikaci_run_id: Option<String>,
    pub pikaci_target_id: Option<String>,
    pub ci_target_key: Option<String>,
    pub target_health: Option<CiTargetHealthSnapshot>,
    pub log_text: Option<String>,
    pub retry_count: i64,
    pub rerun_of_lane_run_id: Option<i64>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub last_heartbeat_at: Option<String>,
    pub lease_expires_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingNightlyLaneJob {
    pub lane_run_id: i64,
    pub claim_token: i64,
    pub nightly_run_id: i64,
    pub source_head_sha: String,
    pub lane_id: String,
    pub command: Vec<String>,
    pub concurrency_group: Option<String>,
}

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

#[derive(Debug, Clone)]
struct BranchLaneRerunSource {
    original_run_id: i64,
    branch_id: i64,
    source_head_sha: String,
    lane_id: String,
    title: String,
    entrypoint: String,
    command_json: String,
    concurrency_group: Option<String>,
    ci_target_key: Option<String>,
    status: String,
}

#[derive(Debug, Clone)]
struct BranchLaneRecoverySource {
    run_id: i64,
    lane_id: String,
    status: String,
    claim_token: i64,
    log_text: Option<String>,
}

#[derive(Debug, Clone)]
struct NightlyLaneRerunSource {
    original_run_id: i64,
    repo_id: i64,
    source_ref: String,
    source_head_sha: String,
    lane_id: String,
    title: String,
    entrypoint: String,
    command_json: String,
    concurrency_group: Option<String>,
    ci_target_key: Option<String>,
    status: String,
}

#[derive(Debug, Clone)]
struct NightlyLaneRecoverySource {
    nightly_run_id: i64,
    lane_id: String,
    status: String,
    claim_token: i64,
    log_text: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BranchActionTarget {
    pub branch_id: i64,
    pub repo: String,
    pub branch_name: String,
    pub title: String,
    pub branch_state: String,
    pub target_branch: String,
    pub head_sha: String,
    pub merge_base_sha: String,
}

impl Store {
    pub fn ensure_forge_repo_metadata(
        &self,
        repo: &str,
        canonical_git_dir: &str,
        default_branch: &str,
        ci_entrypoint: &str,
    ) -> anyhow::Result<i64> {
        self.with_connection(|conn| {
            ensure_repo_metadata(conn, repo, canonical_git_dir, default_branch, ci_entrypoint)
        })
    }

    pub fn upsert_branch_record(
        &self,
        input: &BranchUpsertInput,
    ) -> anyhow::Result<BranchUpsertOutcome> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start branch upsert transaction")?;

            let repo_id = ensure_repo_metadata(
                &tx,
                &input.repo,
                &input.canonical_git_dir,
                &input.default_branch,
                &input.ci_entrypoint,
            )?;
            let existing: Option<(i64, String)> = tx
                .query_row(
                    "SELECT id, head_sha
                     FROM branch_records
                     WHERE repo_id = ?1 AND branch_name = ?2 AND state = 'open'
                     ORDER BY id DESC
                     LIMIT 1",
                    params![repo_id, input.branch_name],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .context("lookup existing open branch record")?;

            let (branch_id, inserted, previous_head_sha) = match existing {
                Some((branch_id, previous_head_sha)) => {
                    tx.execute(
                        "UPDATE branch_records
                         SET title = ?1,
                             target_branch = ?2,
                             head_sha = ?3,
                             merge_base_sha = ?4,
                             author_name = ?5,
                             author_email = ?6,
                             updated_at = ?7,
                             last_seen_at = CURRENT_TIMESTAMP,
                             deleted_at = NULL
                         WHERE id = ?8",
                        params![
                            input.title,
                            input.default_branch,
                            input.head_sha,
                            input.merge_base_sha,
                            input.author_name,
                            input.author_email,
                            input.updated_at,
                            branch_id,
                        ],
                    )
                    .context("update existing branch record")?;
                    (branch_id, false, Some(previous_head_sha))
                }
                None => {
                    tx.execute(
                        "INSERT INTO branch_records(
                            repo_id,
                            branch_name,
                            target_branch,
                            title,
                            state,
                            head_sha,
                            merge_base_sha,
                            author_name,
                            author_email,
                            updated_at,
                            opened_at,
                            last_seen_at
                         ) VALUES (?1, ?2, ?3, ?4, 'open', ?5, ?6, ?7, ?8, ?9, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
                        params![
                            repo_id,
                            input.branch_name,
                            input.default_branch,
                            input.title,
                            input.head_sha,
                            input.merge_base_sha,
                            input.author_name,
                            input.author_email,
                            input.updated_at,
                        ],
                    )
                    .context("insert branch record")?;
                    (tx.last_insert_rowid(), true, None)
                }
            };

            let head_changed = previous_head_sha
                .as_deref()
                .map(|previous| previous != input.head_sha)
                .unwrap_or(true);
            let queued_generation = ensure_branch_artifact_for_head(
                &tx,
                branch_id,
                &input.head_sha,
                &input.merge_base_sha,
            )?;

            tx.commit().context("commit branch upsert transaction")?;
            Ok(BranchUpsertOutcome {
                branch_id,
                inserted,
                head_changed,
                queued_generation,
                queued_ci: false,
            })
        })
    }

    pub fn close_missing_open_branches(
        &self,
        repo: &str,
        present_branch_names: &[String],
    ) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let repo_id: Option<i64> = conn
                .query_row(
                    "SELECT id FROM repos WHERE repo = ?1",
                    params![repo],
                    |row| row.get(0),
                )
                .optional()
                .context("lookup repo for stale branch close")?;
            let Some(repo_id) = repo_id else {
                return Ok(0);
            };

            let mut sql = String::from(
                "UPDATE branch_records
                 SET state = 'closed',
                     closed_at = COALESCE(closed_at, CURRENT_TIMESTAMP),
                     deleted_at = COALESCE(deleted_at, CURRENT_TIMESTAMP),
                     updated_at = CURRENT_TIMESTAMP
                 WHERE repo_id = ?1
                   AND state = 'open'",
            );
            if !present_branch_names.is_empty() {
                let placeholders = (0..present_branch_names.len())
                    .map(|idx| format!("?{}", idx + 2))
                    .collect::<Vec<_>>()
                    .join(", ");
                sql.push_str(&format!(" AND branch_name NOT IN ({placeholders})"));
            }
            let mut stmt = conn
                .prepare(&sql)
                .context("prepare close stale branch query")?;
            let mut params_dyn: Vec<&dyn rusqlite::ToSql> =
                Vec::with_capacity(present_branch_names.len() + 1);
            params_dyn.push(&repo_id);
            for name in present_branch_names {
                params_dyn.push(name);
            }
            let changed = stmt
                .execute(params_dyn.as_slice())
                .context("close stale branch records")?;
            Ok(changed)
        })
    }

    pub fn list_branch_feed_items(&self) -> anyhow::Result<Vec<BranchFeedItem>> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT br.id,
                            r.repo,
                            br.branch_name,
                            br.title,
                            br.state,
                            br.updated_at,
                            br.head_sha,
                            COALESCE(ba.status, 'pending'),
                            COALESCE(ci.status, 'queued')
                     FROM branch_records br
                     JOIN repos r ON r.id = br.repo_id
                     LEFT JOIN branch_artifact_versions ba ON ba.id = (
                        SELECT bav.id
                        FROM branch_artifact_versions bav
                        WHERE bav.branch_id = br.id AND bav.status != 'superseded'
                        ORDER BY bav.version DESC
                        LIMIT 1
                     )
                     LEFT JOIN branch_ci_runs ci ON ci.id = (
                        SELECT bcr.id
                        FROM branch_ci_runs bcr
                        WHERE bcr.branch_id = br.id
                        ORDER BY bcr.id DESC
                        LIMIT 1
                     )
                     WHERE br.state IN ('open', 'merged', 'closed')
                     ORDER BY CASE br.state WHEN 'open' THEN 0 ELSE 1 END, br.updated_at DESC, br.id DESC",
                )
                .context("prepare branch feed query")?;

            let rows = stmt
                .query_map([], |row| {
                    Ok(BranchFeedItem {
                        branch_id: row.get(0)?,
                        repo: row.get(1)?,
                        branch_name: row.get(2)?,
                        title: row.get(3)?,
                        state: row.get(4)?,
                        updated_at: row.get(5)?,
                        head_sha: row.get(6)?,
                        tutorial_status: row.get(7)?,
                        ci_status: row.get(8)?,
                    })
                })
                .context("query branch feed items")?;

            let mut items = Vec::new();
            for row in rows {
                items.push(row.context("read branch feed row")?);
            }
            Ok(items)
        })
    }

    pub fn get_branch_detail(&self, branch_id: i64) -> anyhow::Result<Option<BranchDetailRecord>> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT br.id,
                        ba_current.id,
                        r.repo,
                        br.branch_name,
                        br.title,
                        br.state,
                        br.updated_at,
                        br.target_branch,
                        br.head_sha,
                        br.merge_base_sha,
                        br.merge_commit_sha,
                        COALESCE(ba_latest.status, 'pending'),
                        ba_current.tutorial_json,
                        ba_current.unified_diff,
                        ba_current.claude_session_id,
                        ba_latest.error_message,
                        COALESCE(ci.status, 'queued')
                 FROM branch_records br
                 JOIN repos r ON r.id = br.repo_id
                 LEFT JOIN branch_artifact_versions ba_latest ON ba_latest.id = (
                    SELECT bav.id
                    FROM branch_artifact_versions bav
                    WHERE bav.branch_id = br.id AND bav.status != 'superseded'
                    ORDER BY bav.version DESC
                    LIMIT 1
                 )
                 LEFT JOIN branch_artifact_versions ba_current ON ba_current.id = (
                    SELECT bav.id
                    FROM branch_artifact_versions bav
                    WHERE bav.branch_id = br.id AND bav.is_current = 1 AND bav.status = 'ready'
                    ORDER BY bav.version DESC
                    LIMIT 1
                 )
                 LEFT JOIN branch_ci_runs ci ON ci.id = (
                    SELECT bcr.id
                    FROM branch_ci_runs bcr
                    WHERE bcr.branch_id = br.id
                    ORDER BY bcr.id DESC
                    LIMIT 1
                 )
                 WHERE br.id = ?1",
                params![branch_id],
                |row| {
                    Ok(BranchDetailRecord {
                        branch_id: row.get(0)?,
                        current_artifact_id: row.get(1)?,
                        repo: row.get(2)?,
                        branch_name: row.get(3)?,
                        title: row.get(4)?,
                        branch_state: row.get(5)?,
                        updated_at: row.get(6)?,
                        target_branch: row.get(7)?,
                        head_sha: row.get(8)?,
                        merge_base_sha: row.get(9)?,
                        merge_commit_sha: row.get(10)?,
                        tutorial_status: row.get(11)?,
                        tutorial_json: row.get(12)?,
                        unified_diff: row.get(13)?,
                        claude_session_id: row.get(14)?,
                        error_message: row.get(15)?,
                        ci_status: row.get(16)?,
                    })
                },
            )
            .optional()
            .context("query branch detail")
        })
    }

    pub fn find_branch_by_name(
        &self,
        repo: &str,
        branch_name: &str,
    ) -> anyhow::Result<Option<BranchLookupRecord>> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT br.id, r.repo, br.branch_name, br.state
                 FROM branch_records br
                 JOIN repos r ON r.id = br.repo_id
                 WHERE r.repo = ?1
                   AND br.branch_name = ?2
                 ORDER BY br.id DESC
                 LIMIT 1",
                params![repo, branch_name],
                |row| {
                    Ok(BranchLookupRecord {
                        branch_id: row.get(0)?,
                        repo: row.get(1)?,
                        branch_name: row.get(2)?,
                        branch_state: row.get(3)?,
                    })
                },
            )
            .optional()
            .context("query branch by name")
        })
    }

    pub fn list_branch_ci_runs(
        &self,
        branch_id: i64,
        limit: usize,
    ) -> anyhow::Result<Vec<BranchCiRunRecord>> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, source_head_sha, status, rerun_of_run_id, created_at, started_at, finished_at
                     FROM branch_ci_runs
                     WHERE branch_id = ?1
                     ORDER BY id DESC
                     LIMIT ?2",
                )
                .context("prepare branch ci runs query")?;
            let rows = stmt
                .query_map(params![branch_id, limit as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                })
                .context("query branch ci runs")?;

            let mut runs = Vec::new();
            for row in rows {
                let (
                    run_id,
                    source_head_sha,
                    status,
                    rerun_of_run_id,
                    created_at,
                    started_at,
                    finished_at,
                ) = row.context("read branch ci run row")?;
                let lanes = list_branch_ci_run_lanes(conn, run_id)?;
                let lane_count = lanes.len();
                runs.push(BranchCiRunRecord {
                    id: run_id,
                    source_head_sha,
                    status,
                    lane_count,
                    rerun_of_run_id,
                    created_at,
                    started_at,
                    finished_at,
                    lanes,
                });
            }
            Ok(runs)
        })
    }

    pub fn queue_branch_ci_run_for_head(
        &self,
        branch_id: i64,
        head_sha: &str,
        lanes: &[ForgeLane],
    ) -> anyhow::Result<bool> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start queue branch ci suite transaction")?;
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id
                     FROM branch_ci_runs
                     WHERE branch_id = ?1 AND source_head_sha = ?2
                     ORDER BY id DESC
                     LIMIT 1",
                    params![branch_id, head_sha],
                    |row| row.get(0),
                )
                .optional()
                .context("lookup existing branch ci suite for head")?;
            if existing.is_some() {
                tx.commit().context("commit existing branch ci suite lookup")?;
                return Ok(false);
            }

            let suite_status = if lanes.is_empty() { "success" } else { "queued" };
            let suite_log = if lanes.is_empty() {
                Some("No lanes selected for this branch head.".to_string())
            } else {
                None
            };
            let selected_lane_ids = serde_json::to_string(
                &lanes.iter().map(|lane| lane.id.clone()).collect::<Vec<_>>(),
            )
            .context("serialize selected branch lane ids")?;
            tx.execute(
                "INSERT INTO branch_ci_runs(
                    branch_id,
                    source_head_sha,
                    entrypoint,
                    command_json,
                    status,
                    log_text,
                    finished_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, CASE WHEN ?5 = 'success' THEN CURRENT_TIMESTAMP ELSE NULL END)",
                params![
                    branch_id,
                    head_sha,
                    "ci/forge-lanes.toml",
                    selected_lane_ids,
                    suite_status,
                    suite_log,
                ],
            )
            .context("insert branch ci suite")?;
            let suite_id = tx.last_insert_rowid();

            for lane in lanes {
                let command_json =
                    serde_json::to_string(&lane.command).context("serialize branch lane command")?;
                let ci_target_key = configured_target_key_for_lane(lane);
                tx.execute(
                    "INSERT INTO branch_ci_run_lanes(
                        branch_ci_run_id,
                        lane_id,
                        title,
                        entrypoint,
                        command_json,
                        concurrency_group,
                        ci_target_key,
                        execution_reason,
                        status
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', 'queued')",
                    params![
                        suite_id,
                        lane.id,
                        lane.title,
                        lane.entrypoint,
                        command_json,
                        lane.concurrency_group,
                        ci_target_key,
                    ],
                )
                .with_context(|| format!("insert branch ci lane for suite {}", suite_id))?;
            }

            tx.commit().context("commit queue branch ci suite transaction")?;
            Ok(true)
        })
    }

    pub fn queue_branch_ci_failure_for_head(
        &self,
        branch_id: i64,
        head_sha: &str,
        log_text: &str,
    ) -> anyhow::Result<bool> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start queue failed branch ci suite transaction")?;
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id
                     FROM branch_ci_runs
                     WHERE branch_id = ?1 AND source_head_sha = ?2
                     ORDER BY id DESC
                     LIMIT 1",
                    params![branch_id, head_sha],
                    |row| row.get(0),
                )
                .optional()
                .context("lookup existing failed branch ci suite for head")?;
            if existing.is_some() {
                tx.commit()
                    .context("commit existing failed branch ci suite lookup")?;
                return Ok(false);
            }

            tx.execute(
                "INSERT INTO branch_ci_runs(
                    branch_id,
                    source_head_sha,
                    entrypoint,
                    command_json,
                    status,
                    log_text,
                    finished_at
                 ) VALUES (?1, ?2, ?3, ?4, 'failed', ?5, CURRENT_TIMESTAMP)",
                params![branch_id, head_sha, "ci/forge-lanes.toml", "[]", log_text],
            )
            .context("insert failed branch ci suite")?;

            tx.commit()
                .context("commit queue failed branch ci suite transaction")?;
            Ok(true)
        })
    }

    pub fn list_recent_nightly_runs(&self, limit: usize) -> anyhow::Result<Vec<NightlyFeedItem>> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT nr.id, r.repo, nr.source_head_sha, nr.status, nr.summary, nr.scheduled_for, nr.created_at
                     FROM nightly_runs nr
                     JOIN repos r ON r.id = nr.repo_id
                     ORDER BY nr.scheduled_for DESC, nr.id DESC
                     LIMIT ?1",
                )
                .context("prepare nightly feed query")?;
            let rows = stmt
                .query_map(params![limit as i64], |row| {
                    Ok(NightlyFeedItem {
                        nightly_run_id: row.get(0)?,
                        repo: row.get(1)?,
                        source_head_sha: row.get(2)?,
                        status: row.get(3)?,
                        summary: row.get(4)?,
                        scheduled_for: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                })
                .context("query nightly feed items")?;
            let mut items = Vec::new();
            for row in rows {
                items.push(row.context("read nightly feed row")?);
            }
            Ok(items)
        })
    }

    pub fn get_nightly_run(&self, nightly_run_id: i64) -> anyhow::Result<Option<NightlyRunRecord>> {
        self.with_connection(|conn| {
            let row = conn
                .query_row(
                    "SELECT nr.id, r.repo, nr.source_ref, nr.source_head_sha, nr.status, nr.summary, nr.scheduled_for, nr.rerun_of_run_id, nr.created_at, nr.started_at, nr.finished_at
                     FROM nightly_runs nr
                     JOIN repos r ON r.id = nr.repo_id
                     WHERE nr.id = ?1",
                    params![nightly_run_id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, Option<i64>>(7)?,
                            row.get::<_, String>(8)?,
                            row.get::<_, Option<String>>(9)?,
                            row.get::<_, Option<String>>(10)?,
                        ))
                    },
                )
                .optional()
                .context("query nightly run detail")?;
            let Some((id, repo, source_ref, source_head_sha, status, summary, scheduled_for, rerun_of_run_id, created_at, started_at, finished_at)) = row else {
                return Ok(None);
            };
            let lanes = list_nightly_run_lanes(conn, id)?;
            Ok(Some(NightlyRunRecord {
                nightly_run_id: id,
                repo,
                source_ref,
                source_head_sha,
                status,
                summary,
                scheduled_for,
                rerun_of_run_id,
                created_at,
                started_at,
                finished_at,
                lanes,
            }))
        })
    }

    pub fn queue_nightly_run(
        &self,
        repo_id: i64,
        source_ref: &str,
        source_head_sha: &str,
        scheduled_for: &str,
        lanes: &[ForgeLane],
    ) -> anyhow::Result<bool> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start queue nightly transaction")?;
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM nightly_runs WHERE repo_id = ?1 AND scheduled_for = ?2",
                    params![repo_id, scheduled_for],
                    |row| row.get(0),
                )
                .optional()
                .context("lookup existing nightly run")?;
            if existing.is_some() {
                tx.commit().context("commit existing nightly lookup")?;
                return Ok(false);
            }
            let status = if lanes.is_empty() { "success" } else { "queued" };
            let summary = if lanes.is_empty() {
                Some("No nightly lanes configured.".to_string())
            } else {
                None
            };
            tx.execute(
                "INSERT INTO nightly_runs(repo_id, source_ref, source_head_sha, status, summary, scheduled_for, finished_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, CASE WHEN ?4 = 'success' THEN CURRENT_TIMESTAMP ELSE NULL END)",
                params![repo_id, source_ref, source_head_sha, status, summary, scheduled_for],
            )
            .context("insert nightly run")?;
            let nightly_run_id = tx.last_insert_rowid();
            for lane in lanes {
                let command_json =
                    serde_json::to_string(&lane.command).context("serialize nightly lane command")?;
                let ci_target_key = configured_target_key_for_lane(lane);
                tx.execute(
                    "INSERT INTO nightly_run_lanes(
                        nightly_run_id,
                        lane_id,
                        title,
                        entrypoint,
                        command_json,
                        concurrency_group,
                        ci_target_key,
                        execution_reason,
                        status
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', 'queued')",
                    params![
                        nightly_run_id,
                        lane.id,
                        lane.title,
                        lane.entrypoint,
                        command_json,
                        lane.concurrency_group,
                        ci_target_key,
                    ],
                )
                .with_context(|| format!("insert nightly lane for run {}", nightly_run_id))?;
            }
            tx.commit().context("commit queue nightly transaction")?;
            Ok(true)
        })
    }

    pub fn rerun_branch_ci_lane(
        &self,
        branch_id: i64,
        lane_run_id: i64,
    ) -> anyhow::Result<Option<i64>> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start rerun branch ci lane transaction")?;
            let row: Option<BranchLaneRerunSource> = tx
                .query_row(
                    "SELECT lane.branch_ci_run_id,
                            suite.branch_id,
                            suite.source_head_sha,
                            lane.lane_id,
                            lane.title,
                            lane.entrypoint,
                            lane.command_json,
                            lane.concurrency_group,
                            lane.ci_target_key,
                            lane.status
                     FROM branch_ci_run_lanes lane
                     JOIN branch_ci_runs suite ON suite.id = lane.branch_ci_run_id
                     WHERE lane.id = ?1 AND suite.branch_id = ?2",
                    params![lane_run_id, branch_id],
                    |row| {
                        Ok(BranchLaneRerunSource {
                            original_run_id: row.get(0)?,
                            branch_id: row.get(1)?,
                            source_head_sha: row.get(2)?,
                            lane_id: row.get(3)?,
                            title: row.get(4)?,
                            entrypoint: row.get(5)?,
                            command_json: row.get(6)?,
                            concurrency_group: row.get(7)?,
                            ci_target_key: row.get(8)?,
                            status: row.get(9)?,
                        })
                    },
                )
                .optional()
                .context("lookup branch ci lane for rerun")?;
            let Some(row) = row else {
                return Ok(None);
            };
            if matches!(row.status.as_str(), "queued" | "running") {
                bail!("lane {} is still {}", row.lane_id, row.status);
            }

            tx.execute(
                "INSERT INTO branch_ci_runs(
                    branch_id,
                    source_head_sha,
                    entrypoint,
                    command_json,
                    status,
                    log_text,
                    rerun_of_run_id
                 ) VALUES (?1, ?2, ?3, ?4, 'queued', ?5, ?6)",
                params![
                    row.branch_id,
                    row.source_head_sha,
                    "manual-rerun",
                    serde_json::to_string(&vec![row.lane_id.clone()])
                        .context("serialize rerun branch lane id")?,
                    format!(
                        "Manual rerun of lane {} from branch CI run #{}",
                        row.lane_id, row.original_run_id
                    ),
                    row.original_run_id,
                ],
            )
            .context("insert rerun branch ci suite")?;
            let rerun_suite_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO branch_ci_run_lanes(
                    branch_ci_run_id,
                    lane_id,
                    title,
                    entrypoint,
                    command_json,
                    concurrency_group,
                    ci_target_key,
                    execution_reason,
                    status,
                    rerun_of_lane_run_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', 'queued', ?8)",
                params![
                    rerun_suite_id,
                    row.lane_id,
                    row.title,
                    row.entrypoint,
                    row.command_json,
                    row.concurrency_group,
                    row.ci_target_key,
                    lane_run_id
                ],
            )
            .context("insert rerun branch ci lane")?;
            tx.commit()
                .context("commit rerun branch ci lane transaction")?;
            Ok(Some(rerun_suite_id))
        })
    }

    pub fn rerun_nightly_lane(
        &self,
        nightly_run_id: i64,
        lane_run_id: i64,
    ) -> anyhow::Result<Option<i64>> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start rerun nightly lane transaction")?;
            let row: Option<NightlyLaneRerunSource> = tx
                .query_row(
                    "SELECT lane.nightly_run_id,
                            nightly.repo_id,
                            nightly.source_ref,
                            nightly.source_head_sha,
                            lane.lane_id,
                            lane.title,
                            lane.entrypoint,
                            lane.command_json,
                            lane.concurrency_group,
                            lane.ci_target_key,
                            lane.status
                     FROM nightly_run_lanes lane
                     JOIN nightly_runs nightly ON nightly.id = lane.nightly_run_id
                     WHERE lane.id = ?1 AND nightly.id = ?2",
                    params![lane_run_id, nightly_run_id],
                    |row| {
                        Ok(NightlyLaneRerunSource {
                            original_run_id: row.get(0)?,
                            repo_id: row.get(1)?,
                            source_ref: row.get(2)?,
                            source_head_sha: row.get(3)?,
                            lane_id: row.get(4)?,
                            title: row.get(5)?,
                            entrypoint: row.get(6)?,
                            command_json: row.get(7)?,
                            concurrency_group: row.get(8)?,
                            ci_target_key: row.get(9)?,
                            status: row.get(10)?,
                        })
                    },
                )
                .optional()
                .context("lookup nightly lane for rerun")?;
            let Some(row) = row else {
                return Ok(None);
            };
            if matches!(row.status.as_str(), "queued" | "running") {
                bail!("lane {} is still {}", row.lane_id, row.status);
            }
            let scheduled_for = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            tx.execute(
                "INSERT INTO nightly_runs(
                    repo_id,
                    source_ref,
                    source_head_sha,
                    status,
                    summary,
                    scheduled_for,
                    rerun_of_run_id
                 ) VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6)",
                params![
                    row.repo_id,
                    row.source_ref,
                    row.source_head_sha,
                    format!(
                        "Manual rerun of nightly #{} lane {}",
                        row.original_run_id, row.lane_id
                    ),
                    scheduled_for,
                    row.original_run_id,
                ],
            )
            .context("insert rerun nightly run")?;
            let rerun_run_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO nightly_run_lanes(
                    nightly_run_id,
                    lane_id,
                    title,
                    entrypoint,
                    command_json,
                    concurrency_group,
                    ci_target_key,
                    execution_reason,
                    status,
                    rerun_of_lane_run_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', 'queued', ?8)",
                params![
                    rerun_run_id,
                    row.lane_id,
                    row.title,
                    row.entrypoint,
                    row.command_json,
                    row.concurrency_group,
                    row.ci_target_key,
                    lane_run_id
                ],
            )
            .context("insert rerun nightly lane")?;
            tx.commit()
                .context("commit rerun nightly lane transaction")?;
            Ok(Some(rerun_run_id))
        })
    }

    pub fn fail_branch_ci_lane(
        &self,
        branch_id: i64,
        lane_run_id: i64,
        actor_npub: &str,
    ) -> anyhow::Result<Option<()>> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start fail branch ci lane transaction")?;
            let row: Option<BranchLaneRecoverySource> = tx
                .query_row(
                    "SELECT lane.branch_ci_run_id,
                            lane.lane_id,
                            lane.status,
                            lane.claim_token,
                            lane.log_text,
                            lane.execution_reason
                     FROM branch_ci_run_lanes lane
                     JOIN branch_ci_runs suite ON suite.id = lane.branch_ci_run_id
                     WHERE lane.id = ?1 AND suite.branch_id = ?2",
                    params![lane_run_id, branch_id],
                    |row| {
                        Ok(BranchLaneRecoverySource {
                            run_id: row.get(0)?,
                            lane_id: row.get(1)?,
                            status: row.get(2)?,
                            claim_token: row.get(3)?,
                            log_text: row.get(4)?,
                        })
                    },
                )
                .optional()
                .context("lookup branch ci lane for manual fail")?;
            let Some(row) = row else {
                return Ok(None);
            };
            if !matches!(row.status.as_str(), "queued" | "running") {
                bail!("lane {} is already {}", row.lane_id, row.status);
            }
            let manual_note = manual_lane_failure_note(actor_npub, &row.lane_id);
            let next_log = append_log_note(row.log_text, &manual_note);
            tx.execute(
                "UPDATE branch_ci_run_lanes
                 SET status = 'failed',
                     failure_kind = NULL,
                     log_text = ?1,
                     finished_at = CURRENT_TIMESTAMP,
                     lease_expires_at = NULL,
                     claim_token = ?2
                 WHERE id = ?3",
                params![next_log, row.claim_token + 1, lane_run_id],
            )
            .with_context(|| format!("mark branch ci lane {} failed", lane_run_id))?;
            update_branch_ci_suite_status(&tx, row.run_id)?;
            tx.commit()
                .context("commit fail branch ci lane transaction")?;
            Ok(Some(()))
        })
    }

    pub fn requeue_branch_ci_lane(
        &self,
        branch_id: i64,
        lane_run_id: i64,
    ) -> anyhow::Result<Option<()>> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start requeue branch ci lane transaction")?;
            let row: Option<BranchLaneRecoverySource> = tx
                .query_row(
                    "SELECT lane.branch_ci_run_id,
                            lane.lane_id,
                            lane.status,
                            lane.claim_token,
                            lane.log_text,
                            lane.execution_reason
                     FROM branch_ci_run_lanes lane
                     JOIN branch_ci_runs suite ON suite.id = lane.branch_ci_run_id
                     WHERE lane.id = ?1 AND suite.branch_id = ?2",
                    params![lane_run_id, branch_id],
                    |row| {
                        Ok(BranchLaneRecoverySource {
                            run_id: row.get(0)?,
                            lane_id: row.get(1)?,
                            status: row.get(2)?,
                            claim_token: row.get(3)?,
                            log_text: row.get(4)?,
                        })
                    },
                )
                .optional()
                .context("lookup branch ci lane for manual requeue")?;
            let Some(row) = row else {
                return Ok(None);
            };
            if !matches!(row.status.as_str(), "queued" | "running" | "failed") {
                bail!(
                    "lane {} cannot be requeued from {}",
                    row.lane_id,
                    row.status
                );
            }
            tx.execute(
                "UPDATE branch_ci_run_lanes
                 SET status = 'queued',
                     execution_reason = 'queued',
                     failure_kind = NULL,
                     log_text = NULL,
                     pikaci_run_id = NULL,
                     pikaci_target_id = NULL,
                     retry_count = retry_count + 1,
                     created_at = CURRENT_TIMESTAMP,
                     started_at = NULL,
                     finished_at = NULL,
                     last_heartbeat_at = NULL,
                     lease_expires_at = NULL,
                     claim_token = ?1
                 WHERE id = ?2",
                params![row.claim_token + 1, lane_run_id],
            )
            .with_context(|| format!("requeue branch ci lane {}", lane_run_id))?;
            update_branch_ci_suite_status(&tx, row.run_id)?;
            tx.commit()
                .context("commit requeue branch ci lane transaction")?;
            Ok(Some(()))
        })
    }

    pub fn recover_branch_ci_run(
        &self,
        branch_id: i64,
        run_id: i64,
    ) -> anyhow::Result<Option<usize>> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start recover branch ci run transaction")?;
            let exists: Option<i64> = tx
                .query_row(
                    "SELECT id FROM branch_ci_runs WHERE id = ?1 AND branch_id = ?2",
                    params![run_id, branch_id],
                    |row| row.get(0),
                )
                .optional()
                .context("lookup branch ci run for recovery")?;
            let Some(_) = exists else {
                return Ok(None);
            };
            let recovered = tx
                .execute(
                    "UPDATE branch_ci_run_lanes
                     SET status = 'queued',
                         execution_reason = 'queued',
                         failure_kind = NULL,
                         log_text = NULL,
                         pikaci_run_id = NULL,
                         pikaci_target_id = NULL,
                         retry_count = retry_count + 1,
                         created_at = CURRENT_TIMESTAMP,
                         started_at = NULL,
                         finished_at = NULL,
                         last_heartbeat_at = NULL,
                         lease_expires_at = NULL,
                         claim_token = claim_token + 1
                     WHERE branch_ci_run_id = ?1
                       AND status IN ('queued', 'running', 'failed')",
                    params![run_id],
                )
                .with_context(|| format!("recover branch ci run {}", run_id))?;
            update_branch_ci_suite_status(&tx, run_id)?;
            tx.commit()
                .context("commit recover branch ci run transaction")?;
            Ok(Some(recovered))
        })
    }

    pub fn fail_nightly_lane(
        &self,
        nightly_run_id: i64,
        lane_run_id: i64,
        actor_npub: &str,
    ) -> anyhow::Result<Option<()>> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start fail nightly lane transaction")?;
            let row: Option<NightlyLaneRecoverySource> = tx
                .query_row(
                    "SELECT lane.nightly_run_id,
                            lane.lane_id,
                            lane.status,
                            lane.claim_token,
                            lane.log_text,
                            lane.execution_reason
                     FROM nightly_run_lanes lane
                     JOIN nightly_runs nightly ON nightly.id = lane.nightly_run_id
                     WHERE lane.id = ?1 AND nightly.id = ?2",
                    params![lane_run_id, nightly_run_id],
                    |row| {
                        Ok(NightlyLaneRecoverySource {
                            nightly_run_id: row.get(0)?,
                            lane_id: row.get(1)?,
                            status: row.get(2)?,
                            claim_token: row.get(3)?,
                            log_text: row.get(4)?,
                        })
                    },
                )
                .optional()
                .context("lookup nightly lane for manual fail")?;
            let Some(row) = row else {
                return Ok(None);
            };
            if !matches!(row.status.as_str(), "queued" | "running") {
                bail!("lane {} is already {}", row.lane_id, row.status);
            }
            let manual_note = manual_lane_failure_note(actor_npub, &row.lane_id);
            let next_log = append_log_note(row.log_text, &manual_note);
            tx.execute(
                "UPDATE nightly_run_lanes
                 SET status = 'failed',
                     failure_kind = NULL,
                     log_text = ?1,
                     finished_at = CURRENT_TIMESTAMP,
                     lease_expires_at = NULL,
                     claim_token = ?2
                 WHERE id = ?3",
                params![next_log, row.claim_token + 1, lane_run_id],
            )
            .with_context(|| format!("mark nightly lane {} failed", lane_run_id))?;
            update_nightly_run_status(&tx, row.nightly_run_id)?;
            tx.commit()
                .context("commit fail nightly lane transaction")?;
            Ok(Some(()))
        })
    }

    pub fn requeue_nightly_lane(
        &self,
        nightly_run_id: i64,
        lane_run_id: i64,
    ) -> anyhow::Result<Option<()>> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start requeue nightly lane transaction")?;
            let row: Option<NightlyLaneRecoverySource> = tx
                .query_row(
                    "SELECT lane.nightly_run_id,
                            lane.lane_id,
                            lane.status,
                            lane.claim_token,
                            lane.log_text,
                            lane.execution_reason
                     FROM nightly_run_lanes lane
                     JOIN nightly_runs nightly ON nightly.id = lane.nightly_run_id
                     WHERE lane.id = ?1 AND nightly.id = ?2",
                    params![lane_run_id, nightly_run_id],
                    |row| {
                        Ok(NightlyLaneRecoverySource {
                            nightly_run_id: row.get(0)?,
                            lane_id: row.get(1)?,
                            status: row.get(2)?,
                            claim_token: row.get(3)?,
                            log_text: row.get(4)?,
                        })
                    },
                )
                .optional()
                .context("lookup nightly lane for manual requeue")?;
            let Some(row) = row else {
                return Ok(None);
            };
            if !matches!(row.status.as_str(), "queued" | "running" | "failed") {
                bail!(
                    "lane {} cannot be requeued from {}",
                    row.lane_id,
                    row.status
                );
            }
            tx.execute(
                "UPDATE nightly_run_lanes
                 SET status = 'queued',
                     execution_reason = 'queued',
                     failure_kind = NULL,
                     log_text = NULL,
                     pikaci_run_id = NULL,
                     pikaci_target_id = NULL,
                     retry_count = retry_count + 1,
                     created_at = CURRENT_TIMESTAMP,
                     started_at = NULL,
                     finished_at = NULL,
                     last_heartbeat_at = NULL,
                     lease_expires_at = NULL,
                     claim_token = ?1
                 WHERE id = ?2",
                params![row.claim_token + 1, lane_run_id],
            )
            .with_context(|| format!("requeue nightly lane {}", lane_run_id))?;
            update_nightly_run_status(&tx, row.nightly_run_id)?;
            tx.commit()
                .context("commit requeue nightly lane transaction")?;
            Ok(Some(()))
        })
    }

    pub fn recover_nightly_run(&self, nightly_run_id: i64) -> anyhow::Result<Option<usize>> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start recover nightly run transaction")?;
            let exists: Option<i64> = tx
                .query_row(
                    "SELECT id FROM nightly_runs WHERE id = ?1",
                    params![nightly_run_id],
                    |row| row.get(0),
                )
                .optional()
                .context("lookup nightly run for recovery")?;
            let Some(_) = exists else {
                return Ok(None);
            };
            let recovered = tx
                .execute(
                    "UPDATE nightly_run_lanes
                     SET status = 'queued',
                         execution_reason = 'queued',
                         failure_kind = NULL,
                         log_text = NULL,
                         pikaci_run_id = NULL,
                         pikaci_target_id = NULL,
                         retry_count = retry_count + 1,
                         created_at = CURRENT_TIMESTAMP,
                         started_at = NULL,
                         finished_at = NULL,
                         last_heartbeat_at = NULL,
                         lease_expires_at = NULL,
                         claim_token = claim_token + 1
                     WHERE nightly_run_id = ?1
                       AND status IN ('queued', 'running', 'failed')",
                    params![nightly_run_id],
                )
                .with_context(|| format!("recover nightly run {}", nightly_run_id))?;
            update_nightly_run_status(&tx, nightly_run_id)?;
            tx.commit()
                .context("commit recover nightly run transaction")?;
            Ok(Some(recovered))
        })
    }

    pub fn record_mirror_sync_run(&self, input: &MirrorSyncRunInput) -> anyhow::Result<i64> {
        self.with_connection(|conn| {
            let repo_id = ensure_repo_metadata(
                conn,
                &input.repo,
                &input.canonical_git_dir,
                &input.default_branch,
                "ci/forge-lanes.toml",
            )?;
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
            let current_lagging_ref_count = last_attempt.as_ref().and_then(|run| run.lagging_ref_count);
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

    pub fn get_branch_action_target(
        &self,
        branch_id: i64,
    ) -> anyhow::Result<Option<BranchActionTarget>> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT br.id, r.repo, br.branch_name, br.title, br.state, br.target_branch, br.head_sha, br.merge_base_sha
                 FROM branch_records br
                 JOIN repos r ON r.id = br.repo_id
                 WHERE br.id = ?1",
                params![branch_id],
                |row| {
                    Ok(BranchActionTarget {
                        branch_id: row.get(0)?,
                        repo: row.get(1)?,
                        branch_name: row.get(2)?,
                        title: row.get(3)?,
                        branch_state: row.get(4)?,
                        target_branch: row.get(5)?,
                        head_sha: row.get(6)?,
                        merge_base_sha: row.get(7)?,
                    })
                },
            )
            .optional()
            .context("query branch action target")
        })
    }

    pub fn mark_branch_merged(
        &self,
        branch_id: i64,
        merged_by: &str,
        merge_commit_sha: &str,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start mark branch merged transaction")?;
            let branch: BranchActionTarget = tx
                .query_row(
                    "SELECT br.id, r.repo, br.branch_name, br.title, br.state, br.target_branch, br.head_sha, br.merge_base_sha
                     FROM branch_records br
                     JOIN repos r ON r.id = br.repo_id
                     WHERE br.id = ?1",
                    params![branch_id],
                    |row| {
                        Ok(BranchActionTarget {
                            branch_id: row.get(0)?,
                            repo: row.get(1)?,
                            branch_name: row.get(2)?,
                            title: row.get(3)?,
                            branch_state: row.get(4)?,
                            target_branch: row.get(5)?,
                            head_sha: row.get(6)?,
                            merge_base_sha: row.get(7)?,
                        })
                    },
                )
                .context("lookup branch for merge record")?;
            tx.execute(
                "UPDATE branch_records
                 SET state = 'merged',
                     merge_commit_sha = ?1,
                     merged_by = ?2,
                     merged_at = CURRENT_TIMESTAMP,
                     closed_at = CURRENT_TIMESTAMP,
                     deleted_at = CURRENT_TIMESTAMP,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?3",
                params![merge_commit_sha, merged_by, branch_id],
            )
            .context("update branch merged state")?;
            tx.execute(
                "INSERT INTO merge_records(
                    branch_id,
                    repo,
                    branch_name,
                    target_branch,
                    head_sha,
                    merge_base_sha,
                    merge_commit_sha,
                    merged_by
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    branch.branch_id,
                    branch.repo,
                    branch.branch_name,
                    branch.target_branch,
                    branch.head_sha,
                    branch.merge_base_sha,
                    merge_commit_sha,
                    merged_by,
                ],
            )
            .context("insert merge record")?;
            tx.commit().context("commit merged branch transaction")?;
            Ok(())
        })
    }

    pub fn mark_branch_closed(&self, branch_id: i64, closed_by: &str) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            conn.execute(
                "UPDATE branch_records
                 SET state = 'closed',
                     closed_by = ?1,
                     closed_at = CURRENT_TIMESTAMP,
                     deleted_at = COALESCE(deleted_at, CURRENT_TIMESTAMP),
                     updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?2",
                params![closed_by, branch_id],
            )
            .context("update branch closed state")?;
            Ok(())
        })
    }

    pub fn claim_pending_branch_generation_jobs(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<BranchGenerationJob>> {
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start branch generation claim transaction")?;
            let mut jobs = Vec::new();
            {
                let mut stmt = tx
                    .prepare(
                        "SELECT bav.id, br.id, r.repo, br.branch_name, br.title, bav.source_head_sha, bav.merge_base_sha
                         FROM branch_artifact_versions bav
                         JOIN branch_records br ON br.id = bav.branch_id
                         JOIN repos r ON r.id = br.repo_id
                         WHERE bav.status = 'pending'
                           AND (bav.next_retry_at IS NULL OR bav.next_retry_at <= CURRENT_TIMESTAMP)
                         ORDER BY bav.updated_at ASC
                         LIMIT ?1",
                    )
                    .context("prepare branch generation claim query")?;
                let rows = stmt
                    .query_map(params![limit as i64], |row| {
                        Ok(BranchGenerationJob {
                            artifact_id: row.get(0)?,
                            branch_id: row.get(1)?,
                            repo: row.get(2)?,
                            branch_name: row.get(3)?,
                            title: row.get(4)?,
                            head_sha: row.get(5)?,
                            merge_base_sha: row.get(6)?,
                        })
                    })
                    .context("query pending branch generation jobs")?;
                for row in rows {
                    let job = row.context("read branch generation job row")?;
                    tx.execute(
                        "UPDATE branch_artifact_versions
                         SET status = 'generating', updated_at = CURRENT_TIMESTAMP
                         WHERE id = ?1 AND status = 'pending'",
                        params![job.artifact_id],
                    )
                    .with_context(|| format!("mark branch artifact {} generating", job.artifact_id))?;
                    jobs.push(job);
                }
            }
            tx.commit()
                .context("commit branch generation claim transaction")?;
            Ok(jobs)
        })
    }

    pub fn mark_branch_generation_ready(
        &self,
        artifact_id: i64,
        tutorial_json: &str,
        html: &str,
        generated_head_sha: &str,
        unified_diff: &str,
        claude_session_id: Option<&str>,
    ) -> anyhow::Result<bool> {
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start mark branch generation ready transaction")?;
            let (branch_id, current_status, source_head_sha, version): (i64, String, String, i64) =
                tx.query_row(
                    "SELECT branch_id, status, source_head_sha, version
                     FROM branch_artifact_versions
                     WHERE id = ?1",
                    params![artifact_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .with_context(|| format!("lookup branch artifact {}", artifact_id))?;
            let branch_head_sha: String = tx
                .query_row(
                    "SELECT head_sha
                     FROM branch_records
                     WHERE id = ?1",
                    params![branch_id],
                    |row| row.get(0),
                )
                .with_context(|| format!("lookup branch head for {}", branch_id))?;
            let newer_exists: bool = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1
                        FROM branch_artifact_versions
                        WHERE branch_id = ?1 AND version > ?2 AND status != 'superseded'
                    )",
                    params![branch_id, version],
                    |row| row.get(0),
                )
                .with_context(|| format!("check newer branch artifacts for {}", branch_id))?;
            let activate = current_status != "superseded"
                && source_head_sha == branch_head_sha
                && !newer_exists;
            if activate {
                tx.execute(
                    "UPDATE branch_artifact_versions
                     SET is_current = 0
                     WHERE branch_id = ?1 AND is_current = 1",
                    params![branch_id],
                )
                .with_context(|| format!("clear current branch artifact for {}", branch_id))?;
            }
            tx.execute(
                "UPDATE branch_artifact_versions
                 SET status = 'ready',
                     tutorial_json = ?1,
                     html = ?2,
                     generated_head_sha = ?3,
                     unified_diff = ?4,
                     claude_session_id = ?5,
                     is_current = ?6,
                     ready_at = CURRENT_TIMESTAMP,
                     error_message = NULL,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?7",
                params![
                    tutorial_json,
                    html,
                    generated_head_sha,
                    unified_diff,
                    claude_session_id,
                    if activate { 1 } else { 0 },
                    artifact_id,
                ],
            )
            .with_context(|| format!("mark branch artifact {} ready", artifact_id))?;
            tx.commit()
                .context("commit mark branch generation ready transaction")?;
            Ok(activate)
        })
    }

    pub fn mark_branch_generation_failed(
        &self,
        artifact_id: i64,
        error_message: &str,
        retryable: bool,
        retry_backoff_secs: u64,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let next_retry = if retryable {
                Some(format!("+{} seconds", retry_backoff_secs))
            } else {
                None
            };
            let status = if retryable { "pending" } else { "failed" };
            conn.execute(
                "UPDATE branch_artifact_versions
                 SET status = ?1,
                     error_message = ?2,
                     retry_count = retry_count + 1,
                     next_retry_at = CASE
                        WHEN ?3 IS NULL THEN NULL
                        ELSE datetime('now', ?3)
                     END,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?4",
                params![status, error_message, next_retry, artifact_id],
            )
            .with_context(|| format!("mark branch artifact {} failed", artifact_id))?;
            Ok(())
        })
    }

    pub fn recover_stale_ci_lanes(&self) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start stale ci recovery transaction")?;
            let branch_suite_ids = stale_parent_ids(&tx, "branch_ci_run_lanes", "branch_ci_run_id")
                .context("load stale branch ci suite ids")?;
            let nightly_run_ids = stale_parent_ids(&tx, "nightly_run_lanes", "nightly_run_id")
                .context("load stale nightly run ids")?;
            let branch_rows = tx
                .execute(
                    "UPDATE branch_ci_run_lanes
                     SET status = 'queued',
                         execution_reason = 'stale_recovered',
                         failure_kind = NULL,
                         log_text = NULL,
                         pikaci_run_id = NULL,
                         pikaci_target_id = NULL,
                         retry_count = retry_count + 1,
                         started_at = NULL,
                         finished_at = NULL,
                         last_heartbeat_at = NULL,
                         lease_expires_at = NULL
                     WHERE status = 'running'
                       AND lease_expires_at IS NOT NULL
                       AND lease_expires_at <= CURRENT_TIMESTAMP",
                    [],
                )
                .context("reset stale branch ci lanes")?;
            let nightly_rows = tx
                .execute(
                    "UPDATE nightly_run_lanes
                     SET status = 'queued',
                         execution_reason = 'stale_recovered',
                         failure_kind = NULL,
                         log_text = NULL,
                         pikaci_run_id = NULL,
                         pikaci_target_id = NULL,
                         retry_count = retry_count + 1,
                         started_at = NULL,
                         finished_at = NULL,
                         last_heartbeat_at = NULL,
                         lease_expires_at = NULL
                     WHERE status = 'running'
                       AND lease_expires_at IS NOT NULL
                       AND lease_expires_at <= CURRENT_TIMESTAMP",
                    [],
                )
                .context("reset stale nightly lanes")?;
            for suite_id in branch_suite_ids {
                update_branch_ci_suite_status(&tx, suite_id)?;
            }
            for nightly_run_id in nightly_run_ids {
                update_nightly_run_status(&tx, nightly_run_id)?;
            }
            tx.commit()
                .context("commit stale ci recovery transaction")?;
            Ok(branch_rows + nightly_rows)
        })
    }

    pub fn count_queued_ci_lane_runs(&self) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let branch_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM branch_ci_run_lanes WHERE status = 'queued'",
                    [],
                    |row| row.get(0),
                )
                .context("count queued branch ci lanes")?;
            let nightly_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM nightly_run_lanes WHERE status = 'queued'",
                    [],
                    |row| row.get(0),
                )
                .context("count queued nightly ci lanes")?;
            Ok((branch_count + nightly_count).max(0) as usize)
        })
    }

    pub fn count_running_ci_lane_runs(&self) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let branch_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM branch_ci_run_lanes WHERE status = 'running'",
                    [],
                    |row| row.get(0),
                )
                .context("count running branch ci lanes")?;
            let nightly_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM nightly_run_lanes WHERE status = 'running'",
                    [],
                    |row| row.get(0),
                )
                .context("count running nightly ci lanes")?;
            Ok((branch_count + nightly_count).max(0) as usize)
        })
    }

    pub fn refresh_ci_lane_execution_reasons(
        &self,
        ci_concurrency: Option<usize>,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start refresh ci execution reasons transaction")?;
            refresh_ci_lane_execution_reasons_tx(&tx, ci_concurrency)?;
            tx.commit()
                .context("commit refresh ci execution reasons transaction")?;
            Ok(())
        })
    }

    pub fn claim_pending_branch_ci_lane_runs(
        &self,
        limit: usize,
        lease_secs: u64,
    ) -> anyhow::Result<Vec<PendingBranchCiLaneJob>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start branch ci lane claim transaction")?;
            let lease_window = format!("+{} seconds", lease_secs);
            let mut running_groups =
                running_concurrency_groups(&tx).context("load running ci concurrency groups")?;
            let unhealthy_targets =
                current_unhealthy_target_ids(&tx, Utc::now()).context("load unhealthy ci targets")?;
            let mut jobs = Vec::new();
            {
                let mut stmt = tx
                    .prepare(
                        "SELECT lane.id, lane.claim_token, lane.branch_ci_run_id, suite.branch_id, suite.source_head_sha, lane.lane_id, lane.title, lane.entrypoint, lane.command_json, lane.concurrency_group, lane.ci_target_key
                         FROM branch_ci_run_lanes lane
                         JOIN branch_ci_runs suite ON suite.id = lane.branch_ci_run_id
                         WHERE lane.status = 'queued'
                         ORDER BY lane.id ASC",
                    )
                    .context("prepare branch ci lane claim query")?;
                let rows = stmt
                    .query_map([], |row| {
                        let command_json: String = row.get(8)?;
                        let command: Vec<String> =
                            serde_json::from_str(&command_json).unwrap_or_default();
                        let _ci_target_key: Option<String> = row.get(10)?;
                        Ok(PendingBranchCiLaneJob {
                            lane_run_id: row.get(0)?,
                            claim_token: row.get::<_, i64>(1)? + 1,
                            suite_id: row.get(2)?,
                            branch_id: row.get(3)?,
                            source_head_sha: row.get(4)?,
                            lane_id: row.get(5)?,
                            title: row.get(6)?,
                            entrypoint: row.get(7)?,
                            command,
                            concurrency_group: row.get(9)?,
                        })
                    })
                    .context("query queued branch ci lanes")?;
                let mut candidates = Vec::new();
                for row in rows {
                    candidates.push(row.context("read queued branch ci lane row")?);
                }
                for job in candidates {
                    let target_key = tx
                        .query_row(
                            "SELECT ci_target_key FROM branch_ci_run_lanes WHERE id = ?1",
                            params![job.lane_run_id],
                            |row| row.get::<_, Option<String>>(0),
                        )
                        .with_context(|| format!("lookup target key for branch lane {}", job.lane_run_id))?;
                    if target_key
                        .as_deref()
                        .is_some_and(|target| unhealthy_targets.contains(target))
                    {
                        continue;
                    }
                    if let Some(group) = job.concurrency_group.as_deref() {
                        if running_groups.contains(group) {
                            continue;
                        }
                    }
                    let updated = tx
                        .execute(
                        "UPDATE branch_ci_run_lanes
                         SET status = 'running',
                             execution_reason = 'running',
                             failure_kind = NULL,
                             log_text = NULL,
                             pikaci_run_id = NULL,
                             pikaci_target_id = NULL,
                             started_at = CURRENT_TIMESTAMP,
                             last_heartbeat_at = CURRENT_TIMESTAMP,
                             lease_expires_at = datetime('now', ?2),
                             claim_token = ?3
                         WHERE id = ?1 AND status = 'queued' AND claim_token = ?4",
                        params![
                            job.lane_run_id,
                            lease_window,
                            job.claim_token,
                            job.claim_token - 1,
                        ],
                    )
                    .with_context(|| format!("mark branch ci lane {} running", job.lane_run_id))?;
                    if updated == 0 {
                        continue;
                    }
                    if let Some(group) = job.concurrency_group.as_ref() {
                        running_groups.insert(group.clone());
                    }
                    tx.execute(
                        "UPDATE branch_ci_runs
                         SET status = 'running',
                             started_at = COALESCE(started_at, CURRENT_TIMESTAMP),
                             finished_at = NULL
                         WHERE id = ?1 AND status IN ('queued', 'running')",
                        params![job.suite_id],
                    )
                    .with_context(|| format!("mark branch ci suite {} running", job.suite_id))?;
                    jobs.push(job);
                    if jobs.len() == limit {
                        break;
                    }
                }
            }
            tx.commit()
                .context("commit branch ci lane claim transaction")?;
            Ok(jobs)
        })
    }

    pub fn heartbeat_branch_ci_lane_run(
        &self,
        lane_run_id: i64,
        claim_token: i64,
        lease_secs: u64,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let lease_window = format!("+{} seconds", lease_secs);
            let updated = conn
                .execute(
                    "UPDATE branch_ci_run_lanes
                 SET last_heartbeat_at = CURRENT_TIMESTAMP,
                     lease_expires_at = datetime('now', ?2)
                 WHERE id = ?1 AND status = 'running' AND claim_token = ?3",
                    params![lane_run_id, lease_window, claim_token],
                )
                .with_context(|| format!("heartbeat branch ci lane {}", lane_run_id))?;
            if updated == 0 {
                bail!("{CI_LANE_LEASE_LOST}");
            }
            Ok(())
        })
    }

    pub fn finish_branch_ci_lane_run(
        &self,
        lane_run_id: i64,
        claim_token: i64,
        status: &str,
        log_text: &str,
    ) -> anyhow::Result<()> {
        let failure_kind = if status == CiLaneStatus::Failed.as_str() {
            Some(classify_ci_failure(log_text))
        } else {
            None
        };
        self.finish_branch_ci_lane_run_with_kind(
            lane_run_id,
            claim_token,
            status,
            log_text,
            failure_kind,
        )
    }

    pub fn finish_branch_ci_lane_run_with_kind(
        &self,
        lane_run_id: i64,
        claim_token: i64,
        status: &str,
        log_text: &str,
        failure_kind: Option<CiLaneFailureKind>,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start finish branch ci lane transaction")?;
            let (suite_id, ci_target_key): (i64, Option<String>) = tx
                .query_row(
                    "SELECT branch_ci_run_id, ci_target_key FROM branch_ci_run_lanes WHERE id = ?1",
                    params![lane_run_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .with_context(|| format!("lookup branch ci suite for lane {}", lane_run_id))?;
            let updated = tx
                .execute(
                    "UPDATE branch_ci_run_lanes
                 SET status = ?1,
                     failure_kind = ?2,
                     log_text = ?3,
                     execution_reason = 'running',
                     finished_at = CURRENT_TIMESTAMP,
                     last_heartbeat_at = CURRENT_TIMESTAMP,
                     lease_expires_at = NULL
                 WHERE id = ?4 AND status = 'running' AND claim_token = ?5",
                    params![
                        status,
                        failure_kind.map(|kind| kind.as_str()),
                        log_text,
                        lane_run_id,
                        claim_token
                    ],
                )
                .with_context(|| format!("finish branch ci lane {}", lane_run_id))?;
            if updated == 0 {
                bail!("{CI_LANE_LEASE_LOST}");
            }
            update_target_health_after_lane_finish(
                &tx,
                ci_target_key.as_deref(),
                status,
                failure_kind,
                Utc::now(),
            )?;
            update_branch_ci_suite_status(&tx, suite_id)?;
            tx.commit()
                .context("commit finish branch ci lane transaction")?;
            Ok(())
        })
    }

    pub fn record_branch_ci_lane_pikaci_run(
        &self,
        lane_run_id: i64,
        claim_token: i64,
        pikaci_run_id: &str,
        pikaci_target_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let updated = conn
                .execute(
                    "UPDATE branch_ci_run_lanes
                     SET pikaci_run_id = ?1,
                         pikaci_target_id = ?2,
                         ci_target_key = COALESCE(ci_target_key, ?2)
                     WHERE id = ?3 AND status = 'running' AND claim_token = ?4",
                    params![pikaci_run_id, pikaci_target_id, lane_run_id, claim_token],
                )
                .with_context(|| format!("record branch lane pikaci run {}", lane_run_id))?;
            if updated == 0 {
                bail!("{CI_LANE_LEASE_LOST}");
            }
            Ok(())
        })
    }

    pub fn claim_pending_nightly_lane_runs(
        &self,
        limit: usize,
        lease_secs: u64,
    ) -> anyhow::Result<Vec<PendingNightlyLaneJob>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start nightly lane claim transaction")?;
            let lease_window = format!("+{} seconds", lease_secs);
            let mut running_groups =
                running_concurrency_groups(&tx).context("load running ci concurrency groups")?;
            let unhealthy_targets =
                current_unhealthy_target_ids(&tx, Utc::now()).context("load unhealthy ci targets")?;
            let mut jobs = Vec::new();
            {
                let mut stmt = tx
                    .prepare(
                        "SELECT lane.id, lane.claim_token, lane.nightly_run_id, nightly.source_head_sha, lane.lane_id, lane.command_json, lane.concurrency_group, lane.ci_target_key
                         FROM nightly_run_lanes lane
                         JOIN nightly_runs nightly ON nightly.id = lane.nightly_run_id
                         WHERE lane.status = 'queued'
                         ORDER BY lane.id ASC",
                    )
                    .context("prepare nightly lane claim query")?;
                let rows = stmt
                    .query_map([], |row| {
                        let command_json: String = row.get(5)?;
                        let command: Vec<String> =
                            serde_json::from_str(&command_json).unwrap_or_default();
                        let _ci_target_key: Option<String> = row.get(7)?;
                        Ok(PendingNightlyLaneJob {
                            lane_run_id: row.get(0)?,
                            claim_token: row.get::<_, i64>(1)? + 1,
                            nightly_run_id: row.get(2)?,
                            source_head_sha: row.get(3)?,
                            lane_id: row.get(4)?,
                            command,
                            concurrency_group: row.get(6)?,
                        })
                    })
                    .context("query queued nightly lanes")?;
                let mut candidates = Vec::new();
                for row in rows {
                    candidates.push(row.context("read queued nightly lane row")?);
                }
                for job in candidates {
                    let target_key = tx
                        .query_row(
                            "SELECT ci_target_key FROM nightly_run_lanes WHERE id = ?1",
                            params![job.lane_run_id],
                            |row| row.get::<_, Option<String>>(0),
                        )
                        .with_context(|| format!("lookup target key for nightly lane {}", job.lane_run_id))?;
                    if target_key
                        .as_deref()
                        .is_some_and(|target| unhealthy_targets.contains(target))
                    {
                        continue;
                    }
                    if let Some(group) = job.concurrency_group.as_deref() {
                        if running_groups.contains(group) {
                            continue;
                        }
                    }
                    let updated = tx
                        .execute(
                        "UPDATE nightly_run_lanes
                         SET status = 'running',
                             execution_reason = 'running',
                             failure_kind = NULL,
                             log_text = NULL,
                             pikaci_run_id = NULL,
                             pikaci_target_id = NULL,
                             started_at = CURRENT_TIMESTAMP,
                             last_heartbeat_at = CURRENT_TIMESTAMP,
                             lease_expires_at = datetime('now', ?2),
                             claim_token = ?3
                         WHERE id = ?1 AND status = 'queued' AND claim_token = ?4",
                        params![
                            job.lane_run_id,
                            lease_window,
                            job.claim_token,
                            job.claim_token - 1,
                        ],
                    )
                    .with_context(|| format!("mark nightly lane {} running", job.lane_run_id))?;
                    if updated == 0 {
                        continue;
                    }
                    if let Some(group) = job.concurrency_group.as_ref() {
                        running_groups.insert(group.clone());
                    }
                    tx.execute(
                        "UPDATE nightly_runs
                         SET status = 'running',
                             started_at = COALESCE(started_at, CURRENT_TIMESTAMP),
                             finished_at = NULL
                         WHERE id = ?1 AND status IN ('queued', 'running')",
                        params![job.nightly_run_id],
                    )
                    .with_context(|| format!("mark nightly run {} running", job.nightly_run_id))?;
                    jobs.push(job);
                    if jobs.len() == limit {
                        break;
                    }
                }
            }
            tx.commit()
                .context("commit nightly lane claim transaction")?;
            Ok(jobs)
        })
    }

    pub fn heartbeat_nightly_lane_run(
        &self,
        lane_run_id: i64,
        claim_token: i64,
        lease_secs: u64,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let lease_window = format!("+{} seconds", lease_secs);
            let updated = conn
                .execute(
                    "UPDATE nightly_run_lanes
                 SET last_heartbeat_at = CURRENT_TIMESTAMP,
                     lease_expires_at = datetime('now', ?2)
                 WHERE id = ?1 AND status = 'running' AND claim_token = ?3",
                    params![lane_run_id, lease_window, claim_token],
                )
                .with_context(|| format!("heartbeat nightly lane {}", lane_run_id))?;
            if updated == 0 {
                bail!("{CI_LANE_LEASE_LOST}");
            }
            Ok(())
        })
    }

    pub fn finish_nightly_lane_run(
        &self,
        lane_run_id: i64,
        claim_token: i64,
        status: &str,
        log_text: &str,
    ) -> anyhow::Result<()> {
        let failure_kind = if status == CiLaneStatus::Failed.as_str() {
            Some(classify_ci_failure(log_text))
        } else {
            None
        };
        self.finish_nightly_lane_run_with_kind(
            lane_run_id,
            claim_token,
            status,
            log_text,
            failure_kind,
        )
    }

    pub fn finish_nightly_lane_run_with_kind(
        &self,
        lane_run_id: i64,
        claim_token: i64,
        status: &str,
        log_text: &str,
        failure_kind: Option<CiLaneFailureKind>,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start finish nightly lane transaction")?;
            let (nightly_run_id, ci_target_key): (i64, Option<String>) = tx
                .query_row(
                    "SELECT nightly_run_id, ci_target_key FROM nightly_run_lanes WHERE id = ?1",
                    params![lane_run_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .with_context(|| format!("lookup nightly run for lane {}", lane_run_id))?;
            let updated = tx
                .execute(
                    "UPDATE nightly_run_lanes
                 SET status = ?1,
                     failure_kind = ?2,
                     log_text = ?3,
                     execution_reason = 'running',
                     finished_at = CURRENT_TIMESTAMP,
                     last_heartbeat_at = CURRENT_TIMESTAMP,
                     lease_expires_at = NULL
                 WHERE id = ?4 AND status = 'running' AND claim_token = ?5",
                    params![
                        status,
                        failure_kind.map(|kind| kind.as_str()),
                        log_text,
                        lane_run_id,
                        claim_token
                    ],
                )
                .with_context(|| format!("finish nightly lane {}", lane_run_id))?;
            if updated == 0 {
                bail!("{CI_LANE_LEASE_LOST}");
            }
            update_target_health_after_lane_finish(
                &tx,
                ci_target_key.as_deref(),
                status,
                failure_kind,
                Utc::now(),
            )?;
            update_nightly_run_status(&tx, nightly_run_id)?;
            tx.commit()
                .context("commit finish nightly lane transaction")?;
            Ok(())
        })
    }

    pub fn record_nightly_lane_pikaci_run(
        &self,
        lane_run_id: i64,
        claim_token: i64,
        pikaci_run_id: &str,
        pikaci_target_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let updated = conn
                .execute(
                    "UPDATE nightly_run_lanes
                     SET pikaci_run_id = ?1,
                         pikaci_target_id = ?2,
                         ci_target_key = COALESCE(ci_target_key, ?2)
                     WHERE id = ?3 AND status = 'running' AND claim_token = ?4",
                    params![pikaci_run_id, pikaci_target_id, lane_run_id, claim_token],
                )
                .with_context(|| format!("record nightly lane pikaci run {}", lane_run_id))?;
            if updated == 0 {
                bail!("{CI_LANE_LEASE_LOST}");
            }
            Ok(())
        })
    }
}

fn ensure_repo_metadata(
    conn: &Connection,
    repo: &str,
    canonical_git_dir: &str,
    default_branch: &str,
    ci_entrypoint: &str,
) -> anyhow::Result<i64> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM repos WHERE repo = ?1",
            params![repo],
            |row| row.get(0),
        )
        .optional()
        .context("lookup repo row")?;
    match existing {
        Some(id) => {
            conn.execute(
                "UPDATE repos
                 SET canonical_git_dir = ?1,
                     default_branch = ?2,
                     ci_entrypoint = ?3,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?4",
                params![canonical_git_dir, default_branch, ci_entrypoint, id],
            )
            .context("update repo metadata")?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO repos(repo, canonical_git_dir, default_branch, ci_entrypoint)
                 VALUES (?1, ?2, ?3, ?4)",
                params![repo, canonical_git_dir, default_branch, ci_entrypoint],
            )
            .context("insert repo metadata")?;
            Ok(conn.last_insert_rowid())
        }
    }
}

fn ensure_branch_artifact_for_head(
    conn: &Connection,
    branch_id: i64,
    head_sha: &str,
    merge_base_sha: &str,
) -> anyhow::Result<bool> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT source_head_sha
             FROM branch_artifact_versions
             WHERE branch_id = ?1 AND status != 'superseded'
             ORDER BY version DESC
             LIMIT 1",
            params![branch_id],
            |row| row.get(0),
        )
        .optional()
        .context("lookup existing branch artifact head")?;
    if existing.as_deref() == Some(head_sha) {
        return Ok(false);
    }

    conn.execute(
        "UPDATE branch_artifact_versions
         SET status = CASE WHEN is_current = 1 THEN status ELSE 'superseded' END,
             is_current = CASE WHEN is_current = 1 THEN is_current ELSE 0 END,
             updated_at = CURRENT_TIMESTAMP
         WHERE branch_id = ?1 AND is_current = 0 AND status != 'superseded'",
        params![branch_id],
    )
    .with_context(|| format!("supersede stale branch artifacts for {}", branch_id))?;
    let next_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) + 1
             FROM branch_artifact_versions
             WHERE branch_id = ?1",
            params![branch_id],
            |row| row.get(0),
        )
        .with_context(|| format!("next branch artifact version for {}", branch_id))?;
    conn.execute(
        "INSERT INTO branch_artifact_versions(
            branch_id,
            version,
            source_head_sha,
            merge_base_sha,
            status
         ) VALUES (?1, ?2, ?3, ?4, 'pending')",
        params![branch_id, next_version, head_sha, merge_base_sha],
    )
    .with_context(|| format!("insert pending branch artifact for {}", branch_id))?;
    Ok(true)
}

fn list_branch_ci_run_lanes(
    conn: &Connection,
    suite_id: i64,
) -> anyhow::Result<Vec<BranchCiLaneRecord>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, lane_id, title, entrypoint, status, execution_reason, failure_kind, pikaci_run_id, pikaci_target_id, ci_target_key, log_text, retry_count, rerun_of_lane_run_id, created_at, started_at, finished_at, last_heartbeat_at, lease_expires_at
             FROM branch_ci_run_lanes
             WHERE branch_ci_run_id = ?1
             ORDER BY id ASC",
        )
        .context("prepare branch ci lane list query")?;
    let rows = stmt
        .query_map(params![suite_id], |row| {
            let ci_target_key: Option<String> = row.get(9)?;
            Ok(BranchCiLaneRecord {
                id: row.get(0)?,
                lane_id: row.get(1)?,
                title: row.get(2)?,
                entrypoint: row.get(3)?,
                status: row.get(4)?,
                execution_reason: parse_execution_reason(&row.get::<_, String>(5)?)?,
                failure_kind: parse_optional_failure_kind(row.get::<_, Option<String>>(6)?)?,
                pikaci_run_id: row.get(7)?,
                pikaci_target_id: row.get(8)?,
                ci_target_key,
                target_health: None,
                log_text: row.get(10)?,
                retry_count: row.get(11)?,
                rerun_of_lane_run_id: row.get(12)?,
                created_at: row.get(13)?,
                started_at: row.get(14)?,
                finished_at: row.get(15)?,
                last_heartbeat_at: row.get(16)?,
                lease_expires_at: row.get(17)?,
            })
        })
        .context("query branch ci lane rows")?;
    let mut lanes = Vec::new();
    for row in rows {
        lanes.push(row.context("read branch ci lane row")?);
    }
    hydrate_lane_target_health(conn, &mut lanes)?;
    Ok(lanes)
}

fn list_nightly_run_lanes(
    conn: &Connection,
    nightly_run_id: i64,
) -> anyhow::Result<Vec<NightlyLaneRecord>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, lane_id, title, entrypoint, status, execution_reason, failure_kind, pikaci_run_id, pikaci_target_id, ci_target_key, log_text, retry_count, rerun_of_lane_run_id, created_at, started_at, finished_at, last_heartbeat_at, lease_expires_at
             FROM nightly_run_lanes
             WHERE nightly_run_id = ?1
             ORDER BY id ASC",
        )
        .context("prepare nightly lane list query")?;
    let rows = stmt
        .query_map(params![nightly_run_id], |row| {
            let ci_target_key: Option<String> = row.get(9)?;
            Ok(NightlyLaneRecord {
                id: row.get(0)?,
                lane_id: row.get(1)?,
                title: row.get(2)?,
                entrypoint: row.get(3)?,
                status: row.get(4)?,
                execution_reason: parse_execution_reason(&row.get::<_, String>(5)?)?,
                failure_kind: parse_optional_failure_kind(row.get::<_, Option<String>>(6)?)?,
                pikaci_run_id: row.get(7)?,
                pikaci_target_id: row.get(8)?,
                ci_target_key,
                target_health: None,
                log_text: row.get(10)?,
                retry_count: row.get(11)?,
                rerun_of_lane_run_id: row.get(12)?,
                created_at: row.get(13)?,
                started_at: row.get(14)?,
                finished_at: row.get(15)?,
                last_heartbeat_at: row.get(16)?,
                lease_expires_at: row.get(17)?,
            })
        })
        .context("query nightly lane rows")?;
    let mut lanes = Vec::new();
    for row in rows {
        lanes.push(row.context("read nightly lane row")?);
    }
    hydrate_nightly_lane_target_health(conn, &mut lanes)?;
    Ok(lanes)
}

fn parse_execution_reason(raw: &str) -> rusqlite::Result<CiLaneExecutionReason> {
    CiLaneExecutionReason::from_str(raw).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            raw.len(),
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid ci execution reason `{raw}`"),
            )),
        )
    })
}

fn parse_optional_failure_kind(raw: Option<String>) -> rusqlite::Result<Option<CiLaneFailureKind>> {
    raw.map(|value| {
        CiLaneFailureKind::from_str(&value).map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                value.len(),
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid ci failure kind `{value}`"),
                )),
            )
        })
    })
    .transpose()
}

fn parse_target_health_state(raw: &str) -> rusqlite::Result<CiTargetHealthState> {
    CiTargetHealthState::from_str(raw).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            raw.len(),
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid ci target health state `{raw}`"),
            )),
        )
    })
}

fn load_ci_target_health_snapshots(
    conn: &Connection,
    target_ids: &[String],
) -> anyhow::Result<HashMap<String, CiTargetHealthSnapshot>> {
    if target_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = (0..target_ids.len())
        .map(|idx| format!("?{}", idx + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT target_id, state, consecutive_infra_failure_count, last_success_at, last_failure_at, last_failure_kind, cooloff_until
         FROM ci_target_health
         WHERE target_id IN ({placeholders})"
    );
    let mut stmt = conn
        .prepare(&sql)
        .context("prepare ci target health query")?;
    let mut params_dyn: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(target_ids.len());
    for target_id in target_ids {
        params_dyn.push(target_id);
    }
    let rows = stmt
        .query_map(params_dyn.as_slice(), |row| {
            let last_failure_kind = parse_optional_failure_kind(row.get(5)?)?;
            Ok(CiTargetHealthSnapshot {
                target_id: row.get(0)?,
                state: parse_target_health_state(&row.get::<_, String>(1)?)?,
                consecutive_infra_failure_count: row.get(2)?,
                last_success_at: row.get(3)?,
                last_failure_at: row.get(4)?,
                last_failure_kind,
                cooloff_until: row.get(6)?,
            })
        })
        .context("query ci target health rows")?;
    let mut snapshots = HashMap::new();
    for row in rows {
        let snapshot = row.context("read ci target health row")?;
        snapshots.insert(snapshot.target_id.clone(), snapshot);
    }
    Ok(snapshots)
}

fn hydrate_lane_target_health(
    conn: &Connection,
    lanes: &mut [BranchCiLaneRecord],
) -> anyhow::Result<()> {
    let target_ids = lanes
        .iter()
        .filter_map(|lane| lane.ci_target_key.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let snapshots = load_ci_target_health_snapshots(conn, &target_ids)?;
    for lane in lanes {
        lane.target_health = lane
            .ci_target_key
            .as_ref()
            .and_then(|target_id| snapshots.get(target_id).cloned());
    }
    Ok(())
}

fn hydrate_nightly_lane_target_health(
    conn: &Connection,
    lanes: &mut [NightlyLaneRecord],
) -> anyhow::Result<()> {
    let target_ids = lanes
        .iter()
        .filter_map(|lane| lane.ci_target_key.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let snapshots = load_ci_target_health_snapshots(conn, &target_ids)?;
    for lane in lanes {
        lane.target_health = lane
            .ci_target_key
            .as_ref()
            .and_then(|target_id| snapshots.get(target_id).cloned());
    }
    Ok(())
}

fn current_unhealthy_target_ids(
    conn: &Connection,
    now: DateTime<Utc>,
) -> anyhow::Result<HashSet<String>> {
    let mut stmt = conn
        .prepare(
            "SELECT target_id, state, consecutive_infra_failure_count, last_success_at, last_failure_at, last_failure_kind, cooloff_until
             FROM ci_target_health",
        )
        .context("prepare unhealthy ci target query")?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CiTargetHealthSnapshot {
                target_id: row.get(0)?,
                state: parse_target_health_state(&row.get::<_, String>(1)?)?,
                consecutive_infra_failure_count: row.get(2)?,
                last_success_at: row.get(3)?,
                last_failure_at: row.get(4)?,
                last_failure_kind: parse_optional_failure_kind(row.get(5)?)?,
                cooloff_until: row.get(6)?,
            })
        })
        .context("query unhealthy ci targets")?;
    let mut unhealthy = HashSet::new();
    for row in rows {
        let snapshot = row.context("read unhealthy ci target row")?;
        if snapshot.is_currently_unhealthy(now) {
            unhealthy.insert(snapshot.target_id);
        }
    }
    Ok(unhealthy)
}

fn refresh_ci_lane_execution_reasons_tx(
    conn: &Connection,
    ci_concurrency: Option<usize>,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE branch_ci_run_lanes
         SET execution_reason = 'running'
         WHERE status = 'running' AND execution_reason != 'running'",
        [],
    )
    .context("refresh running branch execution reasons")?;
    conn.execute(
        "UPDATE nightly_run_lanes
         SET execution_reason = 'running'
         WHERE status = 'running' AND execution_reason != 'running'",
        [],
    )
    .context("refresh running nightly execution reasons")?;

    let running_count = running_lane_count(conn)?;
    let mut available_slots = ci_concurrency.map(|limit| limit.saturating_sub(running_count));
    let mut running_groups = running_concurrency_groups(conn)?;
    let unhealthy_targets = current_unhealthy_target_ids(conn, Utc::now())?;

    refresh_queued_lane_reasons_for_table(
        conn,
        "nightly_run_lanes",
        &mut available_slots,
        &mut running_groups,
        &unhealthy_targets,
    )?;
    refresh_queued_lane_reasons_for_table(
        conn,
        "branch_ci_run_lanes",
        &mut available_slots,
        &mut running_groups,
        &unhealthy_targets,
    )?;
    Ok(())
}

fn refresh_queued_lane_reasons_for_table(
    conn: &Connection,
    table: &str,
    available_slots: &mut Option<usize>,
    running_groups: &mut HashSet<String>,
    unhealthy_targets: &HashSet<String>,
) -> anyhow::Result<()> {
    let sql = format!(
        "SELECT id, execution_reason, concurrency_group, ci_target_key
         FROM {table}
         WHERE status = 'queued'
         ORDER BY id ASC"
    );
    let mut stmt = conn
        .prepare(&sql)
        .with_context(|| format!("prepare queued execution reason query for {table}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                parse_execution_reason(&row.get::<_, String>(1)?)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .with_context(|| format!("query queued execution reasons for {table}"))?;

    for row in rows {
        let (lane_run_id, current_reason, concurrency_group, ci_target_key) =
            row.with_context(|| format!("read queued execution reason row from {table}"))?;
        let next_reason = if ci_target_key
            .as_deref()
            .is_some_and(|target_id| unhealthy_targets.contains(target_id))
        {
            CiLaneExecutionReason::TargetUnhealthy
        } else if concurrency_group
            .as_deref()
            .is_some_and(|group| running_groups.contains(group))
        {
            CiLaneExecutionReason::BlockedByConcurrencyGroup
        } else if available_slots.as_ref().is_some_and(|slots| *slots == 0) {
            CiLaneExecutionReason::WaitingForCapacity
        } else if current_reason == CiLaneExecutionReason::StaleRecovered {
            CiLaneExecutionReason::StaleRecovered
        } else {
            CiLaneExecutionReason::Queued
        };

        if next_reason != current_reason {
            let update_sql = format!(
                "UPDATE {table}
                 SET execution_reason = ?1
                 WHERE id = ?2"
            );
            conn.execute(&update_sql, params![next_reason.as_str(), lane_run_id])
                .with_context(|| {
                    format!("update execution reason for lane {lane_run_id} in {table}")
                })?;
        }

        if matches!(
            next_reason,
            CiLaneExecutionReason::Queued | CiLaneExecutionReason::StaleRecovered
        ) {
            if let Some(slots) = available_slots.as_mut() {
                *slots = slots.saturating_sub(1);
            }
            if let Some(group) = concurrency_group {
                running_groups.insert(group);
            }
        }
    }
    Ok(())
}

fn running_lane_count(conn: &Connection) -> anyhow::Result<usize> {
    let branch_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branch_ci_run_lanes WHERE status = 'running'",
            [],
            |row| row.get(0),
        )
        .context("count running branch lanes for refresh")?;
    let nightly_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM nightly_run_lanes WHERE status = 'running'",
            [],
            |row| row.get(0),
        )
        .context("count running nightly lanes for refresh")?;
    Ok((branch_count + nightly_count).max(0) as usize)
}

fn update_target_health_after_lane_finish(
    conn: &Connection,
    ci_target_key: Option<&str>,
    status: &str,
    failure_kind: Option<CiLaneFailureKind>,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let Some(target_id) = ci_target_key.filter(|value| !value.trim().is_empty()) else {
        return Ok(());
    };
    let current: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT consecutive_infra_failure_count, last_success_at
             FROM ci_target_health
             WHERE target_id = ?1",
            params![target_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .with_context(|| format!("lookup target health for {target_id}"))?;
    let existing_failures = current.as_ref().map(|row| row.0).unwrap_or(0);
    let existing_last_success_at = current.as_ref().and_then(|row| row.1.clone());
    let now_string = now.to_rfc3339_opts(SecondsFormat::Millis, true);

    match (status, failure_kind) {
        (value, Some(kind)) if value == CiLaneStatus::Failed.as_str() => {
            if kind.counts_toward_target_health() {
                let consecutive_failures = existing_failures + 1;
                let state = if consecutive_failures >= CI_TARGET_HEALTH_INFRA_FAILURE_THRESHOLD {
                    CiTargetHealthState::Unhealthy
                } else {
                    CiTargetHealthState::Healthy
                };
                let cooloff_until = if state == CiTargetHealthState::Unhealthy {
                    Some(next_target_cooloff_until(now))
                } else {
                    None
                };
                conn.execute(
                    "INSERT INTO ci_target_health(
                        target_id,
                        state,
                        consecutive_infra_failure_count,
                        last_success_at,
                        last_failure_at,
                        last_failure_kind,
                        cooloff_until,
                        updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(target_id) DO UPDATE SET
                        state = excluded.state,
                        consecutive_infra_failure_count = excluded.consecutive_infra_failure_count,
                        last_success_at = excluded.last_success_at,
                        last_failure_at = excluded.last_failure_at,
                        last_failure_kind = excluded.last_failure_kind,
                        cooloff_until = excluded.cooloff_until,
                        updated_at = excluded.updated_at",
                    params![
                        target_id,
                        state.as_str(),
                        consecutive_failures,
                        existing_last_success_at,
                        now_string,
                        kind.as_str(),
                        cooloff_until,
                        now_string,
                    ],
                )
                .with_context(|| format!("record unhealthy target state for {target_id}"))?;
            } else {
                conn.execute(
                    "INSERT INTO ci_target_health(
                        target_id,
                        state,
                        consecutive_infra_failure_count,
                        last_success_at,
                        last_failure_at,
                        last_failure_kind,
                        cooloff_until,
                        updated_at
                     ) VALUES (?1, 'healthy', 0, ?2, ?3, ?4, NULL, ?3)
                     ON CONFLICT(target_id) DO UPDATE SET
                        state = 'healthy',
                        consecutive_infra_failure_count = 0,
                        last_success_at = COALESCE(excluded.last_success_at, ci_target_health.last_success_at),
                        last_failure_at = excluded.last_failure_at,
                        last_failure_kind = excluded.last_failure_kind,
                        cooloff_until = NULL,
                        updated_at = excluded.updated_at",
                    params![target_id, existing_last_success_at, now_string, kind.as_str()],
                )
                .with_context(|| format!("record healthy target test failure for {target_id}"))?;
            }
        }
        (value, _) if value == CiLaneStatus::Success.as_str() => {
            conn.execute(
                "INSERT INTO ci_target_health(
                    target_id,
                    state,
                    consecutive_infra_failure_count,
                    last_success_at,
                    last_failure_at,
                    last_failure_kind,
                    cooloff_until,
                    updated_at
                 ) VALUES (?1, 'healthy', 0, ?2, NULL, NULL, NULL, ?2)
                 ON CONFLICT(target_id) DO UPDATE SET
                    state = 'healthy',
                    consecutive_infra_failure_count = 0,
                    last_success_at = excluded.last_success_at,
                    cooloff_until = NULL,
                    updated_at = excluded.updated_at",
                params![target_id, now_string],
            )
            .with_context(|| format!("record healthy target success for {target_id}"))?;
        }
        _ => {}
    }
    Ok(())
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

fn aggregate_lane_status(
    conn: &Connection,
    table: &str,
    foreign_key: &str,
    parent_id: i64,
) -> anyhow::Result<String> {
    let sql = format!(
        "SELECT COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'queued' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'skipped' THEN 1 ELSE 0 END), 0)
         FROM {table}
         WHERE {foreign_key} = ?1"
    );
    let (failed, running, queued, success, skipped): (i64, i64, i64, i64, i64) = conn
        .query_row(&sql, params![parent_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .with_context(|| format!("aggregate status from {table} for parent {parent_id}"))?;
    let total = failed + running + queued + success + skipped;
    let status = if total == 0 || success + skipped == total {
        "success"
    } else if running > 0 || (queued > 0 && (failed > 0 || success > 0 || skipped > 0)) {
        "running"
    } else if queued > 0 {
        "queued"
    } else if failed > 0 {
        "failed"
    } else {
        "success"
    };
    Ok(status.to_string())
}

fn running_concurrency_groups(conn: &Connection) -> anyhow::Result<HashSet<String>> {
    let mut groups = HashSet::new();
    for table in ["branch_ci_run_lanes", "nightly_run_lanes"] {
        let sql = format!(
            "SELECT concurrency_group
             FROM {table}
             WHERE status = 'running'
               AND concurrency_group IS NOT NULL
               AND concurrency_group <> ''"
        );
        let mut stmt = conn
            .prepare(&sql)
            .with_context(|| format!("prepare running concurrency group query for {table}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .with_context(|| format!("query running concurrency groups from {table}"))?;
        for row in rows {
            groups.insert(
                row.with_context(|| format!("read running concurrency group from {table}"))?,
            );
        }
    }
    Ok(groups)
}

fn stale_parent_ids(conn: &Connection, table: &str, foreign_key: &str) -> anyhow::Result<Vec<i64>> {
    let sql = format!(
        "SELECT DISTINCT {foreign_key}
         FROM {table}
         WHERE status = 'running'
           AND lease_expires_at IS NOT NULL
           AND lease_expires_at <= CURRENT_TIMESTAMP
         ORDER BY {foreign_key} ASC"
    );
    let mut stmt = conn
        .prepare(&sql)
        .with_context(|| format!("prepare stale parent id query for {table}"))?;
    let rows = stmt
        .query_map([], |row| row.get(0))
        .with_context(|| format!("query stale parent ids from {table}"))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.with_context(|| format!("read stale parent id from {table}"))?);
    }
    Ok(ids)
}

fn manual_lane_failure_note(actor_npub: &str, lane_id: &str) -> String {
    format!("Manual fail by {actor_npub}: marked {lane_id} failed for CI recovery.")
}

fn append_log_note(existing: Option<String>, note: &str) -> String {
    match existing {
        Some(existing) if !existing.trim().is_empty() => format!("{existing}\n{note}"),
        _ => note.to_string(),
    }
}

fn update_branch_ci_suite_status(conn: &Connection, suite_id: i64) -> anyhow::Result<()> {
    let status = aggregate_lane_status(conn, "branch_ci_run_lanes", "branch_ci_run_id", suite_id)?;
    let sql = if status == "success" || status == "failed" {
        "UPDATE branch_ci_runs
         SET status = ?1, finished_at = CURRENT_TIMESTAMP
         WHERE id = ?2"
    } else if status == "queued" {
        "UPDATE branch_ci_runs
         SET status = ?1, started_at = NULL, finished_at = NULL
         WHERE id = ?2"
    } else {
        "UPDATE branch_ci_runs
         SET status = ?1, finished_at = NULL
         WHERE id = ?2"
    };
    conn.execute(sql, params![status, suite_id])
        .with_context(|| format!("update branch ci suite {}", suite_id))?;
    Ok(())
}

fn update_nightly_run_status(conn: &Connection, nightly_run_id: i64) -> anyhow::Result<()> {
    let status =
        aggregate_lane_status(conn, "nightly_run_lanes", "nightly_run_id", nightly_run_id)?;
    let sql = if status == "success" || status == "failed" {
        "UPDATE nightly_runs
         SET status = ?1, finished_at = CURRENT_TIMESTAMP
         WHERE id = ?2"
    } else if status == "queued" {
        "UPDATE nightly_runs
         SET status = ?1, started_at = NULL, finished_at = NULL
         WHERE id = ?2"
    } else {
        "UPDATE nightly_runs
         SET status = ?1, finished_at = NULL
         WHERE id = ?2"
    };
    conn.execute(sql, params![status, nightly_run_id])
        .with_context(|| format!("update nightly run {}", nightly_run_id))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use super::{classify_mirror_failure_kind, BranchUpsertInput, CI_LANE_LEASE_LOST};
    use crate::ci_manifest::ForgeLane;
    use crate::ci_state::CiLaneExecutionReason;
    use crate::storage::Store;

    fn open_store() -> Store {
        let dir = tempfile::tempdir().expect("create temp dir").keep();
        let db_path = dir.join("pika-news.db");
        Store::open(&db_path).expect("open store")
    }

    fn upsert_input(branch_name: &str, head_sha: &str) -> BranchUpsertInput {
        BranchUpsertInput {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: "/tmp/pika.git".to_string(),
            default_branch: "master".to_string(),
            ci_entrypoint: "just pre-merge".to_string(),
            branch_name: branch_name.to_string(),
            title: format!("{} title", branch_name),
            head_sha: head_sha.to_string(),
            merge_base_sha: "base123".to_string(),
            author_name: Some("alice".to_string()),
            author_email: Some("alice@example.com".to_string()),
            updated_at: "2026-03-16T12:00:00Z".to_string(),
        }
    }

    fn latest_branch_artifact_id(store: &Store, branch_id: i64) -> i64 {
        store
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT id
                     FROM branch_artifact_versions
                     WHERE branch_id = ?1
                     ORDER BY version DESC
                     LIMIT 1",
                    params![branch_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("lookup latest branch artifact")
    }

    fn sample_lane(id: &str) -> ForgeLane {
        ForgeLane {
            id: id.to_string(),
            title: format!("{id} title"),
            entrypoint: format!("just checks::{id}"),
            command: vec!["just".to_string(), format!("checks::{id}")],
            paths: vec![],
            concurrency_group: None,
            staged_linux_target: None,
        }
    }

    fn sample_lane_with_group(id: &str, concurrency_group: &str) -> ForgeLane {
        let mut lane = sample_lane(id);
        lane.concurrency_group = Some(concurrency_group.to_string());
        lane
    }

    fn sample_lane_with_target(id: &str, target_id: &str) -> ForgeLane {
        let mut lane = sample_lane(id);
        lane.staged_linux_target = Some(target_id.to_string());
        lane
    }

    fn target_health_row(
        store: &Store,
        target_id: &str,
    ) -> (String, i64, Option<String>, Option<String>, Option<String>) {
        store
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT state, consecutive_infra_failure_count, last_success_at, last_failure_kind, cooloff_until
                     FROM ci_target_health
                     WHERE target_id = ?1",
                    params![target_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .map_err(Into::into)
            })
            .expect("lookup target health row")
    }

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
                Some("stale mirror sync still holds repo lock (pid 42, trigger background, elapsed 999s)")
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
                Some("mirror sync already running (pid 42, trigger background, elapsed 3s): mirror is already in sync; treating this trigger as obsolete")
            ),
            Some("obsolete")
        );
    }

    #[test]
    fn branch_name_reuse_creates_new_numeric_record() {
        let store = open_store();
        let first = store
            .upsert_branch_record(&upsert_input("feature/reuse", "head111"))
            .expect("insert first branch lifecycle");
        store
            .mark_branch_closed(first.branch_id, "npub1trusted")
            .expect("close first branch lifecycle");

        let second = store
            .upsert_branch_record(&upsert_input("feature/reuse", "head222"))
            .expect("insert second branch lifecycle");

        assert_ne!(first.branch_id, second.branch_id);
        let items = store.list_branch_feed_items().expect("list branch feed");
        assert_eq!(items.len(), 2);
        assert!(items
            .iter()
            .any(|item| item.branch_id == first.branch_id && item.state == "closed"));
        assert!(items
            .iter()
            .any(|item| item.branch_id == second.branch_id && item.state == "open"));
    }

    #[test]
    fn find_branch_by_name_returns_latest_closed_lifecycle() {
        let store = open_store();
        let first = store
            .upsert_branch_record(&upsert_input("feature/history", "head111"))
            .expect("insert first branch lifecycle");
        store
            .mark_branch_closed(first.branch_id, "npub1trusted")
            .expect("close first branch lifecycle");

        let resolved = store
            .find_branch_by_name("sledtools/pika", "feature/history")
            .expect("resolve branch by name")
            .expect("branch exists");

        assert_eq!(resolved.branch_id, first.branch_id);
        assert_eq!(resolved.branch_state, "closed");
        assert_eq!(resolved.branch_name, "feature/history");
    }

    #[test]
    fn head_change_queues_new_ci_run() {
        let store = open_store();
        let first = store
            .upsert_branch_record(&upsert_input("feature/ci", "head111"))
            .expect("insert branch");
        assert!(store
            .queue_branch_ci_run_for_head(first.branch_id, "head111", &[sample_lane("pika")])
            .expect("queue first branch ci suite"));
        let second = store
            .upsert_branch_record(&upsert_input("feature/ci", "head222"))
            .expect("update branch with new head");
        assert!(second.head_changed);
        assert!(store
            .queue_branch_ci_run_for_head(second.branch_id, "head222", &[sample_lane("pika")])
            .expect("queue second branch ci suite"));
        let runs = store
            .list_branch_ci_runs(second.branch_id, 10)
            .expect("list ci runs");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].source_head_sha, "head222");
        assert_eq!(runs[1].source_head_sha, "head111");
        assert_eq!(runs[0].lanes.len(), 1);
    }

    #[test]
    fn rerun_failed_branch_lane_creates_new_single_lane_suite() {
        let store = open_store();
        let branch = store
            .upsert_branch_record(&upsert_input("feature/rerun", "head777"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head777",
                &[sample_lane("pika"), sample_lane("fixture")],
            )
            .expect("queue ci suite");
        let jobs = store
            .claim_pending_branch_ci_lane_runs(2, 120)
            .expect("claim ci jobs");
        let failed_lane = jobs
            .iter()
            .find(|job| job.lane_id == "pika")
            .expect("failed lane");
        let passed_lane = jobs
            .iter()
            .find(|job| job.lane_id == "fixture")
            .expect("passed lane");
        store
            .finish_branch_ci_lane_run(
                failed_lane.lane_run_id,
                failed_lane.claim_token,
                "failed",
                "boom",
            )
            .expect("finish failed lane");
        store
            .finish_branch_ci_lane_run(
                passed_lane.lane_run_id,
                passed_lane.claim_token,
                "success",
                "ok",
            )
            .expect("finish passed lane");

        let rerun_suite_id = store
            .rerun_branch_ci_lane(branch.branch_id, failed_lane.lane_run_id)
            .expect("rerun branch lane")
            .expect("rerun suite id");
        let runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("list branch runs");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, rerun_suite_id);
        assert_eq!(runs[0].status, "queued");
        assert_eq!(runs[0].rerun_of_run_id, Some(failed_lane.suite_id));
        assert_eq!(runs[0].lanes.len(), 1);
        assert_eq!(runs[0].lanes[0].lane_id, "pika");
        assert_eq!(
            runs[0].lanes[0].rerun_of_lane_run_id,
            Some(failed_lane.lane_run_id)
        );
    }

    #[test]
    fn branch_lane_persists_pikaci_run_metadata() {
        let store = open_store();
        let branch = store
            .upsert_branch_record(&upsert_input("feature/pikaci-meta", "head-meta"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(branch.branch_id, "head-meta", &[sample_lane("pika")])
            .expect("queue ci suite");
        let job = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim ci job")
            .into_iter()
            .next()
            .expect("ci lane");
        store
            .record_branch_ci_lane_pikaci_run(
                job.lane_run_id,
                job.claim_token,
                "pikaci-run-1",
                Some("pre-merge-pika-rust"),
            )
            .expect("persist branch pikaci metadata");

        let runs = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list ci runs");
        assert_eq!(
            runs[0].lanes[0].pikaci_run_id.as_deref(),
            Some("pikaci-run-1")
        );
        assert_eq!(
            runs[0].lanes[0].pikaci_target_id.as_deref(),
            Some("pre-merge-pika-rust")
        );
    }

    #[test]
    fn rerun_failed_nightly_lane_creates_new_single_lane_run() {
        let store = open_store();
        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "nightly-head",
                "2026-03-17T08:00:00Z",
                &[sample_lane("nightly_pika"), sample_lane("nightly_fixture")],
            )
            .expect("queue nightly");
        let jobs = store
            .claim_pending_nightly_lane_runs(2, 120)
            .expect("claim nightly jobs");
        let failed_lane = jobs
            .iter()
            .find(|job| job.lane_id == "nightly_pika")
            .expect("failed nightly lane");
        let passed_lane = jobs
            .iter()
            .find(|job| job.lane_id == "nightly_fixture")
            .expect("passed nightly lane");
        store
            .finish_nightly_lane_run(
                failed_lane.lane_run_id,
                failed_lane.claim_token,
                "failed",
                "nightly boom",
            )
            .expect("finish failed nightly lane");
        store
            .finish_nightly_lane_run(
                passed_lane.lane_run_id,
                passed_lane.claim_token,
                "success",
                "nightly ok",
            )
            .expect("finish passed nightly lane");

        let rerun_run_id = store
            .rerun_nightly_lane(failed_lane.nightly_run_id, failed_lane.lane_run_id)
            .expect("rerun nightly lane")
            .expect("rerun nightly run");
        let nightlies = store.list_recent_nightly_runs(8).expect("nightly feed");
        assert_eq!(nightlies.len(), 2);
        assert_eq!(nightlies[0].nightly_run_id, rerun_run_id);
        let rerun = store
            .get_nightly_run(rerun_run_id)
            .expect("rerun nightly detail")
            .expect("rerun nightly exists");
        assert_eq!(rerun.status, "queued");
        assert_eq!(rerun.rerun_of_run_id, Some(failed_lane.nightly_run_id));
        assert_eq!(rerun.lanes.len(), 1);
        assert_eq!(rerun.lanes[0].lane_id, "nightly_pika");
        assert_eq!(
            rerun.lanes[0].rerun_of_lane_run_id,
            Some(failed_lane.lane_run_id)
        );
    }

    #[test]
    fn nightly_lane_persists_pikaci_run_metadata() {
        let store = open_store();
        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "nightly-meta",
                "2026-03-17T08:00:00Z",
                &[sample_lane("nightly_pika")],
            )
            .expect("queue nightly");
        let job = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim nightly job")
            .into_iter()
            .next()
            .expect("nightly lane");
        store
            .record_nightly_lane_pikaci_run(
                job.lane_run_id,
                job.claim_token,
                "pikaci-nightly-1",
                Some("pre-merge-pika-rust"),
            )
            .expect("persist nightly pikaci metadata");

        let nightly = store
            .get_nightly_run(job.nightly_run_id)
            .expect("nightly detail")
            .expect("nightly exists");
        assert_eq!(
            nightly.lanes[0].pikaci_run_id.as_deref(),
            Some("pikaci-nightly-1")
        );
        assert_eq!(
            nightly.lanes[0].pikaci_target_id.as_deref(),
            Some("pre-merge-pika-rust")
        );
    }

    #[test]
    fn rerun_branch_lane_rejects_parent_mismatch() {
        let store = open_store();
        let first = store
            .upsert_branch_record(&upsert_input("feature/first", "head-a"))
            .expect("insert first branch");
        let second = store
            .upsert_branch_record(&upsert_input("feature/second", "head-b"))
            .expect("insert second branch");
        store
            .queue_branch_ci_run_for_head(first.branch_id, "head-a", &[sample_lane("pika")])
            .expect("queue first suite");
        let job = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim first job")
            .into_iter()
            .next()
            .expect("job");
        store
            .finish_branch_ci_lane_run(job.lane_run_id, job.claim_token, "failed", "boom")
            .expect("finish lane");

        let rerun = store
            .rerun_branch_ci_lane(second.branch_id, job.lane_run_id)
            .expect("rerun mismatch");
        assert!(rerun.is_none());
    }

    #[test]
    fn rerun_nightly_lane_rejects_parent_mismatch() {
        let store = open_store();
        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "nightly-a",
                "2026-03-17T08:00:00Z",
                &[sample_lane("nightly_pika")],
            )
            .expect("queue first nightly");
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "nightly-b",
                "2026-03-18T08:00:00Z",
                &[sample_lane("nightly_fixture")],
            )
            .expect("queue second nightly");
        let job = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim nightly job")
            .into_iter()
            .next()
            .expect("job");
        store
            .finish_nightly_lane_run(job.lane_run_id, job.claim_token, "failed", "boom")
            .expect("finish nightly lane");
        let wrong_nightly = store
            .list_recent_nightly_runs(8)
            .expect("list nightlies")
            .into_iter()
            .find(|run| run.nightly_run_id != job.nightly_run_id)
            .expect("wrong nightly");

        let rerun = store
            .rerun_nightly_lane(wrong_nightly.nightly_run_id, job.lane_run_id)
            .expect("rerun mismatch");
        assert!(rerun.is_none());
    }

    #[test]
    fn failed_lane_does_not_finish_suite_while_another_lane_is_running() {
        let store = open_store();
        let branch = store
            .upsert_branch_record(&upsert_input("feature/parallel-status", "head-status"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-status",
                &[sample_lane("first"), sample_lane("second")],
            )
            .expect("queue ci suite");
        let jobs = store
            .claim_pending_branch_ci_lane_runs(2, 120)
            .expect("claim ci jobs");
        assert_eq!(jobs.len(), 2);

        store
            .finish_branch_ci_lane_run(jobs[0].lane_run_id, jobs[0].claim_token, "failed", "boom")
            .expect("finish first lane");

        let runs = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list branch runs");
        assert_eq!(runs[0].status, "running");
        assert!(runs[0].finished_at.is_none());
        assert_eq!(runs[0].lanes[0].status, "failed");
        assert_eq!(runs[0].lanes[1].status, "running");

        store
            .finish_branch_ci_lane_run(jobs[1].lane_run_id, jobs[1].claim_token, "success", "ok")
            .expect("finish second lane");
        let runs = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list branch runs");
        assert_eq!(runs[0].status, "failed");
        assert!(runs[0].finished_at.is_some());
    }

    #[test]
    fn same_concurrency_group_is_claimed_serially() {
        let store = open_store();
        let first = store
            .upsert_branch_record(&upsert_input("feature/one", "head-one"))
            .expect("insert first branch");
        let second = store
            .upsert_branch_record(&upsert_input("feature/two", "head-two"))
            .expect("insert second branch");
        let third = store
            .upsert_branch_record(&upsert_input("feature/three", "head-three"))
            .expect("insert third branch");
        store
            .queue_branch_ci_run_for_head(
                first.branch_id,
                "head-one",
                &[sample_lane_with_group("linux_one", "staged-linux:shared")],
            )
            .expect("queue first");
        store
            .queue_branch_ci_run_for_head(
                second.branch_id,
                "head-two",
                &[sample_lane_with_group("linux_two", "staged-linux:shared")],
            )
            .expect("queue second");
        store
            .queue_branch_ci_run_for_head(
                third.branch_id,
                "head-three",
                &[sample_lane_with_group("linux_other", "staged-linux:other")],
            )
            .expect("queue third");

        let jobs = store
            .claim_pending_branch_ci_lane_runs(3, 120)
            .expect("claim jobs");
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().any(|job| job.lane_id == "linux_one"));
        assert!(jobs.iter().any(|job| job.lane_id == "linux_other"));
        assert!(!jobs.iter().any(|job| job.lane_id == "linux_two"));

        let first_job = jobs
            .iter()
            .find(|job| job.lane_id == "linux_one")
            .expect("first shared job");
        store
            .finish_branch_ci_lane_run(
                first_job.lane_run_id,
                first_job.claim_token,
                "success",
                "ok",
            )
            .expect("finish first shared job");

        let next_jobs = store
            .claim_pending_branch_ci_lane_runs(3, 120)
            .expect("claim remaining jobs");
        assert_eq!(next_jobs.len(), 1);
        assert_eq!(next_jobs[0].lane_id, "linux_two");
    }

    #[test]
    fn queue_reasons_and_unhealthy_targets_do_not_block_unrelated_claims() {
        let store = open_store();
        let running_branch = store
            .upsert_branch_record(&upsert_input("feature/running", "head-running"))
            .expect("insert running branch");
        let blocked_branch = store
            .upsert_branch_record(&upsert_input("feature/blocked", "head-blocked"))
            .expect("insert blocked branch");
        let unhealthy_branch = store
            .upsert_branch_record(&upsert_input("feature/unhealthy", "head-unhealthy"))
            .expect("insert unhealthy branch");
        let ready_branch = store
            .upsert_branch_record(&upsert_input("feature/ready", "head-ready"))
            .expect("insert ready branch");
        let waiting_branch = store
            .upsert_branch_record(&upsert_input("feature/waiting", "head-waiting"))
            .expect("insert waiting branch");

        store
            .queue_branch_ci_run_for_head(
                running_branch.branch_id,
                "head-running",
                &[sample_lane_with_group("shared-holder", "shared-group")],
            )
            .expect("queue running branch");
        let running_job = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim running holder")
            .into_iter()
            .next()
            .expect("running holder");
        store
            .queue_branch_ci_run_for_head(
                blocked_branch.branch_id,
                "head-blocked",
                &[sample_lane_with_group("shared-blocked", "shared-group")],
            )
            .expect("queue blocked branch");
        store
            .queue_branch_ci_run_for_head(
                unhealthy_branch.branch_id,
                "head-unhealthy",
                &[sample_lane_with_target("apple-sanity", "apple-host")],
            )
            .expect("queue unhealthy branch");
        store
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO ci_target_health(
                        target_id,
                        state,
                        consecutive_infra_failure_count,
                        last_failure_at,
                        last_failure_kind,
                        cooloff_until,
                        updated_at
                     ) VALUES (?1, 'unhealthy', 2, CURRENT_TIMESTAMP, 'infrastructure', datetime('now', '+15 minutes'), CURRENT_TIMESTAMP)",
                    params!["apple-host"],
                )?;
                Ok::<(), anyhow::Error>(())
            })
            .expect("insert unhealthy target");
        store
            .queue_branch_ci_run_for_head(
                ready_branch.branch_id,
                "head-ready",
                &[sample_lane("ready-now")],
            )
            .expect("queue ready branch");
        store
            .queue_branch_ci_run_for_head(
                waiting_branch.branch_id,
                "head-waiting",
                &[sample_lane("wait-capacity")],
            )
            .expect("queue waiting branch");

        let claimed = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim next ready lane");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].lane_id, "ready-now");

        store
            .refresh_ci_lane_execution_reasons(Some(2))
            .expect("refresh execution reasons");

        let blocked_runs = store
            .list_branch_ci_runs(blocked_branch.branch_id, 1)
            .expect("list blocked runs");
        assert_eq!(
            blocked_runs[0].lanes[0].execution_reason,
            CiLaneExecutionReason::BlockedByConcurrencyGroup
        );
        let unhealthy_runs = store
            .list_branch_ci_runs(unhealthy_branch.branch_id, 1)
            .expect("list unhealthy runs");
        assert_eq!(
            unhealthy_runs[0].lanes[0].execution_reason,
            CiLaneExecutionReason::TargetUnhealthy
        );
        assert_eq!(
            unhealthy_runs[0].lanes[0]
                .target_health
                .as_ref()
                .expect("target health")
                .consecutive_infra_failure_count,
            2
        );
        let waiting_runs = store
            .list_branch_ci_runs(waiting_branch.branch_id, 1)
            .expect("list waiting runs");
        assert_eq!(
            waiting_runs[0].lanes[0].execution_reason,
            CiLaneExecutionReason::WaitingForCapacity
        );
        let ready_runs = store
            .list_branch_ci_runs(ready_branch.branch_id, 1)
            .expect("list ready runs");
        assert_eq!(ready_runs[0].lanes[0].status, "running");
        assert_eq!(
            ready_runs[0].lanes[0].execution_reason,
            CiLaneExecutionReason::Running
        );

        store
            .finish_branch_ci_lane_run(
                running_job.lane_run_id,
                running_job.claim_token,
                "success",
                "ok",
            )
            .expect("finish running holder");
        store
            .finish_branch_ci_lane_run(
                claimed[0].lane_run_id,
                claimed[0].claim_token,
                "success",
                "ok",
            )
            .expect("finish claimed ready lane");
    }

    #[test]
    fn target_health_tracks_infra_failures_and_resets_after_success() {
        let store = open_store();
        let branch = store
            .upsert_branch_record(&upsert_input("feature/target-health", "head-target-health"))
            .expect("insert branch");
        let lane = sample_lane_with_target("pika-rust", "pre-merge-pika-rust");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-target-health",
                std::slice::from_ref(&lane),
            )
            .expect("queue ci");

        let first = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim first run")
            .into_iter()
            .next()
            .expect("first run");
        store
            .finish_branch_ci_lane_run(
                first.lane_run_id,
                first.claim_token,
                "failed",
                "ci runner error: permission denied",
            )
            .expect("finish first infra failure");
        let first_health = target_health_row(&store, "pre-merge-pika-rust");
        assert_eq!(first_health.0, "healthy");
        assert_eq!(first_health.1, 1);
        assert_eq!(first_health.3.as_deref(), Some("infrastructure"));
        assert!(first_health.4.is_none());

        store
            .requeue_branch_ci_lane(branch.branch_id, first.lane_run_id)
            .expect("requeue first lane")
            .expect("lane exists");
        let second = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim second run")
            .into_iter()
            .next()
            .expect("second run");
        store
            .finish_branch_ci_lane_run(
                second.lane_run_id,
                second.claim_token,
                "failed",
                "ci runner error: unbound variable",
            )
            .expect("finish second infra failure");
        let second_health = target_health_row(&store, "pre-merge-pika-rust");
        assert_eq!(second_health.0, "unhealthy");
        assert_eq!(second_health.1, 2);
        assert_eq!(second_health.3.as_deref(), Some("infrastructure"));
        assert!(second_health.4.is_some());

        store
            .requeue_branch_ci_lane(branch.branch_id, second.lane_run_id)
            .expect("requeue second lane")
            .expect("lane exists");
        assert!(store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim while unhealthy")
            .is_empty());

        store
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE ci_target_health
                     SET cooloff_until = datetime('now', '-1 minute')
                     WHERE target_id = ?1",
                    params!["pre-merge-pika-rust"],
                )?;
                Ok::<(), anyhow::Error>(())
            })
            .expect("expire cooloff");
        store
            .refresh_ci_lane_execution_reasons(Some(1))
            .expect("refresh after cooloff");
        let third = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim after cooloff")
            .into_iter()
            .next()
            .expect("third run");
        store
            .finish_branch_ci_lane_run(third.lane_run_id, third.claim_token, "success", "ok")
            .expect("finish healthy success");
        let final_health = target_health_row(&store, "pre-merge-pika-rust");
        assert_eq!(final_health.0, "healthy");
        assert_eq!(final_health.1, 0);
        assert!(final_health.2.is_some());
    }

    #[test]
    fn duplicate_claims_are_prevented_across_parallel_workers() {
        let store = open_store();
        let branch = store
            .upsert_branch_record(&upsert_input("feature/claim-race", "head-race"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-race",
                &[sample_lane("one"), sample_lane("two")],
            )
            .expect("queue ci suite");

        let first_store = store.clone();
        let second_store = store.clone();
        let first = std::thread::spawn(move || {
            first_store
                .claim_pending_branch_ci_lane_runs(1, 120)
                .expect("first claim")
        });
        let second = std::thread::spawn(move || {
            second_store
                .claim_pending_branch_ci_lane_runs(1, 120)
                .expect("second claim")
        });

        let mut lane_ids = first
            .join()
            .expect("join first claim")
            .into_iter()
            .map(|job| job.lane_run_id)
            .collect::<Vec<_>>();
        lane_ids.extend(
            second
                .join()
                .expect("join second claim")
                .into_iter()
                .map(|job| job.lane_run_id),
        );
        lane_ids.sort_unstable();
        lane_ids.dedup();
        assert_eq!(lane_ids.len(), 2);
    }

    #[test]
    fn merged_branch_detail_remains_readable_after_deletion() {
        let store = open_store();
        let branch = store
            .upsert_branch_record(&upsert_input("feature/history", "head333"))
            .expect("insert branch");
        let artifact_id = latest_branch_artifact_id(&store, branch.branch_id);
        store
            .mark_branch_generation_ready(
                artifact_id,
                r#"{"executive_summary":"ok","media_links":[],"steps":[{"title":"Step","intent":"Intent","affected_files":["src/main.rs"],"evidence_snippets":["@@ -1 +1 @@"],"body_markdown":"body"}]}"#,
                "<p>ok</p>",
                "head333",
                "@@ -1 +1 @@",
                None,
            )
            .expect("mark branch artifact ready");
        store
            .queue_branch_ci_run_for_head(branch.branch_id, "head333", &[sample_lane("pika")])
            .expect("queue ci suite");
        let jobs = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim ci lane");
        store
            .finish_branch_ci_lane_run(jobs[0].lane_run_id, jobs[0].claim_token, "success", "ci ok")
            .expect("persist ci lane result");
        store
            .mark_branch_merged(branch.branch_id, "npub1trusted", "merge999")
            .expect("mark merged");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("load branch detail")
            .expect("detail exists");
        assert_eq!(detail.branch_state, "merged");
        assert_eq!(detail.merge_commit_sha.as_deref(), Some("merge999"));
        assert!(detail.tutorial_json.is_some());
        assert_eq!(detail.ci_status, "success");
        assert_eq!(detail.unified_diff.as_deref(), Some("@@ -1 +1 @@"));
    }

    #[test]
    fn stale_recovery_only_requeues_old_running_work() {
        let store = open_store();
        let branch = store
            .upsert_branch_record(&upsert_input("feature/stale", "head555"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head555",
                &[sample_lane("old"), sample_lane("fresh")],
            )
            .expect("queue branch suite");

        let nightly_lanes = vec![sample_lane("nightly_old"), sample_lane("nightly_fresh")];
        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "nightly-head",
                "2026-03-17T08:00:00Z",
                &nightly_lanes,
            )
            .expect("queue nightly");

        let branch_jobs = store
            .claim_pending_branch_ci_lane_runs(2, 120)
            .expect("claim branch lanes");
        let nightly_jobs = store
            .claim_pending_nightly_lane_runs(2, 120)
            .expect("claim nightly lanes");
        assert_eq!(branch_jobs.len(), 2);
        assert_eq!(nightly_jobs.len(), 2);

        store
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE branch_ci_run_lanes
                     SET last_heartbeat_at = datetime('now', '-20 minutes'),
                         lease_expires_at = datetime('now', '-1 minute')
                     WHERE id = ?1",
                    params![branch_jobs[0].lane_run_id],
                )?;
                conn.execute(
                    "UPDATE branch_ci_run_lanes
                     SET last_heartbeat_at = datetime('now', '-2 minutes'),
                         lease_expires_at = datetime('now', '+2 minutes')
                     WHERE id = ?1",
                    params![branch_jobs[1].lane_run_id],
                )?;
                conn.execute(
                    "UPDATE nightly_run_lanes
                     SET last_heartbeat_at = datetime('now', '-20 minutes'),
                         lease_expires_at = datetime('now', '-1 minute')
                     WHERE id = ?1",
                    params![nightly_jobs[0].lane_run_id],
                )?;
                conn.execute(
                    "UPDATE nightly_run_lanes
                     SET last_heartbeat_at = datetime('now', '-2 minutes'),
                         lease_expires_at = datetime('now', '+2 minutes')
                     WHERE id = ?1",
                    params![nightly_jobs[1].lane_run_id],
                )?;
                Ok::<(), anyhow::Error>(())
            })
            .expect("age lane timestamps");

        let recovered = store.recover_stale_ci_lanes().expect("recover stale work");
        assert_eq!(recovered, 2);

        let branch_runs = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list branch runs");
        assert_eq!(branch_runs[0].status, "running");
        let old_branch_lane = branch_runs[0]
            .lanes
            .iter()
            .find(|lane| lane.lane_id == "old")
            .expect("old branch lane");
        let fresh_branch_lane = branch_runs[0]
            .lanes
            .iter()
            .find(|lane| lane.lane_id == "fresh")
            .expect("fresh branch lane");
        assert_eq!(old_branch_lane.status, "queued");
        assert_eq!(old_branch_lane.retry_count, 1);
        assert!(old_branch_lane.log_text.is_none());
        assert!(old_branch_lane.pikaci_run_id.is_none());
        assert!(old_branch_lane.pikaci_target_id.is_none());
        assert_eq!(fresh_branch_lane.status, "running");
        assert_eq!(fresh_branch_lane.retry_count, 0);

        let nightlies = store.list_recent_nightly_runs(4).expect("nightly feed");
        let nightly = store
            .get_nightly_run(nightlies[0].nightly_run_id)
            .expect("get nightly")
            .expect("nightly exists");
        assert_eq!(nightly.status, "running");
        let old_nightly_lane = nightly
            .lanes
            .iter()
            .find(|lane| lane.lane_id == "nightly_old")
            .expect("old nightly lane");
        let fresh_nightly_lane = nightly
            .lanes
            .iter()
            .find(|lane| lane.lane_id == "nightly_fresh")
            .expect("fresh nightly lane");
        assert_eq!(old_nightly_lane.status, "queued");
        assert_eq!(old_nightly_lane.retry_count, 1);
        assert!(old_nightly_lane.log_text.is_none());
        assert!(old_nightly_lane.pikaci_run_id.is_none());
        assert!(old_nightly_lane.pikaci_target_id.is_none());
        assert_eq!(fresh_nightly_lane.status, "running");
        assert_eq!(fresh_nightly_lane.retry_count, 0);
    }

    #[test]
    fn recovered_lane_claim_clears_stale_pikaci_metadata_before_retry() {
        let store = open_store();
        let branch = store
            .upsert_branch_record(&upsert_input("feature/recovered-meta", "head-meta-retry"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-meta-retry",
                &[sample_lane("pika")],
            )
            .expect("queue branch suite");
        let first_job = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim first job")
            .pop()
            .expect("first job");
        store
            .record_branch_ci_lane_pikaci_run(
                first_job.lane_run_id,
                first_job.claim_token,
                "pikaci-stale",
                Some("pre-merge-pika-rust"),
            )
            .expect("record stale pikaci metadata");
        store
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE branch_ci_run_lanes
                     SET lease_expires_at = datetime('now', '-1 minute'),
                         log_text = 'stale log from prior attempt'
                     WHERE id = ?1",
                    params![first_job.lane_run_id],
                )?;
                Ok::<(), anyhow::Error>(())
            })
            .expect("expire first lease");
        assert_eq!(store.recover_stale_ci_lanes().expect("recover stale"), 1);

        let recovered = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list recovered runs");
        let queued_lane = &recovered[0].lanes[0];
        assert!(queued_lane.log_text.is_none());
        assert!(queued_lane.pikaci_run_id.is_none());
        assert!(queued_lane.pikaci_target_id.is_none());

        let second_job = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim retried job")
            .pop()
            .expect("retried job");
        store
            .finish_branch_ci_lane_run(
                second_job.lane_run_id,
                second_job.claim_token,
                "failed",
                "retry failed before run_started",
            )
            .expect("finish retried job");

        let finished = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list finished runs");
        let lane = &finished[0].lanes[0];
        assert_eq!(lane.status, "failed");
        assert_eq!(
            lane.log_text.as_deref(),
            Some("retry failed before run_started")
        );
        assert!(lane.pikaci_run_id.is_none());
        assert!(lane.pikaci_target_id.is_none());
    }

    #[test]
    fn reclaimed_lane_rejects_old_worker_heartbeats_and_finishes() {
        let store = open_store();
        let branch = store
            .upsert_branch_record(&upsert_input("feature/fence", "head666"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(branch.branch_id, "head666", &[sample_lane("pika")])
            .expect("queue branch suite");
        let first_job = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim first branch worker")
            .pop()
            .expect("first branch job");

        store
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE branch_ci_run_lanes
                     SET lease_expires_at = datetime('now', '-1 minute')
                     WHERE id = ?1",
                    params![first_job.lane_run_id],
                )?;
                Ok::<(), anyhow::Error>(())
            })
            .expect("expire branch lease");
        assert_eq!(
            store
                .recover_stale_ci_lanes()
                .expect("recover stale branch"),
            1
        );
        let second_job = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim second branch worker")
            .pop()
            .expect("second branch job");
        assert!(second_job.claim_token > first_job.claim_token);
        assert!(store
            .heartbeat_branch_ci_lane_run(first_job.lane_run_id, first_job.claim_token, 120)
            .expect_err("old branch heartbeat should fail")
            .to_string()
            .contains(CI_LANE_LEASE_LOST));
        assert!(store
            .finish_branch_ci_lane_run(
                first_job.lane_run_id,
                first_job.claim_token,
                "success",
                "stale worker",
            )
            .expect_err("old branch finish should fail")
            .to_string()
            .contains(CI_LANE_LEASE_LOST));
        store
            .heartbeat_branch_ci_lane_run(second_job.lane_run_id, second_job.claim_token, 120)
            .expect("current branch heartbeat");
        store
            .finish_branch_ci_lane_run(
                second_job.lane_run_id,
                second_job.claim_token,
                "success",
                "current worker",
            )
            .expect("current branch finish");

        let nightly_lanes = vec![sample_lane("nightly_fence")];
        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "nightly-fence-head",
                "2026-03-17T08:00:00Z",
                &nightly_lanes,
            )
            .expect("queue nightly");
        let first_nightly = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim first nightly worker")
            .pop()
            .expect("first nightly job");
        store
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE nightly_run_lanes
                     SET lease_expires_at = datetime('now', '-1 minute')
                     WHERE id = ?1",
                    params![first_nightly.lane_run_id],
                )?;
                Ok::<(), anyhow::Error>(())
            })
            .expect("expire nightly lease");
        assert_eq!(
            store
                .recover_stale_ci_lanes()
                .expect("recover stale nightly"),
            1
        );
        let second_nightly = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim second nightly worker")
            .pop()
            .expect("second nightly job");
        assert!(second_nightly.claim_token > first_nightly.claim_token);
        assert!(store
            .heartbeat_nightly_lane_run(first_nightly.lane_run_id, first_nightly.claim_token, 120)
            .expect_err("old nightly heartbeat should fail")
            .to_string()
            .contains(CI_LANE_LEASE_LOST));
        assert!(store
            .finish_nightly_lane_run(
                first_nightly.lane_run_id,
                first_nightly.claim_token,
                "success",
                "stale nightly worker",
            )
            .expect_err("old nightly finish should fail")
            .to_string()
            .contains(CI_LANE_LEASE_LOST));
        store
            .heartbeat_nightly_lane_run(second_nightly.lane_run_id, second_nightly.claim_token, 120)
            .expect("current nightly heartbeat");
        store
            .finish_nightly_lane_run(
                second_nightly.lane_run_id,
                second_nightly.claim_token,
                "success",
                "current nightly worker",
            )
            .expect("current nightly finish");
    }

    #[test]
    fn manual_requeue_rejects_old_workers_for_branch_and_nightly_lanes() {
        let store = open_store();
        let branch = store
            .upsert_branch_record(&upsert_input("feature/manual-requeue", "head777"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(branch.branch_id, "head777", &[sample_lane("pika")])
            .expect("queue branch suite");
        let first_branch = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim branch worker")
            .pop()
            .expect("branch worker");
        store
            .requeue_branch_ci_lane(branch.branch_id, first_branch.lane_run_id)
            .expect("requeue branch lane")
            .expect("branch lane exists");
        assert!(store
            .heartbeat_branch_ci_lane_run(first_branch.lane_run_id, first_branch.claim_token, 120)
            .expect_err("stale branch heartbeat should fail")
            .to_string()
            .contains(CI_LANE_LEASE_LOST));
        assert!(store
            .finish_branch_ci_lane_run(
                first_branch.lane_run_id,
                first_branch.claim_token,
                "success",
                "stale branch worker",
            )
            .expect_err("stale branch finish should fail")
            .to_string()
            .contains(CI_LANE_LEASE_LOST));
        let second_branch = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim requeued branch worker")
            .pop()
            .expect("second branch worker");
        assert!(second_branch.claim_token > first_branch.claim_token);

        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "nightly-manual-requeue",
                "2026-03-17T08:00:00Z",
                &[sample_lane("nightly_pika")],
            )
            .expect("queue nightly");
        let first_nightly = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim nightly worker")
            .pop()
            .expect("nightly worker");
        store
            .requeue_nightly_lane(first_nightly.nightly_run_id, first_nightly.lane_run_id)
            .expect("requeue nightly lane")
            .expect("nightly lane exists");
        assert!(store
            .heartbeat_nightly_lane_run(first_nightly.lane_run_id, first_nightly.claim_token, 120)
            .expect_err("stale nightly heartbeat should fail")
            .to_string()
            .contains(CI_LANE_LEASE_LOST));
        assert!(store
            .finish_nightly_lane_run(
                first_nightly.lane_run_id,
                first_nightly.claim_token,
                "success",
                "stale nightly worker",
            )
            .expect_err("stale nightly finish should fail")
            .to_string()
            .contains(CI_LANE_LEASE_LOST));
        let second_nightly = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim requeued nightly worker")
            .pop()
            .expect("second nightly worker");
        assert!(second_nightly.claim_token > first_nightly.claim_token);
    }

    #[test]
    fn recover_run_requeues_only_nonterminal_lanes() {
        let store = open_store();
        let branch = store
            .upsert_branch_record(&upsert_input("feature/recover-run", "head888"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head888",
                &[
                    sample_lane("first"),
                    sample_lane("second"),
                    sample_lane("third"),
                ],
            )
            .expect("queue branch suite");
        let mut jobs = store
            .claim_pending_branch_ci_lane_runs(3, 120)
            .expect("claim branch jobs");
        jobs.sort_by_key(|job| job.lane_run_id);
        store
            .finish_branch_ci_lane_run(jobs[0].lane_run_id, jobs[0].claim_token, "success", "ok")
            .expect("finish success lane");
        store
            .finish_branch_ci_lane_run(jobs[1].lane_run_id, jobs[1].claim_token, "failed", "boom")
            .expect("finish failed lane");
        let recovered = store
            .recover_branch_ci_run(branch.branch_id, jobs[2].suite_id)
            .expect("recover branch run")
            .expect("run exists");
        assert_eq!(recovered, 2);
        let lanes = store
            .list_branch_ci_runs(branch.branch_id, 1)
            .expect("list branch runs")[0]
            .lanes
            .clone();
        assert_eq!(lanes[0].status, "success");
        assert_eq!(lanes[1].status, "queued");
        assert_eq!(lanes[1].retry_count, 1);
        assert_eq!(lanes[2].status, "queued");
        assert_eq!(lanes[2].retry_count, 1);

        let repo_id = store
            .ensure_forge_repo_metadata(
                "sledtools/pika",
                "/tmp/pika.git",
                "master",
                "ci/forge-lanes.toml",
            )
            .expect("ensure repo metadata");
        store
            .queue_nightly_run(
                repo_id,
                "refs/heads/master",
                "nightly-recover-run",
                "2026-03-18T08:00:00Z",
                &[sample_lane("nightly_first"), sample_lane("nightly_second")],
            )
            .expect("queue nightly");
        let nightly_run_id =
            store.list_recent_nightly_runs(1).expect("nightly feed")[0].nightly_run_id;
        let mut nightly_jobs = store
            .claim_pending_nightly_lane_runs(2, 120)
            .expect("claim nightly jobs");
        nightly_jobs.sort_by_key(|job| job.lane_run_id);
        store
            .finish_nightly_lane_run(
                nightly_jobs[0].lane_run_id,
                nightly_jobs[0].claim_token,
                "success",
                "ok",
            )
            .expect("finish nightly success lane");
        let recovered = store
            .recover_nightly_run(nightly_run_id)
            .expect("recover nightly run")
            .expect("nightly exists");
        assert_eq!(recovered, 1);
        let nightly = store
            .get_nightly_run(nightly_run_id)
            .expect("nightly detail")
            .expect("nightly run");
        assert_eq!(nightly.lanes[0].status, "success");
        assert_eq!(nightly.lanes[1].status, "queued");
        assert_eq!(nightly.lanes[1].retry_count, 1);
    }
}
