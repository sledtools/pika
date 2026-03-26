ALTER TABLE branch_ci_run_lanes
ADD COLUMN pikaci_run_id TEXT;

ALTER TABLE branch_ci_run_lanes
ADD COLUMN pikaci_target_id TEXT;

ALTER TABLE nightly_run_lanes
ADD COLUMN pikaci_run_id TEXT;

ALTER TABLE nightly_run_lanes
ADD COLUMN pikaci_target_id TEXT;
