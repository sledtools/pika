use askama::Template;
use axum::body::Bytes;
use axum::extract::Form;
use axum::extract::Query;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Json};

use crate::agent_api::{
    list_recent_managed_environment_events, load_current_ready_managed_environment,
    load_launchable_managed_environment, load_managed_environment_backup_status,
    load_managed_environment_status, provision_managed_environment_if_missing,
    recover_agent_for_owner, reset_agent_for_owner, spawner_base_url, AgentApiError,
    ManagedEnvironmentBackupFreshness, ManagedEnvironmentBackupStatus, ManagedEnvironmentStatus,
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
const OPENCLAW_UI_HOST_PREFIX: &str = "openclaw.";
const OPENCLAW_LAUNCH_TICKET_KIND: &str = "openclaw_ui_launch_ticket";
const OPENCLAW_UI_SESSION_KIND: &str = "openclaw_ui_session";
const OPENCLAW_UI_SESSION_COOKIE: &str = "pika_openclaw_ui_session";
const OPENCLAW_LAUNCH_TTL_SECS: i64 = 60;
const OPENCLAW_UI_SESSION_TTL_SECS: i64 = 15 * 60;
const RESET_CONFIRMATION_KIND: &str = "customer_reset_confirmation";
const RESET_CONFIRMATION_TTL_SECS: i64 = 15 * 60;
pub(crate) const OPENCLAW_INTERNAL_LAUNCH_PATH: &str = "/_openclaw_launch";
pub(crate) const OPENCLAW_INTERNAL_PROXY_PREFIX: &str = "/_openclaw_proxy";
pub(crate) const OPENCLAW_INTERNAL_PROXY_PATH: &str = "/_openclaw_proxy/*path";

#[derive(Debug, serde::Deserialize)]
pub struct VerifyRequest {
    event: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
pub struct ActionForm {
    csrf_token: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct ResetActionForm {
    csrf_token: String,
    confirmation_token: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct LaunchTicketQuery {
    ticket: String,
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

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct ResetConfirmationTicket {
    kind: String,
    npub: String,
    agent_id: String,
    vm_id: Option<String>,
    exp: i64,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct OpenClawLaunchTicket {
    kind: String,
    npub: String,
    agent_id: String,
    vm_id: String,
    ui_host: String,
    exp: i64,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct OpenClawUiSession {
    kind: String,
    npub: String,
    agent_id: String,
    vm_id: String,
    ui_host: String,
    exp: i64,
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
    backup_state_label: &'static str,
    backup_state_tone: &'static str,
    backup_status_copy: String,
    backup_last_successful_at: String,
    backup_host: String,
    has_backup_host: bool,
    reset_requires_confirmation: bool,
    reset_safety_copy: String,
    can_launch_openclaw: bool,
    launch_status_copy: &'static str,
    recent_activity: Vec<DashboardActivityItem>,
    has_recent_activity: bool,
}

#[derive(Template)]
#[template(path = "customer/reset_confirm.html")]
struct ResetConfirmTemplate {
    owner_npub: String,
    csrf_token: String,
    confirmation_token: String,
    backup_state_label: &'static str,
    backup_state_tone: &'static str,
    backup_status_copy: String,
    backup_last_successful_at: String,
    backup_host: String,
    has_backup_host: bool,
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

fn format_rfc3339_timestamp(value: Option<&str>) -> String {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&chrono::Utc))
        .map(|value| format!("{}", value.format("%Y-%m-%d %H:%M:%S UTC")))
        .unwrap_or_else(|| "not_available".to_string())
}

fn backup_state_label(freshness: ManagedEnvironmentBackupFreshness) -> &'static str {
    match freshness {
        ManagedEnvironmentBackupFreshness::NotProvisioned => "not_provisioned",
        ManagedEnvironmentBackupFreshness::Healthy => "healthy",
        ManagedEnvironmentBackupFreshness::Stale => "stale",
        ManagedEnvironmentBackupFreshness::Missing => "missing",
        ManagedEnvironmentBackupFreshness::Unavailable => "unavailable",
    }
}

fn backup_state_tone(freshness: ManagedEnvironmentBackupFreshness) -> &'static str {
    match freshness {
        ManagedEnvironmentBackupFreshness::Healthy => "ok",
        ManagedEnvironmentBackupFreshness::Stale => "warm",
        ManagedEnvironmentBackupFreshness::Missing
        | ManagedEnvironmentBackupFreshness::Unavailable => "error",
        ManagedEnvironmentBackupFreshness::NotProvisioned => "idle",
    }
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

fn verify_reset_action_csrf(
    authenticated: &AuthenticatedCustomer,
    form: &ResetActionForm,
) -> Result<(), (StatusCode, String)> {
    if authenticated.csrf_token == form.csrf_token {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "invalid csrf token".to_string()))
    }
}

fn request_host(state: &State, headers: &HeaderMap) -> Result<String, (StatusCode, String)> {
    expected_host_from_headers(headers, state.trust_forwarded_host)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing host header".to_string()))
}

fn external_request_scheme(state: &State, headers: &HeaderMap) -> &'static str {
    if state.trust_forwarded_host {
        if let Some(proto) = headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
        {
            if proto.eq_ignore_ascii_case("http") {
                return "http";
            }
            if proto.eq_ignore_ascii_case("https") {
                return "https";
            }
        }
    }
    match headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
    {
        Some(host)
            if host.starts_with("localhost")
                || host.starts_with("127.0.0.1")
                || host.starts_with("[::1]") =>
        {
            "http"
        }
        _ => "https",
    }
}

fn openclaw_ui_host_for_dashboard_host(host: &str) -> String {
    if host.starts_with(OPENCLAW_UI_HOST_PREFIX) {
        host.to_string()
    } else {
        format!("{OPENCLAW_UI_HOST_PREFIX}{host}")
    }
}

fn cookie_value_from_headers(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    let cookie = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for pair in cookie.split(';') {
        let (name, value) = pair.trim().split_once('=')?;
        if name == cookie_name {
            return Some(value.to_string());
        }
    }
    None
}

fn issue_reset_confirmation_ticket(
    state: &State,
    npub: &str,
    agent_id: &str,
    vm_id: Option<&str>,
) -> Result<String, (StatusCode, String)> {
    browser_auth(state)
        .sign_payload(&ResetConfirmationTicket {
            kind: RESET_CONFIRMATION_KIND.to_string(),
            npub: npub.to_string(),
            agent_id: agent_id.to_string(),
            vm_id: vm_id.map(ToOwned::to_owned),
            exp: chrono::Utc::now().timestamp() + RESET_CONFIRMATION_TTL_SECS,
        })
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

fn verify_reset_confirmation_ticket(
    state: &State,
    npub: &str,
    agent_id: &str,
    vm_id: Option<&str>,
    token: Option<&str>,
) -> Result<(), (StatusCode, String)> {
    let token = token.ok_or_else(|| {
        (
            StatusCode::CONFLICT,
            "destructive reset requires confirmation because backup protection is not healthy"
                .to_string(),
        )
    })?;
    let ticket: ResetConfirmationTicket =
        browser_auth(state).verify_payload(token).map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                "invalid reset confirmation".to_string(),
            )
        })?;
    if ticket.kind != RESET_CONFIRMATION_KIND {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid reset confirmation".to_string(),
        ));
    }
    if ticket.exp < chrono::Utc::now().timestamp() {
        return Err((
            StatusCode::UNAUTHORIZED,
            "reset confirmation expired".to_string(),
        ));
    }
    if ticket.npub != npub {
        return Err((
            StatusCode::FORBIDDEN,
            "reset confirmation owner mismatch".to_string(),
        ));
    }
    if ticket.agent_id != agent_id || ticket.vm_id.as_deref() != vm_id {
        return Err((
            StatusCode::CONFLICT,
            "reset confirmation no longer matches the current managed environment".to_string(),
        ));
    }
    Ok(())
}

fn issue_openclaw_launch_ticket(
    state: &State,
    npub: &str,
    agent_id: &str,
    vm_id: &str,
    ui_host: &str,
) -> Result<String, (StatusCode, String)> {
    browser_auth(state)
        .sign_payload(&OpenClawLaunchTicket {
            kind: OPENCLAW_LAUNCH_TICKET_KIND.to_string(),
            npub: npub.to_string(),
            agent_id: agent_id.to_string(),
            vm_id: vm_id.to_string(),
            ui_host: ui_host.to_string(),
            exp: chrono::Utc::now().timestamp() + OPENCLAW_LAUNCH_TTL_SECS,
        })
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

fn verify_openclaw_launch_ticket(
    state: &State,
    token: &str,
    actual_host: &str,
) -> Result<OpenClawLaunchTicket, (StatusCode, String)> {
    let ticket: OpenClawLaunchTicket = browser_auth(state).verify_payload(token).map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            "invalid launch ticket".to_string(),
        )
    })?;
    if ticket.kind != OPENCLAW_LAUNCH_TICKET_KIND {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid launch ticket".to_string(),
        ));
    }
    if ticket.exp < chrono::Utc::now().timestamp() {
        return Err((
            StatusCode::UNAUTHORIZED,
            "launch ticket expired".to_string(),
        ));
    }
    if ticket.ui_host != actual_host {
        return Err((
            StatusCode::UNAUTHORIZED,
            "launch ticket host mismatch".to_string(),
        ));
    }
    Ok(ticket)
}

fn issue_openclaw_ui_session(
    state: &State,
    ticket: &OpenClawLaunchTicket,
) -> Result<String, (StatusCode, String)> {
    browser_auth(state)
        .sign_payload(&OpenClawUiSession {
            kind: OPENCLAW_UI_SESSION_KIND.to_string(),
            npub: ticket.npub.clone(),
            agent_id: ticket.agent_id.clone(),
            vm_id: ticket.vm_id.clone(),
            ui_host: ticket.ui_host.clone(),
            exp: chrono::Utc::now().timestamp() + OPENCLAW_UI_SESSION_TTL_SECS,
        })
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

fn openclaw_ui_session_from_headers(
    state: &State,
    headers: &HeaderMap,
    actual_host: &str,
) -> Result<Option<OpenClawUiSession>, (StatusCode, String)> {
    let Some(token) = cookie_value_from_headers(headers, OPENCLAW_UI_SESSION_COOKIE) else {
        return Ok(None);
    };
    let session: OpenClawUiSession = browser_auth(state).verify_payload(&token).map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            "invalid openclaw ui session".to_string(),
        )
    })?;
    if session.kind != OPENCLAW_UI_SESSION_KIND {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid openclaw ui session".to_string(),
        ));
    }
    if session.exp < chrono::Utc::now().timestamp() {
        return Err((
            StatusCode::UNAUTHORIZED,
            "openclaw ui session expired".to_string(),
        ));
    }
    if session.ui_host != actual_host {
        return Err((
            StatusCode::UNAUTHORIZED,
            "openclaw ui session host mismatch".to_string(),
        ));
    }
    Ok(Some(session))
}

fn origin_matches_host(origin: &str, actual_host: &str) -> bool {
    let Ok(origin_url) = reqwest::Url::parse(origin) else {
        return false;
    };
    let Some(origin_host) = origin_url.host_str() else {
        return false;
    };
    let origin_authority = match origin_url.port() {
        Some(port) => format!("{origin_host}:{port}"),
        None => origin_host.to_string(),
    };
    origin_authority.eq_ignore_ascii_case(actual_host)
}

fn require_same_origin_ui_request(
    headers: &HeaderMap,
    method: &Method,
    actual_host: &str,
) -> Result<(), (StatusCode, String)> {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return Ok(());
    }
    if let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        return origin_matches_host(origin, actual_host)
            .then_some(())
            .ok_or_else(|| {
                (
                    StatusCode::FORBIDDEN,
                    "cross-origin OpenClaw UI mutation is not allowed".to_string(),
                )
            });
    }
    let same_origin_fetch = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .map(|value| value.eq_ignore_ascii_case("same-origin") || value == "none")
        .unwrap_or(false);
    if same_origin_fetch {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            "missing same-origin proof for OpenClaw UI mutation".to_string(),
        ))
    }
}

fn header_is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn copy_proxy_response_headers(target: &mut HeaderMap, source: &reqwest::header::HeaderMap) {
    for (name, value) in source {
        let name_str = name.as_str();
        if header_is_hop_by_hop(name_str) || name_str == "set-cookie" {
            continue;
        }
        let Ok(header_name) = HeaderName::from_bytes(name.as_str().as_bytes()) else {
            continue;
        };
        let Ok(header_value) = HeaderValue::from_bytes(value.as_bytes()) else {
            continue;
        };
        target.append(header_name, header_value);
    }
}

fn dashboard_template(
    authenticated: AuthenticatedCustomer,
    status: ManagedEnvironmentStatus,
    backup: ManagedEnvironmentBackupStatus,
    recent_activity: Vec<DashboardActivityItem>,
) -> DashboardTemplate {
    let row = status.row;
    let app_state = status.app_state;
    let startup_phase = status.startup_phase;
    let inflight_without_vm = row
        .as_ref()
        .map(|row| {
            row.phase == crate::models::agent_instance::AGENT_PHASE_CREATING && row.vm_id.is_none()
        })
        .unwrap_or(false);
    let recoverable_vm_exists = row.as_ref().and_then(|row| row.vm_id.as_deref()).is_some();
    let can_launch_openclaw = app_state == Some(crate::agent_api_v1_contract::AgentAppState::Ready)
        && startup_phase == Some(pika_agent_control_plane::AgentStartupPhase::Ready)
        && recoverable_vm_exists;
    let has_backup_host = backup.backup_host.is_some();
    let backup_host = backup
        .backup_host
        .unwrap_or_else(|| "not_available".to_string());
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
        backup_state_label: backup_state_label(backup.freshness),
        backup_state_tone: backup_state_tone(backup.freshness),
        backup_status_copy: backup.status_copy,
        backup_last_successful_at: format_rfc3339_timestamp(
            backup.latest_successful_backup_at.as_deref(),
        ),
        backup_host,
        has_backup_host,
        reset_requires_confirmation: backup.reset_requires_confirmation,
        reset_safety_copy: if backup.reset_requires_confirmation {
            "Because backup protection is stale, missing, or unavailable, destructive reset now requires an explicit confirmation step."
                .to_string()
        } else {
            "Recent backup protection is healthy, so destructive reset remains available directly from the dashboard."
                .to_string()
        },
        can_launch_openclaw,
        launch_status_copy: if can_launch_openclaw {
            "Open the built-in OpenClaw UI on its own platform-managed origin. Launch uses a short-lived platform ticket and a scoped UI session rather than the dashboard cookie."
        } else if row.is_none() {
            "Provision Managed OpenClaw before launching the built-in UI."
        } else if inflight_without_vm {
            "OpenClaw launch unlocks after the current VM assignment finishes."
        } else if app_state == Some(crate::agent_api_v1_contract::AgentAppState::Error) {
            "Recover or reprovision the managed environment before opening OpenClaw."
        } else {
            "OpenClaw launch becomes available once the managed environment is fully ready."
        },
        has_recent_activity: !recent_activity.is_empty(),
        recent_activity,
    }
}

fn reset_confirm_template(
    authenticated: &AuthenticatedCustomer,
    backup: ManagedEnvironmentBackupStatus,
    confirmation_token: String,
) -> ResetConfirmTemplate {
    let has_backup_host = backup.backup_host.is_some();
    let backup_host = backup
        .backup_host
        .unwrap_or_else(|| "not_available".to_string());
    ResetConfirmTemplate {
        owner_npub: authenticated.npub.clone(),
        csrf_token: authenticated.csrf_token.clone(),
        confirmation_token,
        backup_state_label: backup_state_label(backup.freshness),
        backup_state_tone: backup_state_tone(backup.freshness),
        backup_status_copy: backup.status_copy,
        backup_last_successful_at: format_rfc3339_timestamp(
            backup.latest_successful_backup_at.as_deref(),
        ),
        backup_host,
        has_backup_host,
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
    let backup = load_managed_environment_backup_status(&status, &request_context.request_id).await;
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
        backup,
        recent_activity_items(activity),
    ))
}

pub async fn reset_confirm_page(
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
    let Some(row) = status.row.as_ref() else {
        return Ok(Redirect::to("/dashboard").into_response());
    };
    let inflight_without_vm =
        row.phase == crate::models::agent_instance::AGENT_PHASE_CREATING && row.vm_id.is_none();
    if inflight_without_vm {
        return Ok(Redirect::to("/dashboard").into_response());
    }
    let backup = load_managed_environment_backup_status(&status, &request_context.request_id).await;
    if !backup.reset_requires_confirmation {
        return Ok(Redirect::to("/dashboard").into_response());
    }
    let confirmation_token = issue_reset_confirmation_ticket(
        &state,
        &authenticated.npub,
        &row.agent_id,
        row.vm_id.as_deref(),
    )?;
    render_template(&reset_confirm_template(
        &authenticated,
        backup,
        confirmation_token,
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
    Form(form): Form<ResetActionForm>,
) -> Result<Response, (StatusCode, String)> {
    let Some(authenticated) = allowlisted_customer_from_session(&state, &headers).await? else {
        return redirect_to_login(&state, true);
    };
    verify_reset_action_csrf(&authenticated, &form)?;
    let status =
        load_managed_environment_status(&state, &authenticated.npub, &request_context.request_id)
            .await
            .map_err(map_agent_api_error)?;
    let Some(row) = status.row.as_ref() else {
        return Err((
            StatusCode::CONFLICT,
            "destructive reset requires a current managed environment".to_string(),
        ));
    };
    let inflight_without_vm =
        row.phase == crate::models::agent_instance::AGENT_PHASE_CREATING && row.vm_id.is_none();
    if inflight_without_vm {
        return Err((
            StatusCode::CONFLICT,
            "destructive reset stays locked while the current VM assignment is still in flight"
                .to_string(),
        ));
    }
    let backup = load_managed_environment_backup_status(&status, &request_context.request_id).await;
    if backup.reset_requires_confirmation {
        verify_reset_confirmation_ticket(
            &state,
            &authenticated.npub,
            &row.agent_id,
            row.vm_id.as_deref(),
            form.confirmation_token.as_deref(),
        )?;
    }

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

pub async fn openclaw_launch(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    headers: HeaderMap,
    Form(form): Form<ActionForm>,
) -> Result<Response, (StatusCode, String)> {
    let Some(authenticated) = allowlisted_customer_from_session(&state, &headers).await? else {
        return redirect_to_login(&state, true);
    };
    verify_action_csrf(&authenticated, &form)?;

    let launch_target = load_launchable_managed_environment(
        &state,
        &authenticated.npub,
        &request_context.request_id,
    )
    .await
    .map_err(map_agent_api_error)?;
    let dashboard_host = request_host(&state, &headers)?;
    let ui_host = openclaw_ui_host_for_dashboard_host(&dashboard_host);
    let origin = format!(
        "{}://{}",
        external_request_scheme(&state, &headers),
        ui_host
    );
    let ticket = issue_openclaw_launch_ticket(
        &state,
        &launch_target.owner_npub,
        &launch_target.agent_id,
        &launch_target.vm_id,
        &ui_host,
    )?;
    let mut launch_url =
        reqwest::Url::parse(&format!("{origin}/launch")).expect("openclaw launch url");
    launch_url.query_pairs_mut().append_pair("ticket", &ticket);
    Ok(Redirect::to(launch_url.as_str()).into_response())
}

pub async fn openclaw_launch_exchange(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    headers: HeaderMap,
    Query(query): Query<LaunchTicketQuery>,
) -> Result<Response, (StatusCode, String)> {
    let actual_host = request_host(&state, &headers)?;
    let ticket = verify_openclaw_launch_ticket(&state, &query.ticket, &actual_host)?;

    let mut conn = state
        .db_pool
        .get()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    if !AgentAllowlistEntry::is_active(&mut conn, &ticket.npub)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "npub is not allowlisted".to_string()));
    }
    drop(conn);

    let launch_target =
        load_launchable_managed_environment(&state, &ticket.npub, &request_context.request_id)
            .await
            .map_err(map_agent_api_error)?;
    if launch_target.agent_id != ticket.agent_id || launch_target.vm_id != ticket.vm_id {
        return Err((
            StatusCode::CONFLICT,
            "managed openclaw environment changed before launch".to_string(),
        ));
    }

    let ui_session = issue_openclaw_ui_session(&state, &ticket)?;
    let mut response = Redirect::to("/").into_response();
    browser_auth(&state)
        .set_session_cookie_with_path(
            &mut response,
            OPENCLAW_UI_SESSION_COOKIE,
            &ui_session,
            OPENCLAW_UI_SESSION_TTL_SECS,
            "/",
        )
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(response)
}

pub async fn openclaw_proxy(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, (StatusCode, String)> {
    let actual_host = request_host(&state, &headers)?;
    let Some(ui_session) = openclaw_ui_session_from_headers(&state, &headers, &actual_host)? else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "missing openclaw ui session".to_string(),
        ));
    };
    require_same_origin_ui_request(&headers, &method, &actual_host)?;

    let mut conn = state
        .db_pool
        .get()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    if !AgentAllowlistEntry::is_active(&mut conn, &ui_session.npub)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "npub is not allowlisted".to_string()));
    }
    drop(conn);

    let Some(current) = load_current_ready_managed_environment(
        &state,
        &ui_session.npub,
        &request_context.request_id,
    )
    .map_err(map_agent_api_error)?
    else {
        return Err((
            StatusCode::CONFLICT,
            "managed openclaw environment is not launchable".to_string(),
        ));
    };
    if current.agent_id != ui_session.agent_id || current.vm_id != ui_session.vm_id {
        return Err((
            StatusCode::CONFLICT,
            "managed openclaw environment changed; relaunch from the dashboard".to_string(),
        ));
    }

    let internal_path = uri.path();
    let upstream_path = internal_path
        .strip_prefix(OPENCLAW_INTERNAL_PROXY_PREFIX)
        .unwrap_or("/");
    let upstream_path = if upstream_path.is_empty() {
        "/"
    } else {
        upstream_path
    };
    let spawner_url = spawner_base_url(&request_context.request_id).map_err(map_agent_api_error)?;
    let upstream_url = if let Some(query) = uri.query() {
        format!(
            "{spawner_url}/vms/{}/openclaw{upstream_path}?{query}",
            current.vm_id
        )
    } else {
        format!(
            "{spawner_url}/vms/{}/openclaw{upstream_path}",
            current.vm_id
        )
    };

    let client = reqwest::Client::new();
    let upstream_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid proxied method: {err}"),
            )
        })?;
    let mut upstream = client.request(upstream_method, &upstream_url);
    for (name, value) in &headers {
        let name_str = name.as_str();
        if header_is_hop_by_hop(name_str)
            || matches!(name_str, "host" | "cookie" | "content-length")
        {
            continue;
        }
        let reqwest_name = reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes())
            .map_err(|err| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("invalid proxied header name: {err}"),
                )
            })?;
        let reqwest_value =
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()).map_err(|err| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("invalid proxied header value: {err}"),
                )
            })?;
        upstream = upstream.header(reqwest_name, reqwest_value);
    }
    if !body.is_empty() {
        upstream = upstream.body(body.to_vec());
    }
    let upstream = upstream.send().await.map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            format!("openclaw proxy upstream failed: {err}"),
        )
    })?;

    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let upstream_headers = upstream.headers().clone();
    let upstream_body = upstream.bytes().await.map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            format!("openclaw proxy upstream body failed: {err}"),
        )
    })?;
    let mut response = (status, upstream_body).into_response();
    copy_proxy_response_headers(response.headers_mut(), &upstream_headers);
    Ok(response)
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
    use crate::models::agent_instance::{
        AgentInstance, AGENT_PHASE_CREATING, AGENT_PHASE_ERROR, AGENT_PHASE_READY,
    };
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

    fn customer_reset_form(state: &State, headers: &HeaderMap) -> ResetActionForm {
        let session = state
            .admin_config
            .browser_auth
            .session_from_headers(headers, CUSTOMER_SESSION_COOKIE, CUSTOMER_SESSION_KIND)
            .expect("session info from cookie");
        ResetActionForm {
            csrf_token: session.csrf_token,
            confirmation_token: None,
        }
    }

    fn customer_headers_for_host(state: &State, npub: &str, host: &str) -> HeaderMap {
        let mut headers = customer_cookie_header(state, npub);
        headers.insert(header::HOST, host.parse().expect("host header"));
        headers
    }

    fn openclaw_launch_ticket_for_test(
        state: &State,
        npub: &str,
        agent_id: &str,
        vm_id: &str,
        ui_host: &str,
        exp: i64,
    ) -> String {
        state
            .admin_config
            .browser_auth
            .sign_payload(&OpenClawLaunchTicket {
                kind: OPENCLAW_LAUNCH_TICKET_KIND.to_string(),
                npub: npub.to_string(),
                agent_id: agent_id.to_string(),
                vm_id: vm_id.to_string(),
                ui_host: ui_host.to_string(),
                exp,
            })
            .expect("sign launch ticket")
    }

    fn openclaw_ui_headers_for_host(
        state: &State,
        npub: &str,
        agent_id: &str,
        vm_id: &str,
        host: &str,
    ) -> HeaderMap {
        let token = state
            .admin_config
            .browser_auth
            .sign_payload(&OpenClawUiSession {
                kind: OPENCLAW_UI_SESSION_KIND.to_string(),
                npub: npub.to_string(),
                agent_id: agent_id.to_string(),
                vm_id: vm_id.to_string(),
                ui_host: host.to_string(),
                exp: chrono::Utc::now().timestamp() + 600,
            })
            .expect("sign ui session");
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, host.parse().expect("host header"));
        headers.insert(
            header::COOKIE,
            format!("{OPENCLAW_UI_SESSION_COOKIE}={token}")
                .parse()
                .expect("cookie header"),
        );
        headers
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
        assert!(!body.contains("Open OpenClaw"));
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
        let events = recent_activity(&db_pool, &npub);
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
        let npub = generate_npub();
        upsert_allowlist(&db_pool, &npub, true);
        let headers = customer_cookie_header(&state, &npub);
        let form = customer_action_form(&state, &headers);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            &npub,
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

        let events = recent_activity(&db_pool, &npub);
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
        let form = customer_reset_form(&state, &headers);
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
        let events = recent_activity(&db_pool, &npub);
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].event_kind, "provision_accepted");
        assert_eq!(events[1].event_kind, "provision_requested");
        assert_eq!(events[2].event_kind, "reset_destroyed_old_vm");
        assert_eq!(events[3].event_kind, "reset_requested");

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn reset_requires_confirmation_when_backup_is_weak() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = generate_npub();
        upsert_allowlist(&db_pool, &npub, true);
        let headers = customer_cookie_header(&state, &npub);
        let form = customer_reset_form(&state, &headers);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            &npub,
            "agent-reset-weak",
            Some("vm-reset-weak"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        let (base_url, rx) = spawn_scripted_server(vec![
            (
                "200 OK",
                r#"{"id":"vm-reset-weak","status":"running","guest_ready":true}"#,
            ),
            (
                "200 OK",
                r#"{"vm_id":"vm-reset-weak","backup_host":"pika-build","durable_home_path":"/var/lib/microvms/vm-reset-weak/home","successful_backup_known":true,"freshness":"stale","latest_successful_backup_at":"2026-03-09T00:00:00Z","observed_at":"2026-03-09T00:00:00Z"}"#,
            ),
        ]);
        let _env = MicrovmEnvGuard::set(&base_url);

        let err = reset(Extension(state), request_context(), headers, Form(form))
            .await
            .expect_err("reset without confirmation should fail");
        assert_eq!(err.0, StatusCode::CONFLICT);

        let status_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured status request");
        assert_eq!(status_request.path, "/vms/vm-reset-weak");
        let backup_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured backup request");
        assert_eq!(backup_request.path, "/vms/vm-reset-weak/backup-status");

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn reset_rejects_when_no_managed_environment_exists() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = generate_npub();
        upsert_allowlist(&db_pool, &npub, true);
        let headers = customer_cookie_header(&state, &npub);
        let form = customer_reset_form(&state, &headers);

        let err = reset(Extension(state), request_context(), headers, Form(form))
            .await
            .expect_err("reset without an environment should fail");
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert!(err.1.contains("requires a current managed environment"));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn reset_rejects_while_initial_vm_assignment_is_inflight() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = generate_npub();
        upsert_allowlist(&db_pool, &npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            &npub,
            "agent-reset-inflight",
            None,
            AGENT_PHASE_CREATING,
        )
        .expect("seed inflight creating agent");
        let headers = customer_cookie_header(&state, &npub);
        let form = customer_reset_form(&state, &headers);

        let err = reset(Extension(state), request_context(), headers, Form(form))
            .await
            .expect_err("reset should stay locked during inflight VM assignment");
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert!(err.1.contains("still in flight"));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn reset_confirm_page_renders_for_weak_backup() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = generate_npub();
        upsert_allowlist(&db_pool, &npub, true);
        let headers = customer_cookie_header(&state, &npub);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            &npub,
            "agent-reset-confirm",
            Some("vm-reset-confirm"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        let (base_url, _rx) = spawn_scripted_server(vec![
            (
                "200 OK",
                r#"{"id":"vm-reset-confirm","status":"running","guest_ready":true}"#,
            ),
            (
                "200 OK",
                r#"{"vm_id":"vm-reset-confirm","backup_host":"pika-build","durable_home_path":"/var/lib/microvms/vm-reset-confirm/home","successful_backup_known":false,"freshness":"missing","latest_successful_backup_at":null,"observed_at":null}"#,
            ),
        ]);
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = reset_confirm_page(Extension(state), request_context(), headers)
            .await
            .expect("confirm page response");
        let body = response_body_string(response).await;
        assert!(body.contains("Confirm Destructive Reset"));
        assert!(body.contains("Reset Without Recent Backup"));
        assert!(body.contains("No successful durable-home backup is known yet"));

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn reset_accepts_confirmation_ticket_when_backup_is_weak() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = generate_npub();
        upsert_allowlist(&db_pool, &npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        AgentInstance::create(
            &mut conn,
            &npub,
            "agent-reset-confirmed",
            Some("vm-reset-confirmed"),
            AGENT_PHASE_READY,
        )
        .expect("seed ready agent");
        let headers = customer_cookie_header(&state, &npub);
        let confirmation_token = issue_reset_confirmation_ticket(
            &state,
            &npub,
            "agent-reset-confirmed",
            Some("vm-reset-confirmed"),
        )
        .expect("issue confirmation ticket");
        let form = ResetActionForm {
            csrf_token: customer_reset_form(&state, &headers).csrf_token,
            confirmation_token: Some(confirmation_token),
        };
        let (base_url, rx) = spawn_scripted_server(vec![
            (
                "200 OK",
                r#"{"id":"vm-reset-confirmed","status":"running","guest_ready":true}"#,
            ),
            (
                "200 OK",
                r#"{"vm_id":"vm-reset-confirmed","backup_host":"pika-build","durable_home_path":"/var/lib/microvms/vm-reset-confirmed/home","successful_backup_known":true,"freshness":"stale","latest_successful_backup_at":"2026-03-09T00:00:00Z","observed_at":"2026-03-09T00:00:00Z"}"#,
            ),
            ("204 No Content", ""),
            ("200 OK", r#"{"id":"vm-reset-fresh","status":"starting"}"#),
        ]);
        let _env = MicrovmEnvGuard::set(&base_url);

        let response = reset(Extension(state), request_context(), headers, Form(form))
            .await
            .expect("confirmed reset response");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let status_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured status request");
        assert_eq!(status_request.path, "/vms/vm-reset-confirmed");
        let backup_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured backup request");
        assert_eq!(backup_request.path, "/vms/vm-reset-confirmed/backup-status");
        let delete_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured delete request");
        assert_eq!(delete_request.path, "/vms/vm-reset-confirmed");
        let create_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured create request");
        assert_eq!(create_request.path, "/vms");

        clear_test_database(&db_pool);
    }

    #[tokio::test]
    async fn reset_rejects_confirmation_ticket_for_changed_environment() {
        let _guard = serial_test_guard();
        let Some(db_pool) = init_test_db_pool() else {
            return;
        };
        clear_test_database(&db_pool);
        let state = test_state(db_pool.clone());
        let npub = generate_npub();
        upsert_allowlist(&db_pool, &npub, true);
        let mut conn = db_pool.get().expect("get seed connection");
        let original = AgentInstance::create(
            &mut conn,
            &npub,
            "agent-reset-original",
            Some("vm-reset-original"),
            AGENT_PHASE_READY,
        )
        .expect("seed original ready agent");
        let confirmation_token = issue_reset_confirmation_ticket(
            &state,
            &npub,
            &original.agent_id,
            original.vm_id.as_deref(),
        )
        .expect("issue original confirmation ticket");
        AgentInstance::update_phase(&mut conn, &original.agent_id, AGENT_PHASE_ERROR, None)
            .expect("retire original agent");
        AgentInstance::create(
            &mut conn,
            &npub,
            "agent-reset-replacement",
            Some("vm-reset-replacement"),
            AGENT_PHASE_READY,
        )
        .expect("seed replacement ready agent");
        let headers = customer_cookie_header(&state, &npub);
        let form = ResetActionForm {
            csrf_token: customer_reset_form(&state, &headers).csrf_token,
            confirmation_token: Some(confirmation_token),
        };
        let (base_url, rx) = spawn_scripted_server(vec![
            (
                "200 OK",
                r#"{"id":"vm-reset-replacement","status":"running","guest_ready":true}"#,
            ),
            (
                "200 OK",
                r#"{"vm_id":"vm-reset-replacement","backup_host":"pika-build","durable_home_path":"/var/lib/microvms/vm-reset-replacement/home","successful_backup_known":true,"freshness":"stale","latest_successful_backup_at":"2026-03-09T00:00:00Z","observed_at":"2026-03-09T00:00:00Z"}"#,
            ),
        ]);
        let _env = MicrovmEnvGuard::set(&base_url);

        let err = reset(Extension(state), request_context(), headers, Form(form))
            .await
            .expect_err("stale confirmation ticket should fail");
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert!(err.1.contains("no longer matches"));

        let status_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured status request");
        assert_eq!(status_request.path, "/vms/vm-reset-replacement");
        let backup_request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured backup request");
        assert_eq!(
            backup_request.path,
            "/vms/vm-reset-replacement/backup-status"
        );

        clear_test_database(&db_pool);
    }
}
