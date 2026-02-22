use std::collections::HashMap;

use rusqlite::Connection;

use super::ProfileCache;

pub fn open_profile_db(data_dir: &str) -> Result<Connection, rusqlite::Error> {
    let path = std::path::Path::new(data_dir).join("profiles.sqlite3");
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS profiles (
            pubkey TEXT PRIMARY KEY,
            metadata JSONB,
            name TEXT,
            about TEXT,
            picture_url TEXT,
            event_created_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS follows (
            pubkey TEXT PRIMARY KEY
        );",
    )?;
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
         FROM profiles",
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

/// Load the full metadata JSON for a single profile (used for profile editing).
pub fn load_metadata_json(conn: &Connection, pubkey: &str) -> Option<String> {
    conn.query_row(
        "SELECT json(metadata) FROM profiles WHERE pubkey = ?1",
        [pubkey],
        |row| row.get(0),
    )
    .ok()
}

pub fn save_profile(conn: &Connection, pubkey: &str, cache: &ProfileCache) {
    if let Err(e) = conn.execute(
        "INSERT INTO profiles (pubkey, metadata, name, about, picture_url, event_created_at)
         VALUES (?1, jsonb(?2), ?3, ?4, ?5, ?6)
         ON CONFLICT(pubkey) DO UPDATE SET
            metadata = jsonb(excluded.metadata),
            name = excluded.name,
            about = excluded.about,
            picture_url = excluded.picture_url,
            event_created_at = excluded.event_created_at",
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

/// Delete all cached profiles and follows (used on logout).
pub fn clear_all(conn: &Connection) {
    if let Err(e) = conn.execute_batch("DELETE FROM profiles; DELETE FROM follows;") {
        tracing::warn!(%e, "failed to clear profile cache db");
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
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS profiles (
                pubkey TEXT PRIMARY KEY,
                metadata JSONB,
                name TEXT,
                about TEXT,
                picture_url TEXT,
                event_created_at INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS follows (
                pubkey TEXT PRIMARY KEY
            );",
        )
        .unwrap();
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
    fn clear_all_clears_follows() {
        let conn = test_db();
        let metadata = r#"{"name":"alice"}"#;
        let cache = ProfileCache::from_metadata_json(Some(metadata.to_string()), 1, 0);
        save_profile(&conn, "pk1", &cache);
        save_follows(&conn, &["pk1".to_string(), "pk2".to_string()]);

        clear_all(&conn);

        assert!(load_profiles(&conn).is_empty());
        assert!(load_follows(&conn).is_empty());
    }
}
