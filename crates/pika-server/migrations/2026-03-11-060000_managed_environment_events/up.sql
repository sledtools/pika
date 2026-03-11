CREATE TABLE managed_environment_events (
    id BIGSERIAL PRIMARY KEY,
    owner_npub TEXT NOT NULL,
    agent_id TEXT,
    vm_id TEXT,
    event_kind TEXT NOT NULL,
    message TEXT NOT NULL,
    request_id TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX managed_environment_events_owner_created_idx
    ON managed_environment_events (owner_npub, created_at DESC, id DESC);
