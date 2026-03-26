ALTER TABLE branch_ci_run_lanes
    ADD COLUMN claim_token INTEGER NOT NULL DEFAULT 0;

ALTER TABLE nightly_run_lanes
    ADD COLUMN claim_token INTEGER NOT NULL DEFAULT 0;
