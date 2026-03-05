use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq, Eq)]
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

fn record_from_row(row: &rusqlite::Row) -> rusqlite::Result<ChatMediaRecord> {
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
}

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
            encrypted_hash_hex = CASE WHEN excluded.encrypted_hash_hex = '' THEN chat_media.encrypted_hash_hex ELSE excluded.encrypted_hash_hex END,
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
        record_from_row,
    )
    .optional()
    .ok()
    .flatten()
}

pub(super) fn get_all_chat_media_map(
    conn: &Connection,
    account_pubkey: &str,
    chat_id: &str,
) -> std::collections::HashMap<String, ChatMediaRecord> {
    get_all_chat_media(conn, account_pubkey, chat_id)
        .into_iter()
        .map(|r| (r.original_hash_hex.clone(), r))
        .collect()
}

pub(super) fn get_all_chat_media(
    conn: &Connection,
    account_pubkey: &str,
    chat_id: &str,
) -> Vec<ChatMediaRecord> {
    let mut stmt = match conn.prepare(
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
        WHERE account_pubkey = ?1 AND chat_id = ?2
        ORDER BY created_at DESC
        LIMIT 500
        "#,
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%e, "failed to prepare get_all_chat_media query");
            return vec![];
        }
    };

    let rows = match stmt.query_map(params![account_pubkey, chat_id], record_from_row) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, "failed to query get_all_chat_media");
            return vec![];
        }
    };

    rows.filter_map(|r| r.ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record(
        account_pubkey: &str,
        chat_id: &str,
        original_hash_hex: &str,
        created_at: i64,
    ) -> ChatMediaRecord {
        ChatMediaRecord {
            account_pubkey: account_pubkey.to_string(),
            chat_id: chat_id.to_string(),
            original_hash_hex: original_hash_hex.to_string(),
            encrypted_hash_hex: format!("enc-{original_hash_hex}"),
            url: format!("https://example.test/{chat_id}/{original_hash_hex}"),
            mime_type: "image/jpeg".to_string(),
            filename: "photo.jpg".to_string(),
            nonce_hex: "deadbeef".to_string(),
            scheme_version: "v1".to_string(),
            created_at,
        }
    }

    #[test]
    fn opens_db_and_returns_none_when_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_chat_media_db(&dir.path().to_string_lossy()).expect("open db");

        let got = get_chat_media(&conn, "acc-a", "chat-a", "hash-a");
        assert!(got.is_none());
    }

    #[test]
    fn upsert_then_get_round_trips_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_chat_media_db(&dir.path().to_string_lossy()).expect("open db");
        let record = sample_record("acc-a", "chat-a", "hash-a", 111);

        upsert_chat_media(&conn, &record).expect("upsert");
        let got = get_chat_media(&conn, "acc-a", "chat-a", "hash-a").expect("record");

        assert_eq!(got, record);
    }

    #[test]
    fn upsert_updates_existing_primary_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_chat_media_db(&dir.path().to_string_lossy()).expect("open db");
        let first = sample_record("acc-a", "chat-a", "hash-a", 111);
        let mut second = sample_record("acc-a", "chat-a", "hash-a", 222);
        second.encrypted_hash_hex = "enc-updated".to_string();
        second.filename = "updated.png".to_string();
        second.mime_type = "image/png".to_string();
        second.nonce_hex = "beadfeed".to_string();
        second.scheme_version = "v2".to_string();
        second.url = "https://example.test/updated".to_string();

        upsert_chat_media(&conn, &first).expect("insert");
        upsert_chat_media(&conn, &second).expect("update");

        let got = get_chat_media(&conn, "acc-a", "chat-a", "hash-a").expect("record");
        assert_eq!(got, second);
    }

    #[test]
    fn keys_are_isolated_by_account_chat_and_original_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_chat_media_db(&dir.path().to_string_lossy()).expect("open db");
        let a = sample_record("acc-a", "chat-a", "hash-a", 1);
        let b = sample_record("acc-b", "chat-a", "hash-a", 2);
        let c = sample_record("acc-a", "chat-b", "hash-a", 3);
        let d = sample_record("acc-a", "chat-a", "hash-b", 4);

        upsert_chat_media(&conn, &a).expect("upsert a");
        upsert_chat_media(&conn, &b).expect("upsert b");
        upsert_chat_media(&conn, &c).expect("upsert c");
        upsert_chat_media(&conn, &d).expect("upsert d");

        assert_eq!(
            get_chat_media(&conn, "acc-a", "chat-a", "hash-a")
                .expect("acc-a/chat-a/hash-a")
                .created_at,
            1
        );
        assert_eq!(
            get_chat_media(&conn, "acc-b", "chat-a", "hash-a")
                .expect("acc-b/chat-a/hash-a")
                .created_at,
            2
        );
        assert_eq!(
            get_chat_media(&conn, "acc-a", "chat-b", "hash-a")
                .expect("acc-a/chat-b/hash-a")
                .created_at,
            3
        );
        assert_eq!(
            get_chat_media(&conn, "acc-a", "chat-a", "hash-b")
                .expect("acc-a/chat-a/hash-b")
                .created_at,
            4
        );
    }

    #[test]
    fn records_persist_after_reopening_db() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data_dir = dir.path().to_string_lossy().to_string();

        {
            let conn = open_chat_media_db(&data_dir).expect("open db");
            let record = sample_record("acc-a", "chat-a", "hash-a", 111);
            upsert_chat_media(&conn, &record).expect("upsert");
        }

        let reopened = open_chat_media_db(&data_dir).expect("reopen db");
        let got = get_chat_media(&reopened, "acc-a", "chat-a", "hash-a")
            .expect("expected record after reopening database");
        assert_eq!(got.created_at, 111);
    }

    #[test]
    fn get_all_returns_records_sorted_by_created_at_desc() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_chat_media_db(&dir.path().to_string_lossy()).expect("open db");

        let a = sample_record("acc-a", "chat-a", "hash-1", 100);
        let b = sample_record("acc-a", "chat-a", "hash-2", 300);
        let c = sample_record("acc-a", "chat-a", "hash-3", 200);
        upsert_chat_media(&conn, &a).expect("upsert a");
        upsert_chat_media(&conn, &b).expect("upsert b");
        upsert_chat_media(&conn, &c).expect("upsert c");

        let results = get_all_chat_media(&conn, "acc-a", "chat-a");
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].created_at, 300);
        assert_eq!(results[1].created_at, 200);
        assert_eq!(results[2].created_at, 100);
    }

    #[test]
    fn get_all_empty_when_no_media() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_chat_media_db(&dir.path().to_string_lossy()).expect("open db");

        let results = get_all_chat_media(&conn, "acc-a", "chat-a");
        assert!(results.is_empty());
    }

    #[test]
    fn get_all_isolated_by_account_and_chat() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_chat_media_db(&dir.path().to_string_lossy()).expect("open db");

        upsert_chat_media(&conn, &sample_record("acc-a", "chat-a", "hash-1", 1)).expect("upsert");
        upsert_chat_media(&conn, &sample_record("acc-a", "chat-b", "hash-2", 2)).expect("upsert");
        upsert_chat_media(&conn, &sample_record("acc-b", "chat-a", "hash-3", 3)).expect("upsert");

        let results = get_all_chat_media(&conn, "acc-a", "chat-a");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].original_hash_hex, "hash-1");
    }

    // --- encrypted_hash_hex preservation tests ---

    #[test]
    fn upsert_with_empty_encrypted_hash_does_not_overwrite_existing() {
        // Simulates: upload sets real hash, then receive-time upsert writes "".
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_chat_media_db(&dir.path().to_string_lossy()).expect("open db");

        let mut uploaded = sample_record("acc-a", "chat-a", "hash-a", 100);
        uploaded.encrypted_hash_hex = "real-encrypted-hash".to_string();
        upsert_chat_media(&conn, &uploaded).expect("upsert uploaded");

        let mut received = sample_record("acc-a", "chat-a", "hash-a", 100);
        received.encrypted_hash_hex = String::new();
        upsert_chat_media(&conn, &received).expect("upsert received");

        let got = get_chat_media(&conn, "acc-a", "chat-a", "hash-a").expect("record");
        assert_eq!(
            got.encrypted_hash_hex, "real-encrypted-hash",
            "empty upsert must not overwrite existing encrypted hash"
        );
    }

    #[test]
    fn upsert_with_real_encrypted_hash_overwrites_empty() {
        // Simulates: receive-time upsert writes "" first, then upload sets real hash.
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_chat_media_db(&dir.path().to_string_lossy()).expect("open db");

        let mut received = sample_record("acc-a", "chat-a", "hash-a", 100);
        received.encrypted_hash_hex = String::new();
        upsert_chat_media(&conn, &received).expect("upsert received");

        let mut uploaded = sample_record("acc-a", "chat-a", "hash-a", 100);
        uploaded.encrypted_hash_hex = "real-encrypted-hash".to_string();
        upsert_chat_media(&conn, &uploaded).expect("upsert uploaded");

        let got = get_chat_media(&conn, "acc-a", "chat-a", "hash-a").expect("record");
        assert_eq!(
            got.encrypted_hash_hex, "real-encrypted-hash",
            "real hash must overwrite empty"
        );
    }

    #[test]
    fn upsert_with_real_encrypted_hash_overwrites_different_real_hash() {
        // Simulates: re-upload or corrected hash replaces old one.
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_chat_media_db(&dir.path().to_string_lossy()).expect("open db");

        let mut first = sample_record("acc-a", "chat-a", "hash-a", 100);
        first.encrypted_hash_hex = "old-hash".to_string();
        upsert_chat_media(&conn, &first).expect("upsert first");

        let mut second = sample_record("acc-a", "chat-a", "hash-a", 100);
        second.encrypted_hash_hex = "new-hash".to_string();
        upsert_chat_media(&conn, &second).expect("upsert second");

        let got = get_chat_media(&conn, "acc-a", "chat-a", "hash-a").expect("record");
        assert_eq!(
            got.encrypted_hash_hex, "new-hash",
            "non-empty hash must overwrite previous"
        );
    }

    #[test]
    fn fresh_insert_with_empty_encrypted_hash_stores_empty() {
        // First insert has no encrypted hash (receive before upload).
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_chat_media_db(&dir.path().to_string_lossy()).expect("open db");

        let mut record = sample_record("acc-a", "chat-a", "hash-a", 100);
        record.encrypted_hash_hex = String::new();
        upsert_chat_media(&conn, &record).expect("upsert");

        let got = get_chat_media(&conn, "acc-a", "chat-a", "hash-a").expect("record");
        assert_eq!(
            got.encrypted_hash_hex, "",
            "initial empty hash stored as-is"
        );
    }
}
