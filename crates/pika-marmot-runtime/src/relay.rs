use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use nostr_sdk::prelude::*;
use tokio::time::Instant;

use crate::key_package::normalize_peer_key_package_event_for_mdk;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    Ok,
    Err(String),
}

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

fn is_retryable_relay_error(err: &str) -> bool {
    err.contains("auth")
        || err.contains("AUTH")
        || err.contains("protected")
        || err.contains("not connected")
        || err.contains("not ready")
        || err.contains("no relays")
}

pub async fn publish_event_with_retry(
    client: &Client,
    relays: &[RelayUrl],
    event: &Event,
    max_attempts: u8,
    context: &str,
    reconnect: bool,
) -> PublishOutcome {
    let mut last_err: Option<String> = None;
    for attempt in 0..max_attempts {
        if reconnect {
            client.connect().await;
            client.wait_for_connection(Duration::from_secs(5)).await;
        }

        match client.send_event_to(relays, event).await {
            Ok(output) if !output.success.is_empty() => {
                tracing::info!(
                    attempt,
                    ok_relays = ?output.success,
                    failed_relays = ?output.failed.keys().collect::<Vec<_>>(),
                    "{context}: publish ok"
                );
                return PublishOutcome::Ok;
            }
            Ok(output) => {
                let err = output
                    .failed
                    .values()
                    .next()
                    .cloned()
                    .unwrap_or_else(|| "no relay accepted event".to_string());
                let retryable = output.failed.values().any(|e| is_retryable_relay_error(e));
                tracing::warn!(attempt, "{context}: publish failed err={err}");
                last_err = Some(err);
                if !retryable {
                    break;
                }
            }
            Err(e) => {
                let es = e.to_string();
                let retryable = is_retryable_relay_error(&es);
                tracing::warn!(attempt, "{context}: publish error err={e:#}");
                last_err = Some(es);
                if !retryable {
                    break;
                }
            }
        }
        if attempt + 1 < max_attempts {
            backoff_sleep(attempt).await;
        }
    }
    PublishOutcome::Err(last_err.unwrap_or_else(|| "unknown error".to_string()))
}

#[allow(clippy::too_many_arguments)]
pub async fn gift_wrap_with_retry(
    client: &Client,
    relays: &[RelayUrl],
    receiver: &PublicKey,
    rumor: UnsignedEvent,
    tags: Vec<Tag>,
    max_attempts: u8,
    context: &str,
    reconnect: bool,
) -> PublishOutcome {
    let mut last_err: Option<String> = None;
    for attempt in 0..max_attempts {
        if reconnect {
            client.connect().await;
            client.wait_for_connection(Duration::from_secs(5)).await;
        }

        match client
            .gift_wrap_to(relays, receiver, rumor.clone(), tags.clone())
            .await
        {
            Ok(output) if !output.success.is_empty() => return PublishOutcome::Ok,
            Ok(output) => {
                let err = output
                    .failed
                    .values()
                    .next()
                    .cloned()
                    .unwrap_or_else(|| "no relay accepted event".to_string());
                let retryable = output.failed.values().any(|e| is_retryable_relay_error(e));
                tracing::warn!(attempt, "{context}: failed err={err}");
                last_err = Some(err);
                if !retryable {
                    break;
                }
            }
            Err(e) => {
                let es = e.to_string();
                let retryable = is_retryable_relay_error(&es);
                tracing::warn!(attempt, "{context}: error err={e:#}");
                last_err = Some(es);
                if !retryable {
                    break;
                }
            }
        }
        if attempt + 1 < max_attempts {
            backoff_sleep(attempt).await;
        }
    }
    PublishOutcome::Err(last_err.unwrap_or_else(|| "unknown error".to_string()))
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

pub async fn fetch_latest_key_package_for_mdk(
    client: &Client,
    author: &PublicKey,
    relay_urls: &[RelayUrl],
    timeout: Duration,
) -> Result<Event> {
    let event = fetch_latest_key_package(client, author, relay_urls, timeout).await?;
    Ok(normalize_fetched_key_package_for_mdk(&event))
}

fn normalize_fetched_key_package_for_mdk(event: &Event) -> Event {
    normalize_peer_key_package_event_for_mdk(event)
}

async fn backoff_sleep(attempt: u8) {
    let delay_ms = 250u64.saturating_mul(1u64 << attempt);
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key_package_event(content: &str, tags: Vec<Tag>) -> Event {
        EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(&Keys::generate())
            .expect("sign key package event")
    }

    #[test]
    fn normalize_fetched_key_package_for_mdk_applies_shared_interop_rules() {
        let event = key_package_event(
            "68656c6c6f",
            vec![
                Tag::custom(TagKind::MlsProtocolVersion, ["1"]),
                Tag::custom(TagKind::MlsCiphersuite, ["1"]),
            ],
        );

        let normalized = normalize_fetched_key_package_for_mdk(&event);

        assert_eq!(normalized.content, "aGVsbG8=");
        assert!(
            normalized
                .tags
                .iter()
                .any(|tag| tag.as_slice() == ["encoding", "base64"])
        );
        assert!(
            normalized
                .tags
                .iter()
                .any(|tag| tag.as_slice() == ["mls_protocol_version", "1.0"])
        );
        assert!(
            normalized
                .tags
                .iter()
                .any(|tag| tag.as_slice() == ["mls_ciphersuite", "0x0001"])
        );
    }

    #[test]
    fn retryable_relay_error_matches_app_rules() {
        assert!(is_retryable_relay_error("auth required"));
        assert!(is_retryable_relay_error("relay not ready"));
        assert!(is_retryable_relay_error("protected event rejected"));
        assert!(!is_retryable_relay_error("invalid event id"));
    }
}
