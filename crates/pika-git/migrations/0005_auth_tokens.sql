CREATE TABLE IF NOT EXISTS auth_tokens (
    token TEXT PRIMARY KEY,
    npub TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_auth_tokens_created ON auth_tokens(created_at);
