use crate::models::schema::{group_subscriptions, subscription_info};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Queryable, Insertable, AsChangeset, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[diesel(check_for_backend(diesel::pg::Pg))]
#[diesel(table_name = subscription_info)]
pub struct SubscriptionInfo {
    pub id: String,
    pub device_token: String,
    pub platform: String,

    pub created_at: chrono::NaiveDateTime,
}

#[derive(Insertable, AsChangeset)]
#[diesel(table_name = subscription_info)]
pub struct NewSubscriptionInfo<'a> {
    pub id: &'a str,
    pub device_token: &'a str,
    pub platform: &'a str,
}

impl SubscriptionInfo {
    pub fn register(
        conn: &mut PgConnection,
        id: &str,
        device_token: &str,
        platform: &str,
    ) -> anyhow::Result<String> {
        let new = NewSubscriptionInfo {
            id,
            device_token,
            platform,
        };

        let id: String = diesel::insert_into(subscription_info::table)
            .values(&new)
            .returning(subscription_info::id)
            .on_conflict(subscription_info::id)
            .do_update()
            .set(&new)
            .get_result(conn)?;

        Ok(id)
    }

    pub fn get_all(conn: &mut PgConnection) -> anyhow::Result<Vec<Self>> {
        let items = subscription_info::table.load::<Self>(conn)?;
        Ok(items)
    }

    pub fn find_by_group(conn: &mut PgConnection, group_id: &str) -> anyhow::Result<Vec<Self>> {
        let results = subscription_info::table
            .inner_join(group_subscriptions::table)
            .filter(group_subscriptions::group_id.eq(group_id))
            .select(subscription_info::all_columns)
            .load::<Self>(conn)?;

        Ok(results)
    }
}
