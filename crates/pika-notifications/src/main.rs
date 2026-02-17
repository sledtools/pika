mod listener;
mod models;
mod routes;

use crate::models::group_subscription::{GroupFilterInfo, GroupSubscription};
use crate::models::MIGRATIONS;
use crate::routes::{broadcast, health_check, register, subscribe_groups, unsubscribe_groups};
use a2::Client as ApnsClient;
use axum::http::{StatusCode, Uri};
use axum::routing::{get, post};
use axum::{Extension, Router};
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::PgConnection;
use diesel_migrations::MigrationHarness;
use fcm_rs::client::FcmClient;
use std::sync::Arc;
use tokio::sync::{watch, Mutex};
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct State {
    pub db_pool: Pool<ConnectionManager<PgConnection>>,
    pub apns_client: Option<Arc<ApnsClient>>,
    pub fcm_client: Option<Arc<FcmClient>>,
    pub apns_topic: String,
    pub channel: Arc<Mutex<watch::Sender<GroupFilterInfo>>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // APNs configuration (optional — logs only when not configured)
    let apns_topic = std::env::var("APNS_TOPIC").unwrap_or_default();
    let apns_sandbox = std::env::var("APNS_SANDBOX")
        .ok()
        .map(|s| s == "true" || s == "1")
        .unwrap_or(false);
    let apns_client = match (
        std::env::var("APNS_KEY_PATH"),
        std::env::var("APNS_KEY_ID"),
        std::env::var("APNS_TEAM_ID"),
    ) {
        (Ok(path), Ok(key_id), Ok(team_id)) if !path.is_empty() => {
            let endpoint = if apns_sandbox {
                a2::Endpoint::Sandbox
            } else {
                a2::Endpoint::Production
            };
            let mut apns_key_file = std::fs::File::open(&path)?;
            let client = ApnsClient::token(
                &mut apns_key_file,
                key_id,
                team_id,
                a2::ClientConfig::new(endpoint),
            )?;
            info!(sandbox = apns_sandbox, "APNs client configured");
            Some(Arc::new(client))
        }
        _ => {
            warn!("APNs not configured — will log instead of sending");
            None
        }
    };

    // FCM configuration (optional — logs only when not configured)
    let fcm_client = match std::env::var("FCM_CREDENTIALS_PATH") {
        Ok(path) if !path.is_empty() => {
            let client = FcmClient::new(&path).await?;
            info!("FCM client configured");
            Some(Arc::new(client))
        }
        _ => {
            warn!("FCM not configured — will log instead of sending");
            None
        }
    };

    let pg_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let port: u16 = std::env::var("NOTIFICATION_PORT")
        .ok()
        .map(|p| p.parse::<u16>())
        .transpose()?
        .unwrap_or(8080);

    let relays: Vec<String> = std::env::var("RELAYS")
        .expect("RELAYS must be set")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    info!("Relays: {:?}", relays);

    // DB management
    let manager = ConnectionManager::<PgConnection>::new(&pg_url);
    let db_pool = Pool::builder()
        .max_size(10)
        .test_on_check_out(true)
        .build(manager)
        .expect("Could not build connection pool");

    let mut connection = db_pool.get()?;
    connection
        .run_pending_migrations(MIGRATIONS)
        .expect("migrations could not run");
    info!("Database migrations applied");

    let filter_info = GroupSubscription::get_filter_info(&mut connection)?;
    info!(
        "Loaded {} existing group filter(s)",
        filter_info.group_ids.len()
    );
    let (sender, receiver) = watch::channel(filter_info);
    let channel = Arc::new(Mutex::new(sender));

    drop(connection);

    let state = State {
        db_pool: db_pool.clone(),
        apns_client: apns_client.clone(),
        fcm_client: fcm_client.clone(),
        apns_topic: apns_topic.clone(),
        channel,
    };

    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}")
        .parse()
        .expect("Failed to parse bind/port for webserver");

    let server_router = Router::new()
        .route("/health-check", get(health_check))
        .route("/register", post(register))
        .route("/subscribe-groups", post(subscribe_groups))
        .route("/unsubscribe-groups", post(unsubscribe_groups))
        .route("/broadcast", post(broadcast))
        .fallback(fallback)
        .layer(Extension(state));

    let server = axum::Server::bind(&addr).serve(server_router.into_make_service());

    info!("Webserver running on http://{addr}");

    // start the listener
    tokio::spawn(async move {
        loop {
            if let Err(e) = listener::start_listener(
                db_pool.clone(),
                receiver.clone(),
                apns_client.clone(),
                fcm_client.clone(),
                apns_topic.clone(),
                relays.clone(),
            )
            .await
            {
                error!("Listener error: {e}");
            }
        }
    });

    let graceful = server.with_graceful_shutdown(async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to create Ctrl+C shutdown signal");
    });

    if let Err(e) = graceful.await {
        error!("Shutdown error: {e}");
    }

    Ok(())
}

async fn fallback(uri: Uri) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, format!("No route for {uri}"))
}
