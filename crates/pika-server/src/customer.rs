use askama::Template;
use axum::extract::Form;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Json};

use crate::agent_api::{
    list_recent_managed_environment_events, load_managed_environment_status,
    provision_managed_environment_if_missing, recover_agent_for_owner, reset_agent_for_owner,
    AgentApiError, ManagedEnvironmentStatus,
};
use crate::browser_auth::BrowserAuthConfig;
use crate::models::agent_allowlist::AgentAllowlistEntry;
use crate::models::managed_environment_event::ManagedEnvironmentEvent;
use crate::nostr_auth::{expected_host_from_headers, verify_nip98_event};
use crate::{RequestContext, State};

const CUSTOMER_SESSION_COOKIE: &str = "pika_customer_session";
const CUSTOMER_SESSION_TTL_SECS: i64 = 8 * 60 * 60;
const CUSTOMER_CHALLENGE_KIND: &str = "customer_dashboard_challenge";
const CUSTOMER_SESSION_KIND: &str = "customer_dashboard_session";
const RECENT_ACTIVITY_LIMIT: i64 = 20;

#[derive(Debug, serde::Deserialize)]
pub struct VerifyRequest {
    event: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
pub struct ActionForm {
    csrf_token: String,
}

#[derive(Debug, Clone)]
struct AuthenticatedCustomer {
    npub: String,
    csrf_token: String,
}

#[derive(Debug, Clone)]
struct DashboardActivityItem {
    created_at: String,
    message: String,
}

#[derive(Template)]
#[template(path = "customer/login.html")]
struct LoginTemplate;

#[derive(Template)]
#[template(path = "customer/dashboard.html")]
struct DashboardTemplate {
    owner_npub: String,
    template_name: &'static str,
    environment_exists_label: &'static str,
    app_state_label: &'static str,
    startup_phase_label: &'static str,
    status_copy: String,
    state_tone: &'static str,
    csrf_token: String,
    agent_id: String,
    vm_id: String,
    created_at: String,
    updated_at: String,
    can_provision: bool,
    can_recover: bool,
    can_reset: bool,
    control_loop_notice: &'static str,
    has_control_loop_notice: bool,
    recover_action_label: &'static str,
    recover_semantics_copy: &'static str,
    recent_activity: Vec<DashboardActivityItem>,
    has_recent_activity: bool,
}

fn browser_auth(state: &State) -> &BrowserAuthConfig {
    &state.admin_config.browser_auth
}

fn render_template(template: &impl Template) -> Result<Response, (StatusCode, String)> {
    let html = template
        .render()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(axum::response::Html(html).into_response())
}

fn redirect_to_login(state: &State, clear_session: bool) -> Result<Response, (StatusCode, String)> {
    let mut response = Redirect::to("/login").into_response();
    if clear_session {
        browser_auth(state)
            .clear_session_cookie(&mut response, CUSTOMER_SESSION_COOKIE)
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    }
    Ok(response)
}

async fn allowlisted_customer_from_session(
    state: &State,
    headers: &HeaderMap,
) -> Result<Option<AuthenticatedCustomer>, (StatusCode, String)> {
    let Some(session) = browser_auth(state).session_from_headers(
        headers,
        CUSTOMER_SESSION_COOKIE,
        CUSTOMER_SESSION_KIND,
    ) else {
        return Ok(None);
    };

    let mut conn = state
        .db_pool
        .get()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let is_active = AgentAllowlistEntry::is_active(&mut conn, &session.npub)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(is_active.then_some(AuthenticatedCustomer {
        npub: session.npub,
        csrf_token: session.csrf_token,
    }))
}

fn map_agent_api_error(err: AgentApiError) -> (StatusCode, String) {
    let request_id_suffix = err
        .request_id()
        .map(|request_id| format!(" (request_id={request_id})"))
        .unwrap_or_default();
    (
        err.status_code(),
        format!("{}{}", err.error_code(), request_id_suffix),
    )
}

fn app_state_label(state: Option<crate::agent_api_v1_contract::AgentAppState>) -> &'static str {
    match state {
        None => "not_provisioned",
        Some(crate::agent_api_v1_contract::AgentAppState::Creating) => "creating",
        Some(crate::agent_api_v1_contract::AgentAppState::Ready) => "ready",
        Some(crate::agent_api_v1_contract::AgentAppState::Error) => "error",
    }
}

fn startup_phase_label(phase: Option<pika_agent_control_plane::AgentStartupPhase>) -> &'static str {
    match phase {
        None => "not_started",
        Some(pika_agent_control_plane::AgentStartupPhase::Requested) => "requested",
        Some(pika_agent_control_plane::AgentStartupPhase::ProvisioningVm) => "provisioning_vm",
        Some(pika_agent_control_plane::AgentStartupPhase::BootingGuest) => "booting_guest",
        Some(pika_agent_control_plane::AgentStartupPhase::WaitingForServiceReady) => {
            "waiting_for_service_ready"
        }
        Some(pika_agent_control_plane::AgentStartupPhase::Ready) => "ready",
        Some(pika_agent_control_plane::AgentStartupPhase::Failed) => "failed",
    }
}

fn state_tone(state: Option<crate::agent_api_v1_contract::AgentAppState>) -> &'static str {
    match state {
        Some(crate::agent_api_v1_contract::AgentAppState::Ready) => "ok",
        Some(crate::agent_api_v1_contract::AgentAppState::Creating) => "warm",
        Some(crate::agent_api_v1_contract::AgentAppState::Error) => "error",
        None => "idle",
    }
}

fn format_timestamp(value: Option<chrono::NaiveDateTime>) -> String {
    value
        .map(|value| format!("{}", value.format("%Y-%m-%d %H:%M:%S UTC")))
        .unwrap_or_else(|| "not_available".to_string())
}

fn verify_action_csrf(
    authenticated: &AuthenticatedCustomer,
    form: &ActionForm,
) -> Result<(), (StatusCode, String)> {
    if authenticated.csrf_token == form.csrf_token {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "invalid csrf token".to_string()))
    }
}

fn dashboard_template(
    authenticated: AuthenticatedCustomer,
    status: ManagedEnvironmentStatus,
    recent_activity: Vec<DashboardActivityItem>,
) -> DashboardTemplate {
    let row = status.row;
    let inflight_without_vm = row
        .as_ref()
        .map(|row| {
            row.phase == crate::models::agent_instance::AGENT_PHASE_CREATING && row.vm_id.is_none()
        })
        .unwrap_or(false);
    let recoverable_vm_exists = row.as_ref().and_then(|row| row.vm_id.as_deref()).is_some();
    DashboardTemplate {
        owner_npub: authenticated.npub,
        template_name: "OpenClaw",
        environment_exists_label: if status.environment_exists {
            "yes"
        } else {
            "no"
        },
        app_state_label: app_state_label(status.app_state),
        startup_phase_label: startup_phase_label(status.startup_phase),
        status_copy: status.status_copy,
        state_tone: state_tone(status.app_state),
        csrf_token: authenticated.csrf_token,
        agent_id: row
            .as_ref()
            .map(|row| row.agent_id.clone())
            .unwrap_or_else(|| "not_provisioned".to_string()),
        vm_id: row
            .as_ref()
            .and_then(|row| row.vm_id.clone())
            .unwrap_or_else(|| "not_assigned".to_string()),
        created_at: format_timestamp(row.as_ref().map(|row| row.created_at)),
        updated_at: format_timestamp(row.as_ref().map(|row| row.updated_at)),
        can_provision: row.is_none(),
        can_recover: row.is_some() && !inflight_without_vm,
        can_reset: row.is_some() && !inflight_without_vm,
        control_loop_notice: if inflight_without_vm {
            "Provisioning is already in flight. Recovery and reset stay locked until the current VM assignment finishes."
        } else {
            ""
        },
        has_control_loop_notice: inflight_without_vm,
        recover_action_label: if recoverable_vm_exists {
            "Recover Managed Environment"
        } else {
            "Provision Fresh Managed Environment"
        },
        recover_semantics_copy: if inflight_without_vm {
            "stays locked while the initial VM assignment is still in flight. Wait for the current create request to finish before retrying any destructive action."
        } else if recoverable_vm_exists {
            "asks the control plane to bring the managed environment back. If that VM is still recoverable, this path preserves the durable home. If the VM is already gone, Recover falls back to provisioning a fresh environment."
        } else {
            "will provision a fresh Managed OpenClaw environment instead of restoring prior durable state because no recoverable VM is available."
        },
        has_recent_activity: !recent_activity.is_empty(),
        recent_activity,
    }
}

fn recent_activity_items(events: Vec<ManagedEnvironmentEvent>) -> Vec<DashboardActivityItem> {
    events
        .into_iter()
        .map(|event| DashboardActivityItem {
            created_at: format_timestamp(Some(event.created_at)),
            message: event.message,
        })
        .collect()
}

pub async fn home() -> Redirect {
    Redirect::to("/dashboard")
}

pub async fn login_page(
    Extension(state): Extension<State>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    if allowlisted_customer_from_session(&state, &headers)
        .await?
        .is_some()
    {
        return Ok(Redirect::to("/dashboard").into_response());
    }
    render_template(&LoginTemplate)
}

pub async fn challenge(
    Extension(state): Extension<State>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let challenge = browser_auth(&state)
        .issue_challenge(CUSTOMER_CHALLENGE_KIND)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(serde_json::json!({ "challenge": challenge })))
}

pub async fn verify(
    Extension(state): Extension<State>,
    headers: HeaderMap,
    Json(payload): Json<VerifyRequest>,
) -> Result<Response, (StatusCode, String)> {
    let event: nostr_sdk::prelude::Event =
        serde_json::from_value(payload.event).map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid event JSON: {err}"),
            )
        })?;

    let expected_host = expected_host_from_headers(&headers, state.trust_forwarded_host)
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                "missing host for NIP-98 verification".to_string(),
            )
        })?;
    let npub = verify_nip98_event(
        &event,
        "POST",
        "/login/verify",
        Some(expected_host.as_str()),
        None,
    )
    .map_err(|err| (StatusCode::UNAUTHORIZED, err.to_string()))?;

    browser_auth(&state)
        .verify_challenge(event.content.as_str(), CUSTOMER_CHALLENGE_KIND)
        .map_err(|err| (StatusCode::UNAUTHORIZED, err.to_string()))?;

    let mut conn = state
        .db_pool
        .get()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    if !AgentAllowlistEntry::is_active(&mut conn, &npub)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "npub is not allowlisted".to_string()));
    }

    let token = browser_auth(&state)
        .issue_session_token(CUSTOMER_SESSION_KIND, &npub, CUSTOMER_SESSION_TTL_SECS)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    let mut response = Json(serde_json::json!({ "ok": true, "npub": npub })).into_response();
    browser_auth(&state)
        .set_session_cookie(
            &mut response,
            CUSTOMER_SESSION_COOKIE,
            &token,
            CUSTOMER_SESSION_TTL_SECS,
        )
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(response)
}

pub async fn dashboard(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let Some(authenticated) = allowlisted_customer_from_session(&state, &headers).await? else {
        return redirect_to_login(&state, true);
    };

    let status =
        load_managed_environment_status(&state, &authenticated.npub, &request_context.request_id)
            .await
            .map_err(map_agent_api_error)?;
    let activity = list_recent_managed_environment_events(
        &state,
        &authenticated.npub,
        RECENT_ACTIVITY_LIMIT,
        &request_context.request_id,
    )
    .map_err(map_agent_api_error)?;
    render_template(&dashboard_template(
        authenticated,
        status,
        recent_activity_items(activity),
    ))
}

pub async fn provision(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    headers: HeaderMap,
    Form(form): Form<ActionForm>,
) -> Result<Response, (StatusCode, String)> {
    let Some(authenticated) = allowlisted_customer_from_session(&state, &headers).await? else {
        return redirect_to_login(&state, true);
    };
    verify_action_csrf(&authenticated, &form)?;

    provision_managed_environment_if_missing(
        &state,
        &authenticated.npub,
        &request_context.request_id,
        None,
    )
    .await
    .map_err(map_agent_api_error)?;
    Ok(Redirect::to("/dashboard").into_response())
}

pub async fn recover(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    headers: HeaderMap,
    Form(form): Form<ActionForm>,
) -> Result<Response, (StatusCode, String)> {
    let Some(authenticated) = allowlisted_customer_from_session(&state, &headers).await? else {
        return redirect_to_login(&state, true);
    };
    verify_action_csrf(&authenticated, &form)?;

    recover_agent_for_owner(
        &state,
        &authenticated.npub,
        &request_context.request_id,
        None,
    )
    .await
    .map_err(map_agent_api_error)?;
    Ok(Redirect::to("/dashboard").into_response())
}

pub async fn reset(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    headers: HeaderMap,
    Form(form): Form<ActionForm>,
) -> Result<Response, (StatusCode, String)> {
    let Some(authenticated) = allowlisted_customer_from_session(&state, &headers).await? else {
        return redirect_to_login(&state, true);
    };
    verify_action_csrf(&authenticated, &form)?;

    reset_agent_for_owner(
        &state,
        &authenticated.npub,
        &request_context.request_id,
        None,
    )
    .await
    .map_err(map_agent_api_error)?;
    Ok(Redirect::to("/dashboard").into_response())
}

pub async fn logout(
    Extension(state): Extension<State>,
    headers: HeaderMap,
    Form(form): Form<ActionForm>,
) -> Result<Response, (StatusCode, String)> {
    let Some(authenticated) = allowlisted_customer_from_session(&state, &headers).await? else {
        return redirect_to_login(&state, true);
    };
    verify_action_csrf(&authenticated, &form)?;

    let mut response = Redirect::to("/login").into_response();
    browser_auth(&state)
        .clear_session_cookie(&mut response, CUSTOMER_SESSION_COOKIE)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(response)
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use axum::body::HttpBody;
    use axum::http::header;
    use diesel::prelude::*;
    use diesel::r2d2::{ConnectionManager, Pool};
    use diesel::PgConnection;
    use diesel_migrations::MigrationHarness;
    use nostr_sdk::prelude::{EventBuilder, Keys, Kind, Tag, TagKind};
    use nostr_sdk::ToBech32;
    use pika_test_utils::{spawn_one_shot_server, CapturedRequest};

    use crate::admin::AdminConfig;
    use crate::models::agent_instance::{AgentInstance, AGENT_PHASE_ERROR, AGENT_PHASE_READY};
    use crate::models::group_subscription::GroupFilterInfo;
    use crate::models::managed_environment_event::ManagedEnvironmentEvent;
    use crate::models::MIGRATIONS;
    use crate::test_support::serial_test_guard;

    fn init_test_db_pool() -> Option<Pool<ConnectionManager<PgConnection>>> {
        dotenv::dotenv().ok();
        let Some(url) = std::env::var("DATABASE_URL").ok() else {
            eprintln!("SKIP: DATABASE_URL must be set for customer tests");
            return None;
        };
        if let Err(err) = PgConnection::establish(&url) {
            eprintln!("SKIP: postgres unavailable for customer tests: {err}");
            return None;
        }
        let manager = ConnectionManager::<PgConnection>::new(url);
        let db_pool = Pool::builder()
            .max_size(4)
            .build(manager)
            .expect("build test db pool");
        let mut connection = db_pool.get().expect("get migration connection");
        connection
            .run_pending_migrations(MIGRATIONS)
            .expect("run migrations");
        Some(db_pool)
    }

    fn clear_test_database(db_pool: &Pool<ConnectionManager<PgConnection>>) {
        let conn = &mut db_pool.get().expect("get clear db connection");
        diesel::sql_query(
            "TRUNCATE TABLE managed_environment_events, agent_instances, agent_allowlist_audit, agent_allowlist, group_subscriptions, subscription_info RESTART IDENTITY CASCADE",
        )
        .execute(conn)
        .expect("truncate test tables");
    }

    fn test_state(db_pool: Pool<ConnectionManager<PgConnection>>) -> State {
        let (sender, _receiver) = tokio::sync::watch::channel(GroupFilterInfo::default());
        State {
            db_pool,
            apns_client: None,
            fcm_client: None,
            apns_topic: String::new(),
            channel: std::sync::Arc::new(tokio::sync::Mutex::new(sender)),
            admin_config: std::sync::Arc::new(AdminConfig {
                bootstrap_admins: HashSet::new(),
                browser_auth: BrowserAuthConfig::new(
                    b"0123456789abcdef0123456789abcdef".to_vec(),
                    true,
                    false,
                    None,
                )
                .expect("browser auth config"),
            }),
            min_app_version: "0.0.0".to_string(),
            trust_forwarded_host: false,
        }
    }

    fn customer_cookie_header(state: &State, npub: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let token = state
            .admin_config
            .browser_auth
            .issue_session_token(CUSTOMER_SESSION_KIND, npub, CUSTOMER_SESSION_TTL_SECS)
            .expect("issue customer session token");
        headers.insert(
            header::COOKIE,
            format!("{CUSTOMER_SESSION_COOKIE}={token}")
                .parse()
                .expect("cookie header"),
        );
        headers
    }

    fn customer_action_form(state: &State, headers: &HeaderMap) -> ActionForm {
        let session = state
            .admin_config
            .browser_auth
            .session_from_headers(headers, CUSTOMER_SESSION_COOKIE, CUSTOMER_SESSION_KIND)
            .expect("session info from cookie");
        ActionForm {
            csrf_token: session.csrf_token,
        }
    }

    fn verify_headers(host: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, host.parse().expect("host header"));
        headers
    }

    fn signed_verify_payload(challenge: &str, host: &str) -> (VerifyRequest, String) {
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::Custom(27235), challenge)
            .tags([
                Tag::custom(
                    TagKind::custom("u"),
                    [format!("https://{host}/login/verify")],
                ),
                Tag::custom(TagKind::custom("method"), ["POST"]),
            ])
            .sign_with_keys(&keys)
            .expect("sign verify event");
        (
            VerifyRequest {
                event: serde_json::to_value(event).expect("serialize signed event"),
            },
            keys.public_key()
                .to_bech32()
                .expect("encode npub")
                .to_lowercase(),
        )
    }

    async fn response_body_string(response: Response) -> String {
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(chunk) = body.data().await {
            bytes.extend_from_slice(&chunk.expect("read response chunk"));
        }
        String::from_utf8(bytes).expect("utf8 response body")
    }

    fn upsert_allowlist(db_pool: &Pool<ConnectionManager<PgConnection>>, npub: &str, active: bool) {
        let mut conn = db_pool.get().expect("get allowlist connection");
        AgentAllowlistEntry::upsert(&mut conn, npub, active, Some("test"), npub, Some(1))
            .expect("upsert allowlist");
    }

    fn generate_npub() -> String {
        Keys::generate()
            .public_key()
            .to_bech32()
            .expect("encode generated npub")
    }

    fn recent_activity(
        db_pool: &Pool<ConnectionManager<PgConnection>>,
        npub: &str,
    ) -> Vec<ManagedEnvironmentEvent> {
        let mut conn = db_pool.get().expect("get activity connection");
        ManagedEnvironmentEvent::list_recent_by_owner(&mut conn, npub, 20)
            .expect("query recent activity")
    }
    fn request_context() -> Extension<RequestContext> {
        Extension(RequestContext {
            request_id: "req-customer-test".to_string(),
        })
    }

    struct MicrovmEnvGuard {
        prior_spawner: Option<String>,
        prior_kind: Option<String>,
    }

    impl MicrovmEnvGuard {
        fn set(spawner_url: &str) -> Self {
            let prior_spawner = std::env::var("PIKA_AGENT_MICROVM_SPAWNER_URL").ok();
            let prior_kind = std::env::var("PIKA_AGENT_MICROVM_KIND").ok();
            unsafe {
                std::env::set_var("PIKA_AGENT_MICROVM_SPAWNER_URL", spawner_url);
                std::env::set_var("PIKA_AGENT_MICROVM_KIND", "openclaw");
            }
            Self {
                prior_spawner,
                prior_kind,
            }
        }
    }

    impl Drop for MicrovmEnvGuard {
        fn drop(&mut self) {
            match self.prior_spawner.as_deref() {
                Some(prior) => unsafe {
                    std::env::set_var("PIKA_AGENT_MICROVM_SPAWNER_URL", prior)
                },
                None => unsafe { std::env::remove_var("PIKA_AGENT_MICROVM_SPAWNER_URL") },
            }
            match self.prior_kind.as_deref() {
                Some(prior) => unsafe { std::env::set_var("PIKA_AGENT_MICROVM_KIND", prior) },
                None => unsafe { std::env::remove_var("PIKA_AGENT_MICROVM_KIND") },
            }
        }
    }

    fn spawn_scripted_server(
        responses: Vec<(&'static str, &'static str)>,
    ) -> (String, mpsc::Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().expect("read mock server addr");
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            for (status_line, response_body) in responses {
                let (mut stream, _) = listener.accept().expect("accept mock request");
                let req = read_http_request(&mut stream);
                tx.send(req).expect("send captured request");

                let response = format!(
                    "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write mock response");
            }
        });

        (format!("http://{addr}"), rx)
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
        let mut buf = Vec::new();
        let mut header_end = None;
        let mut content_length = 0usize;

        loop {
            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).expect("read request bytes");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if header_end.is_none() {
                header_end = buf
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|idx| idx + 4);
                if let Some(end) = header_end {
                    let headers = String::from_utf8_lossy(&buf[..end]);
                    for line in headers.lines() {
                        if let Some((key, value)) = line.split_once(':') {
                            if key.eq_ignore_ascii_case("content-length") {
                                content_length = value.trim().parse::<usize>().unwrap_or(0);
                            }
                        }
                    }
                }
            }
            if let Some(end) = header_end {
                if buf.len() >= end + content_length {
                    break;
                }
            }
        }

        let end = header_end.expect("request headers must be present");
        let headers_raw = String::from_utf8_lossy(&buf[..end]);
        let mut lines = headers_raw.lines();
        let request_line = lines.next().expect("request line");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().expect("method").to_string();
        let path = parts.next().expect("path").to_string();
        let mut headers = std::collections::HashMap::new();
        for line in lines {
            if line.trim().is_empty() {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }
        let body = String::from_utf8(buf[end..end + content_length].to_vec()).expect("utf8 body");

        CapturedRequest {
            method,
            path,
            headers,
            body,
        }
    }

    #[tokio::test]
    async fn verify_sets_session_cookie_for_allowlisted_npub() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());

        let challenge = state
            .admin_config
            .browser_auth
            .issue_challenge(CUSTOMER_CHALLENGE_KIND)
            .expect("issue challenge");
        let (payload, npub) = signed_verify_payload(&challenge, "example.com");
        upsert_allowlist(&db_pool, &npub, true);

        let response = verify(
            Extension(state),
            verify_headers("example.com"),
            Json(payload),
        )
        .await
        .expect("verify succeeds");
        let set_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("set-cookie header");
        assert!(set_cookie.contains(CUSTOMER_SESSION_COOKIE));
        assert!(set_cookie.contains("HttpOnly"));
        let body = response_body_string(response).await;
        assert!(body.contains(&npub));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn verify_rejects_non_allowlisted_npub() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());

        let challenge = state
            .admin_config
            .browser_auth
            .issue_challenge(CUSTOMER_CHALLENGE_KIND)
            .expect("issue challenge");
        let (payload, _npub) = signed_verify_payload(&challenge, "example.com");

        let err = verify(
            Extension(state),
            verify_headers("example.com"),
            Json(payload),
        )
        .await
        .expect_err("verify should reject non-allowlisted user");
        assert_eq!(err.0, StatusCode::FORBIDDEN);

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn dashboard_redirects_without_allowlisted_session() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());

        let response = dashboard(Extension(state), request_context(), HeaderMap::new())
            .await
            .expect("dashboard response");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("/login")
        );

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn dashboard_renders_missing_environment_state() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y";
        upsert_allowlist(&db_pool, npub, true);
        let headers = customer_cookie_header(&state, npub);

        let response = dashboard(Extension(state), request_context(), headers)
            .await
            .expect("dashboard response");
        let body = response_body_string(response).await;
        assert!(body.contains("Managed OpenClaw"));
        assert!(body.contains("No managed OpenClaw environment has been provisioned yet."));
        assert!(body.contains("Recent Activity"));
        assert!(body.contains("No recent managed-environment activity yet."));
        assert!(body.contains(npub));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn dashboard_renders_existing_environment_state() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1existingdashboardstate";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            npub,
            "agent-existing",
            Some("vm-existing"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        let headers = customer_cookie_header(&state, npub);

        let response = dashboard(Extension(state), request_context(), headers)
            .await
            .expect("dashboard response");
        let body = response_body_string(response).await;
        assert!(body.contains("vm-existing"));
        assert!(body.contains("agent-existing"));
        assert!(body.contains("Managed OpenClaw is running and ready."));
        assert!(body.contains("Open OpenClaw"));
        assert!(body.contains("short-lived platform ticket"));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn dashboard_renders_healthy_backup_state() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1healthybackupdashboard";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            npub,
            "agent-backup-healthy",
            Some("vm-backup-healthy"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        let headers = customer_cookie_header(&state, npub);
        let (base_url, _rx) = spawn_scripted_server(vec![
            (
                "200 OK",
                r#"{"id":"vm-backup-healthy","status":"running","guest_ready":true}"#,
            ),
            (
                "200 OK",
                r#"{"vm_id":"vm-backup-healthy","backup_host":"pika-build","durable_home_path":"/var/lib/microvms/vm-backup-healthy/home","successful_backup_known":true,"freshness":"healthy","latest_successful_backup_at":"2026-03-11T00:00:00Z","observed_at":"2026-03-11T00:00:00Z"}"#,
            ),
        ]);
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = dashboard(Extension(state), request_context(), headers)
            .await
            .expect("dashboard response");
        let body = response_body_string(response).await;
        assert!(body.contains("Backup Protection"));
        assert!(body.contains("healthy"));
        assert!(body.contains("Recent durable-home backup protection is in place"));
        assert!(body.contains("pika-build"));
        assert!(body.contains("Destructive Reset"));
        assert!(!body.contains("Review Destructive Reset"));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn dashboard_renders_stale_backup_state_with_reset_review() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1stalebackupdashboard";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            npub,
            "agent-backup-stale",
            Some("vm-backup-stale"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        let headers = customer_cookie_header(&state, npub);
        let (base_url, _rx) = spawn_scripted_server(vec![
            (
                "200 OK",
                r#"{"id":"vm-backup-stale","status":"running","guest_ready":true}"#,
            ),
            (
                "200 OK",
                r#"{"vm_id":"vm-backup-stale","backup_host":"pika-build","durable_home_path":"/var/lib/microvms/vm-backup-stale/home","successful_backup_known":true,"freshness":"stale","latest_successful_backup_at":"2026-03-09T00:00:00Z","observed_at":"2026-03-09T00:00:00Z"}"#,
            ),
        ]);
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = dashboard(Extension(state), request_context(), headers)
            .await
            .expect("dashboard response");
        let body = response_body_string(response).await;
        assert!(body.contains("stale"));
        assert!(body.contains("Backup protection is stale"));
        assert!(body.contains("Review Destructive Reset"));
        assert!(body.contains("/dashboard/reset/confirm"));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn dashboard_renders_inflight_creating_environment_without_marking_error() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1inflightdashboardstate";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(&mut conn, npub, "agent-inflight", None, "creating")
            .expect("seed inflight agent");
        let headers = customer_cookie_header(&state, npub);

        let response = dashboard(Extension(state), request_context(), headers)
            .await
            .expect("dashboard response");
        let body = response_body_string(response).await;
        assert!(body.contains("Provisioning a managed OpenClaw environment."));
        assert!(body.contains("agent-inflight"));
        assert!(body.contains("Provisioning is already in flight."));
        assert!(body.contains("stays locked while the initial VM assignment is still in flight"));
        assert!(!body.contains("Open OpenClaw"));
        assert!(!body.contains("Recover Managed Environment"));
        assert!(!body.contains("Provision Fresh Managed Environment"));
        assert!(!body.contains("needs recovery"));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn dashboard_renders_recent_activity_newest_first() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1recentactivitydashboard";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        ManagedEnvironmentEvent::record(
            &mut conn,
            npub,
            Some("agent-1"),
            None,
            "provision_requested",
            "Provision requested for a new Managed OpenClaw environment.",
            Some("req-older"),
        )
        .expect("seed older event");
        ManagedEnvironmentEvent::record(
            &mut conn,
            npub,
            Some("agent-1"),
            Some("vm-1"),
            "provision_accepted",
            "Provision accepted. Managed OpenClaw is starting on VM vm-1.",
            Some("req-newer"),
        )
        .expect("seed newer event");
        let headers = customer_cookie_header(&state, npub);

        let response = dashboard(Extension(state), request_context(), headers)
            .await
            .expect("dashboard response");
        let body = response_body_string(response).await;
        let newer_index = body
            .find("Provision accepted. Managed OpenClaw is starting on VM vm-1.")
            .expect("newer activity");
        let older_index = body
            .find("Provision requested for a new Managed OpenClaw environment.")
            .expect("older activity");
        assert!(body.contains("Recent Activity"));
        assert!(
            newer_index < older_index,
            "activity should render newest-first"
        );

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn dashboard_marks_ready_row_failed_when_vm_is_missing() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1deadreadydashboardstate";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            npub,
            "agent-dead",
            Some("vm-dead"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        let headers = customer_cookie_header(&state, npub);
        let (base_url, _rx) = spawn_one_shot_server("404 Not Found", "vm not found");
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = dashboard(Extension(state), request_context(), headers)
            .await
            .expect("dashboard response");
        let body = response_body_string(response).await;
        assert!(body.contains("needs recovery"));
        assert!(body.contains("falls back to provisioning a fresh environment"));
        assert!(!body.contains("running and ready"));
        let events = recent_activity(&db_pool, npub);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "readiness_refresh_missing_vm");

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn dashboard_failed_without_vm_id_explains_recover_provisions_fresh_environment() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1failednovmdashboardstate";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(&mut conn, npub, "agent-failed", None, AGENT_PHASE_ERROR)
            .expect("seed failed agent without vm");
        let headers = customer_cookie_header(&state, npub);

        let response = dashboard(Extension(state), request_context(), headers)
            .await
            .expect("dashboard response");
        let body = response_body_string(response).await;
        assert!(body.contains("No recoverable VM is available"));
        assert!(body.contains("Recover provisions a fresh environment"));
        assert!(body.contains("Provision Fresh Managed Environment"));
        assert!(body.contains("instead of restoring prior durable state"));
        assert!(body.contains("does not restore missing durable state"));
        assert!(!body.contains("Recover Managed Environment"));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn openclaw_launch_redirects_to_separate_ui_host_when_ready() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1openclawlaunchready";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            npub,
            "agent-openclaw",
            Some("vm-openclaw"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        drop(conn);

        let headers = customer_headers_for_host(&state, npub, "agents.example.com");
        let form = customer_action_form(&state, &headers);
        let (base_url, _rx) = spawn_one_shot_server(
            "200 OK",
            r#"{"id":"vm-openclaw","status":"running","guest_ready":true}"#,
        );
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = openclaw_launch(Extension(state), request_context(), headers, Form(form))
            .await
            .expect("launch response");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("location");
        assert!(location.starts_with("https://openclaw.agents.example.com/launch?ticket="));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn openclaw_launch_rejects_when_environment_is_not_ready() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1openclawlaunchcreating";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            npub,
            "agent-openclaw",
            Some("vm-openclaw"),
            crate::models::agent_instance::AGENT_PHASE_CREATING,
        )
        .expect("seed creating agent");

        let headers = customer_headers_for_host(&state, npub, "agents.example.com");
        let form = customer_action_form(&state, &headers);

        let err = openclaw_launch(Extension(state), request_context(), headers, Form(form))
            .await
            .expect_err("launch should reject non-ready environment");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn openclaw_launch_exchange_rejects_expired_ticket() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1openclawlaunchexpired";
        upsert_allowlist(&db_pool, npub, true);
        let headers = verify_headers("openclaw.agents.example.com");
        let ticket = openclaw_launch_ticket_for_test(
            &state,
            npub,
            "agent-openclaw",
            "vm-openclaw",
            "openclaw.agents.example.com",
            chrono::Utc::now().timestamp() - 5,
        );

        let err = openclaw_launch_exchange(
            Extension(state),
            request_context(),
            headers,
            Query(LaunchTicketQuery { ticket }),
        )
        .await
        .expect_err("expired ticket should fail");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn openclaw_launch_exchange_sets_ui_cookie_for_ready_environment() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1openclawexchangeok";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            npub,
            "agent-openclaw",
            Some("vm-openclaw"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        let headers = verify_headers("openclaw.agents.example.com");
        let ticket = openclaw_launch_ticket_for_test(
            &state,
            npub,
            "agent-openclaw",
            "vm-openclaw",
            "openclaw.agents.example.com",
            chrono::Utc::now().timestamp() + 60,
        );
        let (base_url, _rx) = spawn_one_shot_server(
            "200 OK",
            r#"{"id":"vm-openclaw","status":"running","guest_ready":true}"#,
        );
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = openclaw_launch_exchange(
            Extension(state),
            request_context(),
            headers,
            Query(LaunchTicketQuery { ticket }),
        )
        .await
        .expect("launch exchange");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("/")
        );
        let set_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("set-cookie");
        assert!(set_cookie.contains(OPENCLAW_UI_SESSION_COOKIE));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn openclaw_proxy_rejects_missing_ui_session() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());

        let err = openclaw_proxy(
            Extension(state),
            request_context(),
            Method::GET,
            Uri::from_static("/_openclaw_proxy/"),
            verify_headers("openclaw.agents.example.com"),
            Bytes::new(),
        )
        .await
        .expect_err("missing ui session should fail");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn openclaw_proxy_forwards_request_to_private_spawner_path() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1openclawproxyforward";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            npub,
            "agent-openclaw",
            Some("vm-openclaw"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        let headers = openclaw_ui_headers_for_host(
            &state,
            npub,
            "agent-openclaw",
            "vm-openclaw",
            "openclaw.agents.example.com",
        );
        let (base_url, rx) = spawn_one_shot_server("200 OK", r#"{"ok":true}"#);
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = openclaw_proxy(
            Extension(state),
            request_context(),
            Method::GET,
            "/_openclaw_proxy/api/me?view=full"
                .parse()
                .expect("proxy uri"),
            headers,
            Bytes::new(),
        )
        .await
        .expect("proxy response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_body_string(response).await;
        assert!(body.contains("\"ok\":true"));

        let captured = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured proxy request");
        assert_eq!(captured.method, "GET");
        assert_eq!(captured.path, "/vms/vm-openclaw/openclaw/api/me?view=full");

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn openclaw_proxy_rejects_stale_ui_session_after_vm_change() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1openclawproxystale";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            npub,
            "agent-openclaw",
            Some("vm-current"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        let headers = openclaw_ui_headers_for_host(
            &state,
            npub,
            "agent-openclaw",
            "vm-stale",
            "openclaw.agents.example.com",
        );

        let err = openclaw_proxy(
            Extension(state),
            request_context(),
            Method::GET,
            "/_openclaw_proxy/".parse().expect("proxy uri"),
            headers,
            Bytes::new(),
        )
        .await
        .expect_err("stale ui session should fail");
        assert_eq!(err.0, StatusCode::CONFLICT);

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn dashboard_renders_inflight_creating_environment_without_marking_error() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1inflightdashboardstate";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(&mut conn, npub, "agent-inflight", None, "creating")
            .expect("seed inflight agent");
        let headers = customer_cookie_header(&state, npub);

        let response = dashboard(Extension(state), request_context(), headers)
            .await
            .expect("dashboard response");
        let body = response_body_string(response).await;
        assert!(body.contains("Provisioning a managed OpenClaw environment."));
        assert!(body.contains("agent-inflight"));
        assert!(body.contains("Provisioning is already in flight."));
        assert!(body.contains("stays locked while the initial VM assignment is still in flight"));
        assert!(!body.contains("Recover Managed Environment"));
        assert!(!body.contains("Provision Fresh Managed Environment"));
        assert!(!body.contains("needs recovery"));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn dashboard_renders_recent_activity_newest_first() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1recentactivitydashboard";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        ManagedEnvironmentEvent::record(
            &mut conn,
            npub,
            Some("agent-1"),
            None,
            "provision_requested",
            "Provision requested for a new Managed OpenClaw environment.",
            Some("req-older"),
        )
        .expect("seed older event");
        ManagedEnvironmentEvent::record(
            &mut conn,
            npub,
            Some("agent-1"),
            Some("vm-1"),
            "provision_accepted",
            "Provision accepted. Managed OpenClaw is starting on VM vm-1.",
            Some("req-newer"),
        )
        .expect("seed newer event");
        let headers = customer_cookie_header(&state, npub);

        let response = dashboard(Extension(state), request_context(), headers)
            .await
            .expect("dashboard response");
        let body = response_body_string(response).await;
        let newer_index = body
            .find("Provision accepted. Managed OpenClaw is starting on VM vm-1.")
            .expect("newer activity");
        let older_index = body
            .find("Provision requested for a new Managed OpenClaw environment.")
            .expect("older activity");
        assert!(body.contains("Recent Activity"));
        assert!(
            newer_index < older_index,
            "activity should render newest-first"
        );

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn dashboard_marks_ready_row_failed_when_vm_is_missing() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1deadreadydashboardstate";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            npub,
            "agent-dead",
            Some("vm-dead"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        let headers = customer_cookie_header(&state, npub);
        let (base_url, _rx) = spawn_one_shot_server("404 Not Found", "vm not found");
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = dashboard(Extension(state), request_context(), headers)
            .await
            .expect("dashboard response");
        let body = response_body_string(response).await;
        assert!(body.contains("needs recovery"));
        assert!(body.contains("falls back to provisioning a fresh environment"));
        assert!(!body.contains("running and ready"));
        let events = recent_activity(&db_pool, npub);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "readiness_refresh_missing_vm");

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn dashboard_failed_without_vm_id_explains_recover_provisions_fresh_environment() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1failednovmdashboardstate";
        upsert_allowlist(&db_pool, npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(&mut conn, npub, "agent-failed", None, AGENT_PHASE_ERROR)
            .expect("seed failed agent without vm");
        let headers = customer_cookie_header(&state, npub);

        let response = dashboard(Extension(state), request_context(), headers)
            .await
            .expect("dashboard response");
        let body = response_body_string(response).await;
        assert!(body.contains("No recoverable VM is available"));
        assert!(body.contains("Recover provisions a fresh environment"));
        assert!(body.contains("Recover falls back to provisioning a fresh environment"));
        assert!(body.contains("Provision Fresh Managed Environment"));
        assert!(body.contains("instead of restoring prior durable state"));
        assert!(body.contains("does not restore missing durable state"));
        assert!(!body.contains("Recover Managed Environment"));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn provision_action_creates_environment_when_missing() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = generate_npub();
        upsert_allowlist(&db_pool, &npub, true);
        let headers = customer_cookie_header(&state, &npub);
        let form = customer_action_form(&state, &headers);
        let (base_url, rx) =
            spawn_one_shot_server("200 OK", r#"{"id":"vm-new","status":"starting"}"#);
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = provision(Extension(state), request_context(), headers, Form(form))
            .await
            .expect("provision response");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let captured = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured provision request");
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.path, "/vms");

        let mut conn = db_pool.get().expect("get verify connection");
        let active = AgentInstance::find_active_by_owner(&mut conn, &npub)
            .expect("query active row")
            .expect("active row");
        assert_eq!(active.vm_id.as_deref(), Some("vm-new"));
        let events = recent_activity(&db_pool, npub);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_kind, "provision_accepted");
        assert_eq!(events[1].event_kind, "provision_requested");

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn provision_rejects_invalid_csrf_token() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1csrfrejectcustomerflow";
        upsert_allowlist(&db_pool, npub, true);
        let headers = customer_cookie_header(&state, npub);

        let err = provision(
            Extension(state),
            request_context(),
            headers,
            Form(ActionForm {
                csrf_token: "wrong-token".to_string(),
            }),
        )
        .await
        .expect_err("provision should reject invalid csrf");
        assert_eq!(err.0, StatusCode::FORBIDDEN);

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn recover_action_calls_spawner_recover_for_ready_row() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1recovercustomerflow";
        upsert_allowlist(&db_pool, npub, true);
        let headers = customer_cookie_header(&state, npub);
        let form = customer_action_form(&state, &headers);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            npub,
            "agent-recover",
            Some("vm-recover"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        let (base_url, rx) = spawn_one_shot_server(
            "200 OK",
            r#"{"id":"vm-recover","status":"running","guest_ready":true}"#,
        );
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = recover(Extension(state), request_context(), headers, Form(form))
            .await
            .expect("recover response");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let captured = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured recover request");
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.path, "/vms/vm-recover/recover");
        let events = recent_activity(&db_pool, npub);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_kind, "recover_succeeded");
        assert_eq!(events[1].event_kind, "recover_requested");

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn recover_action_calls_spawner_recover_for_error_row_with_vm_id() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1recovererrorcustomerflow";
        upsert_allowlist(&db_pool, npub, true);
        let headers = customer_cookie_header(&state, npub);
        let form = customer_action_form(&state, &headers);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            npub,
            "agent-error",
            Some("vm-error"),
            crate::models::agent_instance::AGENT_PHASE_ERROR,
        )
        .expect("seed errored agent");
        let (base_url, rx) = spawn_one_shot_server(
            "200 OK",
            r#"{"id":"vm-error","status":"running","guest_ready":true}"#,
        );
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = recover(Extension(state), request_context(), headers, Form(form))
            .await
            .expect("recover response");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let captured = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured recover request");
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.path, "/vms/vm-error/recover");

        let mut conn = db_pool.get().expect("get verify connection");
        let latest = AgentInstance::find_latest_by_owner(&mut conn, npub)
            .expect("query latest row")
            .expect("latest row");
        assert_eq!(latest.agent_id, "agent-error");
        assert_eq!(latest.phase, AGENT_PHASE_READY);
        let events = recent_activity(&db_pool, npub);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_kind, "recover_succeeded");
        assert_eq!(events[1].event_kind, "recover_requested");

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn recover_action_falls_back_to_fresh_when_vm_is_missing() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = "npub1recovermissingvmactivity";
        upsert_allowlist(&db_pool, npub, true);
        let headers = customer_cookie_header(&state, npub);
        let form = customer_action_form(&state, &headers);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            npub,
            "agent-missing",
            Some("vm-missing"),
            AGENT_PHASE_ERROR,
        )
        .expect("seed errored agent");
        let (base_url, rx) = spawn_scripted_server(vec![
            ("404 Not Found", r#"{"error":"vm not found: vm-missing"}"#),
            ("200 OK", r#"{"id":"vm-fresh","status":"starting"}"#),
        ]);
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = recover(Extension(state), request_context(), headers, Form(form))
            .await
            .expect("recover response");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let recover_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured recover request");
        assert_eq!(recover_request.method, "POST");
        assert_eq!(recover_request.path, "/vms/vm-missing/recover");
        let create_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured create request");
        assert_eq!(create_request.method, "POST");
        assert_eq!(create_request.path, "/vms");

        let events = recent_activity(&db_pool, npub);
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].event_kind, "provision_accepted");
        assert_eq!(events[1].event_kind, "provision_requested");
        assert_eq!(events[2].event_kind, "recover_fell_back_to_fresh");
        assert_eq!(events[3].event_kind, "recover_requested");

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn reset_action_destroys_old_vm_and_provisions_fresh_environment() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = generate_npub();
        upsert_allowlist(&db_pool, &npub, true);
        let headers = customer_cookie_header(&state, &npub);
        let form = customer_action_form(&state, &headers);
        let mut conn = db_pool.get().expect("get seed connection");
        let existing = AgentInstance::create(
            &mut conn,
            &npub,
            "agent-old",
            Some("vm-old"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        let (base_url, rx) = spawn_scripted_server(vec![
            (
                "200 OK",
                r#"{"id":"vm-old","status":"running","guest_ready":true}"#,
            ),
            (
                "200 OK",
                r#"{"vm_id":"vm-old","backup_host":"pika-build","durable_home_path":"/var/lib/microvms/vm-old/home","successful_backup_known":true,"freshness":"healthy","latest_successful_backup_at":"2026-03-11T00:00:00Z","observed_at":"2026-03-11T00:00:00Z"}"#,
            ),
            ("204 No Content", ""),
            ("200 OK", r#"{"id":"vm-fresh","status":"starting"}"#),
        ]);
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = reset(Extension(state), request_context(), headers, Form(form))
            .await
            .expect("reset response");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let status_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured status request");
        assert_eq!(status_request.method, "GET");
        assert_eq!(status_request.path, "/vms/vm-old");
        let backup_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured backup status request");
        assert_eq!(backup_request.method, "GET");
        assert_eq!(backup_request.path, "/vms/vm-old/backup-status");
        let delete_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured delete request");
        assert_eq!(delete_request.method, "DELETE");
        assert_eq!(delete_request.path, "/vms/vm-old");
        let create_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured create request");
        assert_eq!(create_request.method, "POST");
        assert_eq!(create_request.path, "/vms");

        let mut conn = db_pool.get().expect("get verify connection");
        let active = AgentInstance::find_active_by_owner(&mut conn, &npub)
            .expect("query active row")
            .expect("active row");
        assert_eq!(active.vm_id.as_deref(), Some("vm-fresh"));

        let retired = AgentInstance::find_by_agent_id(&mut conn, &existing.agent_id)
            .expect("query retired row")
            .expect("retired row");
        assert_eq!(
            retired.phase,
            crate::models::agent_instance::AGENT_PHASE_ERROR
        );
        let events = recent_activity(&db_pool, npub);
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].event_kind, "provision_accepted");
        assert_eq!(events[1].event_kind, "provision_requested");
        assert_eq!(events[2].event_kind, "reset_destroyed_old_vm");
        assert_eq!(events[3].event_kind, "reset_requested");

        clear_test_database(&db_pool);
    }
}
