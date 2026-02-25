use std::collections::HashMap;

use anyhow::Context;
use serde::{Deserialize, Serialize};

pub struct FlyClient {
    client: reqwest::Client,
    api_token: String,
    app_name: String,
    api_base_url: String,
    region: String,
    image: String,
}

const DEFAULT_FLY_API_BASE_URL: &str = "https://api.machines.dev";

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
        let api_base_url =
            optional_non_empty_env("PIKA_FLY_API_BASE_URL", DEFAULT_FLY_API_BASE_URL);
        let region = optional_non_empty_env("FLY_BOT_REGION", "iad");
        let image = optional_non_empty_env("FLY_BOT_IMAGE", "registry.fly.io/pika-bot:latest");

        Ok(Self {
            client: reqwest::Client::new(),
            api_token,
            app_name,
            api_base_url,
            region,
            image,
        })
    }

    pub fn app_name(&self) -> &str {
        &self.app_name
    }

    fn base_url(&self) -> String {
        format!(
            "{}/v1/apps/{}",
            self.api_base_url.trim_end_matches('/'),
            self.app_name
        )
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[derive(Debug)]
    struct CapturedRequest {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: String,
    }

    fn spawn_one_shot_server(
        status_line: &str,
        response_body: &str,
    ) -> (String, mpsc::Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().expect("read mock server addr");
        let (tx, rx) = mpsc::channel();
        let status_line = status_line.to_string();
        let response_body = response_body.to_string();

        thread::spawn(move || {
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
        let mut headers = HashMap::new();
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

    fn test_client(base_url: String) -> FlyClient {
        FlyClient {
            client: reqwest::Client::new(),
            api_token: "fly-token".to_string(),
            app_name: "pika-test".to_string(),
            api_base_url: base_url,
            region: "iad".to_string(),
            image: "registry.fly.io/pika-bot:test".to_string(),
        }
    }

    #[tokio::test]
    async fn create_volume_contract_request_shape() {
        let (base_url, rx) = spawn_one_shot_server("200 OK", r#"{"id":"vol-123"}"#);
        let fly = test_client(base_url);

        let volume = fly
            .create_volume("state-volume")
            .await
            .expect("create volume succeeds");
        assert_eq!(volume.id, "vol-123");

        let req = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured request");
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/v1/apps/pika-test/volumes");
        assert_eq!(
            req.headers.get("authorization").map(String::as_str),
            Some("Bearer fly-token")
        );

        let json: Value = serde_json::from_str(&req.body).expect("parse json body");
        assert_eq!(json["name"], "state-volume");
        assert_eq!(json["region"], "iad");
        assert_eq!(json["size_gb"], 1);
    }

    #[tokio::test]
    async fn create_machine_contract_request_shape() {
        let (base_url, rx) =
            spawn_one_shot_server("200 OK", r#"{"id":"machine-abc","state":"started"}"#);
        let fly = test_client(base_url);

        let mut env = HashMap::new();
        env.insert("PIKA_OWNER_PUBKEY".to_string(), "pubkey123".to_string());

        let machine = fly
            .create_machine("bot-machine", "vol-123", env)
            .await
            .expect("create machine succeeds");
        assert_eq!(machine.id, "machine-abc");
        assert_eq!(machine.state, "started");

        let req = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured request");
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/v1/apps/pika-test/machines");
        assert_eq!(
            req.headers.get("authorization").map(String::as_str),
            Some("Bearer fly-token")
        );

        let json: Value = serde_json::from_str(&req.body).expect("parse json body");
        assert_eq!(json["name"], "bot-machine");
        assert_eq!(json["region"], "iad");
        assert_eq!(json["config"]["image"], "registry.fly.io/pika-bot:test");
        assert_eq!(json["config"]["guest"]["cpu_kind"], "shared");
        assert_eq!(json["config"]["guest"]["cpus"], 1);
        assert_eq!(json["config"]["guest"]["memory_mb"], 256);
        assert_eq!(json["config"]["mounts"][0]["volume"], "vol-123");
        assert_eq!(json["config"]["mounts"][0]["path"], "/app/state");
        assert_eq!(json["config"]["env"]["PIKA_OWNER_PUBKEY"], "pubkey123");
    }

    #[tokio::test]
    async fn create_volume_surfaces_error_body() {
        let (base_url, _rx) = spawn_one_shot_server("500 Internal Server Error", "no quota");
        let fly = test_client(base_url);

        let err = fly
            .create_volume("state-volume")
            .await
            .expect_err("expected create_volume failure");
        let msg = err.to_string();
        assert!(msg.contains("failed to create volume"));
        assert!(msg.contains("500 Internal Server Error"));
        assert!(msg.contains("no quota"));
    }
}
