/// Shared retry-with-exponential-backoff helpers for publishing Nostr events to relays.
use nostr_sdk::prelude::*;
use std::time::Duration;

/// Result of a relay publish attempt with retries.
pub(super) enum PublishOutcome {
    /// At least one relay accepted the event.
    Ok,
    /// All attempts exhausted or a non-retryable error occurred.
    Err(String),
}

/// Returns true if the relay error string suggests the failure is transient and worth retrying.
fn is_retryable_relay_error(err: &str) -> bool {
    err.contains("auth")
        || err.contains("AUTH")
        || err.contains("protected")
        || err.contains("not connected")
        || err.contains("not ready")
        || err.contains("no relays")
}

/// Publish a Nostr event to relays with exponential-backoff retries.
///
/// - `max_attempts`: how many times to try (e.g. 4, 5, 6)
/// - `context`: label for tracing (e.g. "call signal", "key package")
/// - `reconnect`: if true, calls `client.connect()` + `wait_for_connection()` before each attempt
pub(super) async fn publish_event_with_retry(
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

/// Gift-wrap a rumor to a peer with exponential-backoff retries.
#[allow(clippy::too_many_arguments)]
pub(super) async fn gift_wrap_with_retry(
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
            Ok(output) if !output.success.is_empty() => {
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

async fn backoff_sleep(attempt: u8) {
    let delay_ms = 250u64.saturating_mul(1u64 << attempt);
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
}
