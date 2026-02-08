use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use nostr_sdk::prelude::*;
use tokio::time::Instant;

pub async fn connect_client(keys: &Keys, relay_urls: &[String]) -> Result<Client> {
    let client = Client::new(keys.clone());
    for url in relay_urls {
        client
            .add_relay(url.as_str())
            .await
            .with_context(|| format!("add relay {url}"))?;
    }
    client.connect().await;
    Ok(client)
}

#[allow(dead_code)]
pub async fn check_relay_ready(relay_url: &str, timeout: Duration) -> Result<()> {
    let parsed = RelayUrl::parse(relay_url).context("parse relay url")?;
    let client = Client::new(Keys::generate());
    client.add_relay(parsed.clone()).await?;
    client.connect().await;

    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            client.shutdown().await;
            return Err(anyhow!("timeout waiting for relay {relay_url} to connect"));
        }
        if let Ok(relay) = client.relay(parsed.clone()).await {
            if relay.is_connected() {
                client.shutdown().await;
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub async fn publish_and_confirm(
    client: &Client,
    relay_urls: &[RelayUrl],
    event: &Event,
    label: &str,
) -> Result<()> {
    let out = client
        .send_event_to(relay_urls.to_vec(), event)
        .await
        .with_context(|| format!("send_event_to failed ({label})"))?;
    if out.success.is_empty() {
        let reasons: Vec<String> = out.failed.values().cloned().collect();
        return Err(anyhow!("no relay accepted event ({label}): {reasons:?}"));
    }
    Ok(())
}

pub async fn fetch_latest_key_package(
    client: &Client,
    author: &PublicKey,
    relay_urls: &[RelayUrl],
    timeout: Duration,
) -> Result<Event> {
    let filter = Filter::new()
        .kind(Kind::MlsKeyPackage)
        .author(*author)
        .limit(1);
    let events = client
        .fetch_events_from(relay_urls.to_vec(), filter, timeout)
        .await
        .context("fetch keypackage events")?;
    let found = events.iter().next().cloned();
    found.ok_or_else(|| anyhow!("no keypackage found for {}", author.to_hex()))
}

pub fn parse_relay_urls(urls: &[String]) -> Result<Vec<RelayUrl>> {
    urls.iter()
        .map(|u| RelayUrl::parse(u.as_str()).with_context(|| format!("parse relay url: {u}")))
        .collect()
}
