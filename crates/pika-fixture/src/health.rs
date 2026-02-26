use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};
use tokio::time::sleep;
use tracing::debug;

pub async fn wait_for_http(url: &str, timeout: Duration) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;

    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) => {
                debug!("health check {url} returned {}", resp.status());
            }
            Err(e) => {
                debug!("health check {url} failed: {e}");
            }
        }

        if tokio::time::Instant::now() >= deadline {
            bail!("health check timed out waiting for {url}");
        }
        sleep(Duration::from_millis(250)).await;
    }
}

pub async fn wait_for_pg_isready(pgdata: &Path, timeout: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    let host = pgdata.to_string_lossy().to_string();

    loop {
        let output = tokio::process::Command::new("pg_isready")
            .arg("-h")
            .arg(&host)
            .output()
            .await?;

        if output.status.success() {
            return Ok(());
        }

        if tokio::time::Instant::now() >= deadline {
            bail!("pg_isready timed out for {host}");
        }
        sleep(Duration::from_millis(250)).await;
    }
}

pub async fn wait_for_log_line(
    log_path: &Path,
    pattern: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        if log_path.exists() {
            let content = tokio::fs::read_to_string(log_path)
                .await
                .unwrap_or_default();
            for line in content.lines() {
                if line.contains(pattern) {
                    return Ok(line.to_string());
                }
            }
        }

        if tokio::time::Instant::now() >= deadline {
            bail!(
                "timed out waiting for pattern '{pattern}' in {}",
                log_path.display()
            );
        }
        sleep(Duration::from_millis(500)).await;
    }
}
