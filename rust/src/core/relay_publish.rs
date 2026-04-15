use std::time::Duration;

use nostr_sdk::prelude::{Client, Event, RelayUrl};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PublishOutcome {
    Ok,
    Err(String),
}

pub(super) async fn publish_event_with_retry(
    client: &Client,
    relays: &[RelayUrl],
    event: &Event,
    max_attempts: u8,
    context: &str,
    reconnect: bool,
) -> PublishOutcome {
    let mut last_error: Option<String> = None;
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
                let error = output
                    .failed
                    .values()
                    .next()
                    .cloned()
                    .unwrap_or_else(|| "no relay accepted event".to_string());
                let retryable = output
                    .failed
                    .values()
                    .any(|err| is_retryable_relay_error(err));
                tracing::warn!(attempt, "{context}: publish failed err={error}");
                last_error = Some(error);
                if !retryable {
                    break;
                }
            }
            Err(err) => {
                let error = err.to_string();
                let retryable = is_retryable_relay_error(&error);
                tracing::warn!(attempt, "{context}: publish error err={err:#}");
                last_error = Some(error);
                if !retryable {
                    break;
                }
            }
        }
        if attempt + 1 < max_attempts {
            backoff_sleep(attempt).await;
        }
    }
    PublishOutcome::Err(last_error.unwrap_or_else(|| "unknown error".to_string()))
}

fn is_retryable_relay_error(error: &str) -> bool {
    error.contains("auth")
        || error.contains("AUTH")
        || error.contains("protected")
        || error.contains("not connected")
        || error.contains("not ready")
        || error.contains("no relays")
}

async fn backoff_sleep(attempt: u8) {
    let delay_ms = 250u64.saturating_mul(1u64 << attempt);
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
}
