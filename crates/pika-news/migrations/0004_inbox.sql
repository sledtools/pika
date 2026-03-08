CREATE TABLE IF NOT EXISTS inbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    npub TEXT NOT NULL,
    pr_id INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    reason TEXT NOT NULL DEFAULT 'generation_ready',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(npub, pr_id)
);

CREATE INDEX IF NOT EXISTS idx_inbox_npub_created ON inbox(npub, created_at DESC);
