use askama::Template;
use axum::extract::Form;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Json};

use crate::agent_api::{
    list_recent_managed_environment_events, load_managed_environment_backup_status,
    load_managed_environment_status, provision_managed_environment_if_missing,
    recover_agent_for_owner, reset_agent_for_owner, AgentApiError,
    ManagedEnvironmentBackupFreshness, ManagedEnvironmentBackupStatus, ManagedEnvironmentStatus,
};
use crate::browser_auth::BrowserAuthConfig;
use crate::models::agent_allowlist::AgentAllowlistEntry;
use crate::models::managed_environment_event::ManagedEnvironmentEvent;
use crate::nostr_auth::{expected_host_from_headers, verify_nip98_event};
use crate::{RequestContext, State};
use pika_agent_control_plane::{IncusProvisionParams, ManagedVmProvisionParams};

mod openclaw;

const CUSTOMER_SESSION_COOKIE: &str = "pika_customer_session";
const CUSTOMER_SESSION_TTL_SECS: i64 = 8 * 60 * 60;
const CUSTOMER_CHALLENGE_KIND: &str = "customer_dashboard_challenge";
const CUSTOMER_SESSION_KIND: &str = "customer_dashboard_session";
const RECENT_ACTIVITY_LIMIT: i64 = 20;
const RESET_CONFIRMATION_KIND: &str = "customer_reset_confirmation";
const RESET_CONFIRMATION_TTL_SECS: i64 = 15 * 60;

pub(crate) use openclaw::{
    openclaw_launch, openclaw_launch_exchange, openclaw_proxy, OPENCLAW_INTERNAL_LAUNCH_PATH,
    OPENCLAW_INTERNAL_PROXY_PATH, OPENCLAW_INTERNAL_PROXY_PREFIX,
};

#[derive(Debug, serde::Deserialize)]
pub struct VerifyRequest {
    event: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_api_v1_contract::AgentAppState;
    use askama::Template;
    use pika_agent_control_plane::AgentStartupPhase;

    fn test_backup_status(
        freshness: ManagedEnvironmentBackupFreshness,
    ) -> ManagedEnvironmentBackupStatus {
        ManagedEnvironmentBackupStatus {
            freshness,
            backup_target: Some("default/pika-agent-demo-state".to_string()),
            backup_target_label: "Persistent State Volume".to_string(),
            latest_recovery_point_name: Some("snap0".to_string()),
            latest_successful_backup_at: Some("2026-03-19T00:00:00Z".to_string()),
            status_copy: "Recovery points are stored on the Incus state volume.".to_string(),
            reset_requires_confirmation: true,
        }
    }

    #[test]
    fn customer_dashboard_managed_vm_policy_is_incus_only() {
        let policy = customer_dashboard_managed_vm_policy();
        assert_eq!(policy.incus, IncusProvisionParams::default());
    }

    #[test]
    fn openclaw_launchability_copy_is_openclaw_only() {
        assert_eq!(
            OpenClawLaunchability::RecoverFirst.status_copy(),
            "Recover or reprovision the managed environment before opening OpenClaw."
        );
        assert_eq!(
            OpenClawLaunchability::NotProvisioned.status_copy(),
            "Provision Managed OpenClaw before launching the built-in UI."
        );
    }

    #[test]
    fn dashboard_template_renders_incus_openclaw_only_copy() {
        let template = dashboard_template(
            AuthenticatedCustomer {
                npub: "npub1customer".to_string(),
                csrf_token: "csrf".to_string(),
            },
            ManagedEnvironmentStatus {
                row: None,
                app_state: Some(AgentAppState::Creating),
                startup_phase: Some(AgentStartupPhase::ProvisioningVm),
                environment_exists: false,
                status_copy: "No managed OpenClaw environment has been provisioned yet."
                    .to_string(),
            },
            test_backup_status(ManagedEnvironmentBackupFreshness::NotProvisioned),
            vec![],
        );
        let rendered = template.render().expect("render dashboard");
        assert_eq!(template.template_name, "OpenClaw");
        assert_eq!(template.substrate_label, "Incus");
        assert!(rendered.contains("Environment lane"));
        assert!(rendered.contains("OpenClaw"));
        assert!(!rendered.contains("microVM"));
        assert!(!rendered.contains("Pi runtime"));
    }
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct ActionForm {
    csrf_token: String,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct ResetActionForm {
    csrf_token: String,
    confirmation_token: Option<String>,
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

#[derive(Template)]
#[template(path = "customer/login.html")]
struct LoginTemplate;

#[derive(Template)]
#[template(path = "customer/dashboard.html")]
struct DashboardTemplate {
    owner_npub: String,
    template_name: String,
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
    control_loop_notice: String,
    has_control_loop_notice: bool,
    recover_action_label: &'static str,
    recover_semantics_copy: String,
    backup_state_label: &'static str,
    backup_state_tone: &'static str,
    backup_status_copy: String,
    backup_last_successful_at: String,
    backup_latest_recovery_point_name: String,
    has_backup_latest_recovery_point_name: bool,
    backup_target_label: String,
    backup_target: String,
    has_backup_target: bool,
    reset_requires_confirmation: bool,
    reset_safety_copy: String,
    can_launch_openclaw: bool,
    launch_status_copy: &'static str,
    substrate_label: String,
    recent_activity: Vec<DashboardActivityItem>,
    has_recent_activity: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum OpenClawLaunchability {
    Launchable,
    NotProvisioned,
    Provisioning,
    RecoverFirst,
    WaitingForReady,
}

impl OpenClawLaunchability {
    fn from_status(status: &ManagedEnvironmentStatus) -> Self {
        let row = status.row.as_ref();
        let inflight_without_vm = row
            .map(|row| {
                row.phase == crate::models::agent_instance::AGENT_PHASE_CREATING
                    && row.vm_id.is_none()
            })
            .unwrap_or(false);
        let recoverable_vm_exists = row.and_then(|row| row.vm_id.as_deref()).is_some();
        if status.app_state == Some(crate::agent_api_v1_contract::AgentAppState::Ready)
            && status.startup_phase == Some(pika_agent_control_plane::AgentStartupPhase::Ready)
            && recoverable_vm_exists
        {
            Self::Launchable
        } else if row.is_none() {
            Self::NotProvisioned
        } else if inflight_without_vm {
            Self::Provisioning
        } else if status.app_state == Some(crate::agent_api_v1_contract::AgentAppState::Error) {
            Self::RecoverFirst
        } else {
            Self::WaitingForReady
        }
    }

    fn can_launch(self) -> bool {
        matches!(self, Self::Launchable)
    }

    fn status_copy(self) -> &'static str {
        match self {
            Self::Launchable => {
                "Open the built-in OpenClaw UI on its own platform-managed origin. Launch uses a short-lived platform ticket and a scoped UI session rather than the dashboard cookie."
            }
            Self::NotProvisioned => {
                "Provision Managed OpenClaw before launching the built-in UI."
            }
            Self::Provisioning => {
                "OpenClaw launch unlocks after the current VM assignment finishes."
            }
            Self::RecoverFirst => {
                "Recover or reprovision the managed environment before opening OpenClaw."
            }
            Self::WaitingForReady => {
                "OpenClaw launch becomes available once the managed environment is fully ready."
            }
        }
    }
}

fn dashboard_substrate_label(
    _row: Option<&crate::models::agent_instance::AgentInstance>,
) -> &'static str {
    "Incus"
}

fn customer_dashboard_managed_vm_policy() -> ManagedVmProvisionParams {
    ManagedVmProvisionParams {
        incus: IncusProvisionParams::default(),
    }
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
    backup_latest_recovery_point_name: String,
    has_backup_latest_recovery_point_name: bool,
    backup_target_label: String,
    backup_target: String,
    has_backup_target: bool,
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
        Some(pika_agent_control_plane::AgentStartupPhase::WaitingForKeypackagePublish) => {
            "waiting_for_keypackage_publish"
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
            "destructive reset requires confirmation because recovery-point protection is not healthy"
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

fn dashboard_template(
    authenticated: AuthenticatedCustomer,
    status: ManagedEnvironmentStatus,
    backup: ManagedEnvironmentBackupStatus,
    recent_activity: Vec<DashboardActivityItem>,
) -> DashboardTemplate {
    let launchability = OpenClawLaunchability::from_status(&status);
    let row = status.row;
    let inflight_without_vm = row
        .as_ref()
        .map(|row| {
            row.phase == crate::models::agent_instance::AGENT_PHASE_CREATING && row.vm_id.is_none()
        })
        .unwrap_or(false);
    let recoverable_vm_exists = row.as_ref().and_then(|row| row.vm_id.as_deref()).is_some();
    let has_backup_target = backup.backup_target.is_some();
    let backup_target = backup
        .backup_target
        .unwrap_or_else(|| "not_available".to_string());
    let has_backup_latest_recovery_point_name = backup.latest_recovery_point_name.is_some();
    let backup_latest_recovery_point_name = backup
        .latest_recovery_point_name
        .unwrap_or_else(|| "not_available".to_string());
    DashboardTemplate {
        owner_npub: authenticated.npub,
        template_name: "OpenClaw".to_string(),
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
            "Provisioning is already in flight. Recovery and reset stay locked until the current VM assignment finishes.".to_string()
        } else {
            String::new()
        },
        has_control_loop_notice: inflight_without_vm,
        recover_action_label: if recoverable_vm_exists {
            "Recover Managed Environment"
        } else {
            "Provision Fresh Managed Environment"
        },
        recover_semantics_copy: if inflight_without_vm {
            "stays locked while the initial VM assignment is still in flight. Wait for the current create request to finish before retrying any destructive action.".to_string()
        } else if recoverable_vm_exists {
            "asks the control plane to restart the current Incus appliance around the same persistent state volume. If that VM is already gone, Recover falls back to provisioning a fresh Incus environment.".to_string()
        } else {
            "will provision a fresh Incus-backed Managed OpenClaw environment because no recoverable VM is available.".to_string()
        },
        backup_state_label: backup_state_label(backup.freshness),
        backup_state_tone: backup_state_tone(backup.freshness),
        backup_status_copy: backup.status_copy,
        backup_last_successful_at: format_rfc3339_timestamp(
            backup.latest_successful_backup_at.as_deref(),
        ),
        backup_latest_recovery_point_name,
        has_backup_latest_recovery_point_name,
        backup_target_label: backup.backup_target_label,
        backup_target,
        has_backup_target,
        reset_requires_confirmation: backup.reset_requires_confirmation,
        reset_safety_copy: if backup.reset_requires_confirmation {
            "Because recovery-point protection is stale, missing, or unavailable, destructive reset now requires an explicit confirmation step."
                .to_string()
        } else {
            "Recent recovery-point protection is healthy, so destructive reset remains available directly from the dashboard."
                .to_string()
        },
        can_launch_openclaw: launchability.can_launch(),
        launch_status_copy: launchability.status_copy(),
        substrate_label: dashboard_substrate_label(row.as_ref()).to_string(),
        has_recent_activity: !recent_activity.is_empty(),
        recent_activity,
    }
}

fn reset_confirm_template(
    authenticated: &AuthenticatedCustomer,
    backup: ManagedEnvironmentBackupStatus,
    confirmation_token: String,
) -> ResetConfirmTemplate {
    let has_backup_target = backup.backup_target.is_some();
    let backup_target = backup
        .backup_target
        .unwrap_or_else(|| "not_available".to_string());
    let has_backup_latest_recovery_point_name = backup.latest_recovery_point_name.is_some();
    let backup_latest_recovery_point_name = backup
        .latest_recovery_point_name
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
        backup_latest_recovery_point_name,
        has_backup_latest_recovery_point_name,
        backup_target_label: backup.backup_target_label,
        backup_target,
        has_backup_target,
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
    let requested = customer_dashboard_managed_vm_policy();

    provision_managed_environment_if_missing(
        &state,
        &authenticated.npub,
        &request_context.request_id,
        Some(&requested),
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
    let requested = customer_dashboard_managed_vm_policy();

    recover_agent_for_owner(
        &state,
        &authenticated.npub,
        &request_context.request_id,
        Some(&requested),
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
    let requested = customer_dashboard_managed_vm_policy();

    reset_agent_for_owner(
        &state,
        &authenticated.npub,
        &request_context.request_id,
        Some(&requested),
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

#[cfg(any())]
#[allow(clippy::await_holding_lock)]
mod removed_legacy_tests {
    use super::*;
    use std::collections::HashSet;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use axum::body::{to_bytes, Body};
    use axum::extract::ws::Message as AxumWsMessage;
    use axum::extract::{Path, Query, WebSocketUpgrade};
    use axum::http::{header, HeaderValue, Method, Request};
    use axum::routing::{any, get};
    use axum::Router;
    use diesel::prelude::*;
    use diesel::r2d2::{ConnectionManager, Pool};
    use diesel::PgConnection;
    use diesel_migrations::MigrationHarness;
    use futures::{SinkExt, StreamExt};
    use nostr_sdk::prelude::{EventBuilder, Keys, Kind, Tag, TagKind};
    use nostr_sdk::ToBech32;
    use pika_test_utils::{spawn_one_shot_server, CapturedRequest};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

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

    fn openclaw_proxy_request(method: Method, uri: &str, headers: &HeaderMap) -> Request<Body> {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .expect("proxy request");
        *request.headers_mut() = headers.clone();
        request
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
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        String::from_utf8(bytes.to_vec()).expect("utf8 response body")
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
}
