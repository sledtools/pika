ALTER TABLE agent_instances
    ADD COLUMN provider TEXT NOT NULL DEFAULT 'microvm',
    ADD COLUMN provider_config TEXT;
