use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::Utc;
use nostr::key::PublicKey;
use nostr::nips::nip19::ToBech32;
use rusqlite::{params, Connection, OptionalExtension};
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
        let mut conn = Connection::open(&db_path)
            .with_context(|| format!("open sqlite database {}", db_path.display()))?;
        run_migrations(&mut conn)?;
        Ok(Self { db_path })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn with_connection<T>(
        &self,
        f: impl FnOnce(&Connection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let conn = Connection::open(&self.db_path)
            .with_context(|| format!("open sqlite database {}", self.db_path.display()))?;
        f(&conn)
    }

    pub fn upsert_pull_request(&self, input: &PrUpsertInput) -> anyhow::Result<UpsertOutcome> {
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
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
                        "SELECT ga.id, r.repo, pr.pr_number, pr.title, pr.head_sha, pr.base_ref, pr.id
                         FROM generated_artifacts ga
                         JOIN pull_requests pr ON pr.id = ga.pr_id
                         JOIN repos r ON r.id = pr.repo_id
                         WHERE ga.status = 'pending'
                           AND (ga.next_retry_at IS NULL OR ga.next_retry_at <= CURRENT_TIMESTAMP)
                         ORDER BY ga.updated_at ASC
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
                        "UPDATE generated_artifacts SET status='generating', updated_at=CURRENT_TIMESTAMP WHERE id = ?1",
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
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start mark_generation_ready transaction")?;
            let pr_id: i64 = tx
                .query_row(
                    "SELECT pr_id FROM generated_artifacts WHERE id = ?1",
                    params![artifact_id],
                    |row| row.get(0),
                )
                .with_context(|| format!("lookup pr_id for artifact {}", artifact_id))?;
            tx.execute(
                "UPDATE generated_artifacts
                 SET status='ready', tutorial_json=?1, html=?2, error_message=NULL, next_retry_at=NULL,
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
            .with_context(|| format!("mark artifact {} ready", artifact_id))?;
            tx.execute(
                "DELETE FROM inbox_dismissals WHERE pr_id = ?1",
                params![pr_id],
            )
            .with_context(|| format!("clear inbox dismissals for pr_id {}", pr_id))?;
            tx.commit()
                .with_context(|| format!("commit mark_generation_ready for artifact {}", artifact_id))?;
            Ok(())
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
                    "SELECT retry_count FROM generated_artifacts WHERE id = ?1",
                    params![artifact_id],
                    |row| row.get(0),
                )
                .with_context(|| format!("read retry_count for artifact {}", artifact_id))?;

            let next_retry_count = current_retry + 1;
            let should_retry = retry_safe && next_retry_count <= 5;
            if should_retry {
                let modifier = format!("+{} seconds", retry_backoff_secs);
                conn.execute(
                    "UPDATE generated_artifacts
                     SET status='pending', error_message=?1, retry_count=?2,
                         next_retry_at=datetime('now', ?3), updated_at=CURRENT_TIMESTAMP
                     WHERE id = ?4",
                    params![error_message, next_retry_count, modifier, artifact_id],
                )
                .with_context(|| format!("schedule retry for artifact {}", artifact_id))?;
            } else {
                conn.execute(
                    "UPDATE generated_artifacts
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
                    "SELECT pr.id, r.repo, pr.pr_number, pr.title, pr.url, pr.state, pr.updated_at, ga.status
                     FROM pull_requests pr
                     JOIN repos r ON r.id = pr.repo_id
                     LEFT JOIN generated_artifacts ga ON ga.pr_id = pr.id
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
                        COALESCE(ga.status, 'pending'), ga.tutorial_json, ga.unified_diff, ga.claude_session_id, ga.error_message
                 FROM pull_requests pr
                 JOIN repos r ON r.id = pr.repo_id
                 LEFT JOIN generated_artifacts ga ON ga.pr_id = pr.id
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
                    "UPDATE generated_artifacts
                     SET status='pending', updated_at=CURRENT_TIMESTAMP
                     WHERE status='generating'",
                    [],
                )
                .context("recover stale generating artifacts")?;
            Ok(rows)
        })
    }

    pub fn reset_artifact_to_pending(&self, pr_id: i64) -> anyhow::Result<bool> {
        self.with_connection(|conn| {
            let rows = conn
                .execute(
                    "UPDATE generated_artifacts
                     SET status='pending', tutorial_json=NULL, html=NULL, error_message=NULL,
                         next_retry_at=NULL, claude_session_id=NULL, unified_diff=NULL,
                         updated_at=CURRENT_TIMESTAMP
                     WHERE pr_id = ?1",
                    params![pr_id],
                )
                .context("reset artifact to pending")?;
            Ok(rows > 0)
        })
    }

    pub fn get_artifact_session_id(&self, pr_id: i64) -> anyhow::Result<Option<String>> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT claude_session_id FROM generated_artifacts WHERE pr_id = ?1 AND status = 'ready'",
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
                    "SELECT id FROM generated_artifacts WHERE pr_id = ?1",
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

    pub fn populate_inbox(&self, pr_id: i64, npubs: &[String]) -> anyhow::Result<usize> {
        if npubs.is_empty() {
            return Ok(0);
        }
        self.with_connection(|conn| {
            let mut count = 0;
            for npub in npubs {
                let rows = conn
                    .execute(
                        "INSERT OR IGNORE INTO inbox(npub, pr_id)
                         SELECT ?1, ?2
                         WHERE NOT EXISTS(
                             SELECT 1 FROM inbox_dismissals WHERE npub = ?1 AND pr_id = ?2
                         )",
                        params![npub, pr_id],
                    )
                    .context("populate inbox")?;
                count += rows;
            }
            Ok(count)
        })
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
                    "SELECT i.pr_id, r.repo, pr.pr_number, pr.title, pr.url, pr.state, pr.updated_at,
                            COALESCE(ga.status, 'pending'), i.reason, i.created_at
                     FROM inbox i
                     JOIN pull_requests pr ON pr.id = i.pr_id
                     JOIN repos r ON r.id = pr.repo_id
                     LEFT JOIN generated_artifacts ga ON ga.pr_id = pr.id
                     WHERE i.npub = ?1
                     ORDER BY i.created_at DESC
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
                "SELECT COUNT(*) FROM inbox WHERE npub = ?1",
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
            let insert_sql = format!(
                "INSERT OR IGNORE INTO inbox_dismissals(npub, pr_id)
                 SELECT ?1, pr_id FROM inbox WHERE npub = ?1 AND pr_id IN ({})",
                placeholders.join(", ")
            );
            let delete_sql = format!(
                "DELETE FROM inbox WHERE npub = ?1 AND pr_id IN ({})",
                placeholders.join(", ")
            );
            let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            param_values.push(Box::new(npub.to_string()));
            for id in pr_ids {
                param_values.push(Box::new(*id));
            }
            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|p| p.as_ref()).collect();
            let rows = {
                let mut insert_stmt = tx
                    .prepare(&insert_sql)
                    .context("prepare dismiss tombstone query")?;
                let mut delete_stmt = tx.prepare(&delete_sql).context("prepare dismiss query")?;
                insert_stmt
                    .execute(param_refs.as_slice())
                    .context("insert dismiss tombstones")?;
                delete_stmt
                    .execute(param_refs.as_slice())
                    .context("dismiss inbox items")?
            };
            tx.commit().context("commit dismiss inbox transaction")?;
            Ok(rows)
        })
    }

    pub fn dismiss_all_inbox(&self, npub: &str) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start dismiss all inbox transaction")?;
            tx.execute(
                "INSERT OR IGNORE INTO inbox_dismissals(npub, pr_id)
                 SELECT npub, pr_id FROM inbox WHERE npub = ?1",
                params![npub],
            )
            .context("insert dismiss-all tombstones")?;
            let rows = tx
                .execute("DELETE FROM inbox WHERE npub = ?1", params![npub])
                .context("dismiss all inbox")?;
            tx.commit()
                .context("commit dismiss all inbox transaction")?;
            Ok(rows)
        })
    }

    pub fn inbox_neighbors(
        &self,
        npub: &str,
        pr_id: i64,
    ) -> anyhow::Result<(Option<i64>, Option<i64>)> {
        self.with_connection(|conn| {
            // Items ordered created_at DESC (newest first).
            // "prev" = newer item (higher created_at), "next" = older item (lower created_at).
            let prev: Option<i64> = conn
                .query_row(
                    "SELECT pr_id FROM inbox
                     WHERE npub = ?1 AND created_at > (SELECT created_at FROM inbox WHERE npub = ?1 AND pr_id = ?2)
                     ORDER BY created_at ASC LIMIT 1",
                    params![npub, pr_id],
                    |row| row.get(0),
                )
                .optional()
                .context("inbox prev neighbor")?;

            let next: Option<i64> = conn
                .query_row(
                    "SELECT pr_id FROM inbox
                     WHERE npub = ?1 AND created_at < (SELECT created_at FROM inbox WHERE npub = ?1 AND pr_id = ?2)
                     ORDER BY created_at DESC LIMIT 1",
                    params![npub, pr_id],
                    |row| row.get(0),
                )
                .optional()
                .context("inbox next neighbor")?;

            Ok((prev, next))
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
            let rows = conn
                .execute(
                    "INSERT OR IGNORE INTO inbox(npub, pr_id)
                     SELECT ?1, pr.id
                     FROM generated_artifacts ga
                     JOIN pull_requests pr ON pr.id = ga.pr_id
                     WHERE ga.status = 'ready'
                       AND NOT EXISTS(
                           SELECT 1 FROM inbox_dismissals d
                           WHERE d.npub = ?1 AND d.pr_id = pr.id
                       )",
                    params![npub],
                )
                .context("backfill inbox for allowlisted npub")?;
            Ok(rows)
        })
    }

    pub fn rekey_inbox_npub(&self, from_npub: &str, to_npub: &str) -> anyhow::Result<usize> {
        if from_npub == to_npub {
            return Ok(0);
        }
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start inbox rekey transaction")?;

            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO inbox(npub, pr_id, reason, created_at)
                     SELECT ?2, pr_id, reason, created_at
                     FROM inbox
                     WHERE npub = ?1",
                    params![from_npub, to_npub],
                )
                .context("copy inbox rows to canonical npub")?;

            tx.execute(
                "INSERT OR IGNORE INTO inbox_dismissals(npub, pr_id, dismissed_at)
                 SELECT ?2, pr_id, dismissed_at
                 FROM inbox_dismissals
                 WHERE npub = ?1",
                params![from_npub, to_npub],
            )
            .context("copy inbox dismissal rows to canonical npub")?;

            tx.execute("DELETE FROM inbox WHERE npub = ?1", params![from_npub])
                .context("delete old inbox rows after rekey")?;
            tx.execute(
                "DELETE FROM inbox_dismissals WHERE npub = ?1",
                params![from_npub],
            )
            .context("delete old inbox dismissal rows after rekey")?;

            tx.commit().context("commit inbox rekey transaction")?;
            Ok(inserted)
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
                         FROM (
                           SELECT npub FROM inbox
                           UNION
                           SELECT npub FROM inbox_dismissals
                         )
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

                tx.execute(
                    "INSERT OR IGNORE INTO inbox(npub, pr_id, reason, created_at)
                     SELECT ?2, pr_id, reason, created_at
                     FROM inbox
                     WHERE npub = ?1",
                    params![raw_npub, canonical_npub],
                )
                .context("copy inbox rows to canonical owner")?;

                tx.execute(
                    "INSERT OR IGNORE INTO inbox_dismissals(npub, pr_id, dismissed_at)
                     SELECT ?2, pr_id, dismissed_at
                     FROM inbox_dismissals
                     WHERE npub = ?1",
                    params![raw_npub, canonical_npub],
                )
                .context("copy inbox dismissals to canonical owner")?;

                tx.execute("DELETE FROM inbox WHERE npub = ?1", params![raw_npub])
                    .context("delete raw inbox owner rows after canonicalization")?;
                tx.execute(
                    "DELETE FROM inbox_dismissals WHERE npub = ?1",
                    params![raw_npub],
                )
                .context("delete raw inbox dismissal rows after canonicalization")?;

                rekeyed += 1;
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
    let existing_generated_head: Option<Option<String>> = conn
        .query_row(
            "SELECT generated_head_sha FROM generated_artifacts WHERE pr_id = ?1",
            params![pr_id],
            |row| row.get(0),
        )
        .optional()
        .context("lookup existing artifact row")?;

    match existing_generated_head {
        None => {
            conn.execute(
                "INSERT INTO generated_artifacts(pr_id, status, generated_head_sha, updated_at)
                 VALUES (?1, 'pending', ?2, CURRENT_TIMESTAMP)",
                params![pr_id, head_sha],
            )
            .with_context(|| format!("insert generated artifact for pr_id {}", pr_id))?;

            Ok(UpsertOutcome {
                inserted: false,
                head_changed: false,
                queued: true,
            })
        }
        Some(current_generated_head) => {
            let needs_refresh = current_generated_head
                .as_deref()
                .map(|existing| existing != head_sha)
                .unwrap_or(true);

            if needs_refresh {
                conn.execute(
                    "UPDATE generated_artifacts
                     SET status='pending', tutorial_json=NULL, html=NULL, error_message=NULL, next_retry_at=NULL,
                         generated_head_sha=?1, updated_at=CURRENT_TIMESTAMP
                     WHERE pr_id = ?2",
                    params![head_sha, pr_id],
                )
                .with_context(|| format!("mark artifact pending for pr_id {}", pr_id))?;
            }

            Ok(UpsertOutcome {
                inserted: false,
                head_changed: false,
                queued: needs_refresh,
            })
        }
    }
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
    ]
}

#[cfg(test)]
mod tests {
    use std::fs;

    use nostr::key::PublicKey;
    use rusqlite::params;

    use super::{ChatAllowlistEntry, PrUpsertInput, Store};

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

        // Simulate a completed generation for the first head SHA.
        store
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE generated_artifacts SET status='ready', generated_head_sha='sha-1', tutorial_json='{}'",
                    [],
                )
                .expect("mark artifact ready");
                Ok(())
            })
            .expect("prepare artifact");

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
                let (status, generated_head): (String, String) = conn
                    .query_row(
                        "SELECT status, generated_head_sha FROM generated_artifacts LIMIT 1",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .expect("read artifact row");
                assert_eq!(status, "pending");
                assert_eq!(generated_head, "sha-2");
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
    fn inbox_populate_list_dismiss_and_neighbors() {
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

        let npub = "npub1test".to_string();
        let npubs = vec![npub.clone(), "npub2other".to_string()];

        // Populate inbox for both npubs
        let count = store.populate_inbox(pr_id_1, &npubs).unwrap();
        assert_eq!(count, 2); // 2 users got an item

        // Populate second PR with a later timestamp (CURRENT_TIMESTAMP has 1s resolution)
        store
            .with_connection(|conn| {
                for n in &npubs {
                    conn.execute(
                        "INSERT OR IGNORE INTO inbox(npub, pr_id, created_at) VALUES (?1, ?2, datetime('now', '+1 second'))",
                        params![n, pr_id_2],
                    )
                    .expect("insert inbox item with offset");
                }
                Ok(())
            })
            .unwrap();

        // Duplicate populate should be ignored
        let dup = store.populate_inbox(pr_id_1, &npubs).unwrap();
        assert_eq!(dup, 0);

        // Count
        let count = store.inbox_count(&npub).unwrap();
        assert_eq!(count, 2);

        // List (newest first)
        let items = store.list_inbox(&npub, 50, 0).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].pr_id, pr_id_2); // newest first
        assert_eq!(items[1].pr_id, pr_id_1);

        // Neighbors: pr_id_2 is first (newest), pr_id_1 is second
        let (prev, next) = store.inbox_neighbors(&npub, pr_id_2).unwrap();
        assert_eq!(prev, None); // no newer item
        assert_eq!(next, Some(pr_id_1)); // older item

        let (prev, next) = store.inbox_neighbors(&npub, pr_id_1).unwrap();
        assert_eq!(prev, Some(pr_id_2)); // newer item
        assert_eq!(next, None); // no older item

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

        store
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE generated_artifacts
                     SET status='ready', tutorial_json='{}', html='<p>ok</p>'",
                    [],
                )
                .expect("mark artifact ready");
                Ok(())
            })
            .expect("prepare ready artifact");

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
        let npub = "npub1dismissed";

        store.populate_inbox(pr_id, &[npub.to_string()]).unwrap();
        assert_eq!(store.inbox_count(npub).unwrap(), 1);

        store.dismiss_inbox_items(npub, &[pr_id]).unwrap();
        assert_eq!(store.inbox_count(npub).unwrap(), 0);

        assert_eq!(store.populate_inbox(pr_id, &[npub.to_string()]).unwrap(), 0);
        assert_eq!(store.backfill_inbox_for_npub(npub).unwrap(), 0);
        assert_eq!(store.inbox_count(npub).unwrap(), 0);
    }

    #[test]
    fn reset_artifact_to_pending_preserves_dismissals_until_new_ready_artifact() {
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
        let npub = "npub1regen";

        store.populate_inbox(pr_id, &[npub.to_string()]).unwrap();
        store.dismiss_inbox_items(npub, &[pr_id]).unwrap();
        assert_eq!(store.inbox_count(npub).unwrap(), 0);

        assert!(store.reset_artifact_to_pending(pr_id).unwrap());
        assert_eq!(store.populate_inbox(pr_id, &[npub.to_string()]).unwrap(), 0);
        assert_eq!(store.backfill_inbox_for_npub(npub).unwrap(), 0);
        assert_eq!(store.inbox_count(npub).unwrap(), 0);
    }

    #[test]
    fn mark_generation_ready_clears_dismissals_for_that_pr() {
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
        let artifact_id = store
            .claim_pending_generation_jobs(1)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .artifact_id;
        let npub = "npub1ready";

        store.populate_inbox(pr_id, &[npub.to_string()]).unwrap();
        store.dismiss_inbox_items(npub, &[pr_id]).unwrap();
        assert_eq!(store.inbox_count(npub).unwrap(), 0);

        store
            .mark_generation_ready(
                artifact_id,
                "{}",
                "<p>ok</p>",
                "sha-13",
                "diff",
                Some("sid"),
            )
            .unwrap();
        assert_eq!(store.backfill_inbox_for_npub(npub).unwrap(), 1);
        assert_eq!(store.inbox_count(npub).unwrap(), 1);
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

        let old_npub = "deadbeef".repeat(8);
        let new_npub = "npub1canonical";
        store
            .populate_inbox(pr_id_1, std::slice::from_ref(&old_npub))
            .unwrap();
        store
            .populate_inbox(pr_id_2, std::slice::from_ref(&old_npub))
            .unwrap();
        store.dismiss_inbox_items(&old_npub, &[pr_id_2]).unwrap();

        let moved = store.rekey_inbox_npub(&old_npub, new_npub).unwrap();
        assert_eq!(moved, 1);
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
        let canonical = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y";
        let raw_hex = PublicKey::parse(canonical).unwrap().to_hex();

        store
            .populate_inbox(pr_id, std::slice::from_ref(&raw_hex))
            .unwrap();

        let rekeyed = store.canonicalize_inbox_npubs().unwrap();
        assert_eq!(rekeyed, 1);
        assert_eq!(store.inbox_count(&raw_hex).unwrap(), 0);
        assert_eq!(store.inbox_count(canonical).unwrap(), 1);

        store.dismiss_all_inbox(canonical).unwrap();
        assert_eq!(store.backfill_inbox_for_npub(canonical).unwrap(), 0);
        assert_eq!(store.inbox_count(canonical).unwrap(), 0);
    }
}
