use std::time::Duration;

use base64::Engine;
use nostr_sdk::prelude::{EventBuilder, Keys, Kind, Tag, TagKind};
use pika_cloud::{AgentProvisionRequest, AgentStartupPhase, IncusProvisionParams};
use reqwest::Method;
use serde::Deserialize;

use super::*;

const DEFAULT_AGENT_API_URL: &str = "https://api.pikachat.org";
const AGENT_POLL_MAX_ATTEMPTS: u32 = 45;
const AGENT_POLL_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AgentAppState {
    Creating,
    Ready,
    Error,
}

#[derive(Debug, Deserialize)]
struct AgentStateResponse {
    agent_id: String,
    state: AgentAppState,
    #[serde(default = "default_agent_startup_phase")]
    startup_phase: AgentStartupPhase,
}

#[derive(Debug, Deserialize)]
struct AgentErrorResponse {
    error: String,
}

#[derive(Debug)]
enum AgentFlowError {
    Unauthorized,
    NotWhitelisted,
    AgentNotFound,
    Timeout,
    InvalidResponse,
    SigningFailed,
    Remote(String),
    Transport(String),
}

impl AgentFlowError {
    fn to_user_message(&self) -> String {
        match self {
            Self::Unauthorized => "Agent auth failed. Please sign in again.".to_string(),
            Self::NotWhitelisted => "This account is not allowlisted for agents.".to_string(),
            Self::AgentNotFound => "Agent was not found after creation. Try again.".to_string(),
            Self::Timeout => "Agent is still starting. Try again in a moment.".to_string(),
            Self::InvalidResponse => "Agent API returned an invalid response.".to_string(),
            Self::SigningFailed => "Agent requires local key signer.".to_string(),
            Self::Remote(message) => format!("Agent request failed: {message}"),
            Self::Transport(message) => {
                format!("Network error while starting agent: {message}")
            }
        }
    }
}

fn endpoint(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim().trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn default_agent_startup_phase() -> AgentStartupPhase {
    AgentStartupPhase::ProvisioningVm
}

fn resolve_agent_api_url(
    config_agent_api_url: Option<&str>,
    env_agent_api_url: Option<&str>,
) -> String {
    for candidate in [config_agent_api_url, env_agent_api_url] {
        if let Some(url) = candidate.map(str::trim).filter(|url| !url.is_empty()) {
            return url.to_string();
        }
    }
    DEFAULT_AGENT_API_URL.to_string()
}

fn build_nip98_authorization_header_with_keys(
    keys: &Keys,
    method: &Method,
    url: &str,
) -> Option<String> {
    let event = EventBuilder::new(Kind::Custom(27235), "")
        .tags([
            Tag::custom(TagKind::custom("u"), [url]),
            Tag::custom(
                TagKind::custom("method"),
                [method.as_str().to_ascii_uppercase()],
            ),
        ])
        .sign_with_keys(keys)
        .ok()?;

    let payload = serde_json::to_vec(&event).ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(payload);
    Some(format!("Nostr {encoded}"))
}

async fn decode_error_code(response: reqwest::Response) -> Option<String> {
    let body = response.bytes().await.ok()?;
    let payload = serde_json::from_slice::<AgentErrorResponse>(&body).ok()?;
    let normalized = payload.error.trim().to_string();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

async fn send_agent_request(
    client: &reqwest::Client,
    keys: &Keys,
    method: Method,
    url: &str,
    body: Option<&serde_json::Value>,
) -> Result<reqwest::Response, AgentFlowError> {
    let auth = build_nip98_authorization_header_with_keys(keys, &method, url)
        .ok_or(AgentFlowError::SigningFailed)?;
    let mut request = client
        .request(method.clone(), url)
        .header("Authorization", auth)
        .header("Accept", "application/json");
    if method == Method::POST {
        let request_body = body.cloned().unwrap_or_else(|| serde_json::json!({}));
        request = request
            .header("Content-Type", "application/json")
            .json(&request_body);
    }
    request
        .send()
        .await
        .map_err(|err| AgentFlowError::Transport(err.to_string()))
}

async fn ensure_agent(
    client: &reqwest::Client,
    keys: &Keys,
    base_url: &str,
    _agent_kind: crate::state::AgentKind,
) -> Result<(), AgentFlowError> {
    let body = serde_json::to_value(internal_managed_agent_request())
        .map_err(|_| AgentFlowError::InvalidResponse)?;
    let response = send_agent_request(
        client,
        keys,
        Method::POST,
        &endpoint(base_url, "/v1/agents/ensure"),
        Some(&body),
    )
    .await?;
    match response.status().as_u16() {
        202 => Ok(()),
        401 => Err(AgentFlowError::Unauthorized),
        403 => Err(AgentFlowError::NotWhitelisted),
        409 => {
            let code = decode_error_code(response).await.unwrap_or_default();
            if code == "agent_exists" {
                Ok(())
            } else {
                Err(AgentFlowError::Remote(if code.is_empty() {
                    "conflict".to_string()
                } else {
                    code
                }))
            }
        }
        status => {
            let code = decode_error_code(response).await.unwrap_or_default();
            let detail = if code.is_empty() {
                format!("http {status}")
            } else {
                format!("{code} (http {status})")
            };
            Err(AgentFlowError::Remote(detail))
        }
    }
}

async fn get_my_agent(
    client: &reqwest::Client,
    keys: &Keys,
    base_url: &str,
) -> Result<AgentStateResponse, AgentFlowError> {
    let response = send_agent_request(
        client,
        keys,
        Method::GET,
        &endpoint(base_url, "/v1/agents/me"),
        None,
    )
    .await?;
    match response.status().as_u16() {
        200 => response
            .json::<AgentStateResponse>()
            .await
            .map_err(|_| AgentFlowError::InvalidResponse),
        401 => Err(AgentFlowError::Unauthorized),
        403 => Err(AgentFlowError::NotWhitelisted),
        404 => {
            let code = decode_error_code(response).await.unwrap_or_default();
            if code == "agent_not_found" || code.is_empty() {
                Err(AgentFlowError::AgentNotFound)
            } else {
                Err(AgentFlowError::Remote(code))
            }
        }
        status => {
            let code = decode_error_code(response).await.unwrap_or_default();
            let detail = if code.is_empty() {
                format!("http {status}")
            } else {
                format!("{code} (http {status})")
            };
            Err(AgentFlowError::Remote(detail))
        }
    }
}

async fn recover_my_agent(
    client: &reqwest::Client,
    keys: &Keys,
    base_url: &str,
    _agent_kind: crate::state::AgentKind,
) -> Result<AgentStateResponse, AgentFlowError> {
    let body = serde_json::to_value(internal_managed_agent_request())
        .map_err(|_| AgentFlowError::InvalidResponse)?;
    let response = send_agent_request(
        client,
        keys,
        Method::POST,
        &endpoint(base_url, "/v1/agents/me/recover"),
        Some(&body),
    )
    .await?;
    match response.status().as_u16() {
        200 => response
            .json::<AgentStateResponse>()
            .await
            .map_err(|_| AgentFlowError::InvalidResponse),
        401 => Err(AgentFlowError::Unauthorized),
        403 => Err(AgentFlowError::NotWhitelisted),
        404 => Err(AgentFlowError::AgentNotFound),
        503 => Err(AgentFlowError::Remote("recover_failed".to_string())),
        status => {
            let code = decode_error_code(response).await.unwrap_or_default();
            let detail = if code.is_empty() {
                format!("http {status}")
            } else {
                format!("{code} (http {status})")
            };
            Err(AgentFlowError::Remote(detail))
        }
    }
}

async fn probe_agent_allowlist(
    client: &reqwest::Client,
    keys: &Keys,
    base_url: &str,
) -> Result<bool, AgentFlowError> {
    let response = send_agent_request(
        client,
        keys,
        Method::GET,
        &endpoint(base_url, "/v1/agents/me"),
        None,
    )
    .await?;
    match response.status().as_u16() {
        200 | 404 => Ok(true),
        401 | 403 => Ok(false),
        status => {
            let code = decode_error_code(response).await.unwrap_or_default();
            let detail = if code.is_empty() {
                format!("http {status}")
            } else {
                format!("{code} (http {status})")
            };
            Err(AgentFlowError::Remote(detail))
        }
    }
}

fn send_progress(
    tx: &flume::Sender<CoreMsg>,
    flow_token: u64,
    phase: crate::state::AgentProvisioningPhase,
    agent_npub: Option<String>,
) {
    let _ = tx.send(CoreMsg::Internal(Box::new(
        InternalEvent::AgentFlowProgress {
            flow_token,
            phase,
            agent_npub,
        },
    )));
}

fn internal_managed_agent_request() -> AgentProvisionRequest {
    AgentProvisionRequest {
        incus: IncusProvisionParams::default(),
    }
}

fn provisioning_phase_from_startup(
    startup_phase: AgentStartupPhase,
) -> crate::state::AgentProvisioningPhase {
    // Keep this app-side subset mapping in sync with `AgentProvisioningPhase`.
    // The app owns a couple of extra local phases after the shared startup
    // lifecycle reaches `Ready`.
    match startup_phase {
        AgentStartupPhase::Requested => crate::state::AgentProvisioningPhase::Requested,
        AgentStartupPhase::ProvisioningVm => crate::state::AgentProvisioningPhase::ProvisioningVm,
        AgentStartupPhase::BootingGuest => crate::state::AgentProvisioningPhase::BootingGuest,
        AgentStartupPhase::WaitingForServiceReady => {
            crate::state::AgentProvisioningPhase::WaitingForServiceReady
        }
        AgentStartupPhase::WaitingForKeypackagePublish => {
            crate::state::AgentProvisioningPhase::PublishingKeyPackage
        }
        AgentStartupPhase::Ready => {
            debug_assert!(
                false,
                "ready startup phase should be handled by AgentAppState::Ready before app mapping"
            );
            crate::state::AgentProvisioningPhase::CreatingChat
        }
        AgentStartupPhase::Failed => crate::state::AgentProvisioningPhase::Error,
    }
}

async fn run_agent_flow(
    client: reqwest::Client,
    keys: Keys,
    base_url: String,
    agent_kind: crate::state::AgentKind,
    tx: flume::Sender<CoreMsg>,
    flow_token: u64,
) -> Result<String, AgentFlowError> {
    ensure_agent(&client, &keys, &base_url, agent_kind).await?;

    send_progress(
        &tx,
        flow_token,
        crate::state::AgentProvisioningPhase::Requested,
        None,
    );

    for attempt in 1..=AGENT_POLL_MAX_ATTEMPTS {
        match get_my_agent(&client, &keys, &base_url).await {
            Ok(state) => match state.state {
                AgentAppState::Ready => return Ok(state.agent_id),
                AgentAppState::Creating => {
                    send_progress(
                        &tx,
                        flow_token,
                        provisioning_phase_from_startup(state.startup_phase),
                        Some(state.agent_id.clone()),
                    );
                    if attempt < AGENT_POLL_MAX_ATTEMPTS {
                        tokio::time::sleep(AGENT_POLL_DELAY).await;
                    }
                }
                AgentAppState::Error => {
                    send_progress(
                        &tx,
                        flow_token,
                        crate::state::AgentProvisioningPhase::Recovering,
                        Some(state.agent_id.clone()),
                    );
                    recover_my_agent(&client, &keys, &base_url, agent_kind).await?;
                    if attempt < AGENT_POLL_MAX_ATTEMPTS {
                        tokio::time::sleep(AGENT_POLL_DELAY).await;
                    }
                }
            },
            Err(AgentFlowError::AgentNotFound) => {
                if attempt < AGENT_POLL_MAX_ATTEMPTS {
                    tokio::time::sleep(AGENT_POLL_DELAY).await;
                    continue;
                }
                return Err(AgentFlowError::AgentNotFound);
            }
            Err(err) => return Err(err),
        }
    }
    Err(AgentFlowError::Timeout)
}

pub(super) fn provisioning_status_message(phase: &crate::state::AgentProvisioningPhase) -> String {
    match phase {
        crate::state::AgentProvisioningPhase::Ensuring => "Requesting agent...".to_string(),
        crate::state::AgentProvisioningPhase::Requested => "Request received...".to_string(),
        crate::state::AgentProvisioningPhase::ProvisioningVm => {
            "Provisioning managed environment...".to_string()
        }
        crate::state::AgentProvisioningPhase::BootingGuest => "Booting guest...".to_string(),
        crate::state::AgentProvisioningPhase::WaitingForServiceReady => {
            "Waiting for guest service to become ready...".to_string()
        }
        crate::state::AgentProvisioningPhase::Recovering => "Recovering agent...".to_string(),
        crate::state::AgentProvisioningPhase::PublishingKeyPackage => {
            "Publishing key package...".to_string()
        }
        crate::state::AgentProvisioningPhase::CreatingChat => {
            "Creating encrypted chat...".to_string()
        }
        crate::state::AgentProvisioningPhase::Error => "Error".to_string(),
    }
}

impl AppCore {
    pub(super) fn invalidate_agent_flow(&mut self) {
        self.agent_flow_token = self.agent_flow_token.wrapping_add(1);
        if let Some(handle) = self.agent_flow_task.take() {
            handle.abort();
        }
        self.agent_flow_start = None;
        self.set_busy(|b| b.starting_agent = false);
    }

    fn agent_api_url(&self) -> String {
        let env_agent_api_url = std::env::var("PIKA_AGENT_API_URL").ok();
        resolve_agent_api_url(
            self.config.agent_api_url.as_deref(),
            env_agent_api_url.as_deref(),
        )
    }

    pub(super) fn refresh_agent_allowlist(&mut self) {
        self.invalidate_agent_allowlist_probe();
        if !self.agent_allowlist_probe_enabled() {
            self.agent_allowlist_state = AgentAllowlistState::Unknown;
            self.sync_agent_menu_item_state();
            self.emit_state();
            return;
        }
        let (client, keys, base_url, pubkey) = match (&self.state.auth, self.session.as_ref()) {
            (
                AuthState::LoggedIn {
                    pubkey,
                    mode: AuthMode::LocalNsec,
                    ..
                },
                Some(sess),
            ) if self.network_enabled() => {
                let Some(local_keys) = sess.local_keys.clone() else {
                    self.agent_allowlist_state = AgentAllowlistState::Unknown;
                    self.sync_agent_menu_item_state();
                    self.emit_state();
                    return;
                };
                (
                    self.http_client.clone(),
                    local_keys,
                    self.agent_api_url(),
                    pubkey.clone(),
                )
            }
            _ => {
                self.agent_allowlist_state = AgentAllowlistState::Unknown;
                self.sync_agent_menu_item_state();
                self.emit_state();
                return;
            }
        };
        let token = self.agent_allowlist_probe_token;
        let tx = self.core_sender.clone();
        self.runtime.spawn(async move {
            let event = match probe_agent_allowlist(&client, &keys, &base_url).await {
                Ok(allowlisted) => InternalEvent::AgentAllowlistResolved {
                    token,
                    pubkey,
                    allowlisted: Some(allowlisted),
                    error: None,
                },
                Err(err) => InternalEvent::AgentAllowlistResolved {
                    token,
                    pubkey,
                    allowlisted: None,
                    error: Some(err.to_user_message()),
                },
            };
            let _ = tx.send(CoreMsg::Internal(Box::new(event)));
        });
    }

    pub(super) fn ensure_agent(&mut self) {
        let retry_kind = self
            .state
            .agent_provisioning
            .as_ref()
            .and_then(|provisioning| {
                (provisioning.phase == crate::state::AgentProvisioningPhase::Error)
                    .then_some(provisioning.agent_kind)
            });
        self.ensure_agent_kind(retry_kind.unwrap_or(crate::state::AgentKind::Openclaw));
    }

    pub(super) fn ensure_agent_kind(&mut self, agent_kind: crate::state::AgentKind) {
        if !self.is_logged_in() {
            self.toast("Please log in first");
            return;
        }
        let Some(sess) = self.session.as_ref() else {
            self.toast("Please log in first");
            return;
        };
        if sess.local_keys.is_none()
            || !matches!(
                self.state.auth,
                AuthState::LoggedIn {
                    mode: AuthMode::LocalNsec,
                    ..
                }
            )
        {
            self.toast("Agent requires local key signer");
            return;
        }
        if self.state.busy.starting_agent {
            return;
        }
        match self.agent_allowlist_state {
            AgentAllowlistState::Allowlisted => {}
            AgentAllowlistState::NotAllowlisted => {
                self.toast("This account is not allowlisted for agents.");
                return;
            }
            AgentAllowlistState::Unknown => {
                self.refresh_agent_allowlist();
                self.toast("Checking agent access. Try again in a moment.");
                return;
            }
        }
        if !self.network_enabled() {
            self.toast("Network disabled");
            return;
        }

        // Drop stale completed handles before starting a new flow.
        if let Some(handle) = self.agent_flow_task.take() {
            if !handle.is_finished() {
                self.agent_flow_task = Some(handle);
                return;
            }
        }

        let (client, keys, base_url, tx) = (
            self.http_client.clone(),
            sess.local_keys.clone().expect("checked local keys above"),
            self.agent_api_url(),
            self.core_sender.clone(),
        );

        self.agent_flow_token = self.agent_flow_token.wrapping_add(1);
        let flow_token = self.agent_flow_token;
        self.set_busy(|b| b.starting_agent = true);
        self.agent_flow_start = Some(std::time::Instant::now());

        self.state.agent_provisioning = Some(crate::state::AgentProvisioningState {
            agent_kind,
            phase: crate::state::AgentProvisioningPhase::Ensuring,
            agent_npub: None,
            status_message: "Requesting agent...".to_string(),
            elapsed_secs: 0,
        });
        // Only push the screen if it isn't already on the stack (e.g. retry from error state).
        let already_on_stack = self
            .state
            .router
            .screen_stack
            .iter()
            .any(|s| matches!(s, crate::state::Screen::AgentProvisioning));
        if !already_on_stack {
            self.push_screen(crate::state::Screen::AgentProvisioning);
        }
        self.emit_state();

        let progress_tx = tx.clone();
        let handle = self.runtime.spawn(async move {
            let event =
                match run_agent_flow(client, keys, base_url, agent_kind, progress_tx, flow_token)
                    .await
                {
                    Ok(agent_id) => InternalEvent::AgentFlowCompleted {
                        flow_token,
                        agent_id: Some(agent_id),
                        error: None,
                    },
                    Err(err) => InternalEvent::AgentFlowCompleted {
                        flow_token,
                        agent_id: None,
                        error: Some(err.to_user_message()),
                    },
                };
            let _ = tx.send(CoreMsg::Internal(Box::new(event)));
        });
        self.agent_flow_task = Some(handle);
    }

    pub(super) fn handle_agent_flow_progress(
        &mut self,
        flow_token: u64,
        phase: crate::state::AgentProvisioningPhase,
        agent_npub: Option<String>,
    ) {
        if flow_token != self.agent_flow_token {
            return;
        }

        let elapsed_secs = self
            .agent_flow_start
            .map(|start| start.elapsed().as_secs() as u32)
            .unwrap_or(0);
        let status_message = provisioning_status_message(&phase);

        self.state.agent_provisioning = Some(crate::state::AgentProvisioningState {
            agent_kind: self
                .state
                .agent_provisioning
                .as_ref()
                .map(|state| state.agent_kind)
                .unwrap_or(crate::state::AgentKind::Openclaw),
            phase,
            agent_npub,
            status_message,
            elapsed_secs,
        });
        self.emit_state();
    }

    pub(super) fn handle_agent_flow_completed(
        &mut self,
        flow_token: u64,
        agent_id: Option<String>,
        error: Option<String>,
    ) {
        if flow_token != self.agent_flow_token {
            return;
        }
        self.agent_flow_task = None;
        self.agent_flow_start = None;

        if !self.is_logged_in() {
            self.set_busy(|b| b.starting_agent = false);
            self.state.agent_provisioning = None;
            return;
        }

        if let Some(agent_id) = agent_id {
            // Update provisioning phase to CreatingChat before opening the chat.
            self.handle_agent_flow_progress(
                flow_token,
                crate::state::AgentProvisioningPhase::CreatingChat,
                Some(agent_id.clone()),
            );
            if let Err(message) = self.open_or_create_direct_chat_for_agent(&agent_id) {
                self.set_busy(|b| b.starting_agent = false);
                self.fail_direct_chat_creation(message);
            }
            return;
        }

        self.set_busy(|b| b.starting_agent = false);

        // Show error on the provisioning screen instead of a toast.
        let error_message = error.unwrap_or_else(|| "Agent flow failed".to_string());
        self.set_agent_provisioning_error(error_message);
    }

    fn open_or_create_direct_chat_for_agent(&mut self, peer_key: &str) -> Result<(), String> {
        let normalized = crate::normalize_peer_key(peer_key)
            .trim()
            .to_ascii_lowercase();
        if normalized.is_empty() || !crate::is_valid_peer_key(&normalized) {
            return Err("Agent returned an invalid identity".to_string());
        }

        if let Some(chat_id) = self.existing_direct_chat_for_peer(&normalized) {
            self.open_chat_screen(&chat_id);
            self.refresh_current_chat(&chat_id);
            self.unread_counts.insert(chat_id.clone(), 0);
            self.refresh_chat_list_from_storage();
            self.state.agent_provisioning = None;
            self.emit_router();
            self.set_busy(|b| b.starting_agent = false);
            return Ok(());
        }

        // Keep agent_provisioning alive so the UI continues to show the
        // CreatingChat phase during key-package publish and chat creation.
        // It gets cleared when the chat screen opens (open_chat_screen retains
        // filter removes AgentProvisioning) or on error.
        self.set_busy(|b| b.starting_agent = false);
        self.handle_action(AppAction::CreateChat {
            peer_npub: normalized,
        });
        Ok(())
    }

    pub(super) fn existing_direct_chat_for_peer(&self, peer_key: &str) -> Option<String> {
        self.state
            .chat_list
            .iter()
            .find(|chat| {
                if chat.is_group {
                    return false;
                }
                let Some(member) = chat.members.first() else {
                    return false;
                };
                let member_npub = crate::normalize_peer_key(&member.npub)
                    .trim()
                    .to_ascii_lowercase();
                let member_pubkey = member.pubkey.trim().to_ascii_lowercase();
                member_npub == peer_key || member_pubkey == peer_key
            })
            .map(|chat| chat.chat_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{anyhow, Context};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    type CapturedAgentRequest = (String, String, String);
    type AgentFlowMockJoinHandle = thread::JoinHandle<anyhow::Result<Vec<CapturedAgentRequest>>>;

    fn read_http_request(
        stream: &mut std::net::TcpStream,
    ) -> anyhow::Result<(String, Option<String>, String)> {
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .context("set read timeout")?;
        let mut buf = Vec::new();
        let mut header_end = None;
        while header_end.is_none() {
            let mut chunk = [0u8; 1024];
            let n = stream.read(&mut chunk).context("read request headers")?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(idx) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                header_end = Some(idx + 4);
            }
        }
        let header_end = header_end.ok_or_else(|| anyhow!("missing HTTP header terminator"))?;
        let header_text = String::from_utf8_lossy(&buf[..header_end]);
        let request_line = header_text.lines().next().unwrap_or_default().to_string();

        let mut content_length = 0usize;
        let mut authorization = None;
        for line in header_text.lines().skip(1) {
            let mut parts = line.splitn(2, ':');
            let name = parts.next().unwrap_or_default().trim();
            let value = parts.next().unwrap_or_default().trim().to_string();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse::<usize>().unwrap_or(0);
            }
            if name.eq_ignore_ascii_case("authorization") {
                authorization = Some(value);
            }
        }

        let already_body = buf.len().saturating_sub(header_end);
        let mut body = buf[header_end..].to_vec();
        if content_length > already_body {
            let mut remaining = vec![0u8; content_length - already_body];
            stream
                .read_exact(&mut remaining)
                .context("read request body")?;
            body.extend_from_slice(&remaining);
        }
        Ok((
            request_line,
            authorization,
            String::from_utf8_lossy(&body).to_string(),
        ))
    }

    fn respond_json(
        stream: &mut std::net::TcpStream,
        status: &str,
        body: &str,
    ) -> anyhow::Result<()> {
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .context("write response")?;
        stream.flush().context("flush response")?;
        Ok(())
    }

    fn spawn_agent_flow_mock_server_scripted(
        scripted: Vec<(&'static str, &'static str)>,
    ) -> anyhow::Result<(String, AgentFlowMockJoinHandle)> {
        let listener = TcpListener::bind("127.0.0.1:0").context("bind mock agent server")?;
        let addr = listener
            .local_addr()
            .context("read mock server local address")?;
        let base_url = format!("http://{addr}");
        let handle = thread::spawn(move || -> anyhow::Result<Vec<CapturedAgentRequest>> {
            let mut captured = Vec::new();
            for (expected_prefix, response_body) in scripted {
                let (mut stream, _) = listener.accept().context("accept request")?;
                let (request_line, authorization, body) = read_http_request(&mut stream)?;
                let authorization =
                    authorization.ok_or_else(|| anyhow!("missing Authorization header"))?;
                if !request_line.starts_with(expected_prefix) {
                    respond_json(
                        &mut stream,
                        "404 Not Found",
                        r#"{"error":"unexpected path"}"#,
                    )?;
                    return Err(anyhow!(
                        "unexpected request line: {request_line} (expected prefix {expected_prefix})"
                    ));
                }
                let status = if expected_prefix.starts_with("POST /v1/agents/ensure ") {
                    "202 Accepted"
                } else {
                    "200 OK"
                };
                respond_json(&mut stream, status, response_body)?;
                captured.push((request_line, authorization, body));
            }
            Ok(captured)
        });
        Ok((base_url, handle))
    }

    fn spawn_agent_flow_mock_server() -> anyhow::Result<(String, AgentFlowMockJoinHandle)> {
        spawn_agent_flow_mock_server_scripted(vec![
            (
                "POST /v1/agents/ensure ",
                r#"{"agent_id":"npub1temp","state":"creating","startup_phase":"requested"}"#,
            ),
            (
                "GET /v1/agents/me ",
                r#"{"agent_id":"npub1testagent","state":"ready","startup_phase":"ready"}"#,
            ),
        ])
    }

    #[test]
    fn endpoint_joins_without_double_slashes() {
        assert_eq!(
            endpoint("https://api.pikachat.org/", "/v1/agents/me"),
            "https://api.pikachat.org/v1/agents/me"
        );
    }

    #[test]
    fn resolve_agent_api_url_prefers_config_value() {
        let resolved = resolve_agent_api_url(
            Some("https://api.pikachat.org"),
            Some("https://env.example.com"),
        );
        assert_eq!(resolved, "https://api.pikachat.org");
    }

    #[test]
    fn resolve_agent_api_url_uses_env_when_config_missing() {
        let resolved = resolve_agent_api_url(None, Some("https://env.example.com"));
        assert_eq!(resolved, "https://env.example.com");
    }

    #[test]
    fn resolve_agent_api_url_falls_back_to_default_when_missing_or_blank() {
        assert_eq!(
            resolve_agent_api_url(None, None),
            DEFAULT_AGENT_API_URL.to_string()
        );
        assert_eq!(
            resolve_agent_api_url(Some("  "), Some("")),
            DEFAULT_AGENT_API_URL.to_string()
        );
    }

    #[test]
    fn internal_managed_agent_request_targets_incus_for_hosted_api() {
        let request = internal_managed_agent_request();
        assert_eq!(request.incus, IncusProvisionParams::default());
    }

    #[test]
    fn internal_managed_agent_request_targets_incus_for_custom_server_too() {
        let request = internal_managed_agent_request();
        assert_eq!(request.incus, IncusProvisionParams::default());
    }

    #[test]
    fn agent_flow_error_maps_to_human_messages() {
        let msg = AgentFlowError::NotWhitelisted.to_user_message();
        assert!(msg.contains("allowlisted"));
    }

    #[test]
    fn provisioning_status_message_uses_typed_startup_messages() {
        assert_eq!(
            provisioning_status_message(&crate::state::AgentProvisioningPhase::ProvisioningVm),
            "Provisioning managed environment..."
        );
        assert_eq!(
            provisioning_status_message(&crate::state::AgentProvisioningPhase::BootingGuest),
            "Booting guest..."
        );
        assert_eq!(
            provisioning_phase_from_startup(AgentStartupPhase::WaitingForKeypackagePublish),
            crate::state::AgentProvisioningPhase::PublishingKeyPackage
        );
        assert_eq!(
            provisioning_status_message(
                &crate::state::AgentProvisioningPhase::PublishingKeyPackage
            ),
            "Publishing key package..."
        );
    }

    #[test]
    fn legacy_agent_state_response_without_startup_phase_still_deserializes() {
        let response: AgentStateResponse =
            serde_json::from_str(r#"{"agent_id":"npub1legacy","state":"creating"}"#)
                .expect("legacy response should deserialize");
        assert_eq!(response.agent_id, "npub1legacy");
        assert!(matches!(response.state, AgentAppState::Creating));
        assert_eq!(response.startup_phase, AgentStartupPhase::ProvisioningVm);
    }

    #[tokio::test]
    async fn run_agent_flow_signs_requests_with_nip98_authorization() {
        let (base_url, handle) = spawn_agent_flow_mock_server().expect("start mock server");
        let client = reqwest::Client::new();
        let keys = Keys::generate();

        let (tx, _rx) = flume::unbounded();
        let agent_id = run_agent_flow(
            client,
            keys,
            base_url,
            crate::state::AgentKind::Openclaw,
            tx,
            1,
        )
        .await
        .expect("run agent flow");
        assert_eq!(agent_id, "npub1testagent");

        let captured = handle
            .join()
            .map_err(|_| anyhow!("mock server thread panicked"))
            .and_then(|result| result)
            .expect("collect captured requests");
        assert_eq!(captured.len(), 2);
        assert!(captured[0].0.starts_with("POST /v1/agents/ensure "));
        assert!(captured[1].0.starts_with("GET /v1/agents/me "));
        assert!(captured[0].1.starts_with("Nostr "));
        assert!(captured[1].1.starts_with("Nostr "));
        assert_eq!(captured[0].2.trim(), "{}");
    }

    #[tokio::test]
    async fn run_agent_flow_waits_for_ready_without_recovering_stuck_creating() {
        let (base_url, handle) = spawn_agent_flow_mock_server_scripted(vec![
            (
                "POST /v1/agents/ensure ",
                r#"{"agent_id":"npub1temp","state":"creating","startup_phase":"requested"}"#,
            ),
            (
                "GET /v1/agents/me ",
                r#"{"agent_id":"npub1temp","state":"creating","startup_phase":"provisioning_vm"}"#,
            ),
            (
                "GET /v1/agents/me ",
                r#"{"agent_id":"npub1temp","state":"creating","startup_phase":"booting_guest"}"#,
            ),
            (
                "GET /v1/agents/me ",
                r#"{"agent_id":"npub1testagent","state":"ready","startup_phase":"ready"}"#,
            ),
        ])
        .expect("start mock server");
        let client = reqwest::Client::new();
        let keys = Keys::generate();

        let (tx, _rx) = flume::unbounded();
        let agent_id = run_agent_flow(
            client,
            keys,
            base_url,
            crate::state::AgentKind::Openclaw,
            tx,
            1,
        )
        .await
        .expect("run agent flow");
        assert_eq!(agent_id, "npub1testagent");

        let captured = handle
            .join()
            .map_err(|_| anyhow!("mock server thread panicked"))
            .and_then(|result| result)
            .expect("collect captured requests");
        assert_eq!(captured.len(), 4);
        assert!(captured
            .iter()
            .all(|(request_line, _, _)| !request_line.starts_with("POST /v1/agents/me/recover ")));
    }

    #[tokio::test]
    async fn run_agent_flow_recover_uses_incus_request_shape() {
        let (base_url, handle) = spawn_agent_flow_mock_server_scripted(vec![
            (
                "POST /v1/agents/ensure ",
                r#"{"agent_id":"npub1temp","state":"creating","startup_phase":"requested"}"#,
            ),
            (
                "GET /v1/agents/me ",
                r#"{"agent_id":"npub1temp","state":"error","startup_phase":"failed"}"#,
            ),
            (
                "POST /v1/agents/me/recover ",
                r#"{"agent_id":"npub1temp","state":"creating","startup_phase":"provisioning_vm"}"#,
            ),
            (
                "GET /v1/agents/me ",
                r#"{"agent_id":"npub1recovered","state":"ready","startup_phase":"ready"}"#,
            ),
        ])
        .expect("start mock server");
        let client = reqwest::Client::new();
        let keys = Keys::generate();

        let (tx, _rx) = flume::unbounded();
        let agent_id = run_agent_flow(
            client,
            keys,
            base_url,
            crate::state::AgentKind::Openclaw,
            tx,
            1,
        )
        .await
        .expect("run agent flow");
        assert_eq!(agent_id, "npub1recovered");

        let captured = handle
            .join()
            .map_err(|_| anyhow!("mock server thread panicked"))
            .and_then(|result| result)
            .expect("collect captured requests");
        let recover = captured
            .iter()
            .find(|(request_line, _, _)| request_line.starts_with("POST /v1/agents/me/recover "))
            .expect("recover request");
        assert_eq!(recover.2.trim(), "{}");
    }

    #[tokio::test]
    async fn run_agent_flow_uses_incus_request_shape_for_custom_server() {
        let (base_url, handle) = spawn_agent_flow_mock_server().expect("start mock server");
        let client = reqwest::Client::new();
        let keys = Keys::generate();

        let (tx, _rx) = flume::unbounded();
        let agent_id = run_agent_flow(
            client,
            keys,
            base_url,
            crate::state::AgentKind::Openclaw,
            tx,
            1,
        )
        .await
        .expect("run agent flow");
        assert_eq!(agent_id, "npub1testagent");

        let captured = handle
            .join()
            .map_err(|_| anyhow!("mock server thread panicked"))
            .and_then(|result| result)
            .expect("collect captured requests");
        assert_eq!(captured[0].2.trim(), "{}");
    }
}
