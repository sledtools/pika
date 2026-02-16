use crate::models::schema::group_subscriptions;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Queryable, Insertable, AsChangeset, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[diesel(check_for_backend(diesel::pg::Pg))]
#[diesel(table_name = group_subscriptions)]
pub struct GroupSubscription {
    pub id: String,
    pub group_id: String,
    pub created_at: chrono::NaiveDateTime,
}

#[derive(Insertable)]
#[diesel(table_name = group_subscriptions)]
pub struct NewGroupSubscription<'a> {
    pub id: &'a str,
    pub group_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GroupFilterInfo {
    pub group_ids: Vec<String>,
}

impl GroupSubscription {
    pub fn subscribe(
        conn: &mut PgConnection,
        id: &str,
        group_ids: &[String],
    ) -> anyhow::Result<()> {
        let new_rows: Vec<NewGroupSubscription> = group_ids
            .iter()
            .map(|group_id| NewGroupSubscription {
                id,
                group_id: group_id.as_str(),
            })
            .collect();

        diesel::insert_into(group_subscriptions::table)
            .values(&new_rows)
            .on_conflict_do_nothing()
            .execute(conn)?;

        Ok(())
    }

    pub fn unsubscribe(
        conn: &mut PgConnection,
        id: &str,
        group_ids: &[String],
    ) -> anyhow::Result<()> {
        diesel::delete(
            group_subscriptions::table
                .filter(group_subscriptions::id.eq(id))
                .filter(group_subscriptions::group_id.eq_any(group_ids)),
        )
        .execute(conn)?;

        Ok(())
    }

    pub fn get_filter_info(conn: &mut PgConnection) -> anyhow::Result<GroupFilterInfo> {
        let group_ids = group_subscriptions::table
            .select(group_subscriptions::group_id)
            .distinct()
            .load::<String>(conn)?;

        Ok(GroupFilterInfo { group_ids })
    }
}
