use std::time::Duration;

use anyhow::{Context, Result, anyhow};
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

pub async fn subscribe_group_msgs(
    client: &Client,
    nostr_group_id_hex: &str,
) -> Result<SubscriptionId> {
    let filter = Filter::new()
        .kind(Kind::MlsGroupMessage)
        .custom_tag(SingleLetterTag::lowercase(Alphabet::H), nostr_group_id_hex)
        .limit(200)
        .since(Timestamp::now() - Duration::from_secs(60 * 60));
    let out = client.subscribe(filter, None).await?;
    Ok(out.val)
}

pub async fn check_relay_ready(relay_url: &str, timeout: Duration) -> Result<()> {
    let relay_url = RelayUrl::parse(relay_url).context("parse relay url")?;
    let deadline = Instant::now() + timeout;
    let mut attempt: usize = 0;
    let mut last_detail = String::new();

    loop {
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timeout waiting for relay websocket to become connected (attempts={attempt}, last={last_detail})"
            ));
        }

        attempt += 1;

        let client = Client::new(Keys::generate());
        match client.add_relay(relay_url.clone()).await {
            Ok(_) => {}
            Err(err) => {
                last_detail = format!("add_relay: {err}");
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
        }

        client.connect().await;
        let connect_deadline = Instant::now() + Duration::from_secs(3);
        let mut connected = false;
        while Instant::now() < connect_deadline {
            if let Ok(relay) = client.relay(relay_url.clone()).await
                && relay.is_connected()
            {
                connected = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        if connected {
            client.shutdown().await;
            return Ok(());
        }

        last_detail = "not connected yet".to_string();
        client.shutdown().await;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
