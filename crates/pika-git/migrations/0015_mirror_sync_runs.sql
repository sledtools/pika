CREATE TABLE mirror_sync_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    remote_name TEXT NOT NULL,
    trigger_source TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('success', 'failed', 'not_configured')),
    local_default_head TEXT,
    remote_default_head TEXT,
    lagging_ref_count INTEGER,
    synced_ref_count INTEGER,
    error_text TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_mirror_sync_runs_repo_remote_id
    ON mirror_sync_runs(repo_id, remote_name, id DESC);
