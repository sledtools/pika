PRAGMA foreign_keys = OFF;

ALTER TABLE chat_sessions RENAME TO chat_sessions_legacy;
ALTER TABLE chat_messages RENAME TO chat_messages_legacy;
DROP INDEX IF EXISTS idx_chat_sessions_artifact_npub;
DROP INDEX IF EXISTS idx_chat_messages_session;

CREATE TABLE artifact_versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    pr_id INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    source_head_sha TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending', 'generating', 'ready', 'failed', 'superseded')),
    tutorial_json TEXT,
    html TEXT,
    error_message TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    next_retry_at TEXT,
    generated_head_sha TEXT,
    unified_diff TEXT,
    claude_session_id TEXT,
    is_current INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    ready_at TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(pr_id, version)
);

CREATE INDEX idx_artifact_versions_status_next_retry
    ON artifact_versions(status, next_retry_at, updated_at);

CREATE INDEX idx_artifact_versions_pr_version
    ON artifact_versions(pr_id, version DESC);

CREATE UNIQUE INDEX idx_artifact_versions_current_per_pr
    ON artifact_versions(pr_id)
    WHERE is_current = 1;

INSERT INTO artifact_versions(
    id,
    pr_id,
    version,
    source_head_sha,
    status,
    tutorial_json,
    html,
    error_message,
    retry_count,
    next_retry_at,
    generated_head_sha,
    unified_diff,
    claude_session_id,
    is_current,
    created_at,
    ready_at,
    updated_at
)
SELECT
    ga.id,
    ga.pr_id,
    1,
    COALESCE(ga.generated_head_sha, pr.head_sha),
    ga.status,
    ga.tutorial_json,
    ga.html,
    ga.error_message,
    ga.retry_count,
    ga.next_retry_at,
    ga.generated_head_sha,
    ga.unified_diff,
    ga.claude_session_id,
    CASE WHEN ga.status = 'ready' THEN 1 ELSE 0 END,
    ga.updated_at,
    CASE WHEN ga.status = 'ready' THEN ga.updated_at ELSE NULL END,
    ga.updated_at
FROM generated_artifacts ga
JOIN pull_requests pr ON pr.id = ga.pr_id;

CREATE TABLE chat_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    artifact_id INTEGER NOT NULL REFERENCES artifact_versions(id) ON DELETE CASCADE,
    npub TEXT NOT NULL,
    claude_session_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(artifact_id, npub)
);

INSERT INTO chat_sessions(id, artifact_id, npub, claude_session_id, created_at, updated_at)
SELECT id, artifact_id, npub, claude_session_id, created_at, updated_at
FROM chat_sessions_legacy;

CREATE INDEX idx_chat_sessions_artifact_npub
    ON chat_sessions(artifact_id, npub);

CREATE TABLE chat_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK(role IN ('user', 'assistant')),
    content TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO chat_messages(id, session_id, role, content, created_at)
SELECT id, session_id, role, content, created_at
FROM chat_messages_legacy;

CREATE INDEX idx_chat_messages_session
    ON chat_messages(session_id, created_at ASC);

CREATE TABLE artifact_user_states (
    npub TEXT NOT NULL,
    artifact_id INTEGER NOT NULL REFERENCES artifact_versions(id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK(state IN ('inbox', 'dismissed', 'superseded')),
    reason TEXT NOT NULL DEFAULT 'generation_ready',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    dismissed_at TEXT,
    PRIMARY KEY (npub, artifact_id)
);

CREATE INDEX idx_artifact_user_states_npub_state_created
    ON artifact_user_states(npub, state, created_at DESC);

CREATE INDEX idx_artifact_user_states_artifact_state
    ON artifact_user_states(artifact_id, state, updated_at DESC);

INSERT OR IGNORE INTO artifact_user_states(npub, artifact_id, state, reason, created_at, updated_at, dismissed_at)
SELECT
    i.npub,
    av.id,
    'inbox',
    COALESCE(i.reason, 'generation_ready'),
    i.created_at,
    i.created_at,
    NULL
FROM inbox i
JOIN artifact_versions av ON av.pr_id = i.pr_id AND av.version = 1;

INSERT INTO artifact_user_states(npub, artifact_id, state, reason, created_at, updated_at, dismissed_at)
SELECT
    d.npub,
    av.id,
    'dismissed',
    'generation_ready',
    d.dismissed_at,
    d.dismissed_at,
    d.dismissed_at
FROM inbox_dismissals d
JOIN artifact_versions av ON av.pr_id = d.pr_id AND av.version = 1
ON CONFLICT(npub, artifact_id) DO UPDATE SET
    state = 'dismissed',
    updated_at = excluded.updated_at,
    dismissed_at = excluded.dismissed_at;

DROP TABLE chat_messages_legacy;
DROP TABLE chat_sessions_legacy;
DROP TABLE inbox_dismissals;
DROP TABLE inbox;
DROP TABLE generated_artifacts;

PRAGMA foreign_keys = ON;
