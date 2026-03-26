ALTER TABLE branch_ci_run_lanes
ADD COLUMN structured_pikaci_target_id TEXT;

ALTER TABLE nightly_run_lanes
ADD COLUMN structured_pikaci_target_id TEXT;
