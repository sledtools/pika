use anyhow::Context;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub struct WorkersClient {
    client: reqwest::Client,
    base_url: String,
    api_token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateAgentRequest {
    pub name: Option<String>,
    pub brain: String,
    pub relay_urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_secret_key_hex: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RelayProbe {
    pub relay: String,
    pub ok: bool,
    pub status_code: Option<u16>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentStatus {
    pub id: String,
    pub name: String,
    #[allow(dead_code)]
    pub brain: String,
    #[allow(dead_code)]
    pub status: String,
    #[allow(dead_code)]
    pub created_at_ms: u64,
    #[allow(dead_code)]
    pub updated_at_ms: u64,
    #[allow(dead_code)]
    pub ready_at_ms: u64,
    #[allow(dead_code)]
    pub relay_urls: Vec<String>,
    pub bot_pubkey: String,
    #[serde(default)]
    pub key_package_published_at_ms: Option<u64>,
    #[serde(default)]
    pub relay_probe: Option<RelayProbe>,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeProcessWelcomeRequest<'a> {
    group_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    wrapper_event_id_hex: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    welcome_event_json: Option<&'a str>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeProcessWelcomeResponse {
    #[allow(dead_code)]
    pub group_id: String,
    #[allow(dead_code)]
    pub created_group: bool,
    #[allow(dead_code)]
    pub processed_welcomes: u64,
    #[allow(dead_code)]
    pub mls_group_id_hex: String,
    #[allow(dead_code)]
    pub nostr_group_id_hex: String,
}

impl WorkersClient {
    pub fn from_env() -> anyhow::Result<Self> {
        let base_url = std::env::var("PIKA_WORKERS_BASE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8787".to_string());
        let api_token = std::env::var("PIKA_WORKERS_API_TOKEN")
            .ok()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());

        Self::with_api_token(base_url, api_token)
    }

    pub fn from_base_url(base_url: impl Into<String>) -> anyhow::Result<Self> {
        let api_token = std::env::var("PIKA_WORKERS_API_TOKEN")
            .ok()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
        Self::with_api_token(base_url, api_token)
    }

    fn with_api_token(
        base_url: impl Into<String>,
        api_token: Option<String>,
    ) -> anyhow::Result<Self> {
        let base_url = base_url.into();
        let base_url = base_url.trim_end_matches('/').to_string();
        if base_url.is_empty() {
            anyhow::bail!("workers base URL cannot be empty");
        }
        Ok(Self {
            client: reqwest::Client::new(),
            base_url,
            api_token,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn create_agent(&self, req: &CreateAgentRequest) -> anyhow::Result<AgentStatus> {
        let url = format!("{}/agents", self.base_url);
        let resp = self
            .request(self.client.post(url))
            .json(req)
            .send()
            .await
            .context("send workers create agent request")?;
        Self::decode_response(resp, "create workers agent").await
    }

    pub async fn get_agent(&self, agent_id: &str) -> anyhow::Result<AgentStatus> {
        let url = format!("{}/agents/{agent_id}", self.base_url);
        let resp = self
            .request(self.client.get(url))
            .send()
            .await
            .context("send workers get agent request")?;
        Self::decode_response(resp, "get workers agent").await
    }

    pub async fn runtime_process_welcome_event_json(
        &self,
        agent_id: &str,
        group_id: &str,
        wrapper_event_id_hex: Option<&str>,
        welcome_event_json: Option<&str>,
    ) -> anyhow::Result<RuntimeProcessWelcomeResponse> {
        let url = format!(
            "{}/agents/{agent_id}/runtime/process-welcome",
            self.base_url
        );
        let resp = self
            .request(self.client.post(url))
            .json(&RuntimeProcessWelcomeRequest {
                group_id,
                wrapper_event_id_hex,
                welcome_event_json,
            })
            .send()
            .await
            .context("send workers runtime process welcome request")?;
        Self::decode_response(resp, "runtime process welcome").await
    }

    fn request(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_token {
            Some(token) => builder.bearer_auth(token),
            None => builder,
        }
    }

    async fn decode_response<T: for<'de> Deserialize<'de>>(
        resp: reqwest::Response,
        action: &str,
    ) -> anyhow::Result<T> {
        let status = resp.status();
        if status.is_success() {
            return resp
                .json::<T>()
                .await
                .with_context(|| format!("decode {action} response"));
        }

        let body = resp.text().await.unwrap_or_default();
        if status == StatusCode::NOT_FOUND {
            anyhow::bail!("{action} failed: {status} (not found)");
        }
        if body.is_empty() {
            anyhow::bail!("{action} failed: {status}");
        }
        anyhow::bail!("{action} failed: {status} {body}");
    }
}
