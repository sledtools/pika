use std::collections::HashSet;

use anyhow::Context;
use askama::Template;
use axum::extract::{Form, Path};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Json};
use diesel::Connection;
use nostr_sdk::prelude::PublicKey;
use nostr_sdk::ToBech32;
use serde::Deserialize;

use crate::agent_api::{
    load_managed_environment_backup_status, load_managed_environment_status,
    ManagedEnvironmentBackupFreshness,
};
use crate::browser_auth::BrowserAuthConfig;
use crate::models::agent_allowlist::AgentAllowlistEntry;
use crate::nostr_auth::{expected_host_from_headers, verify_nip98_event};
use crate::{RequestContext, State};

const ADMIN_BOOTSTRAP_ENV: &str = "PIKA_ADMIN_BOOTSTRAP_NPUBS";
const ADMIN_SESSION_SECRET_ENV: &str = "PIKA_ADMIN_SESSION_SECRET";
const ADMIN_DEV_MODE_ENV: &str = "PIKA_ADMIN_DEV_MODE";
const ADMIN_COOKIE_SECURE_ENV: &str = "PIKA_ADMIN_COOKIE_SECURE";

const ADMIN_SESSION_COOKIE: &str = "pika_admin_session";
const ADMIN_SESSION_TTL_SECS: i64 = 8 * 60 * 60;
const ADMIN_CHALLENGE_KIND: &str = "admin_challenge";
const ADMIN_SESSION_KIND: &str = "admin_session";
const MAX_SUPPORTED_AGENTS: i32 = 1;

#[derive(Clone, Debug)]
pub struct AdminConfig {
    pub bootstrap_admins: HashSet<String>,
    pub browser_auth: BrowserAuthConfig,
}

#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    event: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct AllowlistUpsertForm {
    npub: String,
    note: Option<String>,
    active: Option<String>,
    max_agents: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AllowlistToggleForm {
    active: String,
}

#[derive(Clone, Debug)]
struct AdminAllowlistRow {
    npub: String,
    active: bool,
    note: String,
    max_agents: String,
    updated_by: String,
    updated_at: String,
    next_active: String,
    action_label: String,
}

#[derive(Clone, Debug)]
struct AdminManagedEnvironmentRow {
    owner_npub: String,
    agent_id: String,
    vm_id: String,
    app_state: String,
    startup_phase: String,
    backup_freshness: String,
    backup_last_successful_at: String,
    backup_host: String,
    has_backup_host: bool,
    backup_status_copy: String,
}

#[derive(Template)]
#[template(path = "admin/login.html")]
struct LoginTemplate {
    dev_mode: bool,
}

#[derive(Template)]
#[template(path = "admin/dashboard.html")]
struct DashboardTemplate<'a> {
    current_admin_npub: &'a str,
    rows: &'a [AdminAllowlistRow],
    environment_rows: &'a [AdminManagedEnvironmentRow],
    has_environment_rows: bool,
}

impl AdminConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let raw = std::env::var(ADMIN_BOOTSTRAP_ENV)
            .context("missing PIKA_ADMIN_BOOTSTRAP_NPUBS (comma-separated npubs)")?;
        let mut admins = parse_npub_csv(&raw)?;
        let browser_auth = BrowserAuthConfig::from_env(
            ADMIN_SESSION_SECRET_ENV,
            ADMIN_DEV_MODE_ENV,
            ADMIN_COOKIE_SECURE_ENV,
        )?;

        if let Some(ref npub) = browser_auth.dev_npub {
            admins.insert(npub.to_lowercase());
        }

        Ok(Self {
            bootstrap_admins: admins,
            browser_auth,
        })
    }

    fn is_bootstrap_admin(&self, npub: &str) -> bool {
        self.bootstrap_admins.contains(&npub.trim().to_lowercase())
    }

    fn issue_challenge(&self) -> anyhow::Result<String> {
        self.browser_auth.issue_challenge(ADMIN_CHALLENGE_KIND)
    }

    fn verify_challenge(&self, token: &str) -> anyhow::Result<()> {
        self.browser_auth
            .verify_challenge(token, ADMIN_CHALLENGE_KIND)
    }

    fn issue_session_token(&self, npub: &str) -> anyhow::Result<String> {
        self.browser_auth
            .issue_session_token(ADMIN_SESSION_KIND, npub, ADMIN_SESSION_TTL_SECS)
    }

    fn set_session_cookie(&self, response: &mut Response, token: &str) -> anyhow::Result<()> {
        self.browser_auth.set_session_cookie(
            response,
            ADMIN_SESSION_COOKIE,
            token,
            ADMIN_SESSION_TTL_SECS,
        )
    }

    fn clear_session_cookie(&self, response: &mut Response) -> anyhow::Result<()> {
        self.browser_auth
            .clear_session_cookie(response, ADMIN_SESSION_COOKIE)
    }

    fn session_npub_from_headers(&self, headers: &HeaderMap) -> Option<String> {
        let npub = self.browser_auth.session_npub_from_headers(
            headers,
            ADMIN_SESSION_COOKIE,
            ADMIN_SESSION_KIND,
        )?;
        self.is_bootstrap_admin(&npub).then_some(npub)
    }
}

pub fn admin_healthcheck() -> anyhow::Result<()> {
    let _ = AdminConfig::from_env()?;
    Ok(())
}

pub fn parse_npub_csv(raw: &str) -> anyhow::Result<HashSet<String>> {
    let mut out = HashSet::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        anyhow::ensure!(
            trimmed.starts_with("npub1"),
            "bootstrap admin must be npub: {trimmed}"
        );
        let normalized = PublicKey::parse(trimmed)
            .with_context(|| format!("invalid bootstrap npub: {trimmed}"))?
            .to_bech32()
            .context("normalize bootstrap npub")?
            .to_lowercase();
        out.insert(normalized);
    }
    anyhow::ensure!(!out.is_empty(), "bootstrap admin npub set is empty");
    Ok(out)
}

fn normalize_npub(input: &str) -> anyhow::Result<String> {
    let normalized = PublicKey::parse(input.trim())
        .context("invalid npub")?
        .to_bech32()
        .context("normalize npub")?
        .to_lowercase();
    Ok(normalized)
}

fn parse_form_bool(value: &str) -> anyhow::Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Ok(true),
        "0" | "false" | "off" | "no" => Ok(false),
        _ => anyhow::bail!("invalid bool value"),
    }
}

fn admin_config(state: &State) -> &AdminConfig {
    &state.admin_config
}

fn render_template(template: &impl Template) -> Result<Response, (StatusCode, String)> {
    let html = template
        .render()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(axum::response::Html(html).into_response())
}

fn format_rfc3339_timestamp(value: Option<&str>) -> String {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&chrono::Utc))
        .map(|value| format!("{}", value.format("%Y-%m-%d %H:%M:%S UTC")))
        .unwrap_or_else(|| "not_available".to_string())
}

fn admin_app_state_label(
    state: Option<crate::agent_api_v1_contract::AgentAppState>,
) -> &'static str {
    match state {
        None => "not_provisioned",
        Some(crate::agent_api_v1_contract::AgentAppState::Creating) => "creating",
        Some(crate::agent_api_v1_contract::AgentAppState::Ready) => "ready",
        Some(crate::agent_api_v1_contract::AgentAppState::Error) => "error",
    }
}

fn admin_startup_phase_label(
    phase: Option<pika_agent_control_plane::AgentStartupPhase>,
) -> &'static str {
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

fn admin_backup_freshness_label(freshness: ManagedEnvironmentBackupFreshness) -> &'static str {
    match freshness {
        ManagedEnvironmentBackupFreshness::NotProvisioned => "not_provisioned",
        ManagedEnvironmentBackupFreshness::Healthy => "healthy",
        ManagedEnvironmentBackupFreshness::Stale => "stale",
        ManagedEnvironmentBackupFreshness::Missing => "missing",
        ManagedEnvironmentBackupFreshness::Unavailable => "unavailable",
    }
}

pub async fn login_page(
    Extension(state): Extension<State>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    if admin_config(&state)
        .session_npub_from_headers(&headers)
        .is_some()
    {
        return Ok(Redirect::to("/admin").into_response());
    }
    render_template(&LoginTemplate {
        dev_mode: admin_config(&state).browser_auth.dev_mode,
    })
}

pub async fn challenge(
    Extension(state): Extension<State>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let challenge = admin_config(&state)
        .issue_challenge()
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
        "/admin/verify",
        Some(expected_host.as_str()),
        None,
    )
    .map_err(|err| (StatusCode::UNAUTHORIZED, err.to_string()))?;

    admin_config(&state)
        .verify_challenge(event.content.as_str())
        .map_err(|err| (StatusCode::UNAUTHORIZED, err.to_string()))?;

    if !admin_config(&state).is_bootstrap_admin(&npub) {
        return Err((
            StatusCode::FORBIDDEN,
            "npub is not an authorized bootstrap admin".to_string(),
        ));
    }

    let token = admin_config(&state)
        .issue_session_token(&npub)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    let mut response = Json(serde_json::json!({ "ok": true, "npub": npub })).into_response();
    admin_config(&state)
        .set_session_cookie(&mut response, &token)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(response)
}

pub async fn dashboard(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let Some(admin_npub) = admin_config(&state).session_npub_from_headers(&headers) else {
        return Ok(Redirect::to("/admin/login").into_response());
    };

    let mut conn = state
        .db_pool
        .get()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let rows = AgentAllowlistEntry::list(&mut conn)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    let environment_rows = rows
        .iter()
        .filter_map(|row| row.active.then_some(row.npub.clone()))
        .collect::<Vec<_>>();

    let rows = rows
        .into_iter()
        .map(|row| {
            let (next_active, action_label) = if row.active {
                ("0".to_string(), "Disable".to_string())
            } else {
                ("1".to_string(), "Enable".to_string())
            };
            AdminAllowlistRow {
                npub: row.npub,
                active: row.active,
                note: row.note.unwrap_or_default(),
                max_agents: row.max_agents.unwrap_or(MAX_SUPPORTED_AGENTS).to_string(),
                updated_by: row.updated_by,
                updated_at: row.updated_at.to_string(),
                next_active,
                action_label,
            }
        })
        .collect::<Vec<_>>();

    let mut managed_environment_rows = Vec::new();
    for owner_npub in environment_rows {
        let status =
            load_managed_environment_status(&state, &owner_npub, &request_context.request_id)
                .await
                .map_err(|err| (err.status_code(), err.error_code().to_string()))?;
        let Some(row) = status.row.as_ref() else {
            continue;
        };
        let backup =
            load_managed_environment_backup_status(&status, &request_context.request_id).await;
        managed_environment_rows.push(AdminManagedEnvironmentRow {
            owner_npub: owner_npub.clone(),
            agent_id: row.agent_id.clone(),
            vm_id: row
                .vm_id
                .clone()
                .unwrap_or_else(|| "not_assigned".to_string()),
            app_state: admin_app_state_label(status.app_state).to_string(),
            startup_phase: admin_startup_phase_label(status.startup_phase).to_string(),
            backup_freshness: admin_backup_freshness_label(backup.freshness).to_string(),
            backup_last_successful_at: format_rfc3339_timestamp(
                backup.latest_successful_backup_at.as_deref(),
            ),
            backup_host: backup
                .backup_host
                .clone()
                .unwrap_or_else(|| "not_available".to_string()),
            has_backup_host: backup.backup_host.is_some(),
            backup_status_copy: backup.status_copy,
        });
    }

    render_template(&DashboardTemplate {
        current_admin_npub: &admin_npub,
        rows: &rows,
        environment_rows: &managed_environment_rows,
        has_environment_rows: !managed_environment_rows.is_empty(),
    })
}

pub async fn upsert_allowlist(
    Extension(state): Extension<State>,
    headers: HeaderMap,
    Form(form): Form<AllowlistUpsertForm>,
) -> Result<Response, (StatusCode, String)> {
    let Some(admin_npub) = admin_config(&state).session_npub_from_headers(&headers) else {
        return Ok(Redirect::to("/admin/login").into_response());
    };

    let npub = normalize_npub(&form.npub)
        .map_err(|err| (StatusCode::BAD_REQUEST, format!("invalid npub: {err}")))?;
    let active = form.active.is_some();
    let note = form
        .note
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let max_agents = form
        .max_agents
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.parse::<i32>())
        .transpose()
        .map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid max_agents: {err}"),
            )
        })?
        .unwrap_or(MAX_SUPPORTED_AGENTS);
    if max_agents != MAX_SUPPORTED_AGENTS {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "max_agents must be exactly {MAX_SUPPORTED_AGENTS} until the API/client add multi-agent selection"
            ),
        ));
    }

    let mut conn = state
        .db_pool
        .get()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    conn.transaction::<(), anyhow::Error, _>(|conn| {
        AgentAllowlistEntry::upsert(
            conn,
            &npub,
            active,
            note.as_deref(),
            &admin_npub,
            Some(max_agents),
        )?;
        AgentAllowlistEntry::record_audit(
            conn,
            &admin_npub,
            &npub,
            if active {
                "upsert_active"
            } else {
                "upsert_inactive"
            },
            note.as_deref(),
        )?;
        Ok(())
    })
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    Ok(Redirect::to("/admin").into_response())
}

pub async fn toggle_allowlist(
    Extension(state): Extension<State>,
    headers: HeaderMap,
    Path(npub): Path<String>,
    Form(form): Form<AllowlistToggleForm>,
) -> Result<Response, (StatusCode, String)> {
    let Some(admin_npub) = admin_config(&state).session_npub_from_headers(&headers) else {
        return Ok(Redirect::to("/admin/login").into_response());
    };

    let normalized = normalize_npub(&npub)
        .map_err(|err| (StatusCode::BAD_REQUEST, format!("invalid npub: {err}")))?;
    let active =
        parse_form_bool(&form.active).map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;

    let mut conn = state
        .db_pool
        .get()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    conn.transaction::<(), anyhow::Error, _>(|conn| {
        AgentAllowlistEntry::set_active(conn, &normalized, active, &admin_npub)?;
        AgentAllowlistEntry::record_audit(
            conn,
            &admin_npub,
            &normalized,
            if active { "enabled" } else { "disabled" },
            None,
        )?;
        Ok(())
    })
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    Ok(Redirect::to("/admin").into_response())
}

pub async fn logout(Extension(state): Extension<State>) -> Result<Response, (StatusCode, String)> {
    let mut response = Redirect::to("/admin/login").into_response();
    admin_config(&state)
        .clear_session_cookie(&mut response)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(response)
}

pub async fn dev_login(
    Extension(state): Extension<State>,
) -> Result<Response, (StatusCode, String)> {
    if !admin_config(&state).browser_auth.dev_mode {
        return Err((StatusCode::NOT_FOUND, "dev mode disabled".to_string()));
    }
    let npub = admin_config(&state)
        .browser_auth
        .dev_npub
        .clone()
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "PIKA_TEST_NSEC is missing".to_string(),
            )
        })?;

    let token = admin_config(&state)
        .issue_session_token(&npub)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    let mut response = Redirect::to("/admin").into_response();
    admin_config(&state)
        .set_session_cookie(&mut response, &token)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(response)
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    use axum::body::HttpBody;
    use axum::http::header;
    use diesel::prelude::*;
    use diesel::r2d2::{ConnectionManager, Pool};
    use diesel::PgConnection;
    use diesel_migrations::MigrationHarness;
    use nostr_sdk::prelude::Keys;
    use nostr_sdk::ToBech32;
    use pika_test_utils::CapturedRequest;

    use crate::models::agent_instance::{AgentInstance, AGENT_PHASE_READY};
    use crate::models::group_subscription::GroupFilterInfo;
    use crate::models::MIGRATIONS;
    use crate::test_support::serial_test_guard;

    fn test_admin_config(npub: &str) -> AdminConfig {
        let mut bootstrap_admins = HashSet::new();
        bootstrap_admins.insert(npub.to_string());
        AdminConfig {
            bootstrap_admins,
            browser_auth: BrowserAuthConfig::new(
                b"0123456789abcdef0123456789abcdef".to_vec(),
                true,
                false,
                None,
            )
            .expect("browser auth config"),
        }
    }

    fn init_test_db_pool() -> Option<Pool<ConnectionManager<PgConnection>>> {
        dotenv::dotenv().ok();
        let Some(url) = std::env::var("DATABASE_URL").ok() else {
            eprintln!("SKIP: DATABASE_URL must be set for admin tests");
            return None;
        };
        if let Err(err) = PgConnection::establish(&url) {
            eprintln!("SKIP: postgres unavailable for admin tests: {err}");
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

    fn test_state(db_pool: Pool<ConnectionManager<PgConnection>>, admin_npub: &str) -> State {
        let (sender, _receiver) = tokio::sync::watch::channel(GroupFilterInfo::default());
        State {
            db_pool,
            apns_client: None,
            fcm_client: None,
            apns_topic: String::new(),
            channel: std::sync::Arc::new(tokio::sync::Mutex::new(sender)),
            admin_config: std::sync::Arc::new(test_admin_config(admin_npub)),
            min_app_version: "0.0.0".to_string(),
            trust_forwarded_host: false,
        }
    }

    fn admin_cookie_header(state: &State, npub: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let token = state
            .admin_config
            .issue_session_token(npub)
            .expect("issue admin session");
        headers.insert(
            header::COOKIE,
            format!("{ADMIN_SESSION_COOKIE}={token}")
                .parse()
                .expect("cookie header"),
        );
        headers
    }

    fn generate_npub() -> String {
        Keys::generate()
            .public_key()
            .to_bech32()
            .expect("encode generated npub")
            .to_lowercase()
    }

    async fn response_body_string(response: Response) -> String {
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(chunk) = body.data().await {
            bytes.extend_from_slice(&chunk.expect("read response chunk"));
        }
        String::from_utf8(bytes).expect("utf8 response body")
    }

    fn request_context() -> Extension<RequestContext> {
        Extension(RequestContext {
            request_id: "req-admin-test".to_string(),
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
                    let head = String::from_utf8_lossy(&buf[..end]);
                    for line in head.lines().skip(1) {
                        if let Some((name, value)) = line.split_once(':') {
                            if name.eq_ignore_ascii_case("content-length") {
                                content_length = value.trim().parse().unwrap_or(0);
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

    #[test]
    fn parse_npub_csv_rejects_invalid_items() {
        let err = parse_npub_csv("not-a-npub").expect_err("invalid npub must fail");
        assert!(err.to_string().contains("bootstrap admin must be npub"));
    }

    #[test]
    fn parse_npub_csv_normalizes_and_dedupes() {
        let set = parse_npub_csv(
            "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y,npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y",
        )
        .expect("valid set");
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn session_cookie_parsing_skips_malformed_pairs() {
        let npub = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y".to_string();
        let config = test_admin_config(&npub);
        let token = config
            .issue_session_token(&npub)
            .expect("issue session token");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("broken-cookie; {ADMIN_SESSION_COOKIE}={token}")
                .parse()
                .expect("cookie header"),
        );

        assert_eq!(config.session_npub_from_headers(&headers), Some(npub));
    }

    #[tokio::test]
    async fn dashboard_renders_backup_freshness_for_current_environment() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let admin_npub = generate_npub();
        let owner_npub = generate_npub();
        let state = test_state(db_pool.clone(), &admin_npub);
        let headers = admin_cookie_header(&state, &admin_npub);

        let mut conn = db_pool.get().expect("get seed connection");
        AgentAllowlistEntry::upsert(
            &mut conn,
            &owner_npub,
            true,
            Some("test"),
            &admin_npub,
            Some(1),
        )
        .expect("seed allowlist");
        AgentInstance::create(
            &mut conn,
            &owner_npub,
            "agent-admin-backup",
            Some("vm-admin-backup"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");

        let (base_url, _rx) = spawn_scripted_server(vec![
            (
                "200 OK",
                r#"{"id":"vm-admin-backup","status":"running","guest_ready":true}"#,
            ),
            (
                "200 OK",
                r#"{"vm_id":"vm-admin-backup","backup_host":"pika-build","durable_home_path":"/var/lib/microvms/vm-admin-backup/home","successful_backup_known":true,"freshness":"healthy","latest_successful_backup_at":"2026-03-11T00:00:00Z","observed_at":"2026-03-11T00:00:00Z"}"#,
            ),
        ]);
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = dashboard(Extension(state), request_context(), headers)
            .await
            .expect("dashboard response");
        let body = response_body_string(response).await;
        assert!(body.contains("Managed Environment Backups"));
        assert!(body.contains("agent-admin-backup"));
        assert!(body.contains("vm-admin-backup"));
        assert!(body.contains("healthy"));
        assert!(body.contains("pika-build"));

        clear_test_database(&db_pool);
    }
}
