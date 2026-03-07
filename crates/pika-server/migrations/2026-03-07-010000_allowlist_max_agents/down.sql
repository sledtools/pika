-- Mark excess active rows as error so the unique index can be recreated.
-- For each owner with more than one active agent, keep only the newest one.
UPDATE agent_instances SET phase = 'error'
WHERE phase IN ('creating', 'ready')
  AND ctid NOT IN (
    SELECT DISTINCT ON (owner_npub) ctid
    FROM agent_instances
    WHERE phase IN ('creating', 'ready')
    ORDER BY owner_npub, created_at DESC
  );

-- Restore the one-active-agent-per-owner unique index.
CREATE UNIQUE INDEX agent_instances_owner_active_idx
    ON agent_instances (owner_npub)
    WHERE phase IN ('creating', 'ready');

ALTER TABLE agent_allowlist DROP COLUMN IF EXISTS max_agents;
