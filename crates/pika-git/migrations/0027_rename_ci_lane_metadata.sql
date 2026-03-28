ALTER TABLE branch_ci_run_lanes
RENAME COLUMN pikaci_run_id TO ci_run_id;

ALTER TABLE branch_ci_run_lanes
RENAME COLUMN pikaci_target_id TO ci_target_id;

ALTER TABLE nightly_run_lanes
RENAME COLUMN pikaci_run_id TO ci_run_id;

ALTER TABLE nightly_run_lanes
RENAME COLUMN pikaci_target_id TO ci_target_id;
