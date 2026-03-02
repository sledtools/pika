use std::collections::HashMap;

use rusqlite::Connection;

use super::ProfileCache;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS profiles (
        pubkey TEXT NOT NULL,
        chat_id TEXT,
        metadata JSONB,
        name TEXT,
        about TEXT,
        picture_url TEXT,
        event_created_at INTEGER NOT NULL DEFAULT 0
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_profiles_unique
        ON profiles(pubkey, IFNULL(chat_id, ''));
    CREATE TABLE IF NOT EXISTS follows (
        pubkey TEXT PRIMARY KEY
    );
    CREATE TABLE IF NOT EXISTS app_settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
";

pub fn open_profile_db(data_dir: &str) -> Result<Connection, rusqlite::Error> {
    let path = std::path::Path::new(data_dir).join("profiles.sqlite3");

    // Migration: if the old schema (pubkey-only PK, no chat_id) is present, delete and recreate.
    if path.exists() {
        let conn = Connection::open(&path)?;
        let has_chat_id = conn
            .prepare("PRAGMA table_info(profiles)")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| row.get::<_, String>(1))
                    .map(|rows| rows.flatten().any(|col| col == "chat_id"))
            })
            .unwrap_or(false);
        drop(conn);
        if !has_chat_id {
            let _ = std::fs::remove_file(&path);
        }
    }

    let conn = Connection::open(&path)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn load_profiles(conn: &Connection) -> HashMap<String, ProfileCache> {
    let mut map = HashMap::new();
    let mut stmt = match conn.prepare(
        "SELECT pubkey,
                json_extract(metadata, '$.display_name'),
                json_extract(metadata, '$.name'),
                about,
                picture_url,
                event_created_at
         FROM profiles
         WHERE chat_id IS NULL",
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%e, "failed to prepare profile load query");
            return map;
        }
    };
    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, i64>(5)?,
        ))
    }) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, "failed to query profiles from cache db");
            return map;
        }
    };
    for row in rows.flatten() {
        let (pubkey, display_name, name, about, picture_url, event_created_at) = row;
        let display_name = display_name.filter(|s| !s.is_empty());
        let name = name.filter(|s| !s.is_empty());
        map.insert(
            pubkey,
            ProfileCache {
                metadata_json: None,
                name: display_name.or(name.clone()),
                username: name,
                about: about.filter(|s| !s.is_empty()),
                picture_url: picture_url.filter(|s| !s.is_empty()),
                event_created_at,
                last_checked_at: 0,
            },
        );
    }
    map
}

/// Load the full metadata JSON for a single global profile (used for profile editing).
pub fn load_metadata_json(conn: &Connection, pubkey: &str) -> Option<String> {
    conn.query_row(
        "SELECT json(metadata) FROM profiles WHERE pubkey = ?1 AND chat_id IS NULL",
        [pubkey],
        |row| row.get(0),
    )
    .ok()
}

pub fn save_profile(conn: &Connection, pubkey: &str, cache: &ProfileCache) {
    if let Err(e) = conn.execute(
        "INSERT OR REPLACE INTO profiles (pubkey, chat_id, metadata, name, about, picture_url, event_created_at)
         VALUES (?1, NULL, jsonb(?2), ?3, ?4, ?5, ?6)",
        rusqlite::params![
            pubkey,
            cache.metadata_json,
            cache.name,
            cache.about,
            cache.picture_url,
            cache.event_created_at,
        ],
    ) {
        tracing::warn!(%e, pubkey, "failed to save profile to cache db");
    }
}

// ── Group profiles ──────────────────────────────────────────────────

pub fn save_group_profile(conn: &Connection, pubkey: &str, chat_id: &str, cache: &ProfileCache) {
    if let Err(e) = conn.execute(
        "INSERT OR REPLACE INTO profiles (pubkey, chat_id, metadata, name, about, picture_url, event_created_at)
         VALUES (?1, ?2, jsonb(?3), ?4, ?5, ?6, ?7)",
        rusqlite::params![
            pubkey,
            chat_id,
            cache.metadata_json,
            cache.name,
            cache.about,
            cache.picture_url,
            cache.event_created_at,
        ],
    ) {
        tracing::warn!(%e, pubkey, chat_id, "failed to save group profile to cache db");
    }
}

pub fn load_group_profiles(conn: &Connection, chat_id: &str) -> HashMap<String, ProfileCache> {
    let mut map = HashMap::new();
    let mut stmt = match conn.prepare(
        "SELECT pubkey,
                json_extract(metadata, '$.display_name'),
                json_extract(metadata, '$.name'),
                about,
                picture_url,
                event_created_at
         FROM profiles
         WHERE chat_id = ?1",
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%e, chat_id, "failed to prepare group profile load query");
            return map;
        }
    };
    let rows = match stmt.query_map([chat_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, i64>(5)?,
        ))
    }) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, chat_id, "failed to query group profiles from cache db");
            return map;
        }
    };
    for row in rows.flatten() {
        let (pubkey, display_name, name, about, picture_url, event_created_at) = row;
        let display_name = display_name.filter(|s| !s.is_empty());
        let name = name.filter(|s| !s.is_empty());
        map.insert(
            pubkey,
            ProfileCache {
                metadata_json: None,
                name: display_name.or(name.clone()),
                username: name,
                about: about.filter(|s| !s.is_empty()),
                picture_url: picture_url.filter(|s| !s.is_empty()),
                event_created_at,
                last_checked_at: 0,
            },
        );
    }
    map
}

pub fn delete_group_profiles(conn: &Connection, chat_id: &str) {
    if let Err(e) = conn.execute("DELETE FROM profiles WHERE chat_id = ?1", [chat_id]) {
        tracing::warn!(%e, chat_id, "failed to delete group profiles from cache db");
    }
}

/// Delete all cached profiles and follows (used on logout).
pub fn clear_all(conn: &Connection) {
    if let Err(e) = conn.execute_batch("DELETE FROM profiles; DELETE FROM follows;") {
        tracing::warn!(%e, "failed to clear profile cache db");
    }
}

pub fn load_developer_mode(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = 'developer_mode'",
        [],
        |row| row.get::<_, String>(0),
    )
    .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE"))
    .unwrap_or(false)
}

pub fn save_developer_mode(conn: &Connection, enabled: bool) {
    let value = if enabled { "1" } else { "0" };
    if let Err(e) = conn.execute(
        "INSERT INTO app_settings (key, value)
         VALUES ('developer_mode', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [value],
    ) {
        tracing::warn!(%e, enabled, "failed to save developer mode setting");
    }
}

// ── Follow cache ─────────────────────────────────────────────────────

pub fn load_follows(conn: &Connection) -> Vec<String> {
    let mut stmt = match conn.prepare("SELECT pubkey FROM follows") {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%e, "failed to prepare follows load query");
            return vec![];
        }
    };
    let rows = match stmt.query_map([], |row| row.get::<_, String>(0)) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, "failed to query follows from cache db");
            return vec![];
        }
    };
    rows.flatten().collect()
}

pub fn save_follows(conn: &Connection, pubkeys: &[String]) {
    let tx = match conn.unchecked_transaction() {
        Ok(tx) => tx,
        Err(e) => {
            tracing::warn!(%e, "failed to begin follows transaction");
            return;
        }
    };
    if let Err(e) = tx.execute("DELETE FROM follows", []) {
        tracing::warn!(%e, "failed to clear follows cache");
        return;
    }
    for pk in pubkeys {
        if let Err(e) = tx.execute("INSERT OR IGNORE INTO follows (pubkey) VALUES (?1)", [pk]) {
            tracing::warn!(%e, pubkey = pk, "failed to save follow to cache db");
            return;
        }
    }
    if let Err(e) = tx.commit() {
        tracing::warn!(%e, "failed to commit follows transaction");
    }
}

pub fn add_follow(conn: &Connection, pubkey: &str) {
    if let Err(e) = conn.execute(
        "INSERT OR IGNORE INTO follows (pubkey) VALUES (?1)",
        [pubkey],
    ) {
        tracing::warn!(%e, pubkey, "failed to add follow to cache db");
    }
}

pub fn remove_follow(conn: &Connection, pubkey: &str) {
    if let Err(e) = conn.execute("DELETE FROM follows WHERE pubkey = ?1", [pubkey]) {
        tracing::warn!(%e, pubkey, "failed to remove follow from cache db");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open an in-memory DB with the same schema as production.
    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    #[test]
    fn profile_save_load_roundtrip() {
        let conn = test_db();
        let metadata = r#"{"display_name":"Alice","name":"alice","about":"hi","picture":"https://example.com/pic.jpg"}"#;
        let cache = ProfileCache::from_metadata_json(Some(metadata.to_string()), 1000, 0);

        assert_eq!(cache.name.as_deref(), Some("Alice"));
        assert_eq!(cache.username.as_deref(), Some("alice"));
        assert_eq!(cache.about.as_deref(), Some("hi"));
        assert_eq!(
            cache.picture_url.as_deref(),
            Some("https://example.com/pic.jpg")
        );

        save_profile(&conn, "abc123", &cache);
        let loaded = load_profiles(&conn);
        let got = loaded.get("abc123").expect("profile should exist");

        assert_eq!(got.name, cache.name);
        assert_eq!(got.username, cache.username);
        assert_eq!(got.about, cache.about);
        assert_eq!(got.picture_url, cache.picture_url);
        assert_eq!(got.event_created_at, 1000);
    }

    #[test]
    fn profile_load_name_fallback() {
        let conn = test_db();
        // No display_name — should fall back to name.
        let metadata = r#"{"name":"bob"}"#;
        let cache = ProfileCache::from_metadata_json(Some(metadata.to_string()), 1, 0);
        save_profile(&conn, "bob_pk", &cache);

        let loaded = load_profiles(&conn);
        let got = loaded.get("bob_pk").unwrap();
        assert_eq!(got.name.as_deref(), Some("bob"));
        assert_eq!(got.username.as_deref(), Some("bob"));
    }

    #[test]
    fn group_profile_save_load_roundtrip() {
        let conn = test_db();
        let metadata = r#"{"display_name":"Alice in Group","name":"alice","about":"group bio"}"#;
        let cache = ProfileCache::from_metadata_json(Some(metadata.to_string()), 500, 0);

        save_group_profile(&conn, "alice_pk", "chat_abc", &cache);

        let loaded = load_group_profiles(&conn, "chat_abc");
        let got = loaded.get("alice_pk").expect("group profile should exist");
        assert_eq!(got.name.as_deref(), Some("Alice in Group"));
        assert_eq!(got.about.as_deref(), Some("group bio"));
        assert_eq!(got.event_created_at, 500);

        // Global profiles should not include group profiles.
        let global = load_profiles(&conn);
        assert!(!global.contains_key("alice_pk"));
    }

    #[test]
    fn group_profile_separate_from_global() {
        let conn = test_db();
        let global_meta = r#"{"display_name":"Alice Global"}"#;
        let group_meta = r#"{"display_name":"Alice Group"}"#;

        save_profile(
            &conn,
            "alice",
            &ProfileCache::from_metadata_json(Some(global_meta.to_string()), 1, 0),
        );
        save_group_profile(
            &conn,
            "alice",
            "chat1",
            &ProfileCache::from_metadata_json(Some(group_meta.to_string()), 2, 0),
        );

        let global = load_profiles(&conn);
        assert_eq!(
            global.get("alice").unwrap().name.as_deref(),
            Some("Alice Global")
        );

        let group = load_group_profiles(&conn, "chat1");
        assert_eq!(
            group.get("alice").unwrap().name.as_deref(),
            Some("Alice Group")
        );
    }

    #[test]
    fn delete_group_profiles_only_deletes_that_chat() {
        let conn = test_db();
        let meta = r#"{"display_name":"Test"}"#;
        let cache = ProfileCache::from_metadata_json(Some(meta.to_string()), 1, 0);

        save_group_profile(&conn, "alice", "chat1", &cache);
        save_group_profile(&conn, "alice", "chat2", &cache);
        save_profile(&conn, "alice", &cache);

        delete_group_profiles(&conn, "chat1");

        assert!(load_group_profiles(&conn, "chat1").is_empty());
        assert!(!load_group_profiles(&conn, "chat2").is_empty());
        assert!(!load_profiles(&conn).is_empty());
    }

    #[test]
    fn clear_all_clears_group_profiles_too() {
        let conn = test_db();
        let meta = r#"{"name":"alice"}"#;
        let cache = ProfileCache::from_metadata_json(Some(meta.to_string()), 1, 0);
        save_profile(&conn, "pk1", &cache);
        save_group_profile(&conn, "pk1", "chat1", &cache);
        save_follows(&conn, &["pk1".to_string(), "pk2".to_string()]);

        clear_all(&conn);

        assert!(load_profiles(&conn).is_empty());
        assert!(load_group_profiles(&conn, "chat1").is_empty());
        assert!(load_follows(&conn).is_empty());
    }

    #[test]
    fn follows_roundtrip() {
        let conn = test_db();
        assert!(load_follows(&conn).is_empty());

        let pks = vec!["aaa".to_string(), "bbb".to_string(), "ccc".to_string()];
        save_follows(&conn, &pks);

        let mut loaded = load_follows(&conn);
        loaded.sort();
        assert_eq!(loaded, vec!["aaa", "bbb", "ccc"]);

        // Replace with a different set.
        save_follows(&conn, &["bbb".to_string(), "ddd".to_string()]);
        let mut loaded = load_follows(&conn);
        loaded.sort();
        assert_eq!(loaded, vec!["bbb", "ddd"]);
    }

    #[test]
    fn follows_add_remove() {
        let conn = test_db();
        add_follow(&conn, "aaa");
        add_follow(&conn, "bbb");
        add_follow(&conn, "aaa"); // duplicate, should be ignored

        let mut loaded = load_follows(&conn);
        loaded.sort();
        assert_eq!(loaded, vec!["aaa", "bbb"]);

        remove_follow(&conn, "aaa");
        assert_eq!(load_follows(&conn), vec!["bbb"]);
    }

    #[test]
    fn developer_mode_roundtrip() {
        let conn = test_db();
        assert!(!load_developer_mode(&conn));

        save_developer_mode(&conn, true);
        assert!(load_developer_mode(&conn));

        save_developer_mode(&conn, false);
        assert!(!load_developer_mode(&conn));
    }
}
