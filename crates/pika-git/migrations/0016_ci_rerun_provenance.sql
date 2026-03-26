ALTER TABLE branch_ci_runs
ADD COLUMN rerun_of_run_id INTEGER REFERENCES branch_ci_runs(id) ON DELETE SET NULL;

ALTER TABLE branch_ci_run_lanes
ADD COLUMN rerun_of_lane_run_id INTEGER REFERENCES branch_ci_run_lanes(id) ON DELETE SET NULL;

ALTER TABLE nightly_runs
ADD COLUMN rerun_of_run_id INTEGER REFERENCES nightly_runs(id) ON DELETE SET NULL;

ALTER TABLE nightly_run_lanes
ADD COLUMN rerun_of_lane_run_id INTEGER REFERENCES nightly_run_lanes(id) ON DELETE SET NULL;
