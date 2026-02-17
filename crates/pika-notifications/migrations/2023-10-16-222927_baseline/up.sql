CREATE TABLE subscription_info
(
    id           TEXT PRIMARY KEY,
    device_token TEXT      NOT NULL,
    platform     TEXT      NOT NULL,  -- 'ios' or 'android'
    created_at   TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE TABLE group_subscriptions
(
    id         TEXT NOT NULL,    -- FK to subscription_info.id
    group_id   TEXT NOT NULL,    -- the #h tag group identifier
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    PRIMARY KEY (id, group_id),
    FOREIGN KEY (id) REFERENCES subscription_info (id)
);
