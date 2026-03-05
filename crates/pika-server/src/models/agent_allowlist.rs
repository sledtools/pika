use crate::models::schema::{agent_allowlist, agent_allowlist_audit};
use chrono::{NaiveDateTime, Utc};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(
    Queryable, Selectable, Insertable, AsChangeset, Serialize, Deserialize, Debug, Clone, PartialEq,
)]
#[diesel(check_for_backend(diesel::pg::Pg))]
#[diesel(table_name = agent_allowlist)]
pub struct AgentAllowlistEntry {
    pub npub: String,
    pub active: bool,
    pub note: Option<String>,
    pub updated_by: String,
    pub updated_at: NaiveDateTime,
}

#[derive(Insertable)]
#[diesel(table_name = agent_allowlist)]
pub struct NewAgentAllowlistEntry<'a> {
    pub npub: &'a str,
    pub active: bool,
    pub note: Option<&'a str>,
    pub updated_by: &'a str,
}

#[derive(Queryable, Selectable, Insertable, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[diesel(check_for_backend(diesel::pg::Pg))]
#[diesel(table_name = agent_allowlist_audit)]
pub struct AgentAllowlistAuditEntry {
    pub id: i64,
    pub actor_npub: String,
    pub target_npub: String,
    pub action: String,
    pub note: Option<String>,
    pub created_at: NaiveDateTime,
}

#[derive(Insertable)]
#[diesel(table_name = agent_allowlist_audit)]
struct NewAgentAllowlistAuditEntry<'a> {
    actor_npub: &'a str,
    target_npub: &'a str,
    action: &'a str,
    note: Option<&'a str>,
}

impl AgentAllowlistEntry {
    pub fn is_active(conn: &mut PgConnection, npub: &str) -> anyhow::Result<bool> {
        let active = agent_allowlist::table
            .filter(agent_allowlist::npub.eq(npub))
            .filter(agent_allowlist::active.eq(true))
            .select(agent_allowlist::npub)
            .first::<String>(conn)
            .optional()?
            .is_some();
        Ok(active)
    }

    pub fn list(conn: &mut PgConnection) -> anyhow::Result<Vec<Self>> {
        let rows = agent_allowlist::table
            .order(agent_allowlist::npub.asc())
            .select(Self::as_select())
            .load::<Self>(conn)?;
        Ok(rows)
    }

    pub fn upsert(
        conn: &mut PgConnection,
        npub: &str,
        active: bool,
        note: Option<&str>,
        updated_by: &str,
    ) -> anyhow::Result<Self> {
        let row = NewAgentAllowlistEntry {
            npub,
            active,
            note,
            updated_by,
        };
        let updated_at = Utc::now().naive_utc();
        let saved = diesel::insert_into(agent_allowlist::table)
            .values(&row)
            .on_conflict(agent_allowlist::npub)
            .do_update()
            .set((
                agent_allowlist::active.eq(active),
                agent_allowlist::note.eq(note),
                agent_allowlist::updated_by.eq(updated_by),
                agent_allowlist::updated_at.eq(updated_at),
            ))
            .returning(Self::as_returning())
            .get_result(conn)?;
        Ok(saved)
    }

    pub fn set_active(
        conn: &mut PgConnection,
        npub: &str,
        active: bool,
        updated_by: &str,
    ) -> anyhow::Result<Self> {
        let existing_note = agent_allowlist::table
            .filter(agent_allowlist::npub.eq(npub))
            .select(agent_allowlist::note)
            .first::<Option<String>>(conn)
            .optional()?;
        let note = existing_note.flatten();
        Self::upsert(conn, npub, active, note.as_deref(), updated_by)
    }

    pub fn record_audit(
        conn: &mut PgConnection,
        actor_npub: &str,
        target_npub: &str,
        action: &str,
        note: Option<&str>,
    ) -> anyhow::Result<AgentAllowlistAuditEntry> {
        let row = NewAgentAllowlistAuditEntry {
            actor_npub,
            target_npub,
            action,
            note,
        };
        let inserted = diesel::insert_into(agent_allowlist_audit::table)
            .values(&row)
            .returning(AgentAllowlistAuditEntry::as_returning())
            .get_result(conn)?;
        Ok(inserted)
    }
}
