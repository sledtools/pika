CREATE TABLE IF NOT EXISTS repos (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    repo TEXT NOT NULL UNIQUE,
    poll_interval_secs INTEGER NOT NULL DEFAULT 60,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS pull_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    pr_number INTEGER NOT NULL,
    title TEXT NOT NULL,
    url TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('open', 'merged', 'closed')),
    head_sha TEXT NOT NULL,
    base_ref TEXT NOT NULL,
    author_login TEXT,
    merged_at TEXT,
    updated_at TEXT NOT NULL,
    last_synced_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(repo_id, pr_number)
);

CREATE TABLE IF NOT EXISTS generated_artifacts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    pr_id INTEGER NOT NULL UNIQUE REFERENCES pull_requests(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK(status IN ('pending', 'generating', 'ready', 'failed')),
    tutorial_json TEXT,
    html TEXT,
    error_message TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    next_retry_at TEXT,
    generated_head_sha TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS poll_markers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_id INTEGER NOT NULL UNIQUE REFERENCES repos(id) ON DELETE CASCADE,
    last_polled_open_cursor TEXT,
    last_polled_merged_cursor TEXT,
    last_polled_at TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_pull_requests_repo_state_updated
    ON pull_requests(repo_id, state, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_generated_artifacts_status_next_retry
    ON generated_artifacts(status, next_retry_at);
