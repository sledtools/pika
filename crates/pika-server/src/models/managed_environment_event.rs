use crate::models::schema::managed_environment_events;
use chrono::NaiveDateTime;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Queryable, Selectable, Insertable, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[diesel(check_for_backend(diesel::pg::Pg))]
#[diesel(table_name = managed_environment_events)]
pub struct ManagedEnvironmentEvent {
    pub id: i64,
    pub owner_npub: String,
    pub agent_id: Option<String>,
    pub vm_id: Option<String>,
    pub event_kind: String,
    pub message: String,
    pub request_id: Option<String>,
    pub created_at: NaiveDateTime,
}

#[derive(Insertable)]
#[diesel(table_name = managed_environment_events)]
struct NewManagedEnvironmentEvent<'a> {
    owner_npub: &'a str,
    agent_id: Option<&'a str>,
    vm_id: Option<&'a str>,
    event_kind: &'a str,
    message: &'a str,
    request_id: Option<&'a str>,
}

impl ManagedEnvironmentEvent {
    pub fn record(
        conn: &mut PgConnection,
        owner_npub: &str,
        agent_id: Option<&str>,
        vm_id: Option<&str>,
        event_kind: &str,
        message: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<Self> {
        let row = NewManagedEnvironmentEvent {
            owner_npub,
            agent_id,
            vm_id,
            event_kind,
            message,
            request_id,
        };
        let inserted = diesel::insert_into(managed_environment_events::table)
            .values(&row)
            .returning(Self::as_returning())
            .get_result(conn)?;
        Ok(inserted)
    }

    pub fn list_recent_by_owner(
        conn: &mut PgConnection,
        owner_npub: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<Self>> {
        let rows = managed_environment_events::table
            .filter(managed_environment_events::owner_npub.eq(owner_npub))
            .order((
                managed_environment_events::created_at.desc(),
                managed_environment_events::id.desc(),
            ))
            .limit(limit)
            .select(Self::as_select())
            .load::<Self>(conn)?;
        Ok(rows)
    }
}
