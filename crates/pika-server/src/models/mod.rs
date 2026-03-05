use diesel_migrations::{embed_migrations, EmbeddedMigrations};

pub mod agent_instance;
pub mod group_subscription;
mod schema;
pub mod subscription_info;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[cfg(test)]
mod test {
    use super::*;
    use crate::models::agent_instance::{
        AgentInstance, AGENT_PHASE_CREATING, AGENT_PHASE_ERROR, AGENT_PHASE_READY,
    };
    use crate::models::group_subscription::GroupSubscription;
    use crate::models::subscription_info::SubscriptionInfo;
    use diesel::prelude::*;
    use diesel::r2d2::{ConnectionManager, Pool};
    use diesel_migrations::MigrationHarness;
    use std::sync::{Mutex, OnceLock};

    const DEVICE_TOKEN: &str = "abc123devicetoken";
    const PLATFORM: &str = "ios";

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        match GUARD.get_or_init(|| Mutex::new(())).lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn init_db_pool() -> Pool<ConnectionManager<PgConnection>> {
        dotenv::dotenv().ok();
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let manager = ConnectionManager::<PgConnection>::new(url);
        let db_pool = Pool::builder()
            .build(manager)
            .expect("Could not build connection pool");

        // run migrations
        let mut connection = db_pool.get().unwrap();
        connection
            .run_pending_migrations(MIGRATIONS)
            .expect("migrations could not run");

        db_pool
    }

    fn clear_database(db_pool: &Pool<ConnectionManager<PgConnection>>) {
        let conn = &mut db_pool.get().unwrap();

        conn.transaction::<_, anyhow::Error, _>(|conn| {
            diesel::delete(schema::agent_instances::table).execute(conn)?;
            diesel::delete(schema::group_subscriptions::table).execute(conn)?;
            diesel::delete(schema::subscription_info::table).execute(conn)?;
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn test_register() {
        let _guard = test_guard();
        let db_pool = init_db_pool();
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
        let _guard = test_guard();
        let db_pool = init_db_pool();
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
        let _guard = test_guard();
        let db_pool = init_db_pool();
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
        let _guard = test_guard();
        let db_pool = init_db_pool();
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

        clear_database(&db_pool);
    }
}
