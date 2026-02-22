use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone)]
pub(super) struct ChatMediaRecord {
    pub(super) account_pubkey: String,
    pub(super) chat_id: String,
    pub(super) original_hash_hex: String,
    pub(super) encrypted_hash_hex: String,
    pub(super) url: String,
    pub(super) mime_type: String,
    pub(super) filename: String,
    pub(super) nonce_hex: String,
    pub(super) scheme_version: String,
    pub(super) created_at: i64,
}

const CHAT_MEDIA_DB_FILE: &str = "chat_media.sqlite3";

pub(super) fn open_chat_media_db(data_dir: &str) -> rusqlite::Result<Connection> {
    let path = Path::new(data_dir).join(CHAT_MEDIA_DB_FILE);
    let conn = Connection::open(path)?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=WAL;
        PRAGMA foreign_keys=ON;

        CREATE TABLE IF NOT EXISTS chat_media (
            account_pubkey TEXT NOT NULL,
            chat_id TEXT NOT NULL,
            original_hash_hex TEXT NOT NULL,
            encrypted_hash_hex TEXT NOT NULL,
            url TEXT NOT NULL,
            mime_type TEXT NOT NULL,
            filename TEXT NOT NULL,
            nonce_hex TEXT NOT NULL,
            scheme_version TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (account_pubkey, chat_id, original_hash_hex)
        );
        "#,
    )?;
    Ok(conn)
}

pub(super) fn upsert_chat_media(
    conn: &Connection,
    record: &ChatMediaRecord,
) -> rusqlite::Result<()> {
    conn.execute(
        r#"
        INSERT INTO chat_media (
            account_pubkey,
            chat_id,
            original_hash_hex,
            encrypted_hash_hex,
            url,
            mime_type,
            filename,
            nonce_hex,
            scheme_version,
            created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ON CONFLICT(account_pubkey, chat_id, original_hash_hex) DO UPDATE SET
            encrypted_hash_hex = excluded.encrypted_hash_hex,
            url = excluded.url,
            mime_type = excluded.mime_type,
            filename = excluded.filename,
            nonce_hex = excluded.nonce_hex,
            scheme_version = excluded.scheme_version,
            created_at = excluded.created_at
        "#,
        params![
            record.account_pubkey,
            record.chat_id,
            record.original_hash_hex,
            record.encrypted_hash_hex,
            record.url,
            record.mime_type,
            record.filename,
            record.nonce_hex,
            record.scheme_version,
            record.created_at,
        ],
    )?;
    Ok(())
}

pub(super) fn get_chat_media(
    conn: &Connection,
    account_pubkey: &str,
    chat_id: &str,
    original_hash_hex: &str,
) -> Option<ChatMediaRecord> {
    conn.query_row(
        r#"
        SELECT
            account_pubkey,
            chat_id,
            original_hash_hex,
            encrypted_hash_hex,
            url,
            mime_type,
            filename,
            nonce_hex,
            scheme_version,
            created_at
        FROM chat_media
        WHERE account_pubkey = ?1 AND chat_id = ?2 AND original_hash_hex = ?3
        "#,
        params![account_pubkey, chat_id, original_hash_hex],
        |row| {
            Ok(ChatMediaRecord {
                account_pubkey: row.get(0)?,
                chat_id: row.get(1)?,
                original_hash_hex: row.get(2)?,
                encrypted_hash_hex: row.get(3)?,
                url: row.get(4)?,
                mime_type: row.get(5)?,
                filename: row.get(6)?,
                nonce_hex: row.get(7)?,
                scheme_version: row.get(8)?,
                created_at: row.get(9)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}
