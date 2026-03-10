use std::cmp::{max, min};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use nostr::key::PublicKey;
use nostr::nips::nip19::ToBech32;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct Store {
    db_path: PathBuf,
}

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

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let db_path = path.to_path_buf();
        let mut conn = open_connection(&db_path)?;
        run_migrations(&mut conn)?;
        Ok(Self { db_path })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn with_connection<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut conn = open_connection(&self.db_path)?;
        f(&mut conn)
    }

    pub fn upsert_pull_request(&self, input: &PrUpsertInput) -> anyhow::Result<UpsertOutcome> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("start upsert transaction")?;

            let repo_id = ensure_repo(&tx, &input.repo)?;
            let (pr_id, previous_head_sha, inserted) = upsert_pr_row(&tx, repo_id, input)?;
            let mut outcome = upsert_artifact_row(&tx, pr_id, &input.head_sha)?;
            outcome.inserted = inserted;
            outcome.head_changed = previous_head_sha
                .as_deref()
                .map(|previous| previous != input.head_sha)
                .unwrap_or(false);

            tx.commit().context("commit upsert transaction")?;
            Ok(outcome)
        })
    }

    pub fn update_poll_markers(
        &self,
        repo: &str,
        open_cursor: Option<&str>,
        merged_cursor: Option<&str>,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start poll-marker transaction")?;
            let repo_id = ensure_repo(&tx, repo)?;
            let now = Utc::now().to_rfc3339();

            tx.execute(
                "INSERT INTO poll_markers(repo_id, last_polled_open_cursor, last_polled_merged_cursor, last_polled_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(repo_id) DO UPDATE SET
                    last_polled_open_cursor=excluded.last_polled_open_cursor,
                    last_polled_merged_cursor=excluded.last_polled_merged_cursor,
                    last_polled_at=excluded.last_polled_at,
                    updated_at=excluded.updated_at",
                params![repo_id, open_cursor, merged_cursor, now],
            )
            .with_context(|| format!("update poll markers for {}", repo))?;

            tx.commit().context("commit poll-marker transaction")?;
            Ok(())
        })
    }

    pub fn claim_pending_generation_jobs(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<GenerationJob>> {
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start generation-claim transaction")?;

            let mut jobs = Vec::new();
            {
                let mut stmt = tx
                    .prepare(
                        "SELECT av.id, r.repo, pr.pr_number, pr.title, av.source_head_sha, pr.base_ref, pr.id
                         FROM artifact_versions av
                         JOIN pull_requests pr ON pr.id = av.pr_id
                         JOIN repos r ON r.id = pr.repo_id
                         WHERE av.status = 'pending'
                           AND (av.next_retry_at IS NULL OR av.next_retry_at <= CURRENT_TIMESTAMP)
                         ORDER BY av.updated_at ASC
                         LIMIT ?1",
                    )
                    .context("prepare generation-claim query")?;

                let rows = stmt
                    .query_map(params![limit as i64], |row| {
                        Ok(GenerationJob {
                            artifact_id: row.get(0)?,
                            repo: row.get(1)?,
                            pr_number: row.get(2)?,
                            title: row.get(3)?,
                            head_sha: row.get(4)?,
                            base_ref: row.get(5)?,
                            pr_id: row.get(6)?,
                        })
                    })
                    .context("query pending generation jobs")?;

                for row in rows {
                    let job = row.context("read generation job row")?;
                    tx.execute(
                        "UPDATE artifact_versions
                         SET status='generating', updated_at=CURRENT_TIMESTAMP
                         WHERE id = ?1 AND status = 'pending'",
                        params![job.artifact_id],
                    )
                    .with_context(|| format!("mark artifact {} as generating", job.artifact_id))?;
                    jobs.push(job);
                }
            }

            tx.commit().context("commit generation claim transaction")?;
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
                .unchecked_transaction()
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

            // Build a comma-separated placeholder list for the IN clause.
            if current_open_numbers.is_empty() {
                // All open PRs are stale.
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

    pub fn get_artifact_session_id(&self, pr_id: i64) -> anyhow::Result<Option<String>> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT claude_session_id
                 FROM artifact_versions
                 WHERE pr_id = ?1 AND is_current = 1 AND status = 'ready'
                 ORDER BY version DESC
                 LIMIT 1",
                params![pr_id],
                |row| row.get(0),
            )
            .optional()
            .context("query artifact session id")
            .map(|opt| opt.flatten())
        })
    }

    pub fn get_or_create_chat_session(
        &self,
        pr_id: i64,
        npub: &str,
        claude_session_id: &str,
    ) -> anyhow::Result<(i64, Vec<ChatMessage>)> {
        self.with_connection(|conn| {
            let artifact_id: i64 = conn
                .query_row(
                    "SELECT id
                     FROM artifact_versions
                     WHERE pr_id = ?1 AND is_current = 1 AND status = 'ready'
                     ORDER BY version DESC
                     LIMIT 1",
                    params![pr_id],
                    |row| row.get(0),
                )
                .context("lookup artifact for chat session")?;

            let existing: Option<i64> = conn
                .query_row(
                    "SELECT id FROM chat_sessions WHERE artifact_id = ?1 AND npub = ?2",
                    params![artifact_id, npub],
                    |row| row.get(0),
                )
                .optional()
                .context("lookup existing chat session")?;

            let session_id = match existing {
                Some(id) => id,
                None => {
                    conn.execute(
                        "INSERT INTO chat_sessions(artifact_id, npub, claude_session_id) VALUES (?1, ?2, ?3)",
                        params![artifact_id, npub, claude_session_id],
                    )
                    .context("insert chat session")?;
                    conn.last_insert_rowid()
                }
            };

            let mut stmt = conn
                .prepare(
                    "SELECT role, content, created_at FROM chat_messages WHERE session_id = ?1 ORDER BY created_at ASC",
                )
                .context("prepare chat messages query")?;

            let rows = stmt
                .query_map(params![session_id], |row| {
                    Ok(ChatMessage {
                        role: row.get(0)?,
                        content: row.get(1)?,
                        created_at: row.get(2)?,
                    })
                })
                .context("query chat messages")?;

            let mut messages = Vec::new();
            for row in rows {
                messages.push(row.context("read chat message row")?);
            }

            Ok((session_id, messages))
        })
    }

    pub fn get_chat_claude_session_id(&self, session_id: i64) -> anyhow::Result<String> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT claude_session_id FROM chat_sessions WHERE id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .context("query chat claude session id")
        })
    }

    pub fn update_chat_claude_session_id(
        &self,
        session_id: i64,
        claude_session_id: &str,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            conn.execute(
                "UPDATE chat_sessions SET claude_session_id = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
                params![claude_session_id, session_id],
            )
            .context("update chat claude session id")?;
            Ok(())
        })
    }

    pub fn append_chat_message(
        &self,
        session_id: i64,
        role: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO chat_messages(session_id, role, content) VALUES (?1, ?2, ?3)",
                params![session_id, role, content],
            )
            .context("insert chat message")?;
            Ok(())
        })
    }

    // --- Inbox methods ---

    pub fn populate_inbox(&self, artifact_id: i64, npubs: &[String]) -> anyhow::Result<usize> {
        if npubs.is_empty() {
            return Ok(0);
        }
        self.with_connection(|conn| insert_inbox_rows_for_artifact(conn, artifact_id, npubs))
    }

    pub fn list_inbox(
        &self,
        npub: &str,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<InboxItem>> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT av.pr_id, r.repo, pr.pr_number, pr.title, pr.url, pr.state, pr.updated_at,
                            COALESCE(latest.status, av.status), aus.reason, aus.created_at
                     FROM artifact_user_states aus
                     JOIN artifact_versions av ON av.id = aus.artifact_id
                     JOIN pull_requests pr ON pr.id = av.pr_id
                     JOIN repos r ON r.id = pr.repo_id
                     LEFT JOIN artifact_versions latest ON latest.id = (
                        SELECT av2.id
                        FROM artifact_versions av2
                        WHERE av2.pr_id = pr.id AND av2.status != 'superseded'
                        ORDER BY av2.version DESC
                        LIMIT 1
                     )
                     WHERE aus.npub = ?1 AND aus.state = 'inbox'
                     ORDER BY aus.created_at ASC, av.pr_id ASC
                     LIMIT ?2 OFFSET ?3",
                )
                .context("prepare inbox query")?;

            let rows = stmt
                .query_map(params![npub, limit, offset], |row| {
                    Ok(InboxItem {
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
                        reason: row.get(8)?,
                        inbox_created_at: row.get(9)?,
                    })
                })
                .context("query inbox")?;

            let mut items = Vec::new();
            for row in rows {
                items.push(row.context("read inbox row")?);
            }
            Ok(items)
        })
    }

    pub fn inbox_count(&self, npub: &str) -> anyhow::Result<i64> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM artifact_user_states WHERE npub = ?1 AND state = 'inbox'",
                params![npub],
                |row| row.get(0),
            )
            .context("count inbox")
        })
    }

    pub fn dismiss_inbox_items(&self, npub: &str, pr_ids: &[i64]) -> anyhow::Result<usize> {
        if pr_ids.is_empty() {
            return Ok(0);
        }
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start dismiss inbox transaction")?;
            let placeholders: Vec<String> = pr_ids
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 2))
                .collect();
            let update_sql = format!(
                "UPDATE artifact_user_states
                 SET state = 'dismissed', dismissed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
                 WHERE npub = ?1
                   AND state = 'inbox'
                   AND artifact_id IN (
                       SELECT id FROM artifact_versions WHERE pr_id IN ({})
                   )",
                placeholders.join(", ")
            );
            let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            param_values.push(Box::new(npub.to_string()));
            for id in pr_ids {
                param_values.push(Box::new(*id));
            }
            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|p| p.as_ref()).collect();
            let rows = tx
                .prepare(&update_sql)
                .context("prepare dismiss query")?
                .execute(param_refs.as_slice())
                .context("dismiss inbox items")?;
            tx.commit().context("commit dismiss inbox transaction")?;
            Ok(rows)
        })
    }

    pub fn dismiss_all_inbox(&self, npub: &str) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let rows = conn
                .execute(
                    "UPDATE artifact_user_states
                     SET state = 'dismissed', dismissed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
                     WHERE npub = ?1 AND state = 'inbox'",
                    params![npub],
                )
                .context("dismiss all inbox")?;
            Ok(rows)
        })
    }

    pub fn inbox_review_context(
        &self,
        npub: &str,
        pr_id: i64,
    ) -> anyhow::Result<Option<InboxReviewContext>> {
        self.with_connection(|conn| {
            let current = conn
                .query_row(
                    "SELECT aus.created_at, av.pr_id
                     FROM artifact_user_states aus
                     JOIN artifact_versions av ON av.id = aus.artifact_id
                     WHERE aus.npub = ?1 AND aus.state = 'inbox' AND av.pr_id = ?2
                     LIMIT 1",
                    params![npub, pr_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .context("lookup inbox review item")?;

            let Some((created_at, current_pr_id)) = current else {
                return Ok(None);
            };

            let prev: Option<i64> = conn
                .query_row(
                    "SELECT av.pr_id
                     FROM artifact_user_states aus
                     JOIN artifact_versions av ON av.id = aus.artifact_id
                     WHERE aus.npub = ?1
                       AND aus.state = 'inbox'
                       AND (
                           aus.created_at < ?2
                           OR (aus.created_at = ?2 AND av.pr_id < ?3)
                       )
                     ORDER BY aus.created_at DESC, av.pr_id DESC
                     LIMIT 1",
                    params![npub, created_at, current_pr_id],
                    |row| row.get(0),
                )
                .optional()
                .context("inbox prev neighbor")?;

            let next: Option<i64> = conn
                .query_row(
                    "SELECT av.pr_id
                     FROM artifact_user_states aus
                     JOIN artifact_versions av ON av.id = aus.artifact_id
                     WHERE aus.npub = ?1
                       AND aus.state = 'inbox'
                       AND (
                           aus.created_at > ?2
                           OR (aus.created_at = ?2 AND av.pr_id > ?3)
                       )
                     ORDER BY aus.created_at ASC, av.pr_id ASC
                     LIMIT 1",
                    params![npub, created_at, current_pr_id],
                    |row| row.get(0),
                )
                .optional()
                .context("inbox next neighbor")?;

            let position: i64 = conn
                .query_row(
                    "SELECT COUNT(*)
                     FROM artifact_user_states aus
                     JOIN artifact_versions av ON av.id = aus.artifact_id
                     WHERE aus.npub = ?1
                       AND aus.state = 'inbox'
                       AND (
                           aus.created_at < ?2
                           OR (aus.created_at = ?2 AND av.pr_id <= ?3)
                       )",
                    params![npub, created_at, current_pr_id],
                    |row| row.get(0),
                )
                .context("count inbox review position")?;

            let total: i64 = conn
                .query_row(
                    "SELECT COUNT(*)
                     FROM artifact_user_states
                     WHERE npub = ?1 AND state = 'inbox'",
                    params![npub],
                    |row| row.get(0),
                )
                .context("count inbox review total")?;

            Ok(Some(InboxReviewContext {
                prev,
                next,
                position,
                total,
            }))
        })
    }

    // --- Chat allowlist methods ---

    pub fn is_chat_allowlist_active(&self, npub: &str) -> anyhow::Result<bool> {
        self.with_connection(|conn| {
            let active = conn
                .query_row(
                    "SELECT 1 FROM chat_allowlist WHERE npub = ?1 AND active = 1",
                    params![npub],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .context("query active chat allowlist entry")?
                .is_some();
            Ok(active)
        })
    }

    pub fn has_active_chat_allowlist_entries(&self) -> anyhow::Result<bool> {
        self.with_connection(|conn| {
            let active = conn
                .query_row(
                    "SELECT 1 FROM chat_allowlist WHERE active = 1 LIMIT 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .context("query active chat allowlist entries")?
                .is_some();
            Ok(active)
        })
    }

    pub fn list_chat_allowlist_entries(&self) -> anyhow::Result<Vec<ChatAllowlistEntry>> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT npub, active, note, updated_by, updated_at
                     FROM chat_allowlist
                     ORDER BY npub ASC",
                )
                .context("prepare chat allowlist list query")?;

            let rows = stmt
                .query_map([], |row| {
                    Ok(ChatAllowlistEntry {
                        npub: row.get(0)?,
                        active: row.get::<_, i64>(1)? != 0,
                        note: row.get(2)?,
                        updated_by: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                })
                .context("query chat allowlist list")?;

            let mut entries = Vec::new();
            for row in rows {
                entries.push(row.context("read chat allowlist row")?);
            }
            Ok(entries)
        })
    }

    pub fn list_active_chat_allowlist_npubs(&self) -> anyhow::Result<Vec<String>> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT npub FROM chat_allowlist WHERE active = 1 ORDER BY npub ASC")
                .context("prepare active chat allowlist query")?;

            let rows = stmt
                .query_map([], |row| row.get(0))
                .context("query active chat allowlist")?;

            let mut npubs = Vec::new();
            for row in rows {
                npubs.push(row.context("read active chat allowlist row")?);
            }
            Ok(npubs)
        })
    }

    pub fn get_chat_allowlist_entry(
        &self,
        npub: &str,
    ) -> anyhow::Result<Option<ChatAllowlistEntry>> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT npub, active, note, updated_by, updated_at
                 FROM chat_allowlist
                 WHERE npub = ?1",
                params![npub],
                |row| {
                    Ok(ChatAllowlistEntry {
                        npub: row.get(0)?,
                        active: row.get::<_, i64>(1)? != 0,
                        note: row.get(2)?,
                        updated_by: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                },
            )
            .optional()
            .context("query chat allowlist entry")
        })
    }

    pub fn upsert_chat_allowlist_entry(
        &self,
        npub: &str,
        active: bool,
        note: Option<&str>,
        updated_by: &str,
    ) -> anyhow::Result<ChatAllowlistEntry> {
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start chat allowlist transaction")?;

            tx.execute(
                "INSERT INTO chat_allowlist(npub, active, note, updated_by, updated_at)
                 VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
                 ON CONFLICT(npub) DO UPDATE SET
                    active = excluded.active,
                    note = excluded.note,
                    updated_by = excluded.updated_by,
                    updated_at = CURRENT_TIMESTAMP",
                params![npub, if active { 1 } else { 0 }, note, updated_by],
            )
            .context("upsert chat allowlist entry")?;

            let action = if active {
                "upsert_active"
            } else {
                "upsert_inactive"
            };
            tx.execute(
                "INSERT INTO chat_allowlist_audit(actor_npub, target_npub, action, note)
                 VALUES (?1, ?2, ?3, ?4)",
                params![updated_by, npub, action, note],
            )
            .context("insert chat allowlist audit row")?;

            let entry = tx
                .query_row(
                    "SELECT npub, active, note, updated_by, updated_at
                     FROM chat_allowlist
                     WHERE npub = ?1",
                    params![npub],
                    |row| {
                        Ok(ChatAllowlistEntry {
                            npub: row.get(0)?,
                            active: row.get::<_, i64>(1)? != 0,
                            note: row.get(2)?,
                            updated_by: row.get(3)?,
                            updated_at: row.get(4)?,
                        })
                    },
                )
                .context("reload chat allowlist entry")?;

            tx.commit().context("commit chat allowlist transaction")?;
            Ok(entry)
        })
    }

    pub fn backfill_inbox_for_npub(&self, npub: &str) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start inbox backfill transaction")?;
            let artifact_ids: Vec<i64> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT id
                         FROM artifact_versions
                         WHERE is_current = 1 AND status = 'ready'
                         ORDER BY version ASC",
                    )
                    .context("prepare current artifact scan for backfill")?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, i64>(0))
                    .context("query current artifacts for backfill")?;
                let mut artifact_ids = Vec::new();
                for row in rows {
                    artifact_ids.push(row.context("read current artifact row")?);
                }
                artifact_ids
            };
            let recipient = npub.to_string();
            let mut inserted = 0;
            for artifact_id in artifact_ids {
                inserted += insert_inbox_rows_for_artifact(
                    &tx,
                    artifact_id,
                    std::slice::from_ref(&recipient),
                )?;
            }
            tx.commit().context("commit inbox backfill transaction")?;
            Ok(inserted)
        })
    }

    #[cfg(test)]
    fn rekey_inbox_npub(&self, from_npub: &str, to_npub: &str) -> anyhow::Result<usize> {
        if from_npub == to_npub {
            return Ok(0);
        }
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start inbox rekey transaction")?;

            let merged = merge_inbox_state_alias(&tx, from_npub, to_npub)
                .context("merge inbox state rows for rekey")?;

            tx.commit().context("commit inbox rekey transaction")?;
            Ok(merged)
        })
    }

    pub fn canonicalize_inbox_npubs(&self) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start inbox canonicalization transaction")?;

            let raw_npubs: Vec<String> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT DISTINCT npub
                         FROM artifact_user_states
                         ORDER BY npub ASC",
                    )
                    .context("prepare inbox owner scan")?;
                let rows = stmt
                    .query_map([], |row| row.get(0))
                    .context("query inbox owners")?;

                let mut values = Vec::new();
                for row in rows {
                    values.push(row.context("read inbox owner row")?);
                }
                values
            };

            let mut rekeyed = 0;
            for raw_npub in raw_npubs {
                let canonical_npub = match PublicKey::parse(raw_npub.trim()) {
                    Ok(pk) => match pk.to_bech32() {
                        Ok(npub) => npub.to_lowercase(),
                        Err(_) => continue,
                    },
                    Err(_) => continue,
                };

                if canonical_npub == raw_npub {
                    continue;
                }

                if merge_inbox_state_alias(&tx, &raw_npub, &canonical_npub).with_context(|| {
                    format!(
                        "merge inbox owner {} into canonical {}",
                        raw_npub, canonical_npub
                    )
                })? > 0
                {
                    rekeyed += 1;
                }
            }

            tx.commit()
                .context("commit inbox canonicalization transaction")?;
            Ok(rekeyed)
        })
    }

    // --- Auth token methods ---

    pub fn insert_auth_token(&self, token: &str, npub: &str) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO auth_tokens(token, npub) VALUES (?1, ?2)",
                params![token, npub],
            )
            .context("insert auth token")?;
            Ok(())
        })
    }

    pub fn validate_auth_token(
        &self,
        token: &str,
        ttl_days: i64,
    ) -> anyhow::Result<Option<String>> {
        self.with_connection(|conn| {
            let modifier = format!("-{} days", ttl_days);
            conn.query_row(
                "SELECT npub FROM auth_tokens WHERE token = ?1 AND created_at > datetime('now', ?2)",
                params![token, modifier],
                |row| row.get(0),
            )
            .optional()
            .context("validate auth token")
        })
    }

    pub fn cleanup_expired_tokens(&self, ttl_days: i64) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let modifier = format!("-{} days", ttl_days);
            let rows = conn
                .execute(
                    "DELETE FROM auth_tokens WHERE created_at <= datetime('now', ?1)",
                    params![modifier],
                )
                .context("cleanup expired tokens")?;
            Ok(rows)
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InboxItem {
    pub pr_id: i64,
    pub repo: String,
    pub pr_number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub updated_at: String,
    pub generation_status: String,
    pub reason: String,
    pub inbox_created_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InboxReviewContext {
    pub prev: Option<i64>,
    pub next: Option<i64>,
    pub position: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChatAllowlistEntry {
    pub npub: String,
    pub active: bool,
    pub note: Option<String>,
    pub updated_by: String,
    pub updated_at: String,
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

#[derive(Debug, Clone)]
struct ArtifactUserStateRow {
    artifact_id: i64,
    state: String,
    reason: String,
    created_at: String,
    updated_at: String,
    dismissed_at: Option<String>,
}

fn merge_inbox_state_alias(
    conn: &Connection,
    from_npub: &str,
    to_npub: &str,
) -> anyhow::Result<usize> {
    let mut stmt = conn
        .prepare(
            "SELECT artifact_id, state, reason, created_at, updated_at, dismissed_at
             FROM artifact_user_states
             WHERE npub = ?1
             ORDER BY artifact_id ASC",
        )
        .context("prepare source inbox state scan")?;
    let rows = stmt
        .query_map(params![from_npub], |row| {
            Ok(ArtifactUserStateRow {
                artifact_id: row.get(0)?,
                state: row.get(1)?,
                reason: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                dismissed_at: row.get(5)?,
            })
        })
        .context("query source inbox state rows")?;

    let mut source_rows = Vec::new();
    for row in rows {
        source_rows.push(row.context("read source inbox state row")?);
    }

    for source in &source_rows {
        let existing = conn
            .query_row(
                "SELECT artifact_id, state, reason, created_at, updated_at, dismissed_at
                 FROM artifact_user_states
                 WHERE npub = ?1 AND artifact_id = ?2",
                params![to_npub, source.artifact_id],
                |row| {
                    Ok(ArtifactUserStateRow {
                        artifact_id: row.get(0)?,
                        state: row.get(1)?,
                        reason: row.get(2)?,
                        created_at: row.get(3)?,
                        updated_at: row.get(4)?,
                        dismissed_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .context("query destination inbox state row")?;

        if let Some(existing) = existing {
            let merged = merge_artifact_user_state_rows(&existing, source);
            conn.execute(
                "UPDATE artifact_user_states
                 SET state = ?1,
                     reason = ?2,
                     created_at = ?3,
                     updated_at = ?4,
                     dismissed_at = ?5
                 WHERE npub = ?6 AND artifact_id = ?7",
                params![
                    merged.state,
                    merged.reason,
                    merged.created_at,
                    merged.updated_at,
                    merged.dismissed_at,
                    to_npub,
                    merged.artifact_id
                ],
            )
            .context("update merged destination inbox state row")?;
            conn.execute(
                "DELETE FROM artifact_user_states WHERE npub = ?1 AND artifact_id = ?2",
                params![from_npub, source.artifact_id],
            )
            .context("delete merged source inbox state row")?;
        } else {
            conn.execute(
                "UPDATE artifact_user_states
                 SET npub = ?1
                 WHERE npub = ?2 AND artifact_id = ?3",
                params![to_npub, from_npub, source.artifact_id],
            )
            .context("rekey inbox state row")?;
        }
    }

    Ok(source_rows.len())
}

fn merge_artifact_user_state_rows(
    existing: &ArtifactUserStateRow,
    incoming: &ArtifactUserStateRow,
) -> ArtifactUserStateRow {
    debug_assert_eq!(existing.artifact_id, incoming.artifact_id);

    let existing_rank = artifact_user_state_rank(&existing.state);
    let incoming_rank = artifact_user_state_rank(&incoming.state);
    let winner = if incoming_rank > existing_rank
        || (incoming_rank == existing_rank && incoming.updated_at > existing.updated_at)
    {
        incoming
    } else {
        existing
    };

    let state = winner.state.clone();
    let dismissed_at = if state == "dismissed" {
        max(existing.dismissed_at.clone(), incoming.dismissed_at.clone())
    } else {
        None
    };

    ArtifactUserStateRow {
        artifact_id: existing.artifact_id,
        state,
        reason: winner.reason.clone(),
        created_at: min(existing.created_at.clone(), incoming.created_at.clone()),
        updated_at: max(existing.updated_at.clone(), incoming.updated_at.clone()),
        dismissed_at,
    }
}

fn artifact_user_state_rank(state: &str) -> u8 {
    match state {
        "dismissed" => 3,
        "inbox" => 2,
        "superseded" => 1,
        _ => 0,
    }
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

fn insert_inbox_rows_for_artifact(
    conn: &Connection,
    artifact_id: i64,
    npubs: &[String],
) -> anyhow::Result<usize> {
    if npubs.is_empty() {
        return Ok(0);
    }

    let Some((pr_id, is_current)) = conn
        .query_row(
            "SELECT pr_id, is_current = 1
             FROM artifact_versions
             WHERE id = ?1 AND status = 'ready'",
            params![artifact_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? != 0)),
        )
        .optional()
        .context("lookup ready artifact for inbox insert")?
    else {
        return Ok(0);
    };

    if !is_current {
        return Ok(0);
    }

    let mut count = 0;
    for npub in npubs {
        conn.execute(
            "UPDATE artifact_user_states
             SET state = 'superseded', updated_at = CURRENT_TIMESTAMP
             WHERE npub = ?1
               AND state = 'inbox'
               AND artifact_id IN (
                   SELECT id FROM artifact_versions WHERE pr_id = ?2 AND id != ?3
               )",
            params![npub, pr_id, artifact_id],
        )
        .with_context(|| format!("supersede stale inbox rows for {}", npub))?;
        count += conn
            .execute(
                "INSERT OR IGNORE INTO artifact_user_states(npub, artifact_id, state, reason, created_at, updated_at)
                 VALUES (?1, ?2, 'inbox', 'generation_ready', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
                params![npub, artifact_id],
            )
            .context("insert artifact inbox row")?;
    }
    Ok(count)
}

fn run_migrations(conn: &mut Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _pika_news_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );",
    )
    .context("create migrations table")?;

    for migration in migrations() {
        let already_applied: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM _pika_news_migrations WHERE version = ?1)",
                params![migration.version],
                |row| row.get(0),
            )
            .with_context(|| format!("check migration version {}", migration.version))?;

        if already_applied {
            continue;
        }

        let tx = conn
            .unchecked_transaction()
            .with_context(|| format!("start migration transaction {}", migration.name))?;
        tx.execute_batch(migration.sql)
            .with_context(|| format!("apply migration {}", migration.name))?;
        tx.execute(
            "INSERT INTO _pika_news_migrations(version, name) VALUES (?1, ?2)",
            params![migration.version, migration.name],
        )
        .with_context(|| format!("record migration {}", migration.name))?;
        tx.commit()
            .with_context(|| format!("commit migration {}", migration.name))?;
    }

    Ok(())
}

fn open_connection(path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("open sqlite database {}", path.display()))?;
    conn.busy_timeout(Duration::from_secs(5))
        .context("set sqlite busy timeout")?;
    Ok(conn)
}

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

fn migrations() -> Vec<Migration> {
    vec![
        Migration {
            version: 1,
            name: "0001_init",
            sql: include_str!("../migrations/0001_init.sql"),
        },
        Migration {
            version: 2,
            name: "0002_add_unified_diff",
            sql: include_str!("../migrations/0002_add_unified_diff.sql"),
        },
        Migration {
            version: 3,
            name: "0003_chat_sessions",
            sql: include_str!("../migrations/0003_chat_sessions.sql"),
        },
        Migration {
            version: 4,
            name: "0004_inbox",
            sql: include_str!("../migrations/0004_inbox.sql"),
        },
        Migration {
            version: 5,
            name: "0005_auth_tokens",
            sql: include_str!("../migrations/0005_auth_tokens.sql"),
        },
        Migration {
            version: 6,
            name: "0006_allowlist",
            sql: include_str!("../migrations/0006_allowlist.sql"),
        },
        Migration {
            version: 7,
            name: "0007_inbox_dismissals",
            sql: include_str!("../migrations/0007_inbox_dismissals.sql"),
        },
        Migration {
            version: 8,
            name: "0008_artifact_versions",
            sql: include_str!("../migrations/0008_artifact_versions.sql"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use nostr::key::PublicKey;
    use rusqlite::{params, OptionalExtension};

    use super::{ChatAllowlistEntry, InboxReviewContext, PrUpsertInput, Store};

    fn latest_artifact_id(store: &Store, pr_id: i64) -> i64 {
        store
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT id
                     FROM artifact_versions
                     WHERE pr_id = ?1
                     ORDER BY version DESC
                     LIMIT 1",
                    params![pr_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("lookup latest artifact id")
    }

    fn mark_latest_artifact_ready(store: &Store, pr_id: i64, head_sha: &str) -> i64 {
        let artifact_id = latest_artifact_id(store, pr_id);
        store
            .mark_generation_ready(
                artifact_id,
                "{}",
                "<p>ok</p>",
                head_sha,
                "diff",
                Some("sid"),
            )
            .expect("mark artifact ready");
        artifact_id
    }

    fn artifact_user_state(
        store: &Store,
        npub: &str,
        artifact_id: i64,
    ) -> Option<(String, Option<String>)> {
        store
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT state, dismissed_at
                     FROM artifact_user_states
                     WHERE npub = ?1 AND artifact_id = ?2",
                    params![npub, artifact_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(Into::into)
            })
            .expect("query artifact user state")
    }

    fn artifact_version_count(store: &Store, pr_id: i64) -> i64 {
        store
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM artifact_versions WHERE pr_id = ?1",
                    params![pr_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("count artifact versions")
    }

    fn inflight_artifact_count(store: &Store, pr_id: i64, head_sha: &str) -> i64 {
        store
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT COUNT(*)
                     FROM artifact_versions
                     WHERE pr_id = ?1
                       AND source_head_sha = ?2
                       AND status IN ('pending', 'generating')",
                    params![pr_id, head_sha],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("count inflight artifact versions")
    }

    #[test]
    fn migrations_apply_and_reapply_without_data_loss() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");

        let store = Store::open(&db_path).expect("open store");
        store
            .with_connection(|conn| {
                conn.execute("INSERT INTO repos(repo) VALUES (?1)", ["sledtools/pika"])
                    .expect("insert repo");
                Ok(())
            })
            .expect("insert data");

        // Re-open triggers migration runner again and must preserve data.
        let reopened = Store::open(&db_path).expect("re-open store");
        reopened
            .with_connection(|conn| {
                let count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM repos", [], |row| row.get(0))
                    .expect("count repos");
                assert_eq!(count, 1);
                Ok(())
            })
            .expect("verify data");

        fs::remove_file(db_path).expect("cleanup sqlite db file");
    }

    #[test]
    fn upsert_marks_artifact_pending_when_head_sha_changes() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let first = PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: 42,
            title: "Initial title".to_string(),
            url: "https://github.com/sledtools/pika/pull/42".to_string(),
            state: "open".to_string(),
            head_sha: "sha-1".to_string(),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };

        let outcome = store.upsert_pull_request(&first).expect("insert pr");
        assert!(outcome.inserted);
        assert!(outcome.queued);
        assert!(!outcome.head_changed);

        let pr_id = store.list_feed_items().expect("list feed")[0].pr_id;
        mark_latest_artifact_ready(&store, pr_id, "sha-1");

        let same_head = store
            .upsert_pull_request(&PrUpsertInput {
                head_sha: "sha-1".to_string(),
                ..first.clone()
            })
            .expect("upsert same head");
        assert!(!same_head.queued);
        assert!(!same_head.head_changed);

        let changed = store
            .upsert_pull_request(&PrUpsertInput {
                head_sha: "sha-2".to_string(),
                updated_at: "2026-03-02T00:00:00Z".to_string(),
                ..first
            })
            .expect("upsert changed head");
        assert!(changed.queued);
        assert!(changed.head_changed);

        store
            .with_connection(|conn| {
                let (version, status, source_head_sha, current_count): (i64, String, String, i64) =
                    conn
                    .query_row(
                        "SELECT version, status, source_head_sha,
                                (SELECT COUNT(*) FROM artifact_versions WHERE pr_id = ?1 AND is_current = 1)
                         FROM artifact_versions
                         WHERE pr_id = ?1
                         ORDER BY version DESC
                         LIMIT 1",
                        params![pr_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .expect("read artifact row");
                assert_eq!(version, 2);
                assert_eq!(status, "pending");
                assert_eq!(source_head_sha, "sha-2");
                assert_eq!(current_count, 1);
                Ok(())
            })
            .expect("verify pending state");
    }

    #[test]
    fn retry_safe_failure_is_reclaimable_when_backoff_elapses() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let input = PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: 77,
            title: "Retryable".to_string(),
            url: "https://github.com/sledtools/pika/pull/77".to_string(),
            state: "open".to_string(),
            head_sha: "retry-sha".to_string(),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-02T00:00:00Z".to_string(),
            merged_at: None,
        };
        store.upsert_pull_request(&input).expect("upsert PR");

        let claimed = store
            .claim_pending_generation_jobs(1)
            .expect("claim pending job");
        assert_eq!(claimed.len(), 1);
        let artifact_id = claimed[0].artifact_id;

        store
            .mark_generation_failed(artifact_id, "transient error", true, 0)
            .expect("mark retry-safe failure");

        let reclaimed = store
            .claim_pending_generation_jobs(1)
            .expect("reclaim pending job");
        assert_eq!(reclaimed.len(), 1);
        assert_eq!(reclaimed[0].artifact_id, artifact_id);
    }

    #[test]
    fn recover_stale_generating_resets_to_pending() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let input = PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: 99,
            title: "Stuck generating".to_string(),
            url: "https://github.com/sledtools/pika/pull/99".to_string(),
            state: "merged".to_string(),
            head_sha: "stuck-sha".to_string(),
            base_ref: "master".to_string(),
            author_login: Some("bob".to_string()),
            updated_at: "2026-03-03T00:00:00Z".to_string(),
            merged_at: Some("2026-03-03T00:00:00Z".to_string()),
        };
        store.upsert_pull_request(&input).expect("upsert PR");

        // Claim the job (moves to 'generating')
        let claimed = store
            .claim_pending_generation_jobs(1)
            .expect("claim pending job");
        assert_eq!(claimed.len(), 1);

        // Verify it's now generating and not claimable
        let empty = store
            .claim_pending_generation_jobs(1)
            .expect("try claim again");
        assert_eq!(empty.len(), 0);

        // Recover stale generating items
        let recovered = store
            .recover_stale_generating()
            .expect("recover stale generating");
        assert_eq!(recovered, 1);

        // Now it should be claimable again
        let reclaimed = store
            .claim_pending_generation_jobs(1)
            .expect("reclaim after recovery");
        assert_eq!(reclaimed.len(), 1);
    }

    #[test]
    fn close_stale_open_prs_marks_missing_prs_as_closed() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let make_pr = |number: i64, state: &str| PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: number,
            title: format!("PR #{}", number),
            url: format!("https://github.com/sledtools/pika/pull/{}", number),
            state: state.to_string(),
            head_sha: format!("sha-{}", number),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };

        // Insert 3 open PRs
        store.upsert_pull_request(&make_pr(1, "open")).unwrap();
        store.upsert_pull_request(&make_pr(2, "open")).unwrap();
        store.upsert_pull_request(&make_pr(3, "open")).unwrap();

        // Simulate GitHub now only returning PR #1 as open (PRs #2 and #3 were closed)
        let closed = store
            .close_stale_open_prs("sledtools/pika", &[1])
            .expect("close stale PRs");
        assert_eq!(closed, 2);

        // Feed should only show PR #1 (closed PRs are filtered out)
        let feed = store.list_feed_items().expect("list feed");
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].pr_number, 1);
        assert_eq!(feed[0].state, "open");
    }

    #[test]
    fn close_stale_open_prs_with_empty_open_list_closes_all() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let make_pr = |number: i64| PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: number,
            title: format!("PR #{}", number),
            url: format!("https://github.com/sledtools/pika/pull/{}", number),
            state: "open".to_string(),
            head_sha: format!("sha-{}", number),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };

        store.upsert_pull_request(&make_pr(1)).unwrap();
        store.upsert_pull_request(&make_pr(2)).unwrap();

        // No open PRs on GitHub anymore
        let closed = store
            .close_stale_open_prs("sledtools/pika", &[])
            .expect("close all stale PRs");
        assert_eq!(closed, 2);

        let feed = store.list_feed_items().expect("list feed");
        assert_eq!(feed.len(), 0);
    }

    #[test]
    fn close_stale_open_prs_does_not_affect_merged() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        // Insert one open and one merged PR
        store
            .upsert_pull_request(&PrUpsertInput {
                repo: "sledtools/pika".to_string(),
                pr_number: 1,
                title: "Open PR".to_string(),
                url: "https://github.com/sledtools/pika/pull/1".to_string(),
                state: "open".to_string(),
                head_sha: "sha-1".to_string(),
                base_ref: "master".to_string(),
                author_login: Some("alice".to_string()),
                updated_at: "2026-03-01T00:00:00Z".to_string(),
                merged_at: None,
            })
            .unwrap();
        store
            .upsert_pull_request(&PrUpsertInput {
                repo: "sledtools/pika".to_string(),
                pr_number: 2,
                title: "Merged PR".to_string(),
                url: "https://github.com/sledtools/pika/pull/2".to_string(),
                state: "merged".to_string(),
                head_sha: "sha-2".to_string(),
                base_ref: "master".to_string(),
                author_login: Some("bob".to_string()),
                updated_at: "2026-03-01T00:00:00Z".to_string(),
                merged_at: Some("2026-03-01T00:00:00Z".to_string()),
            })
            .unwrap();

        // Close stale open PRs — PR #1 is no longer open on GitHub
        let closed = store
            .close_stale_open_prs("sledtools/pika", &[])
            .expect("close stale PRs");
        assert_eq!(closed, 1);

        // Merged PR should still be in the feed
        let feed = store.list_feed_items().expect("list feed");
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].pr_number, 2);
        assert_eq!(feed[0].state, "merged");
    }

    #[test]
    fn inbox_populate_list_dismiss_and_review_context() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        // Insert two PRs
        let make_pr = |number: i64| PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: number,
            title: format!("PR #{}", number),
            url: format!("https://github.com/sledtools/pika/pull/{}", number),
            state: "open".to_string(),
            head_sha: format!("sha-{}", number),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };

        store.upsert_pull_request(&make_pr(1)).unwrap();
        store.upsert_pull_request(&make_pr(2)).unwrap();

        // Get pr_ids
        let feed = store.list_feed_items().unwrap();
        let pr_id_1 = feed.iter().find(|f| f.pr_number == 1).unwrap().pr_id;
        let pr_id_2 = feed.iter().find(|f| f.pr_number == 2).unwrap().pr_id;
        let artifact_id_1 = mark_latest_artifact_ready(&store, pr_id_1, "sha-1");
        let artifact_id_2 = mark_latest_artifact_ready(&store, pr_id_2, "sha-2");

        let npub = "npub1test".to_string();
        let npubs = vec![npub.clone(), "npub2other".to_string()];

        // Populate inbox for both npubs
        let count = store.populate_inbox(artifact_id_1, &npubs).unwrap();
        assert_eq!(count, 2); // 2 users got an item

        // Populate second PR with a later timestamp (CURRENT_TIMESTAMP has 1s resolution)
        store
            .with_connection(|conn| {
                for n in &npubs {
                    conn.execute(
                        "INSERT OR IGNORE INTO artifact_user_states(npub, artifact_id, state, reason, created_at, updated_at)
                         VALUES (?1, ?2, 'inbox', 'generation_ready', datetime('now', '+1 second'), datetime('now', '+1 second'))",
                        params![n, artifact_id_2],
                    )
                    .expect("insert inbox item with offset");
                }
                Ok(())
            })
            .unwrap();

        // Duplicate populate should be ignored
        let dup = store.populate_inbox(artifact_id_1, &npubs).unwrap();
        assert_eq!(dup, 0);

        // Count
        let count = store.inbox_count(&npub).unwrap();
        assert_eq!(count, 2);

        // List (oldest first)
        let items = store.list_inbox(&npub, 50, 0).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].pr_id, pr_id_1);
        assert_eq!(items[1].pr_id, pr_id_2);

        let context_1 = store
            .inbox_review_context(&npub, pr_id_1)
            .unwrap()
            .expect("review context for oldest item");
        assert_eq!(
            context_1,
            InboxReviewContext {
                prev: None,
                next: Some(pr_id_2),
                position: 1,
                total: 2,
            }
        );

        let context_2 = store
            .inbox_review_context(&npub, pr_id_2)
            .unwrap()
            .expect("review context for newest item");
        assert_eq!(
            context_2,
            InboxReviewContext {
                prev: Some(pr_id_1),
                next: None,
                position: 2,
                total: 2,
            }
        );

        // Dismiss one item
        let dismissed = store.dismiss_inbox_items(&npub, &[pr_id_1]).unwrap();
        assert_eq!(dismissed, 1);
        assert_eq!(store.inbox_count(&npub).unwrap(), 1);

        // Dismiss all
        let dismissed = store.dismiss_all_inbox(&npub).unwrap();
        assert_eq!(dismissed, 1);
        assert_eq!(store.inbox_count(&npub).unwrap(), 0);

        // Other user still has their items
        assert_eq!(store.inbox_count("npub2other").unwrap(), 2);
    }

    #[test]
    fn chat_allowlist_upsert_list_and_active_lookup_work() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        assert!(!store.has_active_chat_allowlist_entries().unwrap());
        assert!(!store.is_chat_allowlist_active("npub1missing").unwrap());

        let saved = store
            .upsert_chat_allowlist_entry("npub1alice", true, Some("friend"), "npub1admin")
            .expect("upsert allowlist entry");
        assert_eq!(
            saved,
            ChatAllowlistEntry {
                npub: "npub1alice".to_string(),
                active: true,
                note: Some("friend".to_string()),
                updated_by: "npub1admin".to_string(),
                updated_at: saved.updated_at.clone(),
            }
        );

        assert!(store.has_active_chat_allowlist_entries().unwrap());
        assert!(store.is_chat_allowlist_active("npub1alice").unwrap());

        let rows = store.list_chat_allowlist_entries().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].npub, "npub1alice");
        assert_eq!(rows[0].note.as_deref(), Some("friend"));

        let updated = store
            .upsert_chat_allowlist_entry("npub1alice", false, Some("paused"), "npub1admin")
            .expect("deactivate allowlist entry");
        assert!(!updated.active);
        assert!(!store.is_chat_allowlist_active("npub1alice").unwrap());

        let active = store.list_active_chat_allowlist_npubs().unwrap();
        assert!(active.is_empty());
    }

    #[test]
    fn backfill_inbox_for_npub_adds_existing_ready_artifacts() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let input = PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: 314,
            title: "Ready tutorial".to_string(),
            url: "https://github.com/sledtools/pika/pull/314".to_string(),
            state: "open".to_string(),
            head_sha: "ready-sha".to_string(),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-03T00:00:00Z".to_string(),
            merged_at: None,
        };
        store.upsert_pull_request(&input).expect("upsert PR");

        let pr_id = store
            .list_feed_items()
            .expect("list feed")
            .into_iter()
            .find(|item| item.pr_number == 314)
            .expect("find PR")
            .pr_id;
        mark_latest_artifact_ready(&store, pr_id, "ready-sha");

        let inserted = store.backfill_inbox_for_npub("npub1newuser").unwrap();
        assert_eq!(inserted, 1);
        assert_eq!(store.inbox_count("npub1newuser").unwrap(), 1);

        let dup = store.backfill_inbox_for_npub("npub1newuser").unwrap();
        assert_eq!(dup, 0);

        let items = store.list_inbox("npub1newuser", 50, 0).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].pr_id, pr_id);
    }

    #[test]
    fn dismissed_items_are_not_recreated_by_populate_or_backfill() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let make_pr = |number: i64| PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: number,
            title: format!("PR #{}", number),
            url: format!("https://github.com/sledtools/pika/pull/{}", number),
            state: "open".to_string(),
            head_sha: format!("sha-{}", number),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };
        store.upsert_pull_request(&make_pr(1)).unwrap();

        let pr_id = store.list_feed_items().unwrap()[0].pr_id;
        let artifact_id = mark_latest_artifact_ready(&store, pr_id, "sha-1");
        let npub = "npub1dismissed";

        store
            .populate_inbox(artifact_id, &[npub.to_string()])
            .unwrap();
        assert_eq!(store.inbox_count(npub).unwrap(), 1);

        store.dismiss_inbox_items(npub, &[pr_id]).unwrap();
        assert_eq!(store.inbox_count(npub).unwrap(), 0);

        assert_eq!(
            store
                .populate_inbox(artifact_id, &[npub.to_string()])
                .unwrap(),
            0
        );
        assert_eq!(store.backfill_inbox_for_npub(npub).unwrap(), 0);
        assert_eq!(store.inbox_count(npub).unwrap(), 0);
    }

    #[test]
    fn queue_regeneration_preserves_dismissals_until_new_ready_artifact() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let input = PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: 12,
            title: "Regenerate me".to_string(),
            url: "https://github.com/sledtools/pika/pull/12".to_string(),
            state: "open".to_string(),
            head_sha: "sha-12".to_string(),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };
        store.upsert_pull_request(&input).unwrap();
        let pr_id = store.list_feed_items().unwrap()[0].pr_id;
        let artifact_id = mark_latest_artifact_ready(&store, pr_id, "sha-12");
        let npub = "npub1regen";

        store
            .populate_inbox(artifact_id, &[npub.to_string()])
            .unwrap();
        store.dismiss_inbox_items(npub, &[pr_id]).unwrap();
        assert_eq!(store.inbox_count(npub).unwrap(), 0);

        assert!(store.queue_regeneration(pr_id).unwrap());
        assert_eq!(
            store
                .populate_inbox(artifact_id, &[npub.to_string()])
                .unwrap(),
            0
        );
        assert_eq!(store.backfill_inbox_for_npub(npub).unwrap(), 0);
        assert_eq!(store.inbox_count(npub).unwrap(), 0);
    }

    #[test]
    fn new_ready_generation_restores_inbox_without_clearing_old_dismissal() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let input = PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: 13,
            title: "New ready artifact".to_string(),
            url: "https://github.com/sledtools/pika/pull/13".to_string(),
            state: "open".to_string(),
            head_sha: "sha-13".to_string(),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };
        store.upsert_pull_request(&input).unwrap();

        let pr_id = store.list_feed_items().unwrap()[0].pr_id;
        let artifact_id = mark_latest_artifact_ready(&store, pr_id, "sha-13");
        let npub = "npub1ready";

        store
            .populate_inbox(artifact_id, &[npub.to_string()])
            .unwrap();
        store.dismiss_inbox_items(npub, &[pr_id]).unwrap();
        assert_eq!(store.inbox_count(npub).unwrap(), 0);

        assert!(store.queue_regeneration(pr_id).unwrap());
        let next_artifact_id = latest_artifact_id(&store, pr_id);
        assert_ne!(artifact_id, next_artifact_id);
        store
            .mark_generation_ready(
                next_artifact_id,
                "{}",
                "<p>new</p>",
                "sha-13",
                "diff-2",
                Some("sid-2"),
            )
            .unwrap();
        assert_eq!(store.backfill_inbox_for_npub(npub).unwrap(), 1);
        assert_eq!(store.inbox_count(npub).unwrap(), 1);
        store.dismiss_inbox_items(npub, &[pr_id]).unwrap();
        assert_eq!(store.backfill_inbox_for_npub(npub).unwrap(), 0);
    }

    #[test]
    fn chat_sessions_are_scoped_to_the_current_artifact_version() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let input = PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: 88,
            title: "Chat scope".to_string(),
            url: "https://github.com/sledtools/pika/pull/88".to_string(),
            state: "open".to_string(),
            head_sha: "sha-88".to_string(),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };
        store.upsert_pull_request(&input).unwrap();

        let pr_id = store.list_feed_items().unwrap()[0].pr_id;
        mark_latest_artifact_ready(&store, pr_id, "sha-88");

        let (first_session_id, first_messages) = store
            .get_or_create_chat_session(pr_id, "npub1chat", "sid-v1")
            .unwrap();
        assert!(first_messages.is_empty());
        store
            .append_chat_message(first_session_id, "user", "first version")
            .unwrap();

        assert!(store.queue_regeneration(pr_id).unwrap());
        let next_artifact_id = latest_artifact_id(&store, pr_id);
        store
            .mark_generation_ready(
                next_artifact_id,
                "{}",
                "<p>new</p>",
                "sha-88",
                "diff-2",
                Some("sid-v2"),
            )
            .unwrap();

        let (second_session_id, second_messages) = store
            .get_or_create_chat_session(pr_id, "npub1chat", "sid-v2")
            .unwrap();
        assert_ne!(first_session_id, second_session_id);
        assert!(second_messages.is_empty());
    }

    #[test]
    fn rekey_inbox_npub_moves_existing_rows_without_recreating_missing_ones() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let make_pr = |number: i64| PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: number,
            title: format!("PR #{}", number),
            url: format!("https://github.com/sledtools/pika/pull/{}", number),
            state: "open".to_string(),
            head_sha: format!("sha-{}", number),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };

        store.upsert_pull_request(&make_pr(1)).unwrap();
        store.upsert_pull_request(&make_pr(2)).unwrap();

        let feed = store.list_feed_items().unwrap();
        let pr_id_1 = feed.iter().find(|f| f.pr_number == 1).unwrap().pr_id;
        let pr_id_2 = feed.iter().find(|f| f.pr_number == 2).unwrap().pr_id;
        let artifact_id_1 = mark_latest_artifact_ready(&store, pr_id_1, "sha-1");
        let artifact_id_2 = mark_latest_artifact_ready(&store, pr_id_2, "sha-2");

        let old_npub = "deadbeef".repeat(8);
        let new_npub = "npub1canonical";
        store
            .populate_inbox(artifact_id_1, std::slice::from_ref(&old_npub))
            .unwrap();
        store
            .populate_inbox(artifact_id_2, std::slice::from_ref(&old_npub))
            .unwrap();
        store.dismiss_inbox_items(&old_npub, &[pr_id_2]).unwrap();

        let moved = store.rekey_inbox_npub(&old_npub, new_npub).unwrap();
        assert_eq!(moved, 2);
        assert_eq!(store.inbox_count(&old_npub).unwrap(), 0);
        assert_eq!(store.inbox_count(new_npub).unwrap(), 1);

        let items = store.list_inbox(new_npub, 50, 0).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].pr_id, pr_id_1);
        assert_eq!(store.backfill_inbox_for_npub(new_npub).unwrap(), 0);
        let items = store.list_inbox(new_npub, 50, 0).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].pr_id, pr_id_1);
    }

    #[test]
    fn rekey_inbox_npub_prefers_dismissed_state_on_conflict() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let input = PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: 11,
            title: "Conflicting inbox owners".to_string(),
            url: "https://github.com/sledtools/pika/pull/11".to_string(),
            state: "open".to_string(),
            head_sha: "sha-11".to_string(),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };
        store.upsert_pull_request(&input).unwrap();

        let pr_id = store.list_feed_items().unwrap()[0].pr_id;
        let artifact_id = mark_latest_artifact_ready(&store, pr_id, "sha-11");
        let old_npub =
            PublicKey::parse("npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y")
                .unwrap()
                .to_hex();
        let canonical_npub = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y";

        store
            .populate_inbox(artifact_id, &[old_npub.clone(), canonical_npub.to_string()])
            .unwrap();
        store.dismiss_all_inbox(&old_npub).unwrap();

        let moved = store.rekey_inbox_npub(&old_npub, canonical_npub).unwrap();
        assert_eq!(moved, 1);
        assert_eq!(store.inbox_count(canonical_npub).unwrap(), 0);
        let state = artifact_user_state(&store, canonical_npub, artifact_id).unwrap();
        assert_eq!(state.0, "dismissed");
        assert!(state.1.as_deref().unwrap_or_default().starts_with("2026-"));
        assert_eq!(store.backfill_inbox_for_npub(canonical_npub).unwrap(), 0);
    }

    #[test]
    fn canonicalize_inbox_npubs_preserves_dismissed_conflicts() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let input = PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: 12,
            title: "Canonicalize conflicting inbox owners".to_string(),
            url: "https://github.com/sledtools/pika/pull/12".to_string(),
            state: "open".to_string(),
            head_sha: "sha-12".to_string(),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };
        store.upsert_pull_request(&input).unwrap();

        let pr_id = store.list_feed_items().unwrap()[0].pr_id;
        let artifact_id = mark_latest_artifact_ready(&store, pr_id, "sha-12");
        let canonical = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y";
        let raw_hex = PublicKey::parse(canonical).unwrap().to_hex();

        store
            .populate_inbox(artifact_id, &[raw_hex.clone(), canonical.to_string()])
            .unwrap();
        store.dismiss_all_inbox(&raw_hex).unwrap();

        let rekeyed = store.canonicalize_inbox_npubs().unwrap();
        assert_eq!(rekeyed, 1);
        assert_eq!(store.inbox_count(canonical).unwrap(), 0);
        let state = artifact_user_state(&store, canonical, artifact_id).unwrap();
        assert_eq!(state.0, "dismissed");
        assert!(state.1.as_deref().unwrap_or_default().starts_with("2026-"));
    }

    #[test]
    fn canonicalize_inbox_npubs_rekeys_parseable_aliases_without_config_context() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let input = PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: 7,
            title: "Canonicalize inbox".to_string(),
            url: "https://github.com/sledtools/pika/pull/7".to_string(),
            state: "open".to_string(),
            head_sha: "sha-7".to_string(),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };
        store.upsert_pull_request(&input).unwrap();

        let pr_id = store.list_feed_items().unwrap()[0].pr_id;
        let artifact_id = mark_latest_artifact_ready(&store, pr_id, "sha-7");
        let canonical = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y";
        let raw_hex = PublicKey::parse(canonical).unwrap().to_hex();

        store
            .populate_inbox(artifact_id, std::slice::from_ref(&raw_hex))
            .unwrap();

        let rekeyed = store.canonicalize_inbox_npubs().unwrap();
        assert_eq!(rekeyed, 1);
        assert_eq!(store.inbox_count(&raw_hex).unwrap(), 0);
        assert_eq!(store.inbox_count(canonical).unwrap(), 1);

        store.dismiss_all_inbox(canonical).unwrap();
        assert_eq!(store.backfill_inbox_for_npub(canonical).unwrap(), 0);
        assert_eq!(store.inbox_count(canonical).unwrap(), 0);
    }

    #[test]
    fn concurrent_regeneration_reuses_single_pending_version() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Arc::new(Store::open(&db_path).expect("open store"));

        let input = PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: 21,
            title: "Concurrent regenerate".to_string(),
            url: "https://github.com/sledtools/pika/pull/21".to_string(),
            state: "open".to_string(),
            head_sha: "sha-21".to_string(),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };
        store.upsert_pull_request(&input).unwrap();
        let pr_id = store.list_feed_items().unwrap()[0].pr_id;
        mark_latest_artifact_ready(&store, pr_id, "sha-21");

        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                store.queue_regeneration(pr_id)
            }));
        }
        barrier.wait();

        for handle in handles {
            assert!(handle.join().unwrap().unwrap());
        }

        assert_eq!(artifact_version_count(&store, pr_id), 2);
        assert_eq!(inflight_artifact_count(&store, pr_id, "sha-21"), 1);
    }
}
