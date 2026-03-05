CREATE TABLE agent_instances (
    agent_id TEXT PRIMARY KEY,
    owner_npub TEXT NOT NULL,
    vm_id TEXT,
    phase TEXT NOT NULL CHECK (phase IN ('creating', 'ready', 'error')),
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX agent_instances_owner_active_idx
    ON agent_instances (owner_npub)
    WHERE phase IN ('creating', 'ready');

CREATE UNIQUE INDEX agent_instances_vm_id_idx
    ON agent_instances (vm_id)
    WHERE vm_id IS NOT NULL;
