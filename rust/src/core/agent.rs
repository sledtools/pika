use std::time::Duration;

use base64::Engine;
use nostr_sdk::prelude::{EventBuilder, Keys, Kind, Tag, TagKind};
use reqwest::Method;
use serde::Deserialize;

use super::*;

const DEFAULT_AGENT_API_URL: &str = "https://test.notifs.benthecarman.com";
const AGENT_POLL_MAX_ATTEMPTS: u32 = 60;
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
    AgentErrorState,
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
            Self::AgentErrorState => "Agent entered an error state. Try again.".to_string(),
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
) -> Result<reqwest::Response, AgentFlowError> {
    let auth = build_nip98_authorization_header_with_keys(keys, &method, url)
        .ok_or(AgentFlowError::SigningFailed)?;
    let mut request = client
        .request(method.clone(), url)
        .header("Authorization", auth)
        .header("Accept", "application/json");
    if method == Method::POST {
        request = request
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({}));
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
) -> Result<(), AgentFlowError> {
    let response = send_agent_request(
        client,
        keys,
        Method::POST,
        &endpoint(base_url, "/v1/agents/ensure"),
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

async fn run_agent_flow(
    client: reqwest::Client,
    keys: Keys,
    base_url: String,
) -> Result<String, AgentFlowError> {
    ensure_agent(&client, &keys, &base_url).await?;

    for attempt in 0..AGENT_POLL_MAX_ATTEMPTS {
        match get_my_agent(&client, &keys, &base_url).await {
            Ok(state) => match state.state {
                AgentAppState::Ready => return Ok(state.agent_id),
                AgentAppState::Creating => {
                    if attempt + 1 < AGENT_POLL_MAX_ATTEMPTS {
                        tokio::time::sleep(AGENT_POLL_DELAY).await;
                    }
                }
                AgentAppState::Error => return Err(AgentFlowError::AgentErrorState),
            },
            Err(AgentFlowError::AgentNotFound) => {
                if attempt + 1 < AGENT_POLL_MAX_ATTEMPTS {
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

impl AppCore {
    pub(super) fn invalidate_agent_flow(&mut self) {
        self.agent_flow_token = self.agent_flow_token.wrapping_add(1);
        if let Some(handle) = self.agent_flow_task.take() {
            handle.abort();
        }
        self.set_busy(|b| b.starting_agent = false);
    }

    fn agent_api_url(&self) -> String {
        let env_agent_api_url = std::env::var("PIKA_AGENT_API_URL").ok();
        resolve_agent_api_url(
            self.config.agent_api_url.as_deref(),
            env_agent_api_url.as_deref(),
        )
    }

    pub(super) fn ensure_agent(&mut self) {
        if !self.is_logged_in() {
            self.toast("Please log in first");
            return;
        }
        if self.state.busy.starting_agent {
            return;
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

        let (client, keys, base_url, tx) = {
            let Some(sess) = self.session.as_ref() else {
                self.toast("Please log in first");
                return;
            };
            let Some(local_keys) = sess.local_keys.clone() else {
                self.toast("Agent requires local key signer");
                return;
            };
            (
                self.http_client.clone(),
                local_keys,
                self.agent_api_url(),
                self.core_sender.clone(),
            )
        };

        self.agent_flow_token = self.agent_flow_token.wrapping_add(1);
        let flow_token = self.agent_flow_token;
        self.set_busy(|b| b.starting_agent = true);

        let handle = self.runtime.spawn(async move {
            let event = match run_agent_flow(client, keys, base_url).await {
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

        if !self.is_logged_in() {
            self.set_busy(|b| b.starting_agent = false);
            return;
        }

        if let Some(agent_id) = agent_id {
            if let Err(message) = self.open_or_create_direct_chat_for_agent(&agent_id) {
                self.set_busy(|b| b.starting_agent = false);
                self.toast(message);
            }
            return;
        }

        self.set_busy(|b| b.starting_agent = false);
        if let Some(message) = error {
            self.toast(message);
        } else {
            self.toast("Agent flow failed");
        }
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
            self.emit_router();
            self.set_busy(|b| b.starting_agent = false);
            return Ok(());
        }

        self.set_busy(|b| b.starting_agent = false);
        self.handle_action(AppAction::CreateChat {
            peer_npub: normalized,
        });
        Ok(())
    }

    fn existing_direct_chat_for_peer(&self, peer_key: &str) -> Option<String> {
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

    type CapturedAgentRequest = (String, String);
    type AgentFlowMockJoinHandle = thread::JoinHandle<anyhow::Result<Vec<CapturedAgentRequest>>>;

    fn read_http_request(
        stream: &mut std::net::TcpStream,
    ) -> anyhow::Result<(String, Option<String>)> {
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
        if content_length > already_body {
            let mut remaining = vec![0u8; content_length - already_body];
            stream
                .read_exact(&mut remaining)
                .context("read request body")?;
        }
        Ok((request_line, authorization))
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

    fn spawn_agent_flow_mock_server() -> anyhow::Result<(String, AgentFlowMockJoinHandle)> {
        let listener = TcpListener::bind("127.0.0.1:0").context("bind mock agent server")?;
        let addr = listener
            .local_addr()
            .context("read mock server local address")?;
        let base_url = format!("http://{addr}");
        let handle = thread::spawn(move || -> anyhow::Result<Vec<CapturedAgentRequest>> {
            let mut captured = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().context("accept request")?;
                let (request_line, authorization) = read_http_request(&mut stream)?;
                let authorization =
                    authorization.ok_or_else(|| anyhow!("missing Authorization header"))?;
                if request_line.starts_with("POST /v1/agents/ensure ") {
                    respond_json(
                        &mut stream,
                        "202 Accepted",
                        r#"{"agent_id":"npub1temp","state":"creating"}"#,
                    )?;
                } else if request_line.starts_with("GET /v1/agents/me ") {
                    respond_json(
                        &mut stream,
                        "200 OK",
                        r#"{"agent_id":"npub1testagent","state":"ready"}"#,
                    )?;
                } else {
                    respond_json(
                        &mut stream,
                        "404 Not Found",
                        r#"{"error":"unexpected path"}"#,
                    )?;
                    return Err(anyhow!("unexpected request line: {request_line}"));
                }
                captured.push((request_line, authorization));
            }
            Ok(captured)
        });
        Ok((base_url, handle))
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
    fn agent_flow_error_maps_to_human_messages() {
        let msg = AgentFlowError::NotWhitelisted.to_user_message();
        assert!(msg.contains("allowlisted"));
    }

    #[tokio::test]
    async fn run_agent_flow_signs_requests_with_nip98_authorization() {
        let (base_url, handle) = spawn_agent_flow_mock_server().expect("start mock server");
        let client = reqwest::Client::new();
        let keys = Keys::generate();

        let agent_id = run_agent_flow(client, keys, base_url)
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
    }
}
