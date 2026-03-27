use anyhow::Context;
use pika_forge_model::{BranchState, ForgeCiStatus, TutorialStatus};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::storage::Store;

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
    pub state: BranchState,
    pub updated_at: String,
    pub head_sha: String,
    pub tutorial_status: TutorialStatus,
    pub ci_status: ForgeCiStatus,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BranchDetailRecord {
    pub branch_id: i64,
    pub current_artifact_id: Option<i64>,
    pub repo: String,
    pub branch_name: String,
    pub title: String,
    pub branch_state: BranchState,
    pub updated_at: String,
    pub target_branch: String,
    pub head_sha: String,
    pub merge_base_sha: String,
    pub merge_commit_sha: Option<String>,
    pub tutorial_status: TutorialStatus,
    pub tutorial_json: Option<String>,
    pub unified_diff: Option<String>,
    pub claude_session_id: Option<String>,
    pub error_message: Option<String>,
    pub ci_status: ForgeCiStatus,
}

#[derive(Debug, Clone)]
pub struct BranchLookupRecord {
    pub branch_id: i64,
    pub repo: String,
    pub branch_name: String,
    pub branch_state: BranchState,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BranchActionTarget {
    pub branch_id: i64,
    pub repo: String,
    pub branch_name: String,
    pub title: String,
    pub branch_state: BranchState,
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
                        state: BranchState::from(row.get::<_, String>(4)?),
                        updated_at: row.get(5)?,
                        head_sha: row.get(6)?,
                        tutorial_status: TutorialStatus::from(row.get::<_, String>(7)?),
                        ci_status: ForgeCiStatus::from(row.get::<_, String>(8)?),
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
                        branch_state: BranchState::from(row.get::<_, String>(5)?),
                        updated_at: row.get(6)?,
                        target_branch: row.get(7)?,
                        head_sha: row.get(8)?,
                        merge_base_sha: row.get(9)?,
                        merge_commit_sha: row.get(10)?,
                        tutorial_status: TutorialStatus::from(row.get::<_, String>(11)?),
                        tutorial_json: row.get(12)?,
                        unified_diff: row.get(13)?,
                        claude_session_id: row.get(14)?,
                        error_message: row.get(15)?,
                        ci_status: ForgeCiStatus::from(row.get::<_, String>(16)?),
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
                        branch_state: BranchState::from(row.get::<_, String>(3)?),
                    })
                },
            )
            .optional()
            .context("query branch by name")
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
                        branch_state: BranchState::from(row.get::<_, String>(4)?),
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
                            branch_state: BranchState::from(row.get::<_, String>(4)?),
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

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use super::BranchUpsertInput;
    use crate::ci_manifest::ForgeLane;
    use crate::ci_state::{CiLaneExecutionReason, CiLaneStatus};
    use crate::ci_store::CI_LANE_LEASE_LOST;
    use crate::storage::Store;
    use pika_forge_model::{BranchState, ForgeCiStatus};

    fn open_store() -> Store {
        let dir = tempfile::tempdir().expect("create temp dir").keep();
        let db_path = dir.join("pika-git.db");
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
            .any(|item| item.branch_id == first.branch_id && item.state == BranchState::Closed));
        assert!(items
            .iter()
            .any(|item| item.branch_id == second.branch_id && item.state == BranchState::Open));
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
        assert_eq!(resolved.branch_state, BranchState::Closed);
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
                crate::ci_state::CiLaneStatus::Failed,
                "boom",
            )
            .expect("finish failed lane");
        store
            .finish_branch_ci_lane_run(
                passed_lane.lane_run_id,
                passed_lane.claim_token,
                crate::ci_state::CiLaneStatus::Success,
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
        assert_eq!(runs[0].status, ForgeCiStatus::Queued);
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
    fn claimed_branch_lane_keeps_structured_pikaci_target_hint() {
        let store = open_store();
        let branch = store
            .upsert_branch_record(&upsert_input("feature/structured", "head-structured"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-structured",
                &[sample_lane_with_target("pika_rust", "pre-merge-pika-rust")],
            )
            .expect("queue branch ci suite");

        let job = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim ci job")
            .into_iter()
            .next()
            .expect("branch lane");

        assert_eq!(
            job.structured_pikaci_target_id.as_deref(),
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
                crate::ci_state::CiLaneStatus::Failed,
                "nightly boom",
            )
            .expect("finish failed nightly lane");
        store
            .finish_nightly_lane_run(
                passed_lane.lane_run_id,
                passed_lane.claim_token,
                crate::ci_state::CiLaneStatus::Success,
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
        assert_eq!(rerun.status, ForgeCiStatus::Queued);
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
    fn claimed_nightly_lane_keeps_structured_pikaci_target_hint() {
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
                "nightly-structured",
                "2026-03-17T08:00:00Z",
                &[sample_lane_with_target(
                    "nightly_pika",
                    "pre-merge-pika-rust",
                )],
            )
            .expect("queue nightly");

        let job = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim nightly job")
            .into_iter()
            .next()
            .expect("nightly lane");

        assert_eq!(
            job.structured_pikaci_target_id.as_deref(),
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
            .finish_branch_ci_lane_run(
                job.lane_run_id,
                job.claim_token,
                crate::ci_state::CiLaneStatus::Failed,
                "boom",
            )
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
            .finish_nightly_lane_run(
                job.lane_run_id,
                job.claim_token,
                crate::ci_state::CiLaneStatus::Failed,
                "boom",
            )
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
            .finish_branch_ci_lane_run(
                jobs[0].lane_run_id,
                jobs[0].claim_token,
                crate::ci_state::CiLaneStatus::Failed,
                "boom",
            )
            .expect("finish first lane");

        let runs = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list branch runs");
        assert_eq!(runs[0].status, ForgeCiStatus::Running);
        assert!(runs[0].finished_at.is_none());
        assert_eq!(runs[0].lanes[0].status, CiLaneStatus::Failed);
        assert_eq!(runs[0].lanes[1].status, CiLaneStatus::Running);

        store
            .finish_branch_ci_lane_run(
                jobs[1].lane_run_id,
                jobs[1].claim_token,
                crate::ci_state::CiLaneStatus::Success,
                "ok",
            )
            .expect("finish second lane");
        let runs = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list branch runs");
        assert_eq!(runs[0].status, ForgeCiStatus::Failed);
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
                crate::ci_state::CiLaneStatus::Success,
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
    fn queue_reasons_do_not_block_unrelated_claims() {
        let store = open_store();
        let running_branch = store
            .upsert_branch_record(&upsert_input("feature/running", "head-running"))
            .expect("insert running branch");
        let blocked_branch = store
            .upsert_branch_record(&upsert_input("feature/blocked", "head-blocked"))
            .expect("insert blocked branch");
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
        assert_eq!(ready_runs[0].lanes[0].status, CiLaneStatus::Running);
        assert_eq!(
            ready_runs[0].lanes[0].execution_reason,
            CiLaneExecutionReason::Running
        );

        store
            .finish_branch_ci_lane_run(
                running_job.lane_run_id,
                running_job.claim_token,
                crate::ci_state::CiLaneStatus::Success,
                "ok",
            )
            .expect("finish running holder");
        store
            .finish_branch_ci_lane_run(
                claimed[0].lane_run_id,
                claimed[0].claim_token,
                crate::ci_state::CiLaneStatus::Success,
                "ok",
            )
            .expect("finish claimed ready lane");
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
            .finish_branch_ci_lane_run(
                jobs[0].lane_run_id,
                jobs[0].claim_token,
                crate::ci_state::CiLaneStatus::Success,
                "ci ok",
            )
            .expect("persist ci lane result");
        store
            .mark_branch_merged(branch.branch_id, "npub1trusted", "merge999")
            .expect("mark merged");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("load branch detail")
            .expect("detail exists");
        assert_eq!(detail.branch_state, BranchState::Merged);
        assert_eq!(detail.merge_commit_sha.as_deref(), Some("merge999"));
        assert!(detail.tutorial_json.is_some());
        assert_eq!(detail.ci_status, ForgeCiStatus::Success);
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
        assert_eq!(branch_runs[0].status, ForgeCiStatus::Running);
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
        assert_eq!(old_branch_lane.status, CiLaneStatus::Queued);
        assert_eq!(old_branch_lane.retry_count, 1);
        assert!(old_branch_lane.log_text.is_none());
        assert!(old_branch_lane.pikaci_run_id.is_none());
        assert!(old_branch_lane.pikaci_target_id.is_none());
        assert_eq!(fresh_branch_lane.status, CiLaneStatus::Running);
        assert_eq!(fresh_branch_lane.retry_count, 0);

        let nightlies = store.list_recent_nightly_runs(4).expect("nightly feed");
        let nightly = store
            .get_nightly_run(nightlies[0].nightly_run_id)
            .expect("get nightly")
            .expect("nightly exists");
        assert_eq!(nightly.status, ForgeCiStatus::Running);
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
        assert_eq!(old_nightly_lane.status, CiLaneStatus::Queued);
        assert_eq!(old_nightly_lane.retry_count, 1);
        assert!(old_nightly_lane.log_text.is_none());
        assert!(old_nightly_lane.pikaci_run_id.is_none());
        assert!(old_nightly_lane.pikaci_target_id.is_none());
        assert_eq!(fresh_nightly_lane.status, CiLaneStatus::Running);
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
                crate::ci_state::CiLaneStatus::Failed,
                "retry failed before run_started",
            )
            .expect("finish retried job");

        let finished = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("list finished runs");
        let lane = &finished[0].lanes[0];
        assert_eq!(lane.status, CiLaneStatus::Failed);
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
                crate::ci_state::CiLaneStatus::Success,
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
                crate::ci_state::CiLaneStatus::Success,
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
                crate::ci_state::CiLaneStatus::Success,
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
                crate::ci_state::CiLaneStatus::Success,
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
                crate::ci_state::CiLaneStatus::Success,
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
                crate::ci_state::CiLaneStatus::Success,
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
            .finish_branch_ci_lane_run(
                jobs[0].lane_run_id,
                jobs[0].claim_token,
                crate::ci_state::CiLaneStatus::Success,
                "ok",
            )
            .expect("finish success lane");
        store
            .finish_branch_ci_lane_run(
                jobs[1].lane_run_id,
                jobs[1].claim_token,
                crate::ci_state::CiLaneStatus::Failed,
                "boom",
            )
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
        assert_eq!(lanes[0].status, CiLaneStatus::Success);
        assert_eq!(lanes[1].status, CiLaneStatus::Queued);
        assert_eq!(lanes[1].retry_count, 1);
        assert_eq!(lanes[2].status, CiLaneStatus::Queued);
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
                crate::ci_state::CiLaneStatus::Success,
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
        assert_eq!(nightly.lanes[0].status, CiLaneStatus::Success);
        assert_eq!(nightly.lanes[1].status, CiLaneStatus::Queued);
        assert_eq!(nightly.lanes[1].retry_count, 1);
    }
}
