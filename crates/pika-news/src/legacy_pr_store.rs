#![allow(dead_code)]

use anyhow::Context;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::storage::Store;

#[derive(Debug, Clone)]
pub struct PrUpsertInput {
    pub repo: String,
    pub pr_number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub head_sha: String,
    pub base_ref: String,
    pub author_login: Option<String>,
    pub updated_at: String,
    pub merged_at: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UpsertOutcome {
    pub inserted: bool,
    pub head_changed: bool,
    pub queued: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct GenerationJob {
    pub artifact_id: i64,
    pub pr_id: i64,
    pub repo: String,
    pub pr_number: i64,
    pub title: String,
    pub head_sha: String,
    pub base_ref: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FeedItem {
    pub pr_id: i64,
    pub repo: String,
    pub pr_number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub updated_at: String,
    pub generation_status: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PrDetailRecord {
    pub repo: String,
    pub pr_number: i64,
    pub title: String,
    pub url: String,
    pub pr_state: String,
    pub updated_at: String,
    pub base_ref: String,
    pub head_sha: String,
    pub generation_status: String,
    pub tutorial_json: Option<String>,
    pub unified_diff: Option<String>,
    pub claude_session_id: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PrSummaryRecord {
    pub repo: String,
    pub pr_number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub updated_at: String,
    pub generation_status: String,
    pub tutorial_json: Option<String>,
}

impl Store {
    pub fn upsert_pull_request(&self, input: &PrUpsertInput) -> anyhow::Result<UpsertOutcome> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start PR upsert transaction")?;
            let repo_id = ensure_repo(&tx, &input.repo)?;
            let (pr_id, previous_head_sha, inserted) = upsert_pr_row(&tx, repo_id, input)?;
            let mut outcome = upsert_artifact_row(&tx, pr_id, &input.head_sha)?;
            outcome.inserted = inserted;
            outcome.head_changed = previous_head_sha
                .as_deref()
                .map(|old| old != input.head_sha)
                .unwrap_or(false);
            tx.commit().context("commit PR upsert transaction")?;
            Ok(outcome)
        })
    }

    #[allow(dead_code)]
    pub fn update_poll_markers(
        &self,
        repo: &str,
        last_open_polled_at: Option<&str>,
        last_merged_polled_at: Option<&str>,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start poll marker update transaction")?;
            let repo_id = ensure_repo(&tx, repo)?;
            let now = Utc::now().to_rfc3339();
            tx.execute(
                "UPDATE repos
                 SET last_open_polled_at = COALESCE(?1, last_open_polled_at),
                     last_merged_polled_at = COALESCE(?2, last_merged_polled_at),
                     updated_at = ?3
                 WHERE id = ?4",
                params![last_open_polled_at, last_merged_polled_at, now, repo_id],
            )
            .with_context(|| format!("update poll markers for repo {}", repo))?;
            tx.commit()
                .context("commit poll marker update transaction")?;
            Ok(())
        })
    }

    pub fn claim_pending_generation_jobs(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<GenerationJob>> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start claim generation jobs transaction")?;

            let ids = {
                let mut stmt = tx
                    .prepare(
                        "SELECT av.id
                         FROM artifact_versions av
                         JOIN pull_requests pr ON pr.id = av.pr_id
                         WHERE av.status = 'pending'
                           AND (av.next_retry_at IS NULL OR av.next_retry_at <= CURRENT_TIMESTAMP)
                           AND pr.state IN ('open', 'merged')
                         ORDER BY av.created_at ASC
                         LIMIT ?1",
                    )
                    .context("prepare pending generation jobs query")?;
                let rows = stmt
                    .query_map(params![limit as i64], |row| row.get::<_, i64>(0))
                    .context("query pending generation jobs")?;
                let mut ids = Vec::new();
                for row in rows {
                    ids.push(row.context("read pending generation job id")?);
                }
                ids
            };

            let mut jobs = Vec::new();
            for artifact_id in ids {
                let claimed = tx
                    .execute(
                        "UPDATE artifact_versions
                         SET status = 'generating', updated_at = CURRENT_TIMESTAMP
                         WHERE id = ?1 AND status = 'pending'",
                        params![artifact_id],
                    )
                    .with_context(|| format!("claim artifact {}", artifact_id))?;
                if claimed == 0 {
                    continue;
                }

                let job = tx
                    .query_row(
                        "SELECT av.id, pr.id, r.repo, pr.pr_number, pr.title, av.source_head_sha, pr.base_ref
                         FROM artifact_versions av
                         JOIN pull_requests pr ON pr.id = av.pr_id
                         JOIN repos r ON r.id = pr.repo_id
                         WHERE av.id = ?1",
                        params![artifact_id],
                        |row| {
                            Ok(GenerationJob {
                                artifact_id: row.get(0)?,
                                pr_id: row.get(1)?,
                                repo: row.get(2)?,
                                pr_number: row.get(3)?,
                                title: row.get(4)?,
                                head_sha: row.get(5)?,
                                base_ref: row.get(6)?,
                            })
                        },
                    )
                    .with_context(|| format!("load claimed generation job {}", artifact_id))?;
                jobs.push(job);
            }

            tx.commit()
                .context("commit claim generation jobs transaction")?;
            Ok(jobs)
        })
    }

    pub fn mark_generation_ready(
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
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start mark_generation_ready transaction")?;
            let (pr_id, current_status, source_head_sha, artifact_version): (i64, String, String, i64) = tx
                .query_row(
                    "SELECT pr_id, status, source_head_sha, version
                     FROM artifact_versions
                     WHERE id = ?1",
                    params![artifact_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .with_context(|| format!("lookup pr_id for artifact {}", artifact_id))?;
            let current_pr_head_sha: String = tx
                .query_row(
                    "SELECT head_sha FROM pull_requests WHERE id = ?1",
                    params![pr_id],
                    |row| row.get(0),
                )
                .with_context(|| format!("lookup current head sha for pr_id {}", pr_id))?;
            let newer_non_superseded_exists: bool = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1
                        FROM artifact_versions
                        WHERE pr_id = ?1 AND version > ?2 AND status != 'superseded'
                    )",
                    params![pr_id, artifact_version],
                    |row| row.get(0),
                )
                .with_context(|| format!("check newer versions for artifact {}", artifact_id))?;
            let activated = current_status != "superseded"
                && source_head_sha == current_pr_head_sha
                && !newer_non_superseded_exists;
            if activated {
                tx.execute(
                    "UPDATE artifact_versions
                     SET is_current = 0, updated_at = CURRENT_TIMESTAMP
                     WHERE pr_id = ?1 AND id != ?2 AND is_current = 1",
                    params![pr_id, artifact_id],
                )
                .with_context(|| format!("retire previous current artifact for pr_id {}", pr_id))?;
                tx.execute(
                    "UPDATE artifact_versions
                     SET status='ready', tutorial_json=?1, html=?2, error_message=NULL, next_retry_at=NULL,
                         generated_head_sha=?3, unified_diff=?4, claude_session_id=?5,
                         is_current=1, ready_at=CURRENT_TIMESTAMP, updated_at=CURRENT_TIMESTAMP
                     WHERE id = ?6",
                    params![
                        tutorial_json,
                        html,
                        generated_head_sha,
                        unified_diff,
                        claude_session_id,
                        artifact_id
                    ],
                )
                .with_context(|| format!("mark artifact {} ready", artifact_id))?;
            } else {
                tx.execute(
                    "UPDATE artifact_versions
                     SET status='superseded', is_current=0, tutorial_json=?1, html=?2, error_message=NULL, next_retry_at=NULL,
                         generated_head_sha=?3, unified_diff=?4, claude_session_id=?5, updated_at=CURRENT_TIMESTAMP
                     WHERE id = ?6",
                    params![
                        tutorial_json,
                        html,
                        generated_head_sha,
                        unified_diff,
                        claude_session_id,
                        artifact_id
                    ],
                )
                .with_context(|| format!("store superseded artifact payload {}", artifact_id))?;
            }
            tx.commit()
                .with_context(|| format!("commit mark_generation_ready for artifact {}", artifact_id))?;
            Ok(activated)
        })
    }

    pub fn mark_generation_failed(
        &self,
        artifact_id: i64,
        error_message: &str,
        retry_safe: bool,
        retry_backoff_secs: u64,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let current_retry: i64 = conn
                .query_row(
                    "SELECT retry_count FROM artifact_versions WHERE id = ?1",
                    params![artifact_id],
                    |row| row.get(0),
                )
                .with_context(|| format!("read retry_count for artifact {}", artifact_id))?;
            let current_status: String = conn
                .query_row(
                    "SELECT status FROM artifact_versions WHERE id = ?1",
                    params![artifact_id],
                    |row| row.get(0),
                )
                .with_context(|| format!("read status for artifact {}", artifact_id))?;

            if current_status == "superseded" {
                return Ok(());
            }

            let next_retry_count = current_retry + 1;
            let should_retry = retry_safe && next_retry_count <= 5;
            if should_retry {
                let modifier = format!("+{} seconds", retry_backoff_secs);
                conn.execute(
                    "UPDATE artifact_versions
                     SET status='pending', error_message=?1, retry_count=?2,
                         next_retry_at=datetime('now', ?3), updated_at=CURRENT_TIMESTAMP
                     WHERE id = ?4",
                    params![error_message, next_retry_count, modifier, artifact_id],
                )
                .with_context(|| format!("schedule retry for artifact {}", artifact_id))?;
            } else {
                conn.execute(
                    "UPDATE artifact_versions
                     SET status='failed', error_message=?1, retry_count=?2, next_retry_at=NULL, updated_at=CURRENT_TIMESTAMP
                     WHERE id = ?3",
                    params![error_message, next_retry_count, artifact_id],
                )
                .with_context(|| format!("mark artifact {} failed", artifact_id))?;
            }
            Ok(())
        })
    }

    /// Mark any PRs stored as "open" for this repo as "closed" if their
    /// pr_number is not in `current_open_numbers`. Returns the count updated.
    pub fn close_stale_open_prs(
        &self,
        repo: &str,
        current_open_numbers: &[i64],
    ) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let repo_id: Option<i64> = conn
                .query_row(
                    "SELECT id FROM repos WHERE repo = ?1",
                    params![repo],
                    |row| row.get(0),
                )
                .optional()
                .context("lookup repo for stale-close")?;

            let repo_id = match repo_id {
                Some(id) => id,
                None => return Ok(0),
            };

            if current_open_numbers.is_empty() {
                let rows = conn
                    .execute(
                        "UPDATE pull_requests SET state='closed', last_synced_at=CURRENT_TIMESTAMP
                         WHERE repo_id = ?1 AND state = 'open'",
                        params![repo_id],
                    )
                    .context("close all stale open PRs")?;
                return Ok(rows);
            }

            let placeholders: Vec<String> = current_open_numbers
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 2))
                .collect();
            let sql = format!(
                "UPDATE pull_requests SET state='closed', last_synced_at=CURRENT_TIMESTAMP
                 WHERE repo_id = ?1 AND state = 'open' AND pr_number NOT IN ({})",
                placeholders.join(", ")
            );

            let mut stmt = conn.prepare(&sql).context("prepare stale-close query")?;
            let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            param_values.push(Box::new(repo_id));
            for n in current_open_numbers {
                param_values.push(Box::new(*n));
            }
            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|p| p.as_ref()).collect();
            let rows = stmt
                .execute(param_refs.as_slice())
                .context("close stale open PRs")?;
            Ok(rows)
        })
    }

    pub fn list_feed_items(&self) -> anyhow::Result<Vec<FeedItem>> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT pr.id, r.repo, pr.pr_number, pr.title, pr.url, pr.state, pr.updated_at, latest.status
                     FROM pull_requests pr
                     JOIN repos r ON r.id = pr.repo_id
                     LEFT JOIN artifact_versions latest ON latest.id = (
                        SELECT av.id
                        FROM artifact_versions av
                        WHERE av.pr_id = pr.id AND av.status != 'superseded'
                        ORDER BY av.version DESC
                        LIMIT 1
                     )
                     WHERE pr.state IN ('open', 'merged')
                     ORDER BY CASE pr.state WHEN 'open' THEN 0 WHEN 'merged' THEN 1 ELSE 2 END, pr.updated_at DESC",
                )
                .context("prepare feed query")?;

            let rows = stmt
                .query_map([], |row| {
                    Ok(FeedItem {
                        pr_id: row.get(0)?,
                        repo: row.get(1)?,
                        pr_number: row.get(2)?,
                        title: row.get(3)?,
                        url: row.get(4)?,
                        state: row.get(5)?,
                        updated_at: row.get(6)?,
                        generation_status: row
                            .get::<_, Option<String>>(7)?
                            .unwrap_or_else(|| "pending".to_string()),
                    })
                })
                .context("query feed items")?;

            let mut items = Vec::new();
            for row in rows {
                items.push(row.context("read feed item row")?);
            }
            Ok(items)
        })
    }

    pub fn list_pr_summaries(
        &self,
        since_pr: Option<i64>,
        since_date: Option<&str>,
    ) -> anyhow::Result<Vec<PrSummaryRecord>> {
        self.with_connection(|conn| {
            let mut conditions = vec!["pr.state IN ('open', 'merged')".to_string()];
            let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

            if let Some(pr_num) = since_pr {
                conditions.push(format!("pr.pr_number >= ?{}", param_values.len() + 1));
                param_values.push(Box::new(pr_num));
            }
            if let Some(date) = since_date {
                conditions.push(format!("pr.updated_at >= ?{}", param_values.len() + 1));
                param_values.push(Box::new(date.to_string()));
            }

            let where_clause = conditions.join(" AND ");
            let sql = format!(
                "SELECT r.repo, pr.pr_number, pr.title, pr.url, pr.state, pr.updated_at,
                        COALESCE(latest.status, 'pending'),
                        CASE
                            WHEN COALESCE(latest.status, 'pending') = 'ready'
                                THEN current_ready.tutorial_json
                            ELSE NULL
                        END
                 FROM pull_requests pr
                 JOIN repos r ON r.id = pr.repo_id
                 LEFT JOIN artifact_versions latest ON latest.id = (
                    SELECT av.id
                    FROM artifact_versions av
                    WHERE av.pr_id = pr.id AND av.status != 'superseded'
                    ORDER BY av.version DESC
                    LIMIT 1
                 )
                 LEFT JOIN artifact_versions current_ready ON current_ready.id = (
                    SELECT av.id
                    FROM artifact_versions av
                    WHERE av.pr_id = pr.id AND av.is_current = 1 AND av.status = 'ready'
                    ORDER BY av.version DESC
                    LIMIT 1
                 )
                 WHERE {}
                 ORDER BY pr.pr_number DESC",
                where_clause
            );

            let mut stmt = conn.prepare(&sql).context("prepare pr summaries query")?;
            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|p| p.as_ref()).collect();
            let rows = stmt
                .query_map(params_ref.as_slice(), |row| {
                    Ok(PrSummaryRecord {
                        repo: row.get(0)?,
                        pr_number: row.get(1)?,
                        title: row.get(2)?,
                        url: row.get(3)?,
                        state: row.get(4)?,
                        updated_at: row.get(5)?,
                        generation_status: row.get(6)?,
                        tutorial_json: row.get(7)?,
                    })
                })
                .context("query pr summaries")?;

            let mut items = Vec::new();
            for row in rows {
                items.push(row.context("read pr summary row")?);
            }
            Ok(items)
        })
    }

    #[allow(dead_code)]
    pub fn get_pr_detail(&self, pr_id: i64) -> anyhow::Result<Option<PrDetailRecord>> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT r.repo, pr.pr_number, pr.title, pr.url, pr.state, pr.updated_at, pr.base_ref, pr.head_sha,
                        COALESCE(latest.status, 'pending'),
                        current_ready.tutorial_json,
                        current_ready.unified_diff,
                        current_ready.claude_session_id,
                        latest.error_message
                 FROM pull_requests pr
                 JOIN repos r ON r.id = pr.repo_id
                 LEFT JOIN artifact_versions latest ON latest.id = (
                    SELECT av.id
                    FROM artifact_versions av
                    WHERE av.pr_id = pr.id AND av.status != 'superseded'
                    ORDER BY av.version DESC
                    LIMIT 1
                 )
                 LEFT JOIN artifact_versions current_ready ON current_ready.id = (
                    SELECT av.id
                    FROM artifact_versions av
                    WHERE av.pr_id = pr.id AND av.is_current = 1 AND av.status = 'ready'
                    ORDER BY av.version DESC
                    LIMIT 1
                 )
                 WHERE pr.id = ?1",
                params![pr_id],
                |row| {
                    Ok(PrDetailRecord {
                        repo: row.get(0)?,
                        pr_number: row.get(1)?,
                        title: row.get(2)?,
                        url: row.get(3)?,
                        pr_state: row.get(4)?,
                        updated_at: row.get(5)?,
                        base_ref: row.get(6)?,
                        head_sha: row.get(7)?,
                        generation_status: row.get(8)?,
                        tutorial_json: row.get(9)?,
                        unified_diff: row.get(10)?,
                        claude_session_id: row.get(11)?,
                        error_message: row.get(12)?,
                    })
                },
            )
            .optional()
            .context("query PR detail")
        })
    }

    /// Reset any artifacts stuck in `generating` state back to `pending`.
    /// Called on startup to recover from unclean shutdowns.
    pub fn recover_stale_generating(&self) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let rows = conn
                .execute(
                    "UPDATE artifact_versions
                     SET status='pending', updated_at=CURRENT_TIMESTAMP
                     WHERE status='generating'",
                    [],
                )
                .context("recover stale generating artifacts")?;
            Ok(rows)
        })
    }

    pub fn queue_regeneration(&self, pr_id: i64) -> anyhow::Result<bool> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start regenerate transaction")?;
            let current_head_sha: Option<String> = tx
                .query_row(
                    "SELECT head_sha FROM pull_requests WHERE id = ?1",
                    params![pr_id],
                    |row| row.get(0),
                )
                .optional()
                .context("lookup PR for regeneration")?;
            let Some(current_head_sha) = current_head_sha else {
                return Ok(false);
            };

            if has_inflight_generation_for_head(&tx, pr_id, &current_head_sha)? {
                tx.commit().context("commit no-op regenerate transaction")?;
                return Ok(true);
            }

            supersede_non_current_generations(&tx, pr_id)?;
            insert_pending_artifact_version(&tx, pr_id, &current_head_sha)?;
            tx.commit().context("commit regenerate transaction")?;
            Ok(true)
        })
    }
}

fn ensure_repo(conn: &Connection, repo: &str) -> anyhow::Result<i64> {
    conn.execute(
        "INSERT INTO repos(repo, updated_at) VALUES (?1, CURRENT_TIMESTAMP)
         ON CONFLICT(repo) DO UPDATE SET updated_at=CURRENT_TIMESTAMP",
        params![repo],
    )
    .with_context(|| format!("ensure repo row {}", repo))?;

    let repo_id = conn
        .query_row(
            "SELECT id FROM repos WHERE repo = ?1",
            params![repo],
            |row| row.get(0),
        )
        .with_context(|| format!("resolve repo id for {}", repo))?;
    Ok(repo_id)
}

fn upsert_pr_row(
    conn: &Connection,
    repo_id: i64,
    input: &PrUpsertInput,
) -> anyhow::Result<(i64, Option<String>, bool)> {
    let existing: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, head_sha FROM pull_requests WHERE repo_id = ?1 AND pr_number = ?2",
            params![repo_id, input.pr_number],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("lookup existing pull request")?;

    if let Some((pr_id, previous_head_sha)) = existing {
        conn.execute(
            "UPDATE pull_requests
             SET title=?1, url=?2, state=?3, head_sha=?4, base_ref=?5, author_login=?6, updated_at=?7, merged_at=?8, last_synced_at=CURRENT_TIMESTAMP
             WHERE id = ?9",
            params![
                input.title,
                input.url,
                input.state,
                input.head_sha,
                input.base_ref,
                input.author_login,
                input.updated_at,
                input.merged_at,
                pr_id
            ],
        )
        .with_context(|| format!("update PR {} for repo_id {}", input.pr_number, repo_id))?;

        Ok((pr_id, Some(previous_head_sha), false))
    } else {
        conn.execute(
            "INSERT INTO pull_requests(repo_id, pr_number, title, url, state, head_sha, base_ref, author_login, updated_at, merged_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                repo_id,
                input.pr_number,
                input.title,
                input.url,
                input.state,
                input.head_sha,
                input.base_ref,
                input.author_login,
                input.updated_at,
                input.merged_at
            ],
        )
        .with_context(|| format!("insert PR {} for repo_id {}", input.pr_number, repo_id))?;

        let pr_id = conn.last_insert_rowid();
        Ok((pr_id, None, true))
    }
}

fn upsert_artifact_row(
    conn: &Connection,
    pr_id: i64,
    head_sha: &str,
) -> anyhow::Result<UpsertOutcome> {
    let latest: Option<(String, String)> = conn
        .query_row(
            "SELECT status, source_head_sha
             FROM artifact_versions
             WHERE pr_id = ?1
             ORDER BY version DESC
             LIMIT 1",
            params![pr_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("lookup latest artifact version")?;

    match latest {
        None => {
            insert_pending_artifact_version(conn, pr_id, head_sha)?;
            Ok(UpsertOutcome {
                inserted: false,
                head_changed: false,
                queued: true,
            })
        }
        Some((status, current_head_sha))
            if current_head_sha == head_sha
                && matches!(
                    status.as_str(),
                    "pending" | "generating" | "ready" | "failed"
                ) =>
        {
            Ok(UpsertOutcome {
                inserted: false,
                head_changed: false,
                queued: false,
            })
        }
        _ => {
            supersede_non_current_generations(conn, pr_id)?;
            insert_pending_artifact_version(conn, pr_id, head_sha)?;
            Ok(UpsertOutcome {
                inserted: false,
                head_changed: false,
                queued: true,
            })
        }
    }
}

fn insert_pending_artifact_version(
    conn: &Connection,
    pr_id: i64,
    head_sha: &str,
) -> anyhow::Result<i64> {
    let next_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM artifact_versions WHERE pr_id = ?1",
            params![pr_id],
            |row| row.get(0),
        )
        .context("compute next artifact version")?;
    conn.execute(
        "INSERT INTO artifact_versions(
            pr_id, version, source_head_sha, status, generated_head_sha,
            created_at, updated_at
         ) VALUES (?1, ?2, ?3, 'pending', NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        params![pr_id, next_version, head_sha],
    )
    .with_context(|| {
        format!(
            "insert pending artifact version {} for pr_id {}",
            next_version, pr_id
        )
    })?;
    Ok(conn.last_insert_rowid())
}

fn supersede_non_current_generations(conn: &Connection, pr_id: i64) -> anyhow::Result<usize> {
    conn.execute(
        "UPDATE artifact_versions
         SET status = 'superseded', next_retry_at = NULL, updated_at = CURRENT_TIMESTAMP
         WHERE pr_id = ?1
           AND is_current = 0
           AND status IN ('pending', 'generating', 'failed')",
        params![pr_id],
    )
    .with_context(|| format!("supersede non-current generations for pr_id {}", pr_id))
}

fn has_inflight_generation_for_head(
    conn: &Connection,
    pr_id: i64,
    head_sha: &str,
) -> anyhow::Result<bool> {
    let exists = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM artifact_versions
                WHERE pr_id = ?1
                  AND source_head_sha = ?2
                  AND status IN ('pending', 'generating')
            )",
            params![pr_id, head_sha],
            |row| row.get(0),
        )
        .context("check inflight generation for head sha")?;
    Ok(exists)
}
