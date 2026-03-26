ALTER TABLE repos ADD COLUMN canonical_git_dir TEXT;
ALTER TABLE repos ADD COLUMN default_branch TEXT NOT NULL DEFAULT 'master';
ALTER TABLE repos ADD COLUMN ci_entrypoint TEXT;

CREATE TABLE branch_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    branch_name TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    title TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('open', 'merged', 'closed')),
    head_sha TEXT NOT NULL,
    merge_base_sha TEXT NOT NULL,
    author_name TEXT,
    author_email TEXT,
    merge_commit_sha TEXT,
    merged_by TEXT,
    merged_at TEXT,
    closed_by TEXT,
    closed_at TEXT,
    deleted_at TEXT,
    updated_at TEXT NOT NULL,
    opened_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX idx_branch_records_open_name
    ON branch_records(repo_id, branch_name)
    WHERE state = 'open';

CREATE INDEX idx_branch_records_state_updated
    ON branch_records(state, updated_at DESC, id DESC);

CREATE TABLE branch_artifact_versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    branch_id INTEGER NOT NULL REFERENCES branch_records(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    source_head_sha TEXT NOT NULL,
    merge_base_sha TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending', 'generating', 'ready', 'failed', 'superseded')),
    tutorial_json TEXT,
    html TEXT,
    error_message TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    next_retry_at TEXT,
    generated_head_sha TEXT,
    unified_diff TEXT,
    is_current INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    ready_at TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(branch_id, version)
);

CREATE INDEX idx_branch_artifact_versions_status_next_retry
    ON branch_artifact_versions(status, next_retry_at, updated_at);

CREATE INDEX idx_branch_artifact_versions_branch_version
    ON branch_artifact_versions(branch_id, version DESC);

CREATE UNIQUE INDEX idx_branch_artifact_versions_current
    ON branch_artifact_versions(branch_id)
    WHERE is_current = 1;

CREATE TABLE branch_ci_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    branch_id INTEGER NOT NULL REFERENCES branch_records(id) ON DELETE CASCADE,
    source_head_sha TEXT NOT NULL,
    entrypoint TEXT NOT NULL,
    command_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued', 'running', 'success', 'failed')),
    log_text TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at TEXT,
    finished_at TEXT
);

CREATE INDEX idx_branch_ci_runs_status_created
    ON branch_ci_runs(status, created_at ASC);

CREATE INDEX idx_branch_ci_runs_branch_created
    ON branch_ci_runs(branch_id, id DESC);

CREATE TABLE merge_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    branch_id INTEGER NOT NULL UNIQUE REFERENCES branch_records(id) ON DELETE CASCADE,
    repo TEXT NOT NULL,
    branch_name TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    head_sha TEXT NOT NULL,
    merge_base_sha TEXT NOT NULL,
    merge_commit_sha TEXT NOT NULL,
    merged_by TEXT NOT NULL,
    merged_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_merge_records_merged_at
    ON merge_records(merged_at DESC, id DESC);
