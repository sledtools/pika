ALTER TABLE branch_ci_run_lanes
ADD COLUMN execution_reason TEXT NOT NULL DEFAULT 'queued';

ALTER TABLE branch_ci_run_lanes
ADD COLUMN failure_kind TEXT;

ALTER TABLE branch_ci_run_lanes
ADD COLUMN ci_target_key TEXT;

ALTER TABLE nightly_run_lanes
ADD COLUMN execution_reason TEXT NOT NULL DEFAULT 'queued';

ALTER TABLE nightly_run_lanes
ADD COLUMN failure_kind TEXT;

ALTER TABLE nightly_run_lanes
ADD COLUMN ci_target_key TEXT;

UPDATE branch_ci_run_lanes
SET execution_reason = CASE
    WHEN status = 'running' THEN 'running'
    ELSE execution_reason
END;

UPDATE nightly_run_lanes
SET execution_reason = CASE
    WHEN status = 'running' THEN 'running'
    ELSE execution_reason
END;

UPDATE branch_ci_run_lanes
SET ci_target_key = CASE
    WHEN pikaci_target_id IS NOT NULL AND TRIM(pikaci_target_id) <> '' THEN pikaci_target_id
    WHEN concurrency_group = 'apple-host' THEN 'apple-host'
    WHEN concurrency_group = 'nightly-android' THEN 'nightly-android'
    ELSE NULL
END
WHERE ci_target_key IS NULL;

UPDATE nightly_run_lanes
SET ci_target_key = CASE
    WHEN pikaci_target_id IS NOT NULL AND TRIM(pikaci_target_id) <> '' THEN pikaci_target_id
    WHEN concurrency_group = 'apple-host' THEN 'apple-host'
    WHEN concurrency_group = 'nightly-android' THEN 'nightly-android'
    ELSE NULL
END
WHERE ci_target_key IS NULL;

CREATE INDEX idx_branch_ci_run_lanes_status_reason
    ON branch_ci_run_lanes(status, execution_reason, id ASC);

CREATE INDEX idx_branch_ci_run_lanes_target_key
    ON branch_ci_run_lanes(ci_target_key, status, id ASC);

CREATE INDEX idx_nightly_run_lanes_status_reason
    ON nightly_run_lanes(status, execution_reason, id ASC);

CREATE INDEX idx_nightly_run_lanes_target_key
    ON nightly_run_lanes(ci_target_key, status, id ASC);

CREATE TABLE ci_target_health (
    target_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    consecutive_infra_failure_count INTEGER NOT NULL DEFAULT 0,
    last_success_at TEXT,
    last_failure_at TEXT,
    last_failure_kind TEXT,
    cooloff_until TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
