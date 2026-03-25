ALTER TABLE branch_artifact_versions
    ADD COLUMN claude_session_id TEXT;

CREATE TABLE branch_chat_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    branch_artifact_id INTEGER NOT NULL REFERENCES branch_artifact_versions(id) ON DELETE CASCADE,
    npub TEXT NOT NULL,
    claude_session_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(branch_artifact_id, npub)
);

CREATE INDEX idx_branch_chat_sessions_artifact_npub
    ON branch_chat_sessions(branch_artifact_id, npub);

CREATE TABLE branch_chat_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL REFERENCES branch_chat_sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK(role IN ('user', 'assistant')),
    content TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_branch_chat_messages_session
    ON branch_chat_messages(session_id, created_at ASC);
