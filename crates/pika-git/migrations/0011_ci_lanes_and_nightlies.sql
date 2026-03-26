CREATE TABLE branch_ci_run_lanes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    branch_ci_run_id INTEGER NOT NULL REFERENCES branch_ci_runs(id) ON DELETE CASCADE,
    lane_id TEXT NOT NULL,
    title TEXT NOT NULL,
    entrypoint TEXT NOT NULL,
    command_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued', 'running', 'success', 'failed', 'skipped')),
    log_text TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at TEXT,
    finished_at TEXT,
    UNIQUE(branch_ci_run_id, lane_id)
);

CREATE INDEX idx_branch_ci_run_lanes_status_created
    ON branch_ci_run_lanes(status, created_at ASC);

CREATE INDEX idx_branch_ci_run_lanes_run_created
    ON branch_ci_run_lanes(branch_ci_run_id, id ASC);

CREATE TABLE nightly_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    source_ref TEXT NOT NULL,
    source_head_sha TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued', 'running', 'success', 'failed', 'skipped')),
    summary TEXT,
    scheduled_for TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at TEXT,
    finished_at TEXT,
    UNIQUE(repo_id, scheduled_for)
);

CREATE INDEX idx_nightly_runs_status_scheduled
    ON nightly_runs(status, scheduled_for DESC, id DESC);

CREATE TABLE nightly_run_lanes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    nightly_run_id INTEGER NOT NULL REFERENCES nightly_runs(id) ON DELETE CASCADE,
    lane_id TEXT NOT NULL,
    title TEXT NOT NULL,
    entrypoint TEXT NOT NULL,
    command_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued', 'running', 'success', 'failed', 'skipped')),
    log_text TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at TEXT,
    finished_at TEXT,
    UNIQUE(nightly_run_id, lane_id)
);

CREATE INDEX idx_nightly_run_lanes_status_created
    ON nightly_run_lanes(status, created_at ASC);

CREATE INDEX idx_nightly_run_lanes_run_created
    ON nightly_run_lanes(nightly_run_id, id ASC);
