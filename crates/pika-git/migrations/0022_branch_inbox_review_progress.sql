ALTER TABLE branch_inbox_states
    ADD COLUMN last_reviewed_artifact_id INTEGER REFERENCES branch_artifact_versions(id) ON DELETE SET NULL;

ALTER TABLE branch_inbox_states
    ADD COLUMN last_reviewed_at TEXT;
