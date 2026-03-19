ALTER TABLE agent_instances
    ADD COLUMN provider TEXT NOT NULL DEFAULT 'incus';

ALTER TABLE agent_instances
    RENAME COLUMN incus_config TO provider_config;
