use crate::models::schema::agent_instances;
use chrono::{NaiveDateTime, Utc};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

pub const AGENT_PHASE_CREATING: &str = "creating";
pub const AGENT_PHASE_READY: &str = "ready";
pub const AGENT_PHASE_ERROR: &str = "error";

fn is_valid_phase(phase: &str) -> bool {
    matches!(
        phase,
        AGENT_PHASE_CREATING | AGENT_PHASE_READY | AGENT_PHASE_ERROR
    )
}

#[derive(
    Queryable, Selectable, Insertable, AsChangeset, Serialize, Deserialize, Debug, Clone, PartialEq,
)]
#[diesel(check_for_backend(diesel::pg::Pg))]
#[diesel(table_name = agent_instances)]
pub struct AgentInstance {
    pub agent_id: String,
    pub owner_npub: String,
    pub vm_id: Option<String>,
    pub phase: String,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

#[derive(Insertable, AsChangeset)]
#[diesel(table_name = agent_instances)]
pub struct NewAgentInstance<'a> {
    pub agent_id: &'a str,
    pub owner_npub: &'a str,
    pub vm_id: Option<&'a str>,
    pub phase: &'a str,
}

impl AgentInstance {
    pub fn create(
        conn: &mut PgConnection,
        owner_npub: &str,
        agent_id: &str,
        vm_id: Option<&str>,
        phase: &str,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(is_valid_phase(phase), "invalid agent phase: {phase}");
        let row = NewAgentInstance {
            owner_npub,
            agent_id,
            vm_id,
            phase,
        };
        let created = diesel::insert_into(agent_instances::table)
            .values(&row)
            .returning(Self::as_returning())
            .get_result(conn)?;
        Ok(created)
    }

    pub fn find_by_agent_id(
        conn: &mut PgConnection,
        agent_id: &str,
    ) -> anyhow::Result<Option<Self>> {
        let found = agent_instances::table
            .filter(agent_instances::agent_id.eq(agent_id))
            .select(Self::as_select())
            .first::<Self>(conn)
            .optional()?;
        Ok(found)
    }

    pub fn find_active_by_owner(
        conn: &mut PgConnection,
        owner_npub: &str,
    ) -> anyhow::Result<Option<Self>> {
        let found = agent_instances::table
            .filter(agent_instances::owner_npub.eq(owner_npub))
            .filter(agent_instances::phase.eq_any([AGENT_PHASE_CREATING, AGENT_PHASE_READY]))
            .order(agent_instances::created_at.desc())
            .select(Self::as_select())
            .first::<Self>(conn)
            .optional()?;
        Ok(found)
    }

    pub fn find_latest_by_owner(
        conn: &mut PgConnection,
        owner_npub: &str,
    ) -> anyhow::Result<Option<Self>> {
        let found = agent_instances::table
            .filter(agent_instances::owner_npub.eq(owner_npub))
            .order(agent_instances::created_at.desc())
            .select(Self::as_select())
            .first::<Self>(conn)
            .optional()?;
        Ok(found)
    }

    pub fn update_phase(
        conn: &mut PgConnection,
        agent_id: &str,
        phase: &str,
        vm_id: Option<&str>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(is_valid_phase(phase), "invalid agent phase: {phase}");
        let updated =
            diesel::update(agent_instances::table.filter(agent_instances::agent_id.eq(agent_id)))
                .set((
                    agent_instances::phase.eq(phase),
                    agent_instances::vm_id.eq(vm_id),
                    agent_instances::updated_at.eq(Utc::now().naive_utc()),
                ))
                .returning(Self::as_returning())
                .get_result(conn)?;
        Ok(updated)
    }
}
