UPDATE branch_ci_run_lanes
SET execution_reason = 'queued'
WHERE execution_reason = 'target_unhealthy';

UPDATE nightly_run_lanes
SET execution_reason = 'queued'
WHERE execution_reason = 'target_unhealthy';

DROP TABLE IF EXISTS ci_target_health;
