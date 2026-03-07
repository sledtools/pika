-- Corrective migration for databases that already applied the earlier
-- allowlist_max_agents migration revision, which dropped the partial unique
-- index before we reverted the API back to single-active-agent semantics.
--
-- If duplicate active rows already exist, keep only the newest one and mark
-- the rest errored so the unique index can be recreated safely.
UPDATE agent_instances SET phase = 'error'
WHERE phase IN ('creating', 'ready')
  AND ctid NOT IN (
    SELECT DISTINCT ON (owner_npub) ctid
    FROM agent_instances
    WHERE phase IN ('creating', 'ready')
    ORDER BY owner_npub, created_at DESC, (phase = 'ready') DESC, updated_at DESC, agent_id DESC
  );

CREATE UNIQUE INDEX IF NOT EXISTS agent_instances_owner_active_idx
    ON agent_instances (owner_npub)
    WHERE phase IN ('creating', 'ready');
