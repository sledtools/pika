ALTER TABLE chat_allowlist
    ADD COLUMN can_forge_write INTEGER NOT NULL DEFAULT 0;
