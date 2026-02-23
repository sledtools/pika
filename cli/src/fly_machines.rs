use std::collections::HashMap;

use anyhow::Context;
use serde::{Deserialize, Serialize};

pub struct FlyClient {
    client: reqwest::Client,
    api_token: String,
    app_name: String,
    region: String,
    image: String,
}

#[derive(Debug, Serialize)]
struct CreateVolumeRequest {
    name: String,
    region: String,
    size_gb: u32,
}

#[derive(Debug, Deserialize)]
pub struct Volume {
    pub id: String,
}

#[derive(Debug, Serialize)]
struct CreateMachineRequest {
    name: String,
    region: String,
    config: MachineConfig,
}

#[derive(Debug, Serialize)]
struct MachineConfig {
    image: String,
    env: HashMap<String, String>,
    guest: GuestConfig,
    mounts: Vec<MachineMount>,
}

#[derive(Debug, Serialize)]
struct GuestConfig {
    cpu_kind: String,
    cpus: u32,
    memory_mb: u32,
}

#[derive(Debug, Serialize)]
struct MachineMount {
    volume: String,
    path: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Machine {
    pub id: String,
    #[serde(default)]
    pub state: String,
}

impl FlyClient {
    pub fn from_env() -> anyhow::Result<Self> {
        let api_token = required_non_empty_env("FLY_API_TOKEN")
            .context("FLY_API_TOKEN must be set (for example in .env)")?;
        let app_name = optional_non_empty_env("FLY_BOT_APP_NAME", "pika-bot");
        let region = optional_non_empty_env("FLY_BOT_REGION", "iad");
        let image = optional_non_empty_env("FLY_BOT_IMAGE", "registry.fly.io/pika-bot:latest");

        Ok(Self {
            client: reqwest::Client::new(),
            api_token,
            app_name,
            region,
            image,
        })
    }

    pub fn app_name(&self) -> &str {
        &self.app_name
    }

    fn base_url(&self) -> String {
        format!("https://api.machines.dev/v1/apps/{}", self.app_name)
    }

    pub async fn create_volume(&self, name: &str) -> anyhow::Result<Volume> {
        let url = format!("{}/volumes", self.base_url());
        let body = CreateVolumeRequest {
            name: name.to_string(),
            region: self.region.clone(),
            size_gb: 1,
        };
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_token)
            .json(&body)
            .send()
            .await
            .context("send create volume request")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("failed to create volume: {status} {text}");
        }
        resp.json().await.context("decode create volume response")
    }

    pub async fn create_machine(
        &self,
        name: &str,
        volume_id: &str,
        env: HashMap<String, String>,
    ) -> anyhow::Result<Machine> {
        let url = format!("{}/machines", self.base_url());
        let body = CreateMachineRequest {
            name: name.to_string(),
            region: self.region.clone(),
            config: MachineConfig {
                image: self.image.clone(),
                env,
                guest: GuestConfig {
                    cpu_kind: "shared".to_string(),
                    cpus: 1,
                    memory_mb: 256,
                },
                mounts: vec![MachineMount {
                    volume: volume_id.to_string(),
                    path: "/app/state".to_string(),
                }],
            },
        };

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_token)
            .json(&body)
            .send()
            .await
            .context("send create machine request")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("failed to create machine: {status} {text}");
        }
        resp.json().await.context("decode create machine response")
    }

    #[allow(dead_code)]
    pub async fn get_machine(&self, machine_id: &str) -> anyhow::Result<Machine> {
        let url = format!("{}/machines/{machine_id}", self.base_url());
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.api_token)
            .send()
            .await
            .context("send get machine request")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("failed to get machine: {status} {text}");
        }
        resp.json().await.context("decode get machine response")
    }
}

fn required_non_empty_env(key: &str) -> anyhow::Result<String> {
    let value = std::env::var(key).with_context(|| format!("{key} must be set"))?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{key} must be non-empty");
    }
    Ok(trimmed.to_string())
}

fn optional_non_empty_env(key: &str, default: &str) -> String {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => default.to_string(),
    }
}
