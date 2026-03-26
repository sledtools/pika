use anyhow::Context;
use rusqlite::{params, OptionalExtension};

use crate::storage::{ChatMessage, Store};

impl Store {
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

    pub fn get_branch_review_artifact_session_id(
        &self,
        branch_id: i64,
        artifact_id: i64,
    ) -> anyhow::Result<Option<String>> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT claude_session_id
                 FROM branch_artifact_versions
                 WHERE branch_id = ?1 AND id = ?2 AND status = 'ready'",
                params![branch_id, artifact_id],
                |row| row.get(0),
            )
            .optional()
            .context("query branch review artifact session id")
            .map(|opt| opt.flatten())
        })
    }

    pub fn get_or_create_branch_review_chat_session(
        &self,
        artifact_id: i64,
        npub: &str,
        claude_session_id: &str,
    ) -> anyhow::Result<(i64, Vec<ChatMessage>)> {
        self.with_connection(|conn| {
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT id
                     FROM branch_chat_sessions
                     WHERE branch_artifact_id = ?1 AND npub = ?2",
                    params![artifact_id, npub],
                    |row| row.get(0),
                )
                .optional()
                .context("lookup existing branch review chat session")?;

            let session_id = match existing {
                Some(id) => id,
                None => {
                    conn.execute(
                        "INSERT INTO branch_chat_sessions(branch_artifact_id, npub, claude_session_id)
                         VALUES (?1, ?2, ?3)",
                        params![artifact_id, npub, claude_session_id],
                    )
                    .context("insert branch review chat session")?;
                    conn.last_insert_rowid()
                }
            };

            let mut stmt = conn
                .prepare(
                    "SELECT role, content, created_at
                     FROM branch_chat_messages
                     WHERE session_id = ?1
                     ORDER BY created_at ASC",
                )
                .context("prepare branch review chat messages query")?;

            let rows = stmt
                .query_map(params![session_id], |row| {
                    Ok(ChatMessage {
                        role: row.get(0)?,
                        content: row.get(1)?,
                        created_at: row.get(2)?,
                    })
                })
                .context("query branch review chat messages")?;

            let mut messages = Vec::new();
            for row in rows {
                messages.push(row.context("read branch review chat message row")?);
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

    pub fn get_branch_review_chat_claude_session_id(
        &self,
        session_id: i64,
    ) -> anyhow::Result<String> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT claude_session_id
                 FROM branch_chat_sessions
                 WHERE id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .context("query branch review chat claude session id")
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

    pub fn update_branch_review_chat_claude_session_id(
        &self,
        session_id: i64,
        claude_session_id: &str,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            conn.execute(
                "UPDATE branch_chat_sessions
                 SET claude_session_id = ?1, updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?2",
                params![claude_session_id, session_id],
            )
            .context("update branch review chat claude session id")?;
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

    pub fn append_branch_review_chat_message(
        &self,
        session_id: i64,
        role: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO branch_chat_messages(session_id, role, content)
                 VALUES (?1, ?2, ?3)",
                params![session_id, role, content],
            )
            .context("insert branch review chat message")?;
            Ok(())
        })
    }
}
