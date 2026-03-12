//! Generic ACP backend/session bridge for the daemon host.
//!
//! This is intentionally separate from the native daemon protocol. The daemon
//! remains the Marmot/Nostr host and can optionally drive an external
//! ACP-speaking agent backend over stdio JSON-RPC.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc, oneshot};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcpBackendConfig {
    pub exec_cmd: String,
    pub cwd: PathBuf,
}

impl AcpBackendConfig {
    pub fn new(exec_cmd: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            exec_cmd: exec_cmd.into(),
            cwd: cwd.into(),
        }
    }

    pub fn normalize(&self) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&self.cwd)
            .with_context(|| format!("create ACP cwd {}", self.cwd.display()))?;
        let cwd = self
            .cwd
            .canonicalize()
            .with_context(|| format!("canonicalize ACP cwd {}", self.cwd.display()))?;
        Ok(Self {
            exec_cmd: self.exec_cmd.clone(),
            cwd,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcpPromptResult {
    pub session_id: String,
    pub stop_reason: Option<String>,
    pub final_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcpTurnCompletion {
    pub conversation_id: String,
    pub result: Result<AcpPromptResult, String>,
}

#[derive(Clone, Debug)]
struct QueuedAcpPrompt {
    conversation_id: String,
    prompt: String,
}

#[derive(Clone)]
pub struct AcpBackendManager {
    session_manager: AcpSessionManager,
    queue_capacity: usize,
    workers: Arc<Mutex<HashMap<String, mpsc::Sender<QueuedAcpPrompt>>>>,
    completion_tx: mpsc::UnboundedSender<AcpTurnCompletion>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: u64,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct JsonRpcNotification {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    method: String,
    params: Value,
}

#[derive(Debug, Deserialize)]
struct SessionUpdateParams {
    #[serde(rename = "sessionId")]
    session_id: String,
    update: SessionUpdate,
}

#[derive(Debug, Deserialize)]
struct SessionUpdate {
    #[serde(rename = "sessionUpdate")]
    session_update: String,
    #[serde(default)]
    content: Option<SessionUpdateContent>,
}

#[derive(Debug, Deserialize)]
struct SessionUpdateContent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

struct AcpJsonRpcClient {
    stdin: Mutex<tokio::process::ChildStdin>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<anyhow::Result<Value>>>>,
    text_chunks: Mutex<HashMap<String, mpsc::UnboundedSender<String>>>,
}

impl AcpJsonRpcClient {
    async fn spawn(exec_cmd: &str) -> anyhow::Result<Arc<Self>> {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(exec_cmd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
        if std::env::var_os("PI_ACP_STARTUP_INFO").is_none() {
            command.env("PI_ACP_STARTUP_INFO", "false");
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("spawn ACP backend: {exec_cmd}"))?;

        let stdin = child.stdin.take().context("ACP backend stdin")?;
        let stdout = child.stdout.take().context("ACP backend stdout")?;
        let client = Arc::new(Self {
            stdin: Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            text_chunks: Mutex::new(HashMap::new()),
        });

        let read_client = client.clone();
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if let Err(err) = read_client.handle_line(&line).await {
                            read_client
                                .fail_all_pending(anyhow!("ACP decode failed: {err:#}"))
                                .await;
                            break;
                        }
                    }
                    Ok(None) => {
                        read_client
                            .fail_all_pending(anyhow!("ACP backend stdout closed"))
                            .await;
                        break;
                    }
                    Err(err) => {
                        read_client
                            .fail_all_pending(anyhow!("ACP backend read failed: {err:#}"))
                            .await;
                        break;
                    }
                }
            }
            let _ = child.wait().await;
        });

        Ok(client)
    }

    async fn initialize(&self) -> anyhow::Result<()> {
        let _ = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientInfo": {
                        "name": "pikachat-sidecar",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "clientCapabilities": {},
                }),
            )
            .await?;
        Ok(())
    }

    async fn request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&request).context("encode ACP request")?;
        let write_result = {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(line.as_bytes())
                .await
                .context("write ACP request")?;
            stdin.write_all(b"\n").await.context("write ACP newline")?;
            stdin.flush().await.context("flush ACP request")
        };
        if let Err(err) = write_result {
            self.pending.lock().await.remove(&id);
            return Err(err);
        }

        rx.await.context("await ACP response")?
    }

    async fn replace_text_chunk_sink(&self, session_id: &str) -> mpsc::UnboundedReceiver<String> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.text_chunks
            .lock()
            .await
            .insert(session_id.to_string(), tx);
        rx
    }

    async fn clear_text_chunk_sink(&self, session_id: &str) {
        self.text_chunks.lock().await.remove(session_id);
    }

    async fn handle_line(&self, line: &str) -> anyhow::Result<()> {
        let value: Value = serde_json::from_str(line).context("parse ACP JSON")?;
        if value.get("id").is_some() {
            let response: JsonRpcResponse =
                serde_json::from_value(value).context("decode ACP response")?;
            let pending = self.pending.lock().await.remove(&response.id);
            if let Some(tx) = pending {
                let result = match (response.result, response.error) {
                    (Some(result), None) => Ok(result),
                    (_, Some(error)) => Err(anyhow!("ACP error: {}", error.message)),
                    _ => Err(anyhow!("ACP response missing result/error")),
                };
                let _ = tx.send(result);
            }
            return Ok(());
        }

        if value.get("method").is_some() {
            let notification: JsonRpcNotification =
                serde_json::from_value(value).context("decode ACP notification")?;
            if notification.method != "session/update" {
                return Ok(());
            }
            let params: SessionUpdateParams =
                serde_json::from_value(notification.params).context("decode session/update")?;
            if params.update.session_update != "agent_message_chunk" {
                return Ok(());
            }
            let Some(content) = params.update.content else {
                return Ok(());
            };
            if content.kind != "text" {
                return Ok(());
            }
            let Some(text) = content.text else {
                return Ok(());
            };
            if let Some(tx) = self
                .text_chunks
                .lock()
                .await
                .get(&params.session_id)
                .cloned()
            {
                let _ = tx.send(text);
            }
        }

        Ok(())
    }

    async fn fail_all_pending(&self, err: anyhow::Error) {
        let mut pending = self.pending.lock().await;
        let senders = pending.drain().map(|(_, tx)| tx).collect::<Vec<_>>();
        let message = format!("{err:#}");
        drop(pending);
        for tx in senders {
            let _ = tx.send(Err(anyhow!(message.clone())));
        }
    }
}

#[derive(Clone)]
struct ManagedSession {
    session_id: String,
    prompt_lock: Arc<Mutex<()>>,
}

#[derive(Clone)]
pub struct AcpSessionManager {
    client: Arc<AcpJsonRpcClient>,
    cwd: PathBuf,
    sessions_by_conversation: Arc<Mutex<HashMap<String, ManagedSession>>>,
}

impl AcpSessionManager {
    pub async fn spawn(config: AcpBackendConfig) -> anyhow::Result<Self> {
        let config = config.normalize()?;
        let client = AcpJsonRpcClient::spawn(&config.exec_cmd).await?;
        client.initialize().await?;
        Ok(Self {
            client,
            cwd: config.cwd,
            sessions_by_conversation: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn prompt_conversation(
        &self,
        conversation_id: &str,
        message: &str,
    ) -> anyhow::Result<AcpPromptResult> {
        if message.trim().is_empty() {
            bail!("ACP prompt must not be empty");
        }

        let session = self.ensure_session(conversation_id).await?;
        let _prompt_guard = session.prompt_lock.lock().await;
        let mut chunks = self
            .client
            .replace_text_chunk_sink(&session.session_id)
            .await;
        let response = self
            .client
            .request(
                "session/prompt",
                json!({
                    "sessionId": session.session_id,
                    "prompt": [{
                        "type": "text",
                        "text": message,
                    }],
                }),
            )
            .await;
        self.client.clear_text_chunk_sink(&session.session_id).await;
        let response = response?;

        let mut final_text = String::new();
        while let Ok(chunk) = chunks.try_recv() {
            final_text.push_str(&chunk);
        }

        Ok(AcpPromptResult {
            session_id: session.session_id,
            stop_reason: response
                .get("stopReason")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            final_text,
        })
    }

    async fn ensure_session(&self, conversation_id: &str) -> anyhow::Result<ManagedSession> {
        if let Some(existing) = self
            .sessions_by_conversation
            .lock()
            .await
            .get(conversation_id)
            .cloned()
        {
            return Ok(existing);
        }

        let result = self
            .client
            .request(
                "session/new",
                json!({
                    "cwd": self.cwd,
                    "mcpServers": [],
                }),
            )
            .await?;
        let session_id = result
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow!("ACP session/new missing sessionId"))?
            .to_string();
        let managed = ManagedSession {
            session_id,
            prompt_lock: Arc::new(Mutex::new(())),
        };
        self.sessions_by_conversation
            .lock()
            .await
            .insert(conversation_id.to_string(), managed.clone());
        Ok(managed)
    }
}

impl AcpBackendManager {
    pub async fn spawn(
        config: AcpBackendConfig,
    ) -> anyhow::Result<(Self, mpsc::UnboundedReceiver<AcpTurnCompletion>)> {
        Self::spawn_with_queue_capacity(config, 8).await
    }

    async fn spawn_with_queue_capacity(
        config: AcpBackendConfig,
        queue_capacity: usize,
    ) -> anyhow::Result<(Self, mpsc::UnboundedReceiver<AcpTurnCompletion>)> {
        let session_manager = AcpSessionManager::spawn(config).await?;
        let (completion_tx, completion_rx) = mpsc::unbounded_channel();
        Ok((
            Self {
                session_manager,
                queue_capacity,
                workers: Arc::new(Mutex::new(HashMap::new())),
                completion_tx,
            },
            completion_rx,
        ))
    }

    pub async fn enqueue_prompt(&self, conversation_id: &str, prompt: &str) -> anyhow::Result<()> {
        if prompt.trim().is_empty() {
            bail!("ACP prompt must not be empty");
        }

        let sender = self.worker_sender(conversation_id).await;
        sender
            .try_send(QueuedAcpPrompt {
                conversation_id: conversation_id.to_string(),
                prompt: prompt.to_string(),
            })
            .map_err(|err| match err {
                mpsc::error::TrySendError::Full(_) => {
                    anyhow!("ACP queue full for conversation {conversation_id}")
                }
                mpsc::error::TrySendError::Closed(_) => {
                    anyhow!("ACP worker closed for conversation {conversation_id}")
                }
            })
    }

    async fn worker_sender(&self, conversation_id: &str) -> mpsc::Sender<QueuedAcpPrompt> {
        let mut workers = self.workers.lock().await;
        if let Some(existing) = workers.get(conversation_id) {
            return existing.clone();
        }

        let (tx, mut rx) = mpsc::channel::<QueuedAcpPrompt>(self.queue_capacity);
        let worker_tx: mpsc::Sender<QueuedAcpPrompt> = tx.clone();
        let session_manager = self.session_manager.clone();
        let completion_tx = self.completion_tx.clone();
        tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                let result = session_manager
                    .prompt_conversation(&job.conversation_id, &job.prompt)
                    .await
                    .map_err(|err| format!("{err:#}"));
                let _ = completion_tx.send(AcpTurnCompletion {
                    conversation_id: job.conversation_id,
                    result,
                });
            }
        });
        workers.insert(conversation_id.to_string(), tx);
        worker_tx
    }
}

pub fn default_acp_cwd(state_dir: &Path) -> PathBuf {
    state_dir.join("acp")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{Duration, timeout};

    fn fake_acp_script(log_path: &Path, delay_ms: u64) -> String {
        format!(
            r#"
import json
import sys
import time
from pathlib import Path

log_path = Path({log_path:?})
session_count = 0

for raw in sys.stdin:
    line = raw.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        print(json.dumps({{"jsonrpc":"2.0","id":msg["id"],"result":{{"protocolVersion":1}}}}), flush=True)
        continue
    if method == "session/new":
        session_count += 1
        session_id = f"s{{session_count}}"
        with log_path.open("a", encoding="utf-8") as fh:
            fh.write(session_id + "\n")
        print(json.dumps({{"jsonrpc":"2.0","id":msg["id"],"result":{{"sessionId":session_id}}}}), flush=True)
        continue
    if method == "session/prompt":
        session_id = msg["params"]["sessionId"]
        prompt = "".join(
            block.get("text", "")
            for block in msg["params"]["prompt"]
            if block.get("type") == "text"
        )
        with log_path.open("a", encoding="utf-8") as fh:
            fh.write(f"start:{{session_id}}:{{prompt}}\n")
        if prompt == "fail":
            print(json.dumps({{"jsonrpc":"2.0","id":msg["id"],"error":{{"code":-32001,"message":"prompt failed"}}}}), flush=True)
            with log_path.open("a", encoding="utf-8") as fh:
                fh.write(f"error:{{session_id}}:{{prompt}}\n")
            continue
        time.sleep({delay_ms} / 1000.0)
        for chunk in ("echo:", prompt):
            print(json.dumps({{
                "jsonrpc":"2.0",
                "method":"session/update",
                "params":{{
                    "sessionId":session_id,
                    "update":{{
                        "sessionUpdate":"agent_message_chunk",
                        "content":{{"type":"text","text":chunk}}
                    }}
                }}
            }}), flush=True)
        with log_path.open("a", encoding="utf-8") as fh:
            fh.write(f"end:{{session_id}}:{{prompt}}\n")
        print(json.dumps({{"jsonrpc":"2.0","id":msg["id"],"result":{{"stopReason":"end_turn"}}}}), flush=True)
        continue
    print(json.dumps({{"jsonrpc":"2.0","id":msg["id"],"error":{{"code":-32601,"message":"unknown method"}}}}), flush=True)
"#
        )
    }

    fn write_fake_acp_backend(temp: &tempfile::TempDir, delay_ms: u64) -> PathBuf {
        let script_path = temp.path().join("fake_acp.py");
        std::fs::write(
            &script_path,
            fake_acp_script(&temp.path().join("sessions.log"), delay_ms),
        )
        .expect("write fake ACP backend");
        script_path
    }

    #[tokio::test]
    async fn acp_session_manager_reuses_session_per_conversation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let script_path = write_fake_acp_backend(&temp, 0);
        let manager = AcpSessionManager::spawn(AcpBackendConfig::new(
            format!("python3 -u {}", script_path.display()),
            temp.path(),
        ))
        .await
        .expect("spawn ACP manager");

        let first = manager
            .prompt_conversation("group-a", "hello")
            .await
            .expect("first prompt");
        let second = manager
            .prompt_conversation("group-a", "again")
            .await
            .expect("second prompt");
        let third = manager
            .prompt_conversation("group-b", "other")
            .await
            .expect("third prompt");

        assert_eq!(first.final_text, "echo:hello");
        assert_eq!(second.final_text, "echo:again");
        assert_eq!(third.final_text, "echo:other");
        assert_eq!(first.session_id, second.session_id);
        assert_ne!(first.session_id, third.session_id);

        let log_path = temp.path().join("sessions.log");
        let created = std::fs::read_to_string(&log_path).expect("read session log");
        let created = created
            .lines()
            .filter(|line| !line.contains(':'))
            .collect::<Vec<_>>();
        assert_eq!(
            created,
            vec![first.session_id.as_str(), third.session_id.as_str()]
        );
    }

    #[tokio::test]
    async fn acp_backend_manager_reuses_sessions_and_serializes_turns_per_conversation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let script_path = write_fake_acp_backend(&temp, 50);
        let (manager, mut completion_rx) = AcpBackendManager::spawn_with_queue_capacity(
            AcpBackendConfig::new(format!("python3 -u {}", script_path.display()), temp.path()),
            4,
        )
        .await
        .expect("spawn ACP backend manager");

        manager
            .enqueue_prompt("group-a", "first")
            .await
            .expect("enqueue first");
        manager
            .enqueue_prompt("group-a", "second")
            .await
            .expect("enqueue second");
        manager
            .enqueue_prompt("group-b", "other")
            .await
            .expect("enqueue third");

        let first = timeout(Duration::from_secs(2), completion_rx.recv())
            .await
            .expect("wait first completion")
            .expect("first completion");
        let second = timeout(Duration::from_secs(2), completion_rx.recv())
            .await
            .expect("wait second completion")
            .expect("second completion");
        let third = timeout(Duration::from_secs(2), completion_rx.recv())
            .await
            .expect("wait third completion")
            .expect("third completion");

        let completions = [first, second, third];
        let group_a: Vec<_> = completions
            .iter()
            .filter(|item| item.conversation_id == "group-a")
            .collect();
        assert_eq!(group_a.len(), 2);
        assert_eq!(
            group_a[0]
                .result
                .as_ref()
                .expect("group-a first")
                .final_text,
            "echo:first"
        );
        assert_eq!(
            group_a[1]
                .result
                .as_ref()
                .expect("group-a second")
                .final_text,
            "echo:second"
        );

        let log_path = temp.path().join("sessions.log");
        let lines = std::fs::read_to_string(&log_path)
            .expect("read session log")
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let first_session = &group_a[0]
            .result
            .as_ref()
            .expect("group-a first")
            .session_id;
        let second_session = &group_a[1]
            .result
            .as_ref()
            .expect("group-a second")
            .session_id;
        assert_eq!(
            first_session, second_session,
            "same conversation should reuse one ACP session"
        );
        let start_first = lines
            .iter()
            .position(|line| line == &format!("start:{first_session}:first"))
            .expect("start first");
        let end_first = lines
            .iter()
            .position(|line| line == &format!("end:{first_session}:first"))
            .expect("end first");
        let start_second = lines
            .iter()
            .position(|line| line == &format!("start:{second_session}:second"))
            .expect("start second");
        let end_second = lines
            .iter()
            .position(|line| line == &format!("end:{second_session}:second"))
            .expect("end second");
        assert!(
            start_first < end_first,
            "first turn should finish after it starts"
        );
        assert!(
            end_first < start_second,
            "same-conversation prompts should not overlap"
        );
        assert!(
            start_second < end_second,
            "second turn should finish after it starts"
        );
    }

    #[tokio::test]
    async fn acp_backend_manager_completes_prompts_via_background_queue() {
        let temp = tempfile::tempdir().expect("tempdir");
        let script_path = write_fake_acp_backend(&temp, 200);
        let (manager, mut completion_rx) = AcpBackendManager::spawn_with_queue_capacity(
            AcpBackendConfig::new(format!("python3 -u {}", script_path.display()), temp.path()),
            2,
        )
        .await
        .expect("spawn ACP backend manager");

        manager
            .enqueue_prompt("group-a", "hello")
            .await
            .expect("enqueue prompt");

        assert!(
            timeout(Duration::from_millis(50), completion_rx.recv())
                .await
                .is_err(),
            "completion should arrive asynchronously, not inline"
        );

        let completion = timeout(Duration::from_secs(2), completion_rx.recv())
            .await
            .expect("wait completion")
            .expect("completion");
        assert_eq!(completion.conversation_id, "group-a");
        assert_eq!(
            completion.result.expect("prompt success").final_text,
            "echo:hello"
        );
    }

    #[tokio::test]
    async fn acp_backend_manager_reports_prompt_failures_and_continues() {
        let temp = tempfile::tempdir().expect("tempdir");
        let script_path = write_fake_acp_backend(&temp, 0);
        let (manager, mut completion_rx) = AcpBackendManager::spawn_with_queue_capacity(
            AcpBackendConfig::new(format!("python3 -u {}", script_path.display()), temp.path()),
            2,
        )
        .await
        .expect("spawn ACP backend manager");

        manager
            .enqueue_prompt("group-a", "fail")
            .await
            .expect("enqueue failing prompt");
        manager
            .enqueue_prompt("group-a", "after")
            .await
            .expect("enqueue recovery prompt");

        let failed = timeout(Duration::from_secs(2), completion_rx.recv())
            .await
            .expect("wait failure")
            .expect("failure completion");
        assert_eq!(failed.conversation_id, "group-a");
        assert!(
            failed
                .result
                .expect_err("expected failure")
                .contains("prompt failed")
        );

        let succeeded = timeout(Duration::from_secs(2), completion_rx.recv())
            .await
            .expect("wait recovery")
            .expect("recovery completion");
        assert_eq!(
            succeeded.result.expect("recovery success").final_text,
            "echo:after"
        );
    }
}
