-- Add per-user agent limit. NULL means unlimited; default 1 preserves
-- existing one-agent-per-user behaviour for regular users.
ALTER TABLE agent_allowlist ADD COLUMN max_agents INTEGER DEFAULT 1;

-- Drop the hard one-active-agent-per-owner unique index.
-- Enforcement moves to application logic so the limit can vary per user.
DROP INDEX IF EXISTS agent_instances_owner_active_idx;
