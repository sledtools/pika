-- Add allowlist metadata for future per-user agent limits.
-- The v1 API/client contract is still single-agent, so keep the existing
-- one-active-agent-per-owner DB guard in place for now.
ALTER TABLE agent_allowlist ADD COLUMN max_agents INTEGER DEFAULT 1;
CREATE UNIQUE INDEX IF NOT EXISTS agent_instances_owner_active_idx
    ON agent_instances (owner_npub)
    WHERE phase IN ('creating', 'ready');
