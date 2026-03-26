CREATE TABLE IF NOT EXISTS branch_inbox_states (
    npub TEXT NOT NULL,
    branch_id INTEGER NOT NULL,
    artifact_id INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('inbox', 'dismissed')),
    reason TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    dismissed_at TEXT,
    PRIMARY KEY (npub, branch_id),
    FOREIGN KEY (branch_id) REFERENCES branch_records(id) ON DELETE CASCADE,
    FOREIGN KEY (artifact_id) REFERENCES branch_artifact_versions(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_branch_inbox_states_npub_state_created
    ON branch_inbox_states(npub, state, created_at);

CREATE INDEX IF NOT EXISTS idx_branch_inbox_states_artifact_id
    ON branch_inbox_states(artifact_id);
