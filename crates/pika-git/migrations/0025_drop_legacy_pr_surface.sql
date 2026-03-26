PRAGMA foreign_keys = OFF;

DROP TABLE IF EXISTS chat_messages;
DROP TABLE IF EXISTS chat_sessions;
DROP TABLE IF EXISTS artifact_user_states;
DROP TABLE IF EXISTS artifact_versions;
DROP TABLE IF EXISTS inbox_dismissals;
DROP TABLE IF EXISTS inbox;
DROP TABLE IF EXISTS generated_artifacts;
DROP TABLE IF EXISTS pull_requests;
DROP TABLE IF EXISTS poll_markers;

PRAGMA foreign_keys = ON;
