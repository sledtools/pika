use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::Utc;
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
                        "SELECT ga.id, r.repo, pr.pr_number, pr.title, pr.head_sha, pr.base_ref
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
            conn.execute(
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub created_at: String,
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
    ]
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{PrUpsertInput, Store};

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
}
