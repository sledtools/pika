#![allow(dead_code)]

use std::cmp::{max, min};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use nostr::key::PublicKey;
use nostr::nips::nip19::ToBech32;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::inbox_store::merge_branch_inbox_state_alias;
#[allow(unused_imports)]
pub use crate::legacy_pr_store::{
    FeedItem, GenerationJob, PrDetailRecord, PrSummaryRecord, PrUpsertInput, UpsertOutcome,
};

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

    // --- Inbox methods ---

    pub fn populate_inbox(&self, artifact_id: i64, npubs: &[String]) -> anyhow::Result<usize> {
        if npubs.is_empty() {
            return Ok(0);
        }
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start populate inbox transaction")?;
            let count = insert_inbox_rows_for_artifact(&tx, artifact_id, npubs)?;
            tx.commit().context("commit populate inbox transaction")?;
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
                        review_id: row.get(0)?,
                        branch_id: None,
                        pr_id: Some(row.get(0)?),
                        repo: row.get(1)?,
                        branch_name: None,
                        pr_number: Some(row.get(2)?),
                        title: row.get(3)?,
                        url: Some(row.get(4)?),
                        state: row.get(5)?,
                        updated_at: row.get(6)?,
                        generation_status: row
                            .get::<_, Option<String>>(7)?
                            .unwrap_or_else(|| "pending".to_string()),
                        reason: row.get(8)?,
                        inbox_created_at: row.get(9)?,
                        needs_review: true,
                        last_reviewed_at: None,
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
                     ORDER BY aus.created_at DESC
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
                       AND av.pr_id != ?3
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
                       AND av.pr_id != ?3
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
                    "SELECT COUNT(DISTINCT av.pr_id)
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
                    "SELECT COUNT(DISTINCT av.pr_id)
                     FROM artifact_user_states aus
                     JOIN artifact_versions av ON av.id = aus.artifact_id
                     WHERE aus.npub = ?1 AND aus.state = 'inbox'",
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
                .context("merge inbox state rows for rekey")?
                + merge_branch_inbox_state_alias(&tx, from_npub, to_npub)
                    .context("merge branch inbox state rows for rekey")?;

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
                         FROM (
                            SELECT npub FROM artifact_user_states
                            UNION
                            SELECT npub FROM branch_inbox_states
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

                let merged_legacy = merge_inbox_state_alias(&tx, &raw_npub, &canonical_npub)
                    .with_context(|| {
                        format!(
                            "merge inbox owner {} into canonical {}",
                            raw_npub, canonical_npub
                        )
                    })?;
                let merged_branch = merge_branch_inbox_state_alias(&tx, &raw_npub, &canonical_npub)
                    .with_context(|| {
                        format!(
                            "merge branch inbox owner {} into canonical {}",
                            raw_npub, canonical_npub
                        )
                    })?;
                if merged_legacy + merged_branch > 0 {
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
    pub branch_id: Option<i64>,
    pub pr_id: Option<i64>,
    pub repo: String,
    pub branch_name: Option<String>,
    pub pr_number: Option<i64>,
    pub title: String,
    pub url: Option<String>,
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

pub(crate) fn artifact_user_state_rank(state: &str) -> u8 {
    match state {
        "dismissed" => 3,
        "inbox" => 2,
        "superseded" => 1,
        _ => 0,
    }
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

        if migration.name == "0020_ci_queue_state_and_target_health"
            && ci_queue_state_schema_present(conn)?
        {
            conn.execute(
                "INSERT INTO _pika_news_migrations(version, name) VALUES (?1, ?2)",
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

fn ci_queue_state_schema_present(conn: &Connection) -> anyhow::Result<bool> {
    let branch_has_execution_reason =
        table_has_column(conn, "branch_ci_run_lanes", "execution_reason")?;
    let nightly_has_execution_reason =
        table_has_column(conn, "nightly_run_lanes", "execution_reason")?;
    let branch_has_failure_kind = table_has_column(conn, "branch_ci_run_lanes", "failure_kind")?;
    let nightly_has_failure_kind = table_has_column(conn, "nightly_run_lanes", "failure_kind")?;
    let has_target_health = table_exists(conn, "ci_target_health")?;
    Ok(branch_has_execution_reason
        && nightly_has_execution_reason
        && branch_has_failure_kind
        && nightly_has_failure_kind
        && has_target_health)
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
    ]
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use nostr::key::PublicKey;
    use rusqlite::{params, Connection, OptionalExtension};

    use super::{
        migrations, table_exists, table_has_column, ChatAllowlistEntry, InboxItem,
        InboxReviewContext, PrUpsertInput, Store,
    };
    use crate::branch_store::BranchUpsertInput;

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
    fn queue_state_migration_applies_after_legacy_version_19() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let conn = Connection::open(&db_path).expect("open sqlite");

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _pika_news_migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );",
        )
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
                "INSERT INTO _pika_news_migrations(version, name) VALUES (?1, ?2)",
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
                assert!(table_exists(conn, "ci_target_health")?);
                let migration_name: String = conn.query_row(
                    "SELECT name FROM _pika_news_migrations WHERE version = 20",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(migration_name, "0020_ci_queue_state_and_target_health");
                Ok::<_, anyhow::Error>(())
            })
            .expect("verify queue state migration");
    }

    #[test]
    fn branch_closed_inbox_restore_migration_applies_after_legacy_version_20() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let conn = Connection::open(&db_path).expect("open sqlite");

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _pika_news_migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );",
        )
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
                "INSERT INTO _pika_news_migrations(version, name) VALUES (?1, ?2)",
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
                let migration_name: String = conn.query_row(
                    "SELECT name FROM _pika_news_migrations WHERE version = 21",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(migration_name, "0021_restore_branch_closed_inbox");
                Ok::<_, anyhow::Error>(())
            })
            .expect("verify branch inbox restore migration");
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
        assert_eq!(items[0].pr_id, Some(pr_id_1));
        assert_eq!(items[1].pr_id, Some(pr_id_2));

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
    fn review_context_never_returns_current_pr_as_neighbor() {
        // Regression: if a PR has two inbox-state artifact entries (race between
        // populate_inbox and backfill), the neighbor query must never return the
        // current pr_id as next/prev.
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let input = PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: 42,
            title: "Duplicate inbox entry".to_string(),
            url: "https://github.com/sledtools/pika/pull/42".to_string(),
            state: "open".to_string(),
            head_sha: "sha-42".to_string(),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: "2026-03-01T00:00:00Z".to_string(),
            merged_at: None,
        };
        store.upsert_pull_request(&input).unwrap();
        let pr_id = store.list_feed_items().unwrap()[0].pr_id;
        let artifact_id_1 = mark_latest_artifact_ready(&store, pr_id, "sha-42");

        let npub = "npub1dup";

        // Populate normally — creates one inbox entry for artifact_id_1
        store
            .populate_inbox(artifact_id_1, &[npub.to_string()])
            .unwrap();

        // Simulate a second artifact for the same PR that also ended up with
        // state='inbox' (e.g. race between populate and backfill).
        store
            .with_connection(|conn| {
                // Create a second artifact version
                conn.execute(
                    "INSERT INTO artifact_versions(pr_id, version, status, is_current, source_head_sha)
                     VALUES (?1, 2, 'ready', 0, 'sha-42-v2')",
                    params![pr_id],
                )?;
                let artifact_id_2 = conn.last_insert_rowid();
                // Force a second inbox entry with a later timestamp
                conn.execute(
                    "INSERT INTO artifact_user_states(npub, artifact_id, state, reason, created_at, updated_at)
                     VALUES (?1, ?2, 'inbox', 'generation_ready', datetime('now', '+1 second'), datetime('now', '+1 second'))",
                    params![npub, artifact_id_2],
                )?;
                Ok(())
            })
            .unwrap();

        // Now the same PR has two inbox entries. The review context must not
        // return pr_id as next or prev.
        let ctx = store
            .inbox_review_context(npub, pr_id)
            .unwrap()
            .expect("review context should exist");

        assert_ne!(ctx.next, Some(pr_id), "next must not be the current PR");
        assert_ne!(ctx.prev, Some(pr_id), "prev must not be the current PR");
        assert_eq!(ctx.next, None);
        assert_eq!(ctx.prev, None);
        // Position and total should count distinct PRs, not duplicate entries
        assert_eq!(ctx.position, 1);
        assert_eq!(ctx.total, 1);
    }

    #[test]
    fn branch_inbox_populate_list_dismiss_and_review_context() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
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
        assert_eq!(items[0].branch_name.as_deref(), Some("feature/two"));
        assert_eq!(items[1].branch_name.as_deref(), Some("feature/one"));
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
        let db_path = dir.path().join("pika-news.db");
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
        let db_path = dir.path().join("pika-news.db");
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
        let db_path = dir.path().join("pika-news.db");
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
        let db_path = dir.path().join("pika-news.db");
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
        let db_path = dir.path().join("pika-news.db");
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
        let db_path = dir.path().join("pika-news.db");
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
        let db_path = dir.path().join("pika-news.db");
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
        assert_eq!(items[0].pr_id, Some(pr_id));
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
    fn branch_chat_sessions_are_scoped_to_the_current_artifact_version() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
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
        assert_eq!(items[0].pr_id, Some(pr_id_1));
        assert_eq!(store.backfill_inbox_for_npub(new_npub).unwrap(), 0);
        let items = store.list_inbox(new_npub, 50, 0).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].pr_id, Some(pr_id_1));
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
    fn canonicalize_inbox_npubs_rekeys_branch_inbox_rows() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
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

    #[test]
    fn list_pr_summaries_filters_by_pr_number_and_date() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");

        let make_pr = |num: i64, date: &str| PrUpsertInput {
            repo: "sledtools/pika".to_string(),
            pr_number: num,
            title: format!("PR {}", num),
            url: format!("https://github.com/sledtools/pika/pull/{}", num),
            state: "merged".to_string(),
            head_sha: format!("sha-{}", num),
            base_ref: "master".to_string(),
            author_login: Some("alice".to_string()),
            updated_at: date.to_string(),
            merged_at: Some(date.to_string()),
        };

        store
            .upsert_pull_request(&make_pr(480, "2026-03-01T00:00:00Z"))
            .expect("insert 480");
        store
            .upsert_pull_request(&make_pr(481, "2026-03-02T00:00:00Z"))
            .expect("insert 481");
        store
            .upsert_pull_request(&make_pr(482, "2026-03-03T00:00:00Z"))
            .expect("insert 482");

        // No filter: all 3
        let all = store.list_pr_summaries(None, None).expect("all");
        assert_eq!(all.len(), 3);

        // since_pr=481: PRs 481 and 482
        let since_481 = store.list_pr_summaries(Some(481), None).expect("since 481");
        assert_eq!(since_481.len(), 2);
        assert!(since_481.iter().all(|r| r.pr_number >= 481));

        // since date: only 482 updated on or after 2026-03-03
        let since_date = store
            .list_pr_summaries(None, Some("2026-03-03"))
            .expect("since date");
        assert_eq!(since_date.len(), 1);
        assert_eq!(since_date[0].pr_number, 482);

        // combined: since_pr=480 AND since date 2026-03-02 → PRs 481, 482
        let combined = store
            .list_pr_summaries(Some(480), Some("2026-03-02"))
            .expect("combined");
        assert_eq!(combined.len(), 2);
    }
}
