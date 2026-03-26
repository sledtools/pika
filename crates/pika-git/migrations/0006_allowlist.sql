CREATE TABLE IF NOT EXISTS chat_allowlist (
    npub TEXT PRIMARY KEY,
    active INTEGER NOT NULL DEFAULT 1,
    note TEXT,
    updated_by TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS chat_allowlist_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    actor_npub TEXT NOT NULL,
    target_npub TEXT NOT NULL,
    action TEXT NOT NULL,
    note TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_chat_allowlist_active
    ON chat_allowlist(active, npub);

CREATE INDEX IF NOT EXISTS idx_chat_allowlist_audit_target_created
    ON chat_allowlist_audit(target_npub, created_at DESC);
