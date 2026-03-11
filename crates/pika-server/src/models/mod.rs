use diesel_migrations::{embed_migrations, EmbeddedMigrations};

pub mod agent_allowlist;
pub mod agent_instance;
pub mod group_subscription;
pub mod managed_environment_event;
mod schema;
pub mod subscription_info;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[cfg(test)]
mod test {
    use super::*;
    use crate::models::agent_allowlist::AgentAllowlistEntry;
    use crate::models::agent_instance::{
        AgentInstance, AGENT_PHASE_CREATING, AGENT_PHASE_ERROR, AGENT_PHASE_READY,
    };
    use crate::models::group_subscription::GroupSubscription;
    use crate::models::managed_environment_event::ManagedEnvironmentEvent;
    use crate::models::subscription_info::SubscriptionInfo;
    use crate::test_support::serial_test_guard;
    use diesel::prelude::*;
    use diesel::r2d2::{ConnectionManager, Pool};
    use diesel_migrations::MigrationHarness;

    const DEVICE_TOKEN: &str = "abc123devicetoken";
    const PLATFORM: &str = "ios";

    fn init_db_pool() -> Option<Pool<ConnectionManager<PgConnection>>> {
        dotenv::dotenv().ok();
        let Some(url) = std::env::var("DATABASE_URL").ok() else {
            eprintln!("SKIP: DATABASE_URL must be set for models db tests");
            return None;
        };
        if let Err(err) = PgConnection::establish(&url) {
            eprintln!("SKIP: postgres unavailable for models db tests: {err}");
            return None;
        }
        let manager = ConnectionManager::<PgConnection>::new(url);
        let db_pool = Pool::builder()
            .build(manager)
            .expect("Could not build connection pool");

        // run migrations
        let mut connection = db_pool.get().unwrap();
        connection
            .run_pending_migrations(MIGRATIONS)
            .expect("migrations could not run");

        Some(db_pool)
    }

    fn clear_database(db_pool: &Pool<ConnectionManager<PgConnection>>) {
        let conn = &mut db_pool.get().unwrap();

        conn.transaction::<_, anyhow::Error, _>(|conn| {
            diesel::delete(schema::agent_instances::table).execute(conn)?;
            diesel::delete(schema::managed_environment_events::table).execute(conn)?;
            diesel::delete(schema::agent_allowlist_audit::table).execute(conn)?;
            diesel::delete(schema::agent_allowlist::table).execute(conn)?;
            diesel::delete(schema::group_subscriptions::table).execute(conn)?;
            diesel::delete(schema::subscription_info::table).execute(conn)?;
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn test_register() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_db_pool() else {
            return;
        };
        clear_database(&db_pool);

        let mut conn = db_pool.get().unwrap();
        let expected_id = "dummy";
        let id =
            SubscriptionInfo::register(&mut conn, expected_id, DEVICE_TOKEN, PLATFORM).unwrap();

        assert_eq!(id, expected_id);

        clear_database(&db_pool);
    }

    #[tokio::test]
    async fn test_register_update() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_db_pool() else {
            return;
        };
        clear_database(&db_pool);

        let mut conn = db_pool.get().unwrap();
        let expected_id = "dummy";
        let id =
            SubscriptionInfo::register(&mut conn, expected_id, DEVICE_TOKEN, PLATFORM).unwrap();

        assert_eq!(id, expected_id);

        // test update with new device token
        let id =
            SubscriptionInfo::register(&mut conn, expected_id, "new_device_token_xyz", "android")
                .unwrap();

        assert_eq!(id, expected_id);

        clear_database(&db_pool);
    }

    #[tokio::test]
    async fn test_subscribe_groups() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_db_pool() else {
            return;
        };
        clear_database(&db_pool);

        let mut conn = db_pool.get().unwrap();
        let expected_id = "dummy";
        let id =
            SubscriptionInfo::register(&mut conn, expected_id, DEVICE_TOKEN, PLATFORM).unwrap();

        assert_eq!(id, expected_id);

        let group_ids = vec!["group1".to_string(), "group2".to_string()];
        GroupSubscription::subscribe(&mut conn, expected_id, &group_ids).unwrap();

        // verify find_by_group
        let subs = SubscriptionInfo::find_by_group(&mut conn, "group1").unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].id, expected_id);

        let subs = SubscriptionInfo::find_by_group(&mut conn, "group2").unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].id, expected_id);

        // verify get_filter_info
        let filter_info = GroupSubscription::get_filter_info(&mut conn).unwrap();
        assert_eq!(filter_info.group_ids.len(), 2);
        assert!(filter_info.group_ids.contains(&"group1".to_string()));
        assert!(filter_info.group_ids.contains(&"group2".to_string()));

        // verify subscribing again with overlap doesn't duplicate
        let group_ids2 = vec!["group2".to_string(), "group3".to_string()];
        GroupSubscription::subscribe(&mut conn, expected_id, &group_ids2).unwrap();

        let filter_info = GroupSubscription::get_filter_info(&mut conn).unwrap();
        assert_eq!(filter_info.group_ids.len(), 3);

        clear_database(&db_pool);
    }

    #[tokio::test]
    async fn test_agent_instance_active_owner_constraint() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_db_pool() else {
            return;
        };
        clear_database(&db_pool);
        let mut conn = db_pool.get().unwrap();
        let owner_npub = "npub1ownerconstrainttest";

        let created = AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-1",
            Some("vm-1"),
            AGENT_PHASE_CREATING,
        )
        .expect("insert initial creating row");
        assert_eq!(created.owner_npub, owner_npub);

        let duplicate_active = AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-2",
            Some("vm-2"),
            AGENT_PHASE_READY,
        )
        .expect_err("second active row should violate unique partial index");
        assert!(
            duplicate_active
                .to_string()
                .contains("agent_instances_owner_active_idx"),
            "unexpected error: {duplicate_active:?}"
        );

        let errored = AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-3",
            Some("vm-3"),
            AGENT_PHASE_ERROR,
        )
        .expect("error-phase rows are not active and should be allowed");
        assert_eq!(errored.phase, AGENT_PHASE_ERROR);

        let active = AgentInstance::find_active_by_owner(&mut conn, owner_npub)
            .expect("query active row")
            .expect("active row should exist");
        assert_eq!(active.agent_id, "agent-1");

        let latest = AgentInstance::find_latest_by_owner(&mut conn, owner_npub)
            .expect("query latest row")
            .expect("latest row should exist");
        assert_eq!(latest.agent_id, "agent-3");
        assert_eq!(latest.phase, AGENT_PHASE_ERROR);

        clear_database(&db_pool);
    }

    #[tokio::test]
    async fn test_agent_instance_latest_owner_row_surfaces_error_without_active_row() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_db_pool() else {
            return;
        };
        clear_database(&db_pool);
        let mut conn = db_pool.get().unwrap();
        let owner_npub = "npub1ownererrortest";

        let errored = AgentInstance::create(
            &mut conn,
            owner_npub,
            "agent-error",
            Some("vm-error"),
            AGENT_PHASE_ERROR,
        )
        .expect("insert error row");
        assert_eq!(errored.phase, AGENT_PHASE_ERROR);

        let active =
            AgentInstance::find_active_by_owner(&mut conn, owner_npub).expect("query active row");
        assert!(active.is_none(), "error row must not be treated as active");

        let latest = AgentInstance::find_latest_by_owner(&mut conn, owner_npub)
            .expect("query latest row")
            .expect("latest row should exist");
        assert_eq!(latest.agent_id, "agent-error");
        assert_eq!(latest.phase, AGENT_PHASE_ERROR);
        clear_database(&db_pool);
    }

    #[tokio::test]
    async fn test_managed_environment_event_record_and_list_recent() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_db_pool() else {
            return;
        };
        clear_database(&db_pool);
        let mut conn = db_pool.get().unwrap();
        let owner_npub = "npub1eventhistorytest";

        ManagedEnvironmentEvent::record(
            &mut conn,
            owner_npub,
            Some("agent-1"),
            None,
            "provision_requested",
            "Provision requested for a new Managed OpenClaw environment.",
            Some("req-1"),
        )
        .expect("insert first event");
        ManagedEnvironmentEvent::record(
            &mut conn,
            owner_npub,
            Some("agent-1"),
            Some("vm-1"),
            "provision_accepted",
            "Provision accepted. Managed OpenClaw is starting on VM vm-1.",
            Some("req-1"),
        )
        .expect("insert second event");

        let recent =
            ManagedEnvironmentEvent::list_recent_by_owner(&mut conn, owner_npub, 20).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].event_kind, "provision_accepted");
        assert_eq!(recent[0].vm_id.as_deref(), Some("vm-1"));
        assert_eq!(recent[1].event_kind, "provision_requested");

        let limited =
            ManagedEnvironmentEvent::list_recent_by_owner(&mut conn, owner_npub, 1).unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].event_kind, "provision_accepted");

        clear_database(&db_pool);
    }

    #[tokio::test]
    async fn test_agent_allowlist_upsert_and_active_lookup() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_db_pool() else {
            return;
        };
        clear_database(&db_pool);
        let mut conn = db_pool.get().unwrap();
        let npub = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y";

        let row = AgentAllowlistEntry::upsert(&mut conn, npub, true, Some("agent"), npub, None)
            .expect("upsert allowlist");
        assert_eq!(row.npub, npub);
        assert!(row.active);

        let active = AgentAllowlistEntry::is_active(&mut conn, npub).expect("active check");
        assert!(active);

        let row = AgentAllowlistEntry::set_active(&mut conn, npub, false, npub)
            .expect("deactivate allowlist");
        assert!(!row.active);
        let active = AgentAllowlistEntry::is_active(&mut conn, npub).expect("active check");
        assert!(!active);

        clear_database(&db_pool);
    }

    #[tokio::test]
    async fn test_agent_allowlist_transaction_rolls_back_when_audit_insert_fails() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_db_pool() else {
            return;
        };
        clear_database(&db_pool);
        let mut conn = db_pool.get().unwrap();
        let npub = "npub1rollbacktestuser";

        let result = conn.transaction::<_, anyhow::Error, _>(|conn| {
            AgentAllowlistEntry::upsert(conn, npub, true, Some("agent"), npub, None)?;
            AgentAllowlistEntry::record_audit(conn, npub, npub, "enabled\0invalid", None)?;
            Ok(())
        });

        assert!(result.is_err(), "expected audit insert to fail");
        let rows = AgentAllowlistEntry::list(&mut conn).expect("list allowlist rows");
        assert!(rows.is_empty(), "allowlist row must be rolled back");

        clear_database(&db_pool);
    }
}
