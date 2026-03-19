// @generated automatically by Diesel CLI.

diesel::table! {
    agent_allowlist (npub) {
        npub -> Text,
        active -> Bool,
        note -> Nullable<Text>,
        updated_by -> Text,
        updated_at -> Timestamp,
        max_agents -> Nullable<Int4>,
    }
}

diesel::table! {
    agent_allowlist_audit (id) {
        id -> Int8,
        actor_npub -> Text,
        target_npub -> Text,
        action -> Text,
        note -> Nullable<Text>,
        created_at -> Timestamp,
    }
}

diesel::table! {
    agent_instances (agent_id) {
        agent_id -> Text,
        owner_npub -> Text,
        vm_id -> Nullable<Text>,
        incus_config -> Nullable<Text>,
        phase -> Text,
        created_at -> Timestamp,
        updated_at -> Timestamp,
    }
}

diesel::table! {
    group_subscriptions (id, group_id) {
        id -> Text,
        group_id -> Text,
        created_at -> Timestamp,
    }
}

diesel::table! {
    managed_environment_events (id) {
        id -> Int8,
        owner_npub -> Text,
        agent_id -> Nullable<Text>,
        vm_id -> Nullable<Text>,
        event_kind -> Text,
        message -> Text,
        request_id -> Nullable<Text>,
        created_at -> Timestamp,
    }
}

diesel::table! {
    subscription_info (id) {
        id -> Text,
        device_token -> Text,
        platform -> Text,
        created_at -> Timestamp,
    }
}

diesel::joinable!(group_subscriptions -> subscription_info (id));

diesel::allow_tables_to_appear_in_same_query!(
    agent_allowlist,
    agent_allowlist_audit,
    agent_instances,
    group_subscriptions,
    managed_environment_events,
    subscription_info,
);
