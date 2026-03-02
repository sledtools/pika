use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

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
    vec![Migration {
        version: 1,
        name: "0001_init",
        sql: include_str!("../migrations/0001_init.sql"),
    }]
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
}
