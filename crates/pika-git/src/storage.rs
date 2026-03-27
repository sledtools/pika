#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use nostr::key::PublicKey;
use nostr::nips::nip19::ToBech32;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::inbox_store::merge_branch_inbox_state_alias;

#[derive(Clone)]
pub struct Store {
    db_path: PathBuf,
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

    pub fn is_chat_allowlist_forge_writer(&self, npub: &str) -> anyhow::Result<bool> {
        self.with_connection(|conn| {
            let active = conn
                .query_row(
                    "SELECT 1
                     FROM chat_allowlist
                     WHERE npub = ?1 AND can_forge_write = 1",
                    params![npub],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .context("query forge writer allowlist entry")?
                .is_some();
            Ok(active)
        })
    }

    pub fn has_chat_allowlist_forge_writers(&self) -> anyhow::Result<bool> {
        self.with_connection(|conn| {
            let active = conn
                .query_row(
                    "SELECT 1 FROM chat_allowlist WHERE can_forge_write = 1 LIMIT 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .context("query forge writer allowlist entries")?
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
                    "SELECT npub, active, can_forge_write, note, updated_by, updated_at
                     FROM chat_allowlist
                     ORDER BY npub ASC",
                )
                .context("prepare chat allowlist list query")?;

            let rows = stmt
                .query_map([], |row| {
                    Ok(ChatAllowlistEntry {
                        npub: row.get(0)?,
                        active: row.get::<_, i64>(1)? != 0,
                        can_forge_write: row.get::<_, i64>(2)? != 0,
                        note: row.get(3)?,
                        updated_by: row.get(4)?,
                        updated_at: row.get(5)?,
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
                "SELECT npub, active, can_forge_write, note, updated_by, updated_at
                 FROM chat_allowlist
                 WHERE npub = ?1",
                params![npub],
                |row| {
                    Ok(ChatAllowlistEntry {
                        npub: row.get(0)?,
                        active: row.get::<_, i64>(1)? != 0,
                        can_forge_write: row.get::<_, i64>(2)? != 0,
                        note: row.get(3)?,
                        updated_by: row.get(4)?,
                        updated_at: row.get(5)?,
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
        can_forge_write: bool,
        note: Option<&str>,
        updated_by: &str,
    ) -> anyhow::Result<ChatAllowlistEntry> {
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start chat allowlist transaction")?;

            tx.execute(
                "INSERT INTO chat_allowlist(npub, active, can_forge_write, note, updated_by, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP)
                 ON CONFLICT(npub) DO UPDATE SET
                    active = excluded.active,
                    can_forge_write = excluded.can_forge_write,
                    note = excluded.note,
                    updated_by = excluded.updated_by,
                    updated_at = CURRENT_TIMESTAMP",
                params![
                    npub,
                    if active { 1 } else { 0 },
                    if can_forge_write { 1 } else { 0 },
                    note,
                    updated_by
                ],
            )
            .context("upsert chat allowlist entry")?;

            let action = if active && can_forge_write {
                "upsert_active_forge_writer"
            } else if active {
                "upsert_active_chat_only"
            } else if can_forge_write {
                "upsert_inactive_forge_writer"
            } else {
                "upsert_inactive_chat_only"
            };
            tx.execute(
                "INSERT INTO chat_allowlist_audit(actor_npub, target_npub, action, note)
                 VALUES (?1, ?2, ?3, ?4)",
                params![updated_by, npub, action, note],
            )
            .context("insert chat allowlist audit row")?;

            let entry = tx
                .query_row(
                    "SELECT npub, active, can_forge_write, note, updated_by, updated_at
                     FROM chat_allowlist
                     WHERE npub = ?1",
                    params![npub],
                    |row| {
                        Ok(ChatAllowlistEntry {
                            npub: row.get(0)?,
                            active: row.get::<_, i64>(1)? != 0,
                            can_forge_write: row.get::<_, i64>(2)? != 0,
                            note: row.get(3)?,
                            updated_by: row.get(4)?,
                            updated_at: row.get(5)?,
                        })
                    },
                )
                .context("reload chat allowlist entry")?;

            tx.commit().context("commit chat allowlist transaction")?;
            Ok(entry)
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
                         FROM branch_inbox_states
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

                let merged_branch = merge_branch_inbox_state_alias(&tx, &raw_npub, &canonical_npub)
                    .with_context(|| {
                        format!(
                            "merge branch inbox owner {} into canonical {}",
                            raw_npub, canonical_npub
                        )
                    })?;
                if merged_branch > 0 {
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
    pub review_id: i64,
    pub branch_id: i64,
    pub repo: String,
    pub branch_name: String,
    pub title: String,
    pub state: String,
    pub updated_at: String,
    pub generation_status: String,
    pub reason: String,
    pub inbox_created_at: String,
    pub needs_review: bool,
    pub last_reviewed_at: Option<String>,
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
    pub can_forge_write: bool,
    pub note: Option<String>,
    pub updated_by: String,
    pub updated_at: String,
}

pub(crate) fn artifact_user_state_rank(state: &str) -> u8 {
    match state {
        "dismissed" => 3,
        "inbox" => 2,
        "superseded" => 1,
        _ => 0,
    }
}

const MIGRATIONS_TABLE: &str = "_pika_git_migrations";
const LEGACY_MIGRATIONS_TABLE: &str = "_pika_news_migrations";

fn run_migrations(conn: &mut Connection) -> anyhow::Result<()> {
    ensure_migrations_table(conn)?;

    let create_migrations_table_sql = format!(
        "CREATE TABLE IF NOT EXISTS {MIGRATIONS_TABLE} (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );"
    );
    conn.execute_batch(&create_migrations_table_sql)
        .context("create migrations table")?;

    for migration in migrations() {
        let already_applied_sql =
            format!("SELECT EXISTS(SELECT 1 FROM {MIGRATIONS_TABLE} WHERE version = ?1)");
        let already_applied: bool = conn
            .query_row(&already_applied_sql, params![migration.version], |row| {
                row.get(0)
            })
            .with_context(|| format!("check migration version {}", migration.version))?;

        if already_applied {
            continue;
        }

        if migration.name == "0020_ci_queue_state_and_target_health"
            && ci_queue_state_schema_present(conn)?
        {
            let record_preapplied_sql =
                format!("INSERT INTO {MIGRATIONS_TABLE}(version, name) VALUES (?1, ?2)");
            conn.execute(
                &record_preapplied_sql,
                params![migration.version, migration.name],
            )
            .with_context(|| format!("record pre-applied migration {}", migration.name))?;
            continue;
        }

        let tx = conn
            .unchecked_transaction()
            .with_context(|| format!("start migration transaction {}", migration.name))?;
        tx.execute_batch(migration.sql)
            .with_context(|| format!("apply migration {}", migration.name))?;
        let record_migration_sql =
            format!("INSERT INTO {MIGRATIONS_TABLE}(version, name) VALUES (?1, ?2)");
        tx.execute(
            &record_migration_sql,
            params![migration.version, migration.name],
        )
        .with_context(|| format!("record migration {}", migration.name))?;
        tx.commit()
            .with_context(|| format!("commit migration {}", migration.name))?;
    }

    Ok(())
}

fn ensure_migrations_table(conn: &Connection) -> anyhow::Result<()> {
    let has_current = table_exists(conn, MIGRATIONS_TABLE)?;
    let has_legacy = table_exists(conn, LEGACY_MIGRATIONS_TABLE)?;

    if has_current && has_legacy {
        let merge_sql = format!(
            "INSERT OR IGNORE INTO {MIGRATIONS_TABLE}(version, name, applied_at)
             SELECT version, name, applied_at FROM {LEGACY_MIGRATIONS_TABLE}"
        );
        conn.execute(&merge_sql, [])
            .context("merge legacy migration ledger into current ledger")?;
        let drop_legacy_sql = format!("DROP TABLE {LEGACY_MIGRATIONS_TABLE}");
        conn.execute_batch(&drop_legacy_sql)
            .context("drop legacy migration ledger after merge")?;
        return Ok(());
    }

    if !has_current && has_legacy {
        let rename_sql =
            format!("ALTER TABLE {LEGACY_MIGRATIONS_TABLE} RENAME TO {MIGRATIONS_TABLE}");
        conn.execute_batch(&rename_sql)
            .context("rename legacy migration ledger to current name")?;
    }

    Ok(())
}

fn ci_queue_state_schema_present(conn: &Connection) -> anyhow::Result<bool> {
    let branch_has_execution_reason =
        table_has_column(conn, "branch_ci_run_lanes", "execution_reason")?;
    let nightly_has_execution_reason =
        table_has_column(conn, "nightly_run_lanes", "execution_reason")?;
    let branch_has_failure_kind = table_has_column(conn, "branch_ci_run_lanes", "failure_kind")?;
    let nightly_has_failure_kind = table_has_column(conn, "nightly_run_lanes", "failure_kind")?;
    Ok(branch_has_execution_reason
        && nightly_has_execution_reason
        && branch_has_failure_kind
        && nightly_has_failure_kind)
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> anyhow::Result<bool> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut stmt = conn
        .prepare(&pragma)
        .with_context(|| format!("prepare table info pragma for {table}"))?;
    let mut rows = stmt
        .query([])
        .with_context(|| format!("query table info pragma for {table}"))?;
    while let Some(row) = rows.next().context("read table info row")? {
        let name: String = row.get(1).context("read table info column name")?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn table_exists(conn: &Connection, table: &str) -> anyhow::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1
             FROM sqlite_master
             WHERE type = 'table' AND name = ?1
         )",
        params![table],
        |row| row.get::<_, i64>(0),
    )
    .map(|exists| exists != 0)
    .with_context(|| format!("query sqlite_master for table {table}"))
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
        Migration {
            version: 9,
            name: "0009_branch_forge",
            sql: include_str!("../migrations/0009_branch_forge.sql"),
        },
        Migration {
            version: 10,
            name: "0010_allowlist_forge_write",
            sql: include_str!("../migrations/0010_allowlist_forge_write.sql"),
        },
        Migration {
            version: 11,
            name: "0011_ci_lanes_and_nightlies",
            sql: include_str!("../migrations/0011_ci_lanes_and_nightlies.sql"),
        },
        Migration {
            version: 12,
            name: "0012_ci_lane_leases",
            sql: include_str!("../migrations/0012_ci_lane_leases.sql"),
        },
        Migration {
            version: 13,
            name: "0013_ci_lane_claim_tokens",
            sql: include_str!("../migrations/0013_ci_lane_claim_tokens.sql"),
        },
        Migration {
            version: 14,
            name: "0014_branch_inbox",
            sql: include_str!("../migrations/0014_branch_inbox.sql"),
        },
        Migration {
            version: 15,
            name: "0015_mirror_sync_runs",
            sql: include_str!("../migrations/0015_mirror_sync_runs.sql"),
        },
        Migration {
            version: 16,
            name: "0016_ci_rerun_provenance",
            sql: include_str!("../migrations/0016_ci_rerun_provenance.sql"),
        },
        Migration {
            version: 17,
            name: "0017_ci_lane_concurrency_groups",
            sql: include_str!("../migrations/0017_ci_lane_concurrency_groups.sql"),
        },
        Migration {
            version: 18,
            name: "0018_ci_lane_pikaci_metadata",
            sql: include_str!("../migrations/0018_ci_lane_pikaci_metadata.sql"),
        },
        Migration {
            version: 19,
            name: "0019_dismiss_closed_branch_inbox",
            sql: include_str!("../migrations/0019_dismiss_closed_branch_inbox.sql"),
        },
        Migration {
            version: 20,
            name: "0020_ci_queue_state_and_target_health",
            sql: include_str!("../migrations/0020_ci_queue_state_and_target_health.sql"),
        },
        Migration {
            version: 21,
            name: "0021_restore_branch_closed_inbox",
            sql: include_str!("../migrations/0021_restore_branch_closed_inbox.sql"),
        },
        Migration {
            version: 22,
            name: "0022_branch_inbox_review_progress",
            sql: include_str!("../migrations/0022_branch_inbox_review_progress.sql"),
        },
        Migration {
            version: 23,
            name: "0023_branch_chat",
            sql: include_str!("../migrations/0023_branch_chat.sql"),
        },
        Migration {
            version: 24,
            name: "0024_ci_lane_structured_pikaci_target",
            sql: include_str!("../migrations/0024_ci_lane_structured_pikaci_target.sql"),
        },
        Migration {
            version: 25,
            name: "0025_drop_legacy_pr_surface",
            sql: include_str!("../migrations/0025_drop_legacy_pr_surface.sql"),
        },
        Migration {
            version: 26,
            name: "0026_drop_ci_target_health",
            sql: include_str!("../migrations/0026_drop_ci_target_health.sql"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::fs;

    use nostr::key::PublicKey;
    use rusqlite::{params, Connection, OptionalExtension};

    use super::{
        migrations, table_exists, table_has_column, ChatAllowlistEntry, InboxItem,
        InboxReviewContext, Store, LEGACY_MIGRATIONS_TABLE, MIGRATIONS_TABLE,
    };
    use crate::branch_store::BranchUpsertInput;

    fn branch_upsert_input(branch_name: &str, head_sha: &str) -> BranchUpsertInput {
        BranchUpsertInput {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: "/tmp/pika.git".to_string(),
            default_branch: "master".to_string(),
            ci_entrypoint: "just pre-merge".to_string(),
            branch_name: branch_name.to_string(),
            title: format!("{branch_name} title"),
            head_sha: head_sha.to_string(),
            merge_base_sha: "base123".to_string(),
            author_name: Some("alice".to_string()),
            author_email: Some("alice@example.com".to_string()),
            updated_at: "2026-03-18T12:00:00Z".to_string(),
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
            .expect("lookup latest branch artifact id")
    }

    fn mark_latest_branch_artifact_ready(store: &Store, branch_id: i64, head_sha: &str) -> i64 {
        let artifact_id = latest_branch_artifact_id(store, branch_id);
        store
            .mark_branch_generation_ready(
                artifact_id,
                r#"{"executive_summary":"ok","media_links":[],"steps":[]}"#,
                "<p>ok</p>",
                head_sha,
                "diff",
                None,
            )
            .expect("mark branch artifact ready");
        artifact_id
    }

    fn branch_inbox_item_ids(items: &[InboxItem]) -> Vec<i64> {
        items.iter().map(|item| item.review_id).collect()
    }

    fn branch_inbox_state(
        store: &Store,
        npub: &str,
        branch_id: i64,
    ) -> Option<(i64, String, Option<i64>, Option<String>)> {
        store
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT artifact_id, state, last_reviewed_artifact_id, last_reviewed_at
                     FROM branch_inbox_states
                     WHERE npub = ?1 AND branch_id = ?2",
                    params![npub, branch_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(Into::into)
            })
            .expect("query branch inbox state")
    }

    #[test]
    fn migrations_apply_and_reapply_without_data_loss() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");

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
                assert!(table_exists(conn, MIGRATIONS_TABLE)?);
                assert!(!table_exists(conn, LEGACY_MIGRATIONS_TABLE)?);
                assert!(!table_exists(conn, "pull_requests")?);
                assert!(!table_exists(conn, "generated_artifacts")?);
                assert!(!table_exists(conn, "artifact_versions")?);
                assert!(!table_exists(conn, "artifact_user_states")?);
                assert!(!table_exists(conn, "chat_sessions")?);
                assert!(!table_exists(conn, "chat_messages")?);
                assert!(!table_exists(conn, "inbox")?);
                assert!(!table_exists(conn, "inbox_dismissals")?);
                assert!(!table_exists(conn, "poll_markers")?);
                Ok(())
            })
            .expect("verify data");

        fs::remove_file(db_path).expect("cleanup sqlite db file");
    }

    #[test]
    fn queue_state_migration_applies_after_legacy_version_19() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let conn = Connection::open(&db_path).expect("open sqlite");

        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {LEGACY_MIGRATIONS_TABLE} (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );"
        ))
        .expect("create migrations table");

        for migration in migrations()
            .into_iter()
            .filter(|migration| migration.version <= 19)
        {
            let tx = conn
                .unchecked_transaction()
                .expect("start migration transaction");
            tx.execute_batch(migration.sql).expect("apply migration");
            tx.execute(
                &format!("INSERT INTO {LEGACY_MIGRATIONS_TABLE}(version, name) VALUES (?1, ?2)"),
                params![migration.version, migration.name],
            )
            .expect("record migration");
            tx.commit().expect("commit migration");
        }
        drop(conn);

        let store = Store::open(&db_path).expect("open store after legacy migrations");
        store
            .with_connection(|conn| {
                assert!(table_has_column(
                    conn,
                    "branch_ci_run_lanes",
                    "execution_reason"
                )?);
                assert!(table_has_column(
                    conn,
                    "branch_ci_run_lanes",
                    "failure_kind"
                )?);
                assert!(table_has_column(
                    conn,
                    "nightly_run_lanes",
                    "execution_reason"
                )?);
                assert!(table_has_column(conn, "nightly_run_lanes", "failure_kind")?);
                assert!(!table_exists(conn, "ci_target_health")?);
                assert!(table_exists(conn, MIGRATIONS_TABLE)?);
                assert!(!table_exists(conn, LEGACY_MIGRATIONS_TABLE)?);
                let migration_name: String = conn.query_row(
                    &format!("SELECT name FROM {MIGRATIONS_TABLE} WHERE version = 20"),
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(migration_name, "0020_ci_queue_state_and_target_health");
                Ok::<_, anyhow::Error>(())
            })
            .expect("verify queue state migration");
    }

    #[test]
    fn drop_target_health_migration_rewrites_legacy_unhealthy_rows() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let conn = Connection::open(&db_path).expect("open sqlite");

        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {MIGRATIONS_TABLE} (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );"
        ))
        .expect("create migrations table");

        for migration in migrations()
            .into_iter()
            .filter(|migration| migration.version <= 25)
        {
            let tx = conn
                .unchecked_transaction()
                .expect("start migration transaction");
            tx.execute_batch(migration.sql).expect("apply migration");
            tx.execute(
                &format!("INSERT INTO {MIGRATIONS_TABLE}(version, name) VALUES (?1, ?2)"),
                params![migration.version, migration.name],
            )
            .expect("record migration");
            tx.commit().expect("commit migration");
        }

        conn.execute(
            "INSERT INTO repos(repo, default_branch) VALUES (?1, ?2)",
            params!["sledtools/pika", "master"],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repos WHERE repo = ?1",
                params!["sledtools/pika"],
                |row| row.get(0),
            )
            .expect("lookup repo id");
        conn.execute(
            "INSERT INTO branch_records(
                repo_id, branch_name, target_branch, title, state, head_sha, merge_base_sha, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                repo_id,
                "feature/rewrite-unhealthy",
                "master",
                "feature/rewrite-unhealthy title",
                "open",
                "head-1",
                "base-1",
                "2026-03-26T12:00:00Z"
            ],
        )
        .expect("insert branch");
        let branch_id: i64 = conn
            .query_row(
                "SELECT id FROM branch_records WHERE branch_name = ?1",
                params!["feature/rewrite-unhealthy"],
                |row| row.get(0),
            )
            .expect("lookup branch id");
        conn.execute(
            "INSERT INTO branch_ci_runs(
                branch_id, source_head_sha, entrypoint, command_json, status
             ) VALUES (?1, ?2, ?3, ?4, 'queued')",
            params![branch_id, "head-1", "just pre-merge", "[]"],
        )
        .expect("insert branch run");
        let branch_run_id: i64 = conn
            .query_row(
                "SELECT id FROM branch_ci_runs WHERE branch_id = ?1",
                params![branch_id],
                |row| row.get(0),
            )
            .expect("lookup branch run");
        conn.execute(
            "INSERT INTO branch_ci_run_lanes(
                branch_ci_run_id, lane_id, title, entrypoint, command_json, status, execution_reason
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'queued', 'target_unhealthy')",
            params![
                branch_run_id,
                "branch-lane",
                "branch lane",
                "just branch",
                "[]"
            ],
        )
        .expect("insert branch lane");

        conn.execute(
            "INSERT INTO nightly_runs(
                repo_id, source_ref, source_head_sha, status, scheduled_for
             ) VALUES (?1, ?2, ?3, 'queued', ?4)",
            params![
                repo_id,
                "refs/heads/master",
                "head-1",
                "2026-03-26T08:00:00Z"
            ],
        )
        .expect("insert nightly run");
        let nightly_run_id: i64 = conn
            .query_row(
                "SELECT id FROM nightly_runs WHERE repo_id = ?1",
                params![repo_id],
                |row| row.get(0),
            )
            .expect("lookup nightly run");
        conn.execute(
            "INSERT INTO nightly_run_lanes(
                nightly_run_id, lane_id, title, entrypoint, command_json, status, execution_reason
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'queued', 'target_unhealthy')",
            params![
                nightly_run_id,
                "nightly-lane",
                "nightly lane",
                "just nightly",
                "[]"
            ],
        )
        .expect("insert nightly lane");

        conn.execute(
            "INSERT INTO ci_target_health(target_id, state, consecutive_infra_failure_count, updated_at)
             VALUES (?1, 'unhealthy', 2, CURRENT_TIMESTAMP)",
            params!["apple-host"],
        )
        .expect("insert target health");
        drop(conn);

        let store = Store::open(&db_path).expect("open store after migration 26");
        store
            .with_connection(|conn| {
                let branch_reason: String = conn.query_row(
                    "SELECT execution_reason FROM branch_ci_run_lanes WHERE branch_ci_run_id = ?1",
                    params![branch_run_id],
                    |row| row.get(0),
                )?;
                let nightly_reason: String = conn.query_row(
                    "SELECT execution_reason FROM nightly_run_lanes WHERE nightly_run_id = ?1",
                    params![nightly_run_id],
                    |row| row.get(0),
                )?;
                assert_eq!(branch_reason, "queued");
                assert_eq!(nightly_reason, "queued");
                assert!(!table_exists(conn, "ci_target_health")?);
                let migration_name: String = conn.query_row(
                    &format!("SELECT name FROM {MIGRATIONS_TABLE} WHERE version = 26"),
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(migration_name, "0026_drop_ci_target_health");
                Ok::<_, anyhow::Error>(())
            })
            .expect("verify drop target health migration");
    }

    #[test]
    fn branch_closed_inbox_restore_migration_applies_after_legacy_version_20() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let conn = Connection::open(&db_path).expect("open sqlite");

        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {LEGACY_MIGRATIONS_TABLE} (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );"
        ))
        .expect("create migrations table");

        for migration in migrations()
            .into_iter()
            .filter(|migration| migration.version <= 20)
        {
            let tx = conn
                .unchecked_transaction()
                .expect("start migration transaction");
            tx.execute_batch(migration.sql).expect("apply migration");
            tx.execute(
                &format!("INSERT INTO {LEGACY_MIGRATIONS_TABLE}(version, name) VALUES (?1, ?2)"),
                params![migration.version, migration.name],
            )
            .expect("record migration");
            tx.commit().expect("commit migration");
        }

        conn.execute(
            "INSERT INTO repos(repo, default_branch) VALUES (?1, ?2)",
            params!["sledtools/pika", "master"],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repos WHERE repo = ?1",
                params!["sledtools/pika"],
                |row| row.get(0),
            )
            .expect("lookup repo id");
        conn.execute(
            "INSERT INTO branch_records(
                repo_id, branch_name, target_branch, title, state, head_sha, merge_base_sha, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                repo_id,
                "feature/restored",
                "master",
                "feature/restored title",
                "closed",
                "head-1",
                "base-1",
                "2026-03-24T12:00:00Z"
            ],
        )
        .expect("insert branch");
        let branch_id: i64 = conn
            .query_row(
                "SELECT id FROM branch_records WHERE branch_name = ?1",
                params!["feature/restored"],
                |row| row.get(0),
            )
            .expect("lookup branch id");
        conn.execute(
            "INSERT INTO branch_artifact_versions(
                branch_id, version, source_head_sha, merge_base_sha, status, is_current, created_at, ready_at, updated_at
             ) VALUES (?1, 1, ?2, ?3, 'ready', 1, ?4, ?4, ?4)",
            params![branch_id, "head-1", "base-1", "2026-03-24T12:00:00Z"],
        )
        .expect("insert branch artifact");
        let artifact_id: i64 = conn
            .query_row(
                "SELECT id FROM branch_artifact_versions WHERE branch_id = ?1",
                params![branch_id],
                |row| row.get(0),
            )
            .expect("lookup branch artifact id");
        conn.execute(
            "INSERT INTO branch_inbox_states(
                npub, branch_id, artifact_id, state, reason, created_at, updated_at, dismissed_at
             ) VALUES (?1, ?2, ?3, 'dismissed', 'branch_closed', ?4, ?4, ?4)",
            params![
                "npub1reviewer",
                branch_id,
                artifact_id,
                "2026-03-24T12:00:00Z"
            ],
        )
        .expect("insert auto-dismissed inbox row");
        drop(conn);

        let store = Store::open(&db_path).expect("open store after legacy migrations");
        store
            .with_connection(|conn| {
                let row: (String, String, Option<String>) = conn.query_row(
                    "SELECT state, reason, dismissed_at
                     FROM branch_inbox_states
                     WHERE npub = ?1 AND branch_id = ?2",
                    params!["npub1reviewer", branch_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
                assert_eq!(row.0, "inbox");
                assert_eq!(row.1, "generation_ready");
                assert_eq!(row.2, None);
                assert!(table_exists(conn, MIGRATIONS_TABLE)?);
                assert!(!table_exists(conn, LEGACY_MIGRATIONS_TABLE)?);
                let migration_name: String = conn.query_row(
                    &format!("SELECT name FROM {MIGRATIONS_TABLE} WHERE version = 21"),
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(migration_name, "0021_restore_branch_closed_inbox");
                Ok::<_, anyhow::Error>(())
            })
            .expect("verify branch inbox restore migration");
    }

    #[test]
    fn branch_inbox_populate_list_dismiss_and_review_context() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");

        let first = store
            .upsert_branch_record(&branch_upsert_input("feature/one", "head-one"))
            .unwrap();
        let second = store
            .upsert_branch_record(&branch_upsert_input("feature/two", "head-two"))
            .unwrap();
        let artifact_id_1 = mark_latest_branch_artifact_ready(&store, first.branch_id, "head-one");
        let artifact_id_2 = mark_latest_branch_artifact_ready(&store, second.branch_id, "head-two");

        let npub = "npub1branch".to_string();
        let npubs = vec![npub.clone(), "npub1other".to_string()];

        let count = store.populate_branch_inbox(artifact_id_1, &npubs).unwrap();
        assert_eq!(count, 2);

        store
            .with_connection(|conn| {
                for n in &npubs {
                    conn.execute(
                        "INSERT INTO branch_inbox_states(npub, branch_id, artifact_id, state, reason, created_at, updated_at)
                         VALUES (?1, ?2, ?3, 'inbox', 'generation_ready', datetime('now', '+1 second'), datetime('now', '+1 second'))
                         ON CONFLICT(npub, branch_id) DO UPDATE SET
                            artifact_id = excluded.artifact_id,
                            state = excluded.state,
                            reason = excluded.reason,
                            created_at = excluded.created_at,
                            updated_at = excluded.updated_at,
                            dismissed_at = NULL",
                        params![n, second.branch_id, artifact_id_2],
                    )?;
                }
                Ok(())
            })
            .unwrap();

        let items = store.list_branch_inbox(&npub, 50, 0).unwrap();
        assert_eq!(
            branch_inbox_item_ids(&items),
            vec![second.branch_id, first.branch_id]
        );
        assert_eq!(items[0].branch_name, "feature/two");
        assert_eq!(items[1].branch_name, "feature/one");
        assert_eq!(store.branch_inbox_count(&npub).unwrap(), 2);

        let first_ctx = store
            .branch_inbox_review_context(&npub, first.branch_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            first_ctx,
            InboxReviewContext {
                prev: Some(second.branch_id),
                next: None,
                position: 2,
                total: 2,
            }
        );
        let second_ctx = store
            .branch_inbox_review_context(&npub, second.branch_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            second_ctx,
            InboxReviewContext {
                prev: None,
                next: Some(first.branch_id),
                position: 1,
                total: 2,
            }
        );

        assert_eq!(
            store
                .mark_branch_inbox_reviewed(&npub, second.branch_id)
                .unwrap(),
            1
        );
        assert_eq!(store.branch_inbox_count(&npub).unwrap(), 1);
        let reviewed_ctx = store
            .branch_inbox_review_context(&npub, second.branch_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            reviewed_ctx,
            InboxReviewContext {
                prev: None,
                next: Some(first.branch_id),
                position: 1,
                total: 2,
            }
        );

        assert_eq!(
            store
                .dismiss_branch_inbox_items(&npub, &[first.branch_id])
                .unwrap(),
            1
        );
        assert_eq!(store.branch_inbox_count(&npub).unwrap(), 0);
        assert_eq!(store.dismiss_all_branch_inbox(&npub).unwrap(), 1);
        assert_eq!(store.branch_inbox_count(&npub).unwrap(), 0);
        assert_eq!(store.branch_inbox_count("npub1other").unwrap(), 2);
    }

    #[test]
    fn branch_inbox_items_survive_branch_lifecycle_changes_until_dismissed() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");

        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/persistent", "head-1"))
            .unwrap();
        let artifact_id = mark_latest_branch_artifact_ready(&store, branch.branch_id, "head-1");
        let npub = "npub1persistent";

        assert_eq!(
            store
                .populate_branch_inbox(artifact_id, &[npub.to_string()])
                .unwrap(),
            1
        );
        store
            .mark_branch_closed(branch.branch_id, "npub1trusted")
            .expect("close branch");

        let items = store.list_branch_inbox(npub, 50, 0).unwrap();
        assert_eq!(branch_inbox_item_ids(&items), vec![branch.branch_id]);
        assert_eq!(items[0].state, "closed");
        assert!(items[0].needs_review);
        assert_eq!(store.branch_inbox_count(npub).unwrap(), 1);
        assert_eq!(
            store
                .branch_inbox_review_context(npub, branch.branch_id)
                .unwrap(),
            Some(InboxReviewContext {
                prev: None,
                next: None,
                position: 1,
                total: 1,
            })
        );

        store
            .dismiss_branch_inbox_items(npub, &[branch.branch_id])
            .expect("dismiss closed branch inbox item");
        assert_eq!(store.branch_inbox_count(npub).unwrap(), 0);
    }

    #[test]
    fn merged_branch_stays_in_inbox_until_user_dismisses_it() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");

        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/merged-persistent", "head-1"))
            .unwrap();
        let artifact_id = mark_latest_branch_artifact_ready(&store, branch.branch_id, "head-1");
        let npub = "npub1merged";

        assert_eq!(
            store
                .populate_branch_inbox(artifact_id, &[npub.to_string()])
                .unwrap(),
            1
        );
        store
            .mark_branch_merged(branch.branch_id, "npub1trusted", "merge-sha-1")
            .expect("merge branch");

        let items = store.list_branch_inbox(npub, 50, 0).unwrap();
        assert_eq!(branch_inbox_item_ids(&items), vec![branch.branch_id]);
        assert_eq!(items[0].state, "merged");
        assert!(items[0].needs_review);
        assert_eq!(store.branch_inbox_count(npub).unwrap(), 1);
    }

    #[test]
    fn branch_backfill_includes_closed_ready_branches() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");

        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/closed-backfill", "head-1"))
            .unwrap();
        mark_latest_branch_artifact_ready(&store, branch.branch_id, "head-1");
        store
            .mark_branch_closed(branch.branch_id, "npub1trusted")
            .expect("close branch");

        let npub = "npub1late-reviewer";
        assert_eq!(store.backfill_branch_inbox_for_npub(npub).unwrap(), 1);

        let items = store.list_branch_inbox(npub, 50, 0).unwrap();
        assert_eq!(branch_inbox_item_ids(&items), vec![branch.branch_id]);
        assert_eq!(items[0].state, "closed");
    }

    #[test]
    fn branch_backfill_and_new_ready_artifact_restore_dismissed_item() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");

        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/review", "head-1"))
            .unwrap();
        let first_artifact_id =
            mark_latest_branch_artifact_ready(&store, branch.branch_id, "head-1");
        let npub = "npub1reviewer";

        assert_eq!(store.backfill_branch_inbox_for_npub(npub).unwrap(), 1);
        let items = store.list_branch_inbox(npub, 50, 0).unwrap();
        assert_eq!(branch_inbox_item_ids(&items), vec![branch.branch_id]);
        assert_eq!(items[0].review_id, branch.branch_id);
        assert!(items[0].needs_review);

        assert_eq!(
            store
                .mark_branch_inbox_reviewed(npub, branch.branch_id)
                .unwrap(),
            1
        );
        let reviewed_items = store.list_branch_inbox(npub, 50, 0).unwrap();
        assert_eq!(reviewed_items.len(), 1);
        assert!(!reviewed_items[0].needs_review);
        assert_eq!(store.branch_inbox_count(npub).unwrap(), 0);
        let (_, _, reviewed_artifact_id, reviewed_at) =
            branch_inbox_state(&store, npub, branch.branch_id).expect("reviewed state");
        assert_eq!(reviewed_artifact_id, Some(first_artifact_id));
        assert!(reviewed_at.is_some());

        store
            .dismiss_branch_inbox_items(npub, &[branch.branch_id])
            .unwrap();
        assert_eq!(store.branch_inbox_count(npub).unwrap(), 0);
        assert_eq!(store.backfill_branch_inbox_for_npub(npub).unwrap(), 0);

        let reopened = store
            .upsert_branch_record(&branch_upsert_input("feature/review", "head-2"))
            .unwrap();
        assert_eq!(reopened.branch_id, branch.branch_id);
        let second_artifact_id =
            mark_latest_branch_artifact_ready(&store, branch.branch_id, "head-2");
        assert_ne!(first_artifact_id, second_artifact_id);

        assert_eq!(
            store
                .populate_branch_inbox(second_artifact_id, &[npub.to_string()])
                .unwrap(),
            1
        );
        let items = store.list_branch_inbox(npub, 50, 0).unwrap();
        assert_eq!(branch_inbox_item_ids(&items), vec![branch.branch_id]);
        assert!(items[0].needs_review);
        assert_eq!(store.branch_inbox_count(npub).unwrap(), 1);
    }

    #[test]
    fn dismiss_is_per_user_and_review_progress_is_stable() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");

        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/per-user", "head-1"))
            .unwrap();
        let artifact_id = mark_latest_branch_artifact_ready(&store, branch.branch_id, "head-1");
        let alice = "npub1alice";
        let bob = "npub1bob";

        store
            .populate_branch_inbox(artifact_id, &[alice.to_string(), bob.to_string()])
            .unwrap();
        assert_eq!(
            store
                .mark_branch_inbox_reviewed(alice, branch.branch_id)
                .unwrap(),
            1
        );
        assert_eq!(store.branch_inbox_count(alice).unwrap(), 0);
        assert_eq!(store.branch_inbox_count(bob).unwrap(), 1);

        store
            .dismiss_branch_inbox_items(alice, &[branch.branch_id])
            .unwrap();
        assert!(store.list_branch_inbox(alice, 50, 0).unwrap().is_empty());
        let bob_items = store.list_branch_inbox(bob, 50, 0).unwrap();
        assert_eq!(branch_inbox_item_ids(&bob_items), vec![branch.branch_id]);
        assert!(bob_items[0].needs_review);
    }

    #[test]
    fn chat_allowlist_upsert_list_and_active_lookup_work() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");

        assert!(!store.has_active_chat_allowlist_entries().unwrap());
        assert!(!store.has_chat_allowlist_forge_writers().unwrap());
        assert!(!store.is_chat_allowlist_active("npub1missing").unwrap());
        assert!(!store
            .is_chat_allowlist_forge_writer("npub1missing")
            .unwrap());

        let saved = store
            .upsert_chat_allowlist_entry("npub1alice", true, true, Some("friend"), "npub1admin")
            .expect("upsert allowlist entry");
        assert_eq!(
            saved,
            ChatAllowlistEntry {
                npub: "npub1alice".to_string(),
                active: true,
                can_forge_write: true,
                note: Some("friend".to_string()),
                updated_by: "npub1admin".to_string(),
                updated_at: saved.updated_at.clone(),
            }
        );

        assert!(store.has_active_chat_allowlist_entries().unwrap());
        assert!(store.has_chat_allowlist_forge_writers().unwrap());
        assert!(store.is_chat_allowlist_active("npub1alice").unwrap());
        assert!(store.is_chat_allowlist_forge_writer("npub1alice").unwrap());

        let rows = store.list_chat_allowlist_entries().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].npub, "npub1alice");
        assert!(rows[0].can_forge_write);
        assert_eq!(rows[0].note.as_deref(), Some("friend"));

        let updated = store
            .upsert_chat_allowlist_entry("npub1alice", false, false, Some("paused"), "npub1admin")
            .expect("deactivate allowlist entry");
        assert!(!updated.active);
        assert!(!updated.can_forge_write);
        assert!(!store.is_chat_allowlist_active("npub1alice").unwrap());
        assert!(!store.is_chat_allowlist_forge_writer("npub1alice").unwrap());
        assert!(!store.has_chat_allowlist_forge_writers().unwrap());

        let active = store.list_active_chat_allowlist_npubs().unwrap();
        assert!(active.is_empty());
        let reviewable = store.list_branch_inbox_allowlist_npubs().unwrap();
        assert!(reviewable.is_empty());
    }

    #[test]
    fn inactive_forge_writer_still_has_forge_write_access() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");
        store
            .upsert_chat_allowlist_entry(
                "npub1forgeonly",
                false,
                true,
                Some("forge only"),
                "npub1admin",
            )
            .expect("upsert forge-only allowlist entry");

        assert!(!store.has_active_chat_allowlist_entries().unwrap());
        assert!(store.has_chat_allowlist_forge_writers().unwrap());
        assert!(!store.is_chat_allowlist_active("npub1forgeonly").unwrap());
        assert!(store
            .is_chat_allowlist_forge_writer("npub1forgeonly")
            .unwrap());
        assert_eq!(
            store.list_branch_inbox_allowlist_npubs().unwrap(),
            vec!["npub1forgeonly".to_string()]
        );
    }

    #[test]
    fn branch_chat_sessions_are_scoped_to_the_current_artifact_version() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");

        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/branch-chat", "head-1"))
            .expect("insert branch");

        mark_latest_branch_artifact_ready(&store, branch.branch_id, "head-1");

        let (first_session_id, first_messages) = store
            .get_or_create_branch_review_chat_session(
                latest_branch_artifact_id(&store, branch.branch_id),
                "npub1chat",
                "sid-v1",
            )
            .expect("create initial branch chat session");
        assert!(first_messages.is_empty());
        store
            .append_branch_review_chat_message(first_session_id, "user", "first branch version")
            .expect("append first branch chat message");

        let reopened = store
            .upsert_branch_record(&branch_upsert_input("feature/branch-chat", "head-2"))
            .expect("upsert reopened branch");
        assert_eq!(reopened.branch_id, branch.branch_id);
        let next_artifact_id = latest_branch_artifact_id(&store, branch.branch_id);
        store
            .mark_branch_generation_ready(
                next_artifact_id,
                "{}",
                "<p>new</p>",
                "head-2",
                "diff-2",
                Some("branch-sid-v2"),
            )
            .expect("mark next branch artifact ready");

        let (second_session_id, second_messages) = store
            .get_or_create_branch_review_chat_session(next_artifact_id, "npub1chat", "sid-v2")
            .expect("create second branch chat session");
        assert_ne!(first_session_id, second_session_id);
        assert!(second_messages.is_empty());
    }

    #[test]
    fn canonicalize_inbox_npubs_rekeys_branch_inbox_rows() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");

        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/canonical", "head-1"))
            .unwrap();
        let artifact_id = mark_latest_branch_artifact_ready(&store, branch.branch_id, "head-1");
        let canonical = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y";
        let raw_hex = PublicKey::parse(canonical).unwrap().to_hex();

        store
            .populate_branch_inbox(artifact_id, std::slice::from_ref(&raw_hex))
            .unwrap();

        let rekeyed = store.canonicalize_inbox_npubs().unwrap();
        assert_eq!(rekeyed, 1);
        assert_eq!(store.branch_inbox_count(&raw_hex).unwrap(), 0);
        assert_eq!(store.branch_inbox_count(canonical).unwrap(), 1);
    }
}
