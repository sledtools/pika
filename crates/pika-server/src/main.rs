mod admin;
mod agent_api;
mod agent_api_v1_contract;
mod browser_auth;
mod customer;
mod listener;
mod managed_openclaw_guest;
mod managed_runtime_contract;
mod models;
mod nostr_auth;
mod routes;
#[cfg(test)]
mod test_support;

use crate::admin::{
    challenge as admin_challenge, dashboard as admin_dashboard, dev_login as admin_dev_login,
    login_page as admin_login_page, logout as admin_logout,
    restore_confirm_page as admin_restore_confirm_page,
    restore_from_backup as admin_restore_from_backup, toggle_allowlist as admin_toggle_allowlist,
    upsert_allowlist as admin_upsert_allowlist, verify as admin_verify,
};
use crate::agent_api::{ensure_agent, get_my_agent, recover_my_agent};
use crate::agent_api_v1_contract::{
    V1_AGENTS_ENSURE_PATH, V1_AGENTS_ME_PATH, V1_AGENTS_RECOVER_PATH,
};
use crate::customer::{
    challenge as customer_challenge, dashboard as customer_dashboard, home as customer_home,
    login_page as customer_login_page, logout as customer_logout, provision as customer_provision,
    recover as customer_recover, reset as customer_reset,
    reset_confirm_page as customer_reset_confirm_page, verify as customer_verify,
    OPENCLAW_INTERNAL_LAUNCH_PATH, OPENCLAW_INTERNAL_PROXY_PATH, OPENCLAW_INTERNAL_PROXY_PREFIX,
};
use crate::models::group_subscription::{GroupFilterInfo, GroupSubscription};
use crate::models::MIGRATIONS;
use crate::routes::{
    broadcast, health_check, min_version, register, subscribe_groups, unsubscribe_groups,
};
use a2::Client as ApnsClient;
use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Request, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{any, get, post};
use axum::{Extension, Router};
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::PgConnection;
use diesel_migrations::MigrationHarness;
use fcm_rs::client::FcmClient;
use rand::RngCore;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{watch, Mutex};
use tower::{make::Shared, ServiceBuilder};
use tracing::{error, info, warn};

pub const REQUEST_ID_HEADER: &str = "x-request-id";

#[derive(Clone)]
pub struct State {
    pub db_pool: Pool<ConnectionManager<PgConnection>>,
    pub apns_client: Option<Arc<ApnsClient>>,
    pub fcm_client: Option<Arc<FcmClient>>,
    pub apns_topic: String,
    pub channel: Arc<Mutex<watch::Sender<GroupFilterInfo>>>,
    pub admin_config: Arc<admin::AdminConfig>,
    pub min_app_version: String,
    pub trust_forwarded_host: bool,
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[derive(Clone, Debug)]
pub struct RequestContext {
    pub request_id: String,
}

fn request_id_from_headers<B>(request: &Request<B>) -> Option<String> {
    request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn generate_request_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

async fn trace_http_request(mut request: Request<Body>, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let request_id = request_id_from_headers(&request).unwrap_or_else(generate_request_id);
    let request_id_header = HeaderName::from_static(REQUEST_ID_HEADER);
    let request_id_value =
        HeaderValue::from_str(&request_id).expect("generated request id must be a valid header");
    request
        .headers_mut()
        .insert(request_id_header.clone(), request_id_value.clone());
    request.extensions_mut().insert(RequestContext {
        request_id: request_id.clone(),
    });

    let started_at = Instant::now();
    let mut response = next.run(request).await;
    let latency_ms = started_at.elapsed().as_millis() as u64;
    let status = response.status();
    response
        .headers_mut()
        .insert(request_id_header, request_id_value);
    info!(
        request_id = %request_id,
        method = %method,
        path = %path,
        status = status.as_u16(),
        latency_ms,
        "http request"
    );
    response
}

async fn route_openclaw_ui_host(mut request: Request<Body>, next: Next) -> Response {
    let Some(host) = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
    else {
        return next.run(request).await;
    };
    if !host.starts_with("openclaw.") {
        return next.run(request).await;
    }

    let request_path = request.uri().path();
    let rewritten_path = if request_path == "/launch" {
        OPENCLAW_INTERNAL_LAUNCH_PATH.to_string()
    } else if request_path == format!("{OPENCLAW_INTERNAL_PROXY_PREFIX}/") {
        OPENCLAW_INTERNAL_PROXY_PREFIX.to_string()
    } else if request_path == OPENCLAW_INTERNAL_LAUNCH_PATH
        || request_path == OPENCLAW_INTERNAL_PROXY_PREFIX
        || request_path.starts_with(&format!("{OPENCLAW_INTERNAL_PROXY_PREFIX}/"))
    {
        request_path.to_string()
    } else if request_path == "/" {
        OPENCLAW_INTERNAL_PROXY_PREFIX.to_string()
    } else {
        format!("{OPENCLAW_INTERNAL_PROXY_PREFIX}{request_path}")
    };
    let rewritten_uri = if let Some(query) = request.uri().query() {
        format!("{rewritten_path}?{query}")
    } else {
        rewritten_path
    };
    if let Ok(uri) = rewritten_uri.parse::<Uri>() {
        *request.uri_mut() = uri;
    }
    next.run(request).await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    agent_api_v1_contract::contract_healthcheck()?;
    agent_api::agent_api_healthcheck().await?;
    admin::admin_healthcheck()?;

    // APNs configuration (optional — logs only when not configured)
    let apns_topic = std::env::var("APNS_TOPIC").unwrap_or_default();
    let apns_sandbox = std::env::var("APNS_SANDBOX")
        .ok()
        .map(|s| s == "true" || s == "1")
        .unwrap_or(false);
    let apns_client = match (
        std::env::var("APNS_KEY_PATH")
            .ok()
            .filter(|s| !s.is_empty()),
        std::env::var("APNS_KEY_BASE64")
            .ok()
            .filter(|s| !s.is_empty()),
        std::env::var("APNS_KEY_ID"),
        std::env::var("APNS_TEAM_ID"),
    ) {
        (path, base64_key, Ok(key_id), Ok(team_id)) if path.is_some() || base64_key.is_some() => {
            let endpoint = if apns_sandbox {
                a2::Endpoint::Sandbox
            } else {
                a2::Endpoint::Production
            };
            let client = if let Some(b64) = base64_key {
                use base64::Engine;
                let key_bytes = base64::engine::general_purpose::STANDARD.decode(&b64)?;
                let mut cursor = std::io::Cursor::new(key_bytes);
                ApnsClient::token(
                    &mut cursor,
                    key_id,
                    team_id,
                    a2::ClientConfig::new(endpoint),
                )?
            } else {
                let mut apns_key_file = std::fs::File::open(path.unwrap())?;
                ApnsClient::token(
                    &mut apns_key_file,
                    key_id,
                    team_id,
                    a2::ClientConfig::new(endpoint),
                )?
            };
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
        Ok(path) if !path.is_empty() => match FcmClient::new(&path).await {
            Ok(client) => {
                info!("FCM client configured");
                Some(Arc::new(client))
            }
            Err(err) => {
                warn!(error = %err, "FCM configuration invalid; continuing without Android push");
                None
            }
        },
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

    let min_app_version = std::env::var("MIN_APP_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    info!(min_app_version = %min_app_version, "Minimum app version configured");

    let state = State {
        db_pool: db_pool.clone(),
        apns_client: apns_client.clone(),
        fcm_client: fcm_client.clone(),
        apns_topic: apns_topic.clone(),
        channel,
        admin_config: Arc::new(admin::AdminConfig::from_env()?),
        min_app_version,
        trust_forwarded_host: env_truthy("PIKA_TRUST_X_FORWARDED_HOST"),
    };

    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}")
        .parse()
        .expect("Failed to parse bind/port for webserver");

    let server_router = Router::new()
        .route("/", get(customer_home))
        .route("/login", get(customer_login_page))
        .route("/login/challenge", post(customer_challenge))
        .route("/login/verify", post(customer_verify))
        .route("/dashboard", get(customer_dashboard))
        .route("/dashboard/provision", post(customer_provision))
        .route("/dashboard/recover", post(customer_recover))
        .route("/dashboard/reset/confirm", get(customer_reset_confirm_page))
        .route("/dashboard/reset", post(customer_reset))
        .route(
            "/dashboard/openclaw/launch",
            post(customer::openclaw_launch),
        )
        .route("/logout", post(customer_logout))
        .route(
            OPENCLAW_INTERNAL_LAUNCH_PATH,
            get(customer::openclaw_launch_exchange),
        )
        .route(
            OPENCLAW_INTERNAL_PROXY_PREFIX,
            any(customer::openclaw_proxy),
        )
        .route(OPENCLAW_INTERNAL_PROXY_PATH, any(customer::openclaw_proxy))
        .route("/health-check", get(health_check))
        .route("/min-version", get(min_version))
        .route("/register", post(register))
        .route("/subscribe-groups", post(subscribe_groups))
        .route("/unsubscribe-groups", post(unsubscribe_groups))
        .route("/broadcast", post(broadcast))
        .route(V1_AGENTS_ENSURE_PATH, post(ensure_agent))
        .route(V1_AGENTS_ME_PATH, get(get_my_agent))
        .route(V1_AGENTS_RECOVER_PATH, post(recover_my_agent))
        .route("/admin/login", get(admin_login_page))
        .route("/admin", get(admin_dashboard))
        .route("/admin/challenge", post(admin_challenge))
        .route("/admin/verify", post(admin_verify))
        .route("/admin/allowlist", post(admin_upsert_allowlist))
        .route(
            "/admin/allowlist/:npub/toggle",
            post(admin_toggle_allowlist),
        )
        .route(
            "/admin/environments/:npub/restore/confirm",
            get(admin_restore_confirm_page),
        )
        .route(
            "/admin/environments/:npub/restore",
            post(admin_restore_from_backup),
        )
        .route("/admin/logout", post(admin_logout))
        .route("/admin/dev-login", post(admin_dev_login))
        .fallback(fallback)
        .layer(Extension(state))
        .layer(middleware::from_fn(trace_http_request));

    let server_service = ServiceBuilder::new()
        // Host-based OpenClaw routing must happen before Axum selects a route.
        .layer(middleware::from_fn(route_openclaw_ui_host))
        .service(server_router);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let server = axum::serve(listener, Shared::new(server_service));

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

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request};
    use tower::ServiceExt;

    async fn response_body_string(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        String::from_utf8(bytes.to_vec()).expect("utf8 response body")
    }

    fn rewrite_test_app(
    ) -> impl tower::Service<Request<Body>, Response = Response, Error = std::convert::Infallible> + Clone
    {
        ServiceBuilder::new()
            .layer(middleware::from_fn(route_openclaw_ui_host))
            .service(
                Router::new()
                    .route(
                        OPENCLAW_INTERNAL_LAUNCH_PATH,
                        get(|uri: Uri| async move { uri.to_string() }),
                    )
                    .route(
                        OPENCLAW_INTERNAL_PROXY_PREFIX,
                        any(|uri: Uri| async move { uri.to_string() }),
                    )
                    .route(
                        OPENCLAW_INTERNAL_PROXY_PATH,
                        any(|uri: Uri| async move { uri.to_string() }),
                    )
                    .fallback(fallback),
            )
    }

    #[tokio::test]
    async fn openclaw_localhost_launch_request_reaches_internal_launch_route() {
        let response = rewrite_test_app()
            .oneshot(
                Request::builder()
                    .uri("/launch?ticket=test-ticket")
                    .header(header::HOST, "openclaw.localhost:19401")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("launch response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_body_string(response).await,
            "/_openclaw_launch?ticket=test-ticket"
        );
    }

    #[tokio::test]
    async fn openclaw_localhost_root_request_reaches_internal_proxy_route() {
        let response = rewrite_test_app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "openclaw.localhost:19401")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("root proxy response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_body_string(response).await, "/_openclaw_proxy");
    }

    #[tokio::test]
    async fn openclaw_localhost_subpath_request_reaches_internal_proxy_route() {
        let response = rewrite_test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/me?view=full")
                    .header(header::HOST, "openclaw.localhost:19401")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("subpath proxy response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_body_string(response).await,
            "/_openclaw_proxy/api/me?view=full"
        );
    }

    #[tokio::test]
    async fn openclaw_localhost_internal_proxy_path_is_not_double_rewritten() {
        let response = rewrite_test_app()
            .oneshot(
                Request::builder()
                    .uri("/_openclaw_proxy/assets/app.js")
                    .header(header::HOST, "openclaw.localhost:19401")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("internal proxy response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_body_string(response).await,
            "/_openclaw_proxy/assets/app.js"
        );
    }

    #[tokio::test]
    async fn openclaw_localhost_internal_proxy_slash_path_normalizes_to_exact_proxy_route() {
        let response = rewrite_test_app()
            .oneshot(
                Request::builder()
                    .uri("/_openclaw_proxy/")
                    .header(header::HOST, "openclaw.localhost:19401")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("internal proxy slash response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_body_string(response).await, "/_openclaw_proxy");
    }

    #[tokio::test]
    async fn dashboard_host_launch_path_does_not_rewrite_to_openclaw_routes() {
        let response = rewrite_test_app()
            .oneshot(
                Request::builder()
                    .uri("/launch?ticket=test-ticket")
                    .header(header::HOST, "localhost:19401")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("dashboard host response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response_body_string(response).await,
            "No route for /launch?ticket=test-ticket"
        );
    }
}
