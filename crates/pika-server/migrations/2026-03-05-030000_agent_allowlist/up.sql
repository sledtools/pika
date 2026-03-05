CREATE TABLE agent_allowlist (
    npub TEXT PRIMARY KEY,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    note TEXT,
    updated_by TEXT NOT NULL,
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE TABLE agent_allowlist_audit (
    id BIGSERIAL PRIMARY KEY,
    actor_npub TEXT NOT NULL,
    target_npub TEXT NOT NULL,
    action TEXT NOT NULL,
    note TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX agent_allowlist_audit_target_idx
    ON agent_allowlist_audit (target_npub, created_at DESC);
