ALTER TABLE branch_ci_run_lanes
    ADD COLUMN last_heartbeat_at TEXT;

ALTER TABLE branch_ci_run_lanes
    ADD COLUMN lease_expires_at TEXT;

UPDATE branch_ci_run_lanes
SET last_heartbeat_at = COALESCE(started_at, CURRENT_TIMESTAMP),
    lease_expires_at = datetime(COALESCE(started_at, CURRENT_TIMESTAMP), '+120 seconds')
WHERE status = 'running'
  AND lease_expires_at IS NULL;

CREATE INDEX idx_branch_ci_run_lanes_status_lease
    ON branch_ci_run_lanes(status, lease_expires_at ASC);

ALTER TABLE nightly_run_lanes
    ADD COLUMN last_heartbeat_at TEXT;

ALTER TABLE nightly_run_lanes
    ADD COLUMN lease_expires_at TEXT;

UPDATE nightly_run_lanes
SET last_heartbeat_at = COALESCE(started_at, CURRENT_TIMESTAMP),
    lease_expires_at = datetime(COALESCE(started_at, CURRENT_TIMESTAMP), '+120 seconds')
WHERE status = 'running'
  AND lease_expires_at IS NULL;

CREATE INDEX idx_nightly_run_lanes_status_lease
    ON nightly_run_lanes(status, lease_expires_at ASC);
