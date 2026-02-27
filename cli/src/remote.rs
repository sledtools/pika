use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow};
use pikachat_sidecar::OutMsg;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// Send a single JSONL command to the daemon socket, wait for Ok or Error response.
pub async fn remote_call(
    state_dir: &Path,
    cmd_json: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let sock_path = state_dir.join("daemon.sock");
    let stream = UnixStream::connect(&sock_path)
        .await
        .with_context(|| format!("connect to daemon at {}", sock_path.display()))?;

    let (reader, mut writer) = stream.into_split();

    // Inject request_id if not present
    let request_id = format!(
        "remote-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let mut cmd = cmd_json;
    if let Some(obj) = cmd.as_object_mut() {
        obj.entry("request_id".to_string())
            .or_insert(serde_json::Value::String(request_id.clone()));
    }

    let mut line = serde_json::to_string(&cmd)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;

    // Read lines until we get Ok or Error with matching request_id
    let mut lines = tokio::io::BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: OutMsg = serde_json::from_str(trimmed)
            .with_context(|| format!("parse daemon response: {trimmed}"))?;
        match &msg {
            OutMsg::Ok {
                request_id: rid,
                result,
            } => {
                if rid.as_deref() == Some(&request_id) {
                    return Ok(result.clone().unwrap_or(serde_json::Value::Null));
                }
            }
            OutMsg::Error {
                request_id: rid,
                code,
                message,
            } => {
                if rid.as_deref() == Some(&request_id) {
                    return Err(anyhow!("daemon error [{code}]: {message}"));
                }
            }
            _ => {
                // Ignore broadcast events
                continue;
            }
        }
    }
    Err(anyhow!("daemon closed connection without response"))
}
