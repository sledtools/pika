ALTER TABLE branch_ci_run_lanes
ADD COLUMN concurrency_group TEXT;

ALTER TABLE nightly_run_lanes
ADD COLUMN concurrency_group TEXT;

CREATE INDEX idx_branch_ci_run_lanes_status_group
    ON branch_ci_run_lanes(status, concurrency_group, id ASC);

CREATE INDEX idx_nightly_run_lanes_status_group
    ON nightly_run_lanes(status, concurrency_group, id ASC);
