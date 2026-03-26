use std::cmp::{max, min};

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::{artifact_user_state_rank, InboxItem, InboxReviewContext, Store};

impl Store {
    pub fn populate_branch_inbox(
        &self,
        artifact_id: i64,
        npubs: &[String],
    ) -> anyhow::Result<usize> {
        if npubs.is_empty() {
            return Ok(0);
        }
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start populate branch inbox transaction")?;
            let count = insert_branch_inbox_rows_for_artifact(&tx, artifact_id, npubs)?;
            tx.commit()
                .context("commit populate branch inbox transaction")?;
            Ok(count)
        })
    }

    pub fn backfill_branch_inbox_for_npub(&self, npub: &str) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start branch inbox backfill transaction")?;
            let artifact_ids: Vec<i64> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT bav.id
                         FROM branch_artifact_versions bav
                         JOIN branch_records br ON br.id = bav.branch_id
                         WHERE bav.is_current = 1
                           AND bav.status = 'ready'
                         ORDER BY bav.version ASC, bav.id ASC",
                    )
                    .context("prepare current branch artifact scan for backfill")?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, i64>(0))
                    .context("query current branch artifacts for backfill")?;
                let mut artifact_ids = Vec::new();
                for row in rows {
                    artifact_ids.push(row.context("read current branch artifact row")?);
                }
                artifact_ids
            };
            let recipient = npub.to_string();
            let mut inserted = 0;
            for artifact_id in artifact_ids {
                inserted += insert_branch_inbox_rows_for_artifact(
                    &tx,
                    artifact_id,
                    std::slice::from_ref(&recipient),
                )?;
            }
            tx.commit()
                .context("commit branch inbox backfill transaction")?;
            Ok(inserted)
        })
    }

    pub fn list_branch_inbox(
        &self,
        npub: &str,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<InboxItem>> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT bis.branch_id,
                            r.repo,
                            br.branch_name,
                            br.title,
                            br.state,
                            br.updated_at,
                            COALESCE(ba_latest.status, 'pending'),
                            bis.reason,
                            bis.created_at,
                            CASE
                                WHEN bis.last_reviewed_artifact_id IS NULL
                                  OR bis.last_reviewed_artifact_id != bis.artifact_id
                                THEN 1
                                ELSE 0
                            END AS needs_review,
                            bis.last_reviewed_at
                     FROM branch_inbox_states bis
                     JOIN branch_records br ON br.id = bis.branch_id
                     JOIN repos r ON r.id = br.repo_id
                     LEFT JOIN branch_artifact_versions ba_latest ON ba_latest.id = (
                        SELECT bav.id
                        FROM branch_artifact_versions bav
                        WHERE bav.branch_id = br.id AND bav.status != 'superseded'
                        ORDER BY bav.version DESC
                        LIMIT 1
                     )
                     WHERE bis.npub = ?1
                       AND bis.state = 'inbox'
                     ORDER BY needs_review DESC, bis.updated_at DESC, bis.branch_id DESC
                     LIMIT ?2 OFFSET ?3",
                )
                .context("prepare branch inbox query")?;

            let rows = stmt
                .query_map(params![npub, limit, offset], |row| {
                    Ok(InboxItem {
                        review_id: row.get(0)?,
                        branch_id: Some(row.get(0)?),
                        pr_id: None,
                        repo: row.get(1)?,
                        branch_name: Some(row.get(2)?),
                        pr_number: None,
                        title: row.get(3)?,
                        url: None,
                        state: row.get(4)?,
                        updated_at: row.get(5)?,
                        generation_status: row
                            .get::<_, Option<String>>(6)?
                            .unwrap_or_else(|| "pending".to_string()),
                        reason: row.get(7)?,
                        inbox_created_at: row.get(8)?,
                        needs_review: row.get(9)?,
                        last_reviewed_at: row.get(10)?,
                    })
                })
                .context("query branch inbox")?;

            let mut items = Vec::new();
            for row in rows {
                items.push(row.context("read branch inbox row")?);
            }
            Ok(items)
        })
    }

    pub fn branch_inbox_count(&self, npub: &str) -> anyhow::Result<i64> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT COUNT(*)
                 FROM branch_inbox_states bis
                 WHERE bis.npub = ?1
                   AND bis.state = 'inbox'
                   AND (
                       bis.last_reviewed_artifact_id IS NULL
                       OR bis.last_reviewed_artifact_id != bis.artifact_id
                   )",
                params![npub],
                |row| row.get(0),
            )
            .context("count branch inbox")
        })
    }

    pub fn branch_inbox_total(&self, npub: &str) -> anyhow::Result<i64> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT COUNT(*)
                 FROM branch_inbox_states bis
                 WHERE bis.npub = ?1
                   AND bis.state = 'inbox'",
                params![npub],
                |row| row.get(0),
            )
            .context("count total branch inbox")
        })
    }

    pub fn mark_branch_inbox_reviewed(&self, npub: &str, branch_id: i64) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let rows = conn
                .execute(
                    "UPDATE branch_inbox_states
                     SET last_reviewed_artifact_id = artifact_id,
                         last_reviewed_at = CURRENT_TIMESTAMP
                     WHERE npub = ?1
                       AND branch_id = ?2
                       AND state = 'inbox'
                       AND (
                           last_reviewed_artifact_id IS NULL
                           OR last_reviewed_artifact_id != artifact_id
                       )",
                    params![npub, branch_id],
                )
                .context("mark branch inbox reviewed")?;
            Ok(rows)
        })
    }

    pub fn dismiss_branch_inbox_items(
        &self,
        npub: &str,
        branch_ids: &[i64],
    ) -> anyhow::Result<usize> {
        if branch_ids.is_empty() {
            return Ok(0);
        }
        self.with_connection(|conn| {
            let tx = conn
                .unchecked_transaction()
                .context("start dismiss branch inbox transaction")?;
            let placeholders: Vec<String> = branch_ids
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 2))
                .collect();
            let update_sql = format!(
                "UPDATE branch_inbox_states
                 SET state = 'dismissed',
                     dismissed_at = CURRENT_TIMESTAMP,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE npub = ?1
                   AND state = 'inbox'
                   AND branch_id IN ({})",
                placeholders.join(", ")
            );
            let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            param_values.push(Box::new(npub.to_string()));
            for id in branch_ids {
                param_values.push(Box::new(*id));
            }
            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|p| p.as_ref()).collect();
            let rows = tx
                .prepare(&update_sql)
                .context("prepare dismiss branch inbox query")?
                .execute(param_refs.as_slice())
                .context("dismiss branch inbox items")?;
            tx.commit()
                .context("commit dismiss branch inbox transaction")?;
            Ok(rows)
        })
    }

    pub fn dismiss_all_branch_inbox(&self, npub: &str) -> anyhow::Result<usize> {
        self.with_connection(|conn| {
            let rows = conn
                .execute(
                    "UPDATE branch_inbox_states
                     SET state = 'dismissed',
                         dismissed_at = CURRENT_TIMESTAMP,
                         updated_at = CURRENT_TIMESTAMP
                     WHERE npub = ?1 AND state = 'inbox'",
                    params![npub],
                )
                .context("dismiss all branch inbox")?;
            Ok(rows)
        })
    }

    pub fn branch_inbox_review_context(
        &self,
        npub: &str,
        branch_id: i64,
    ) -> anyhow::Result<Option<InboxReviewContext>> {
        self.with_connection(|conn| {
            let current = conn
                .query_row(
                    "SELECT bis.updated_at, bis.branch_id
                     FROM branch_inbox_states bis
                     WHERE bis.npub = ?1
                       AND bis.state = 'inbox'
                       AND bis.branch_id = ?2
                     LIMIT 1",
                    params![npub, branch_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .context("lookup branch inbox review item")?;

            let Some((created_at, current_branch_id)) = current else {
                return Ok(None);
            };

            let prev: Option<i64> = conn
                .query_row(
                    "SELECT bis.branch_id
                     FROM branch_inbox_states bis
                     WHERE bis.npub = ?1
                       AND bis.state = 'inbox'
                       AND bis.branch_id != ?3
                       AND (
                           bis.updated_at > ?2
                           OR (bis.updated_at = ?2 AND bis.branch_id > ?3)
                       )
                     ORDER BY bis.updated_at ASC, bis.branch_id ASC
                     LIMIT 1",
                    params![npub, created_at, current_branch_id],
                    |row| row.get(0),
                )
                .optional()
                .context("branch inbox prev neighbor")?;

            let next: Option<i64> = conn
                .query_row(
                    "SELECT bis.branch_id
                     FROM branch_inbox_states bis
                     WHERE bis.npub = ?1
                       AND bis.state = 'inbox'
                       AND bis.branch_id != ?3
                       AND (
                           bis.updated_at < ?2
                           OR (bis.updated_at = ?2 AND bis.branch_id < ?3)
                       )
                     ORDER BY bis.updated_at DESC, bis.branch_id DESC
                     LIMIT 1",
                    params![npub, created_at, current_branch_id],
                    |row| row.get(0),
                )
                .optional()
                .context("branch inbox next neighbor")?;

            let position: i64 = conn
                .query_row(
                    "SELECT COUNT(*)
                     FROM branch_inbox_states bis
                     WHERE bis.npub = ?1
                       AND bis.state = 'inbox'
                       AND (
                           bis.updated_at > ?2
                           OR (bis.updated_at = ?2 AND bis.branch_id >= ?3)
                       )",
                    params![npub, created_at, current_branch_id],
                    |row| row.get(0),
                )
                .context("count branch inbox review position")?;

            let total: i64 = conn
                .query_row(
                    "SELECT COUNT(*)
                     FROM branch_inbox_states bis
                     WHERE bis.npub = ?1
                       AND bis.state = 'inbox'",
                    params![npub],
                    |row| row.get(0),
                )
                .context("count branch inbox review total")?;

            Ok(Some(InboxReviewContext {
                prev,
                next,
                position,
                total,
            }))
        })
    }

    pub fn list_branch_inbox_allowlist_npubs(&self) -> anyhow::Result<Vec<String>> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT npub
                     FROM chat_allowlist
                     WHERE active = 1 OR can_forge_write = 1
                     ORDER BY npub ASC",
                )
                .context("prepare branch inbox allowlist query")?;

            let rows = stmt
                .query_map([], |row| row.get(0))
                .context("query branch inbox allowlist")?;

            let mut npubs = Vec::new();
            for row in rows {
                npubs.push(row.context("read branch inbox allowlist row")?);
            }
            Ok(npubs)
        })
    }
}

#[derive(Debug, Clone)]
struct BranchInboxStateRow {
    branch_id: i64,
    artifact_id: i64,
    state: String,
    reason: String,
    created_at: String,
    updated_at: String,
    dismissed_at: Option<String>,
    last_reviewed_artifact_id: Option<i64>,
    last_reviewed_at: Option<String>,
}

pub(crate) fn merge_branch_inbox_state_alias(
    conn: &Connection,
    from_npub: &str,
    to_npub: &str,
) -> anyhow::Result<usize> {
    let mut stmt = conn
        .prepare(
            "SELECT branch_id, artifact_id, state, reason, created_at, updated_at, dismissed_at,
                    last_reviewed_artifact_id, last_reviewed_at
             FROM branch_inbox_states
             WHERE npub = ?1
             ORDER BY branch_id ASC",
        )
        .context("prepare source branch inbox state scan")?;
    let rows = stmt
        .query_map(params![from_npub], |row| {
            Ok(BranchInboxStateRow {
                branch_id: row.get(0)?,
                artifact_id: row.get(1)?,
                state: row.get(2)?,
                reason: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
                dismissed_at: row.get(6)?,
                last_reviewed_artifact_id: row.get(7)?,
                last_reviewed_at: row.get(8)?,
            })
        })
        .context("query source branch inbox state rows")?;

    let mut source_rows = Vec::new();
    for row in rows {
        source_rows.push(row.context("read source branch inbox state row")?);
    }

    for source in &source_rows {
        let existing = conn
            .query_row(
                "SELECT branch_id, artifact_id, state, reason, created_at, updated_at, dismissed_at,
                        last_reviewed_artifact_id, last_reviewed_at
                 FROM branch_inbox_states
                 WHERE npub = ?1 AND branch_id = ?2",
                params![to_npub, source.branch_id],
                |row| {
                    Ok(BranchInboxStateRow {
                        branch_id: row.get(0)?,
                        artifact_id: row.get(1)?,
                        state: row.get(2)?,
                        reason: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get(5)?,
                        dismissed_at: row.get(6)?,
                        last_reviewed_artifact_id: row.get(7)?,
                        last_reviewed_at: row.get(8)?,
                    })
                },
            )
            .optional()
            .context("query destination branch inbox state row")?;

        if let Some(existing) = existing {
            let merged = merge_branch_inbox_state_rows(&existing, source);
            conn.execute(
                "UPDATE branch_inbox_states
                 SET artifact_id = ?1,
                     state = ?2,
                     reason = ?3,
                     created_at = ?4,
                     updated_at = ?5,
                     dismissed_at = ?6,
                     last_reviewed_artifact_id = ?7,
                     last_reviewed_at = ?8
                 WHERE npub = ?9 AND branch_id = ?10",
                params![
                    merged.artifact_id,
                    merged.state,
                    merged.reason,
                    merged.created_at,
                    merged.updated_at,
                    merged.dismissed_at,
                    merged.last_reviewed_artifact_id,
                    merged.last_reviewed_at,
                    to_npub,
                    merged.branch_id,
                ],
            )
            .context("update merged destination branch inbox state row")?;
            conn.execute(
                "DELETE FROM branch_inbox_states WHERE npub = ?1 AND branch_id = ?2",
                params![from_npub, source.branch_id],
            )
            .context("delete merged source branch inbox state row")?;
        } else {
            conn.execute(
                "UPDATE branch_inbox_states
                 SET npub = ?1
                 WHERE npub = ?2 AND branch_id = ?3",
                params![to_npub, from_npub, source.branch_id],
            )
            .context("rekey branch inbox state row")?;
        }
    }

    Ok(source_rows.len())
}

fn merge_branch_inbox_state_rows(
    existing: &BranchInboxStateRow,
    incoming: &BranchInboxStateRow,
) -> BranchInboxStateRow {
    debug_assert_eq!(existing.branch_id, incoming.branch_id);

    if existing.artifact_id != incoming.artifact_id {
        let winner = if incoming.updated_at > existing.updated_at {
            incoming
        } else {
            existing
        };
        return BranchInboxStateRow {
            branch_id: existing.branch_id,
            artifact_id: winner.artifact_id,
            state: winner.state.clone(),
            reason: winner.reason.clone(),
            created_at: winner.created_at.clone(),
            updated_at: max(existing.updated_at.clone(), incoming.updated_at.clone()),
            dismissed_at: if winner.state == "dismissed" {
                max(existing.dismissed_at.clone(), incoming.dismissed_at.clone())
            } else {
                None
            },
            last_reviewed_artifact_id: max(
                existing.last_reviewed_artifact_id,
                incoming.last_reviewed_artifact_id,
            ),
            last_reviewed_at: max(
                existing.last_reviewed_at.clone(),
                incoming.last_reviewed_at.clone(),
            ),
        };
    }

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

    BranchInboxStateRow {
        branch_id: existing.branch_id,
        artifact_id: winner.artifact_id,
        state,
        reason: winner.reason.clone(),
        created_at: min(existing.created_at.clone(), incoming.created_at.clone()),
        updated_at: max(existing.updated_at.clone(), incoming.updated_at.clone()),
        dismissed_at,
        last_reviewed_artifact_id: max(
            existing.last_reviewed_artifact_id,
            incoming.last_reviewed_artifact_id,
        ),
        last_reviewed_at: max(
            existing.last_reviewed_at.clone(),
            incoming.last_reviewed_at.clone(),
        ),
    }
}

fn insert_branch_inbox_rows_for_artifact(
    conn: &Connection,
    artifact_id: i64,
    npubs: &[String],
) -> anyhow::Result<usize> {
    if npubs.is_empty() {
        return Ok(0);
    }

    let Some(branch_id) = conn
        .query_row(
            "SELECT bav.branch_id
             FROM branch_artifact_versions bav
             WHERE bav.id = ?1
               AND bav.status = 'ready'
               AND bav.is_current = 1",
            params![artifact_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .context("lookup ready branch artifact for inbox insert")?
    else {
        return Ok(0);
    };

    let mut count = 0;
    for npub in npubs {
        let existing = conn
            .query_row(
                "SELECT artifact_id, state
                 FROM branch_inbox_states
                 WHERE npub = ?1 AND branch_id = ?2",
                params![npub, branch_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .with_context(|| format!("lookup branch inbox row for {}", npub))?;

        match existing {
            Some((existing_artifact_id, _)) if existing_artifact_id == artifact_id => {}
            Some(_) => {
                conn.execute(
                    "UPDATE branch_inbox_states
                     SET artifact_id = ?1,
                         state = 'inbox',
                         reason = 'new_commits',
                         updated_at = CURRENT_TIMESTAMP,
                         dismissed_at = NULL
                     WHERE npub = ?2 AND branch_id = ?3",
                    params![artifact_id, npub, branch_id],
                )
                .with_context(|| format!("refresh branch inbox row for {}", npub))?;
                count += 1;
            }
            None => {
                conn.execute(
                    "INSERT INTO branch_inbox_states(
                        npub, branch_id, artifact_id, state, reason, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, 'inbox', 'new_commits', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
                    params![npub, branch_id, artifact_id],
                )
                .context("insert branch inbox row")?;
                count += 1;
            }
        }
    }
    Ok(count)
}
