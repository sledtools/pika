// @generated automatically by Diesel CLI.

diesel::table! {
    group_subscriptions (id, group_id) {
        id -> Text,
        group_id -> Text,
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

diesel::allow_tables_to_appear_in_same_query!(group_subscriptions, subscription_info,);
