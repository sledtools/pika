ALTER TABLE agent_instances
    RENAME COLUMN provider_config TO incus_config;

ALTER TABLE agent_instances
    DROP COLUMN provider;
