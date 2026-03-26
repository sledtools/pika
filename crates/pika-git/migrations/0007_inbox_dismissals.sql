CREATE TABLE IF NOT EXISTS inbox_dismissals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    npub TEXT NOT NULL,
    pr_id INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    dismissed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(npub, pr_id)
);

CREATE INDEX IF NOT EXISTS idx_inbox_dismissals_npub
    ON inbox_dismissals(npub, dismissed_at DESC);
