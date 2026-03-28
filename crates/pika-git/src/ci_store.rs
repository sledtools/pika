use std::collections::HashSet;
use std::str::FromStr;

use anyhow::{bail, Context};
use chrono::{SecondsFormat, Utc};
use pika_forge_model::ForgeCiStatus;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::ci_manifest::ForgeLane;
use crate::ci_state::{
    classify_ci_failure, configured_target_key_for_lane, CiLaneExecutionReason, CiLaneFailureKind,
    CiLaneStatus,
};
use crate::storage::Store;

pub const CI_LANE_LEASE_LOST: &str = "ci lane lease lost";

#[derive(Debug, Clone)]
pub struct BranchCiRunRecord {
    pub id: i64,
    pub source_head_sha: String,
    pub status: ForgeCiStatus,
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
    pub status: CiLaneStatus,
    pub execution_reason: CiLaneExecutionReason,
    pub failure_kind: Option<CiLaneFailureKind>,
    pub pikaci_run_id: Option<String>,
    pub pikaci_target_id: Option<String>,
    pub ci_target_key: Option<String>,
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
    pub structured_pikaci_target_id: Option<String>,
    pub concurrency_group: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NightlyFeedItem {
    pub nightly_run_id: i64,
    pub repo: String,
    pub source_head_sha: String,
    pub status: ForgeCiStatus,
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
    pub status: ForgeCiStatus,
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
    pub status: CiLaneStatus,
    pub execution_reason: CiLaneExecutionReason,
    pub failure_kind: Option<CiLaneFailureKind>,
    pub pikaci_run_id: Option<String>,
    pub pikaci_target_id: Option<String>,
    pub ci_target_key: Option<String>,
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
    pub structured_pikaci_target_id: Option<String>,
    pub concurrency_group: Option<String>,
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
    structured_pikaci_target_id: Option<String>,
    concurrency_group: Option<String>,
    ci_target_key: Option<String>,
    status: CiLaneStatus,
}

#[derive(Debug, Clone)]
struct BranchLaneRecoverySource {
    run_id: i64,
    lane_id: String,
    status: CiLaneStatus,
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
    structured_pikaci_target_id: Option<String>,
    concurrency_group: Option<String>,
    ci_target_key: Option<String>,
    status: CiLaneStatus,
}

#[derive(Debug, Clone)]
struct NightlyLaneRecoverySource {
    nightly_run_id: i64,
    lane_id: String,
    status: CiLaneStatus,
    claim_token: i64,
    log_text: Option<String>,
}

impl Store {
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
                    status: ForgeCiStatus::from(status),
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

            let suite_status = if lanes.is_empty() {
                ForgeCiStatus::Success
            } else {
                ForgeCiStatus::Queued
            };
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
                    crate::ci::FORGE_LANE_DEFINITION_PATH,
                    selected_lane_ids,
                    suite_status.as_str(),
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
                        structured_pikaci_target_id,
                        concurrency_group,
                        ci_target_key,
                        execution_reason,
                        status
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'queued', 'queued')",
                    params![
                        suite_id,
                        lane.id,
                        lane.title,
                        lane.entrypoint,
                        command_json,
                        Some(lane.id.clone()),
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
                params![
                    branch_id,
                    head_sha,
                    crate::ci::FORGE_LANE_DEFINITION_PATH,
                    "[]",
                    log_text
                ],
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
                        status: ForgeCiStatus::from(row.get::<_, String>(3)?),
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
                    status: ForgeCiStatus::from(status),
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
            let status = if lanes.is_empty() {
                ForgeCiStatus::Success
            } else {
                ForgeCiStatus::Queued
            };
            let summary = if lanes.is_empty() {
                Some("No nightly lanes configured.".to_string())
            } else {
                None
            };
            tx.execute(
                "INSERT INTO nightly_runs(repo_id, source_ref, source_head_sha, status, summary, scheduled_for, finished_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, CASE WHEN ?4 = 'success' THEN CURRENT_TIMESTAMP ELSE NULL END)",
                params![
                    repo_id,
                    source_ref,
                    source_head_sha,
                    status.as_str(),
                    summary,
                    scheduled_for
                ],
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
                        structured_pikaci_target_id,
                        concurrency_group,
                        ci_target_key,
                        execution_reason,
                        status
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'queued', 'queued')",
                    params![
                        nightly_run_id,
                        lane.id,
                        lane.title,
                        lane.entrypoint,
                        command_json,
                        Some(lane.id.clone()),
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
                            lane.structured_pikaci_target_id,
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
                            structured_pikaci_target_id: row.get(7)?,
                            concurrency_group: row.get(8)?,
                            ci_target_key: row.get(9)?,
                            status: parse_lane_status(&row.get::<_, String>(10)?)?,
                        })
                    },
                )
                .optional()
                .context("lookup branch ci lane for rerun")?;
            let Some(row) = row else {
                return Ok(None);
            };
            if matches!(row.status, CiLaneStatus::Queued | CiLaneStatus::Running) {
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
                    structured_pikaci_target_id,
                    concurrency_group,
                    ci_target_key,
                    execution_reason,
                    status,
                    rerun_of_lane_run_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'queued', 'queued', ?9)",
                params![
                    rerun_suite_id,
                    row.lane_id,
                    row.title,
                    row.entrypoint,
                    row.command_json,
                    row.structured_pikaci_target_id,
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
                            lane.structured_pikaci_target_id,
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
                            structured_pikaci_target_id: row.get(8)?,
                            concurrency_group: row.get(9)?,
                            ci_target_key: row.get(10)?,
                            status: parse_lane_status(&row.get::<_, String>(11)?)?,
                        })
                    },
                )
                .optional()
                .context("lookup nightly lane for rerun")?;
            let Some(row) = row else {
                return Ok(None);
            };
            if matches!(row.status, CiLaneStatus::Queued | CiLaneStatus::Running) {
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
                    structured_pikaci_target_id,
                    concurrency_group,
                    ci_target_key,
                    execution_reason,
                    status,
                    rerun_of_lane_run_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'queued', 'queued', ?9)",
                params![
                    rerun_run_id,
                    row.lane_id,
                    row.title,
                    row.entrypoint,
                    row.command_json,
                    row.structured_pikaci_target_id,
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
                            status: parse_lane_status(&row.get::<_, String>(2)?)?,
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
            if !matches!(row.status, CiLaneStatus::Queued | CiLaneStatus::Running) {
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
                            status: parse_lane_status(&row.get::<_, String>(2)?)?,
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
            if !matches!(
                row.status,
                CiLaneStatus::Queued | CiLaneStatus::Running | CiLaneStatus::Failed
            ) {
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
                            status: parse_lane_status(&row.get::<_, String>(2)?)?,
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
            if !matches!(row.status, CiLaneStatus::Queued | CiLaneStatus::Running) {
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
                            status: parse_lane_status(&row.get::<_, String>(2)?)?,
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
            if !matches!(
                row.status,
                CiLaneStatus::Queued | CiLaneStatus::Running | CiLaneStatus::Failed
            ) {
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
            let mut jobs = Vec::new();
            {
                let mut stmt = tx
                    .prepare(
                        "SELECT lane.id, lane.claim_token, lane.branch_ci_run_id, suite.branch_id, suite.source_head_sha, lane.lane_id, lane.title, lane.entrypoint, lane.command_json, lane.structured_pikaci_target_id, lane.concurrency_group, lane.ci_target_key
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
                        let _ci_target_key: Option<String> = row.get(11)?;
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
                            structured_pikaci_target_id: row.get(9)?,
                            concurrency_group: row.get(10)?,
                        })
                    })
                    .context("query queued branch ci lanes")?;
                let mut candidates = Vec::new();
                for row in rows {
                    candidates.push(row.context("read queued branch ci lane row")?);
                }
                for job in candidates {
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
        status: CiLaneStatus,
        log_text: &str,
    ) -> anyhow::Result<()> {
        let failure_kind = if status == CiLaneStatus::Failed {
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
        status: CiLaneStatus,
        log_text: &str,
        failure_kind: Option<CiLaneFailureKind>,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start finish branch ci lane transaction")?;
            let suite_id: i64 = tx
                .query_row(
                    "SELECT branch_ci_run_id FROM branch_ci_run_lanes WHERE id = ?1",
                    params![lane_run_id],
                    |row| row.get(0),
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
                        status.as_str(),
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
            let mut jobs = Vec::new();
            {
                let mut stmt = tx
                    .prepare(
                        "SELECT lane.id, lane.claim_token, lane.nightly_run_id, nightly.source_head_sha, lane.lane_id, lane.command_json, lane.structured_pikaci_target_id, lane.concurrency_group, lane.ci_target_key
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
                        let _ci_target_key: Option<String> = row.get(8)?;
                        Ok(PendingNightlyLaneJob {
                            lane_run_id: row.get(0)?,
                            claim_token: row.get::<_, i64>(1)? + 1,
                            nightly_run_id: row.get(2)?,
                            source_head_sha: row.get(3)?,
                            lane_id: row.get(4)?,
                            command,
                            structured_pikaci_target_id: row.get(6)?,
                            concurrency_group: row.get(7)?,
                        })
                    })
                    .context("query queued nightly lanes")?;
                let mut candidates = Vec::new();
                for row in rows {
                    candidates.push(row.context("read queued nightly lane row")?);
                }
                for job in candidates {
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
        status: CiLaneStatus,
        log_text: &str,
    ) -> anyhow::Result<()> {
        let failure_kind = if status == CiLaneStatus::Failed {
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
        status: CiLaneStatus,
        log_text: &str,
        failure_kind: Option<CiLaneFailureKind>,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start finish nightly lane transaction")?;
            let nightly_run_id: i64 = tx
                .query_row(
                    "SELECT nightly_run_id FROM nightly_run_lanes WHERE id = ?1",
                    params![lane_run_id],
                    |row| row.get(0),
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
                        status.as_str(),
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
                status: parse_lane_status(&row.get::<_, String>(4)?)?,
                execution_reason: parse_execution_reason(&row.get::<_, String>(5)?)?,
                failure_kind: parse_optional_failure_kind(row.get::<_, Option<String>>(6)?)?,
                pikaci_run_id: row.get(7)?,
                pikaci_target_id: row.get(8)?,
                ci_target_key,
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
                status: parse_lane_status(&row.get::<_, String>(4)?)?,
                execution_reason: parse_execution_reason(&row.get::<_, String>(5)?)?,
                failure_kind: parse_optional_failure_kind(row.get::<_, Option<String>>(6)?)?,
                pikaci_run_id: row.get(7)?,
                pikaci_target_id: row.get(8)?,
                ci_target_key,
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

fn parse_lane_status(raw: &str) -> rusqlite::Result<CiLaneStatus> {
    CiLaneStatus::from_str(raw).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            raw.len(),
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid ci lane status `{raw}`"),
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

    refresh_queued_lane_reasons_for_table(
        conn,
        "nightly_run_lanes",
        &mut available_slots,
        &mut running_groups,
    )?;
    refresh_queued_lane_reasons_for_table(
        conn,
        "branch_ci_run_lanes",
        &mut available_slots,
        &mut running_groups,
    )?;
    Ok(())
}

fn refresh_queued_lane_reasons_for_table(
    conn: &Connection,
    table: &str,
    available_slots: &mut Option<usize>,
    running_groups: &mut HashSet<String>,
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
        let (lane_run_id, current_reason, concurrency_group, _ci_target_key) =
            row.with_context(|| format!("read queued execution reason row from {table}"))?;
        let next_reason = if concurrency_group
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

fn aggregate_lane_status(
    conn: &Connection,
    table: &str,
    foreign_key: &str,
    parent_id: i64,
) -> anyhow::Result<ForgeCiStatus> {
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
        ForgeCiStatus::Success
    } else if running > 0 || (queued > 0 && (failed > 0 || success > 0 || skipped > 0)) {
        ForgeCiStatus::Running
    } else if queued > 0 {
        ForgeCiStatus::Queued
    } else if failed > 0 {
        ForgeCiStatus::Failed
    } else {
        ForgeCiStatus::Success
    };
    Ok(status)
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
    let sql = if matches!(status, ForgeCiStatus::Success | ForgeCiStatus::Failed) {
        "UPDATE branch_ci_runs
         SET status = ?1, finished_at = CURRENT_TIMESTAMP
         WHERE id = ?2"
    } else if status == ForgeCiStatus::Queued {
        "UPDATE branch_ci_runs
         SET status = ?1, started_at = NULL, finished_at = NULL
         WHERE id = ?2"
    } else {
        "UPDATE branch_ci_runs
         SET status = ?1, finished_at = NULL
         WHERE id = ?2"
    };
    conn.execute(sql, params![status.as_str(), suite_id])
        .with_context(|| format!("update branch ci suite {}", suite_id))?;
    Ok(())
}

fn update_nightly_run_status(conn: &Connection, nightly_run_id: i64) -> anyhow::Result<()> {
    let status =
        aggregate_lane_status(conn, "nightly_run_lanes", "nightly_run_id", nightly_run_id)?;
    let sql = if matches!(status, ForgeCiStatus::Success | ForgeCiStatus::Failed) {
        "UPDATE nightly_runs
         SET status = ?1, finished_at = CURRENT_TIMESTAMP
         WHERE id = ?2"
    } else if status == ForgeCiStatus::Queued {
        "UPDATE nightly_runs
         SET status = ?1, started_at = NULL, finished_at = NULL
         WHERE id = ?2"
    } else {
        "UPDATE nightly_runs
         SET status = ?1, finished_at = NULL
         WHERE id = ?2"
    };
    conn.execute(sql, params![status.as_str(), nightly_run_id])
        .with_context(|| format!("update nightly run {}", nightly_run_id))?;
    Ok(())
}
