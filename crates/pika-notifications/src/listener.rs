use crate::models::group_subscription::GroupFilterInfo;
use crate::models::subscription_info::SubscriptionInfo;
use a2::request::notification::NotificationOptions;
use a2::request::payload::{APSAlert, PayloadLike, APS};
use a2::Client as ApnsClient;
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::PgConnection;
use fcm_rs::client::FcmClient;
use fcm_rs::models::{Message, Notification as FcmNotification};
use nostr_sdk::prelude::*;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch::Receiver;
use tracing::{debug, error, info, trace, warn};

/// Custom APNs payload that includes the nostr event JSON.
/// The `mutable_content` flag tells iOS to pass this to a
/// Notification Service Extension before displaying.
#[derive(Serialize, Debug)]
struct EventNotificationPayload<'a> {
    aps: APS<'a>,
    /// The raw nostr event JSON for the client to decrypt/process.
    nostr_event: String,
    #[serde(skip_serializing)]
    options: NotificationOptions<'a>,
    #[serde(skip_serializing)]
    device_token: &'a str,
}

impl<'a> PayloadLike for EventNotificationPayload<'a> {
    fn get_device_token(&self) -> &str {
        self.device_token
    }
    fn get_options(&self) -> &NotificationOptions<'_> {
        &self.options
    }
}

pub async fn start_listener(
    db_pool: Pool<ConnectionManager<PgConnection>>,
    mut receiver: Receiver<GroupFilterInfo>,
    apns_client: Option<Arc<ApnsClient>>,
    fcm_client: Option<Arc<FcmClient>>,
    apns_topic: String,
    relays: Vec<String>,
) -> anyhow::Result<()> {
    let keys = Keys::generate();
    loop {
        let client = Client::new(keys.clone());

        let filter: GroupFilterInfo = receiver.borrow().clone();
        info!(
            group_count = filter.group_ids.len(),
            "Building subscription filter"
        );
        debug!("Group IDs: {:?}", filter.group_ids);

        for relay in relays.iter() {
            if relay.is_empty() || relay.contains("localhost") {
                continue;
            }
            debug!(relay = %relay, "Adding relay");
            client.add_relay(relay.as_str()).await?;
        }
        client.connect().await;
        info!(relay_count = relays.len(), "Connected to relays");

        let group_filter = Filter::new()
            .kind(Kind::MlsGroupMessage)
            .custom_tags(SingleLetterTag::lowercase(Alphabet::H), filter.group_ids)
            .since(Timestamp::now());

        debug!(?group_filter, "Subscribing");
        client.subscribe(group_filter, None).await?;

        info!("Listening for kind 445 events...");

        let mut notifications = client.notifications();
        loop {
            tokio::select! {
                Ok(notification) = notifications.recv() => {
                    match notification {
                        RelayPoolNotification::Event { event, relay_url, .. } => {
                            trace!(
                                kind = event.kind.as_u16(),
                                event_id = %event.id,
                                relay = %relay_url,
                                "Received event"
                            );
                            if event.kind == Kind::MlsGroupMessage {
                                // Skip events expiring within 30 seconds (e.g. call signals, typing indicators).
                                let expiration = event.tags.iter().find_map(|tag| {
                                    if tag.kind() == TagKind::Expiration {
                                        tag.content().and_then(|s| s.parse::<u64>().ok())
                                    } else {
                                        None
                                    }
                                });
                                if let Some(exp) = expiration {
                                    if exp <= Timestamp::now().as_secs() + 30 {
                                        debug!(event_id = %event.id, "Skipping near-expired event");
                                        continue;
                                    }
                                }
                                info!(
                                    event_id = %event.id,
                                    author = %event.pubkey,
                                    relay = %relay_url,
                                    "Got kind 445 event"
                                );
                                debug!(tags = ?event.tags, "Event tags");
                                tokio::spawn({
                                    let db_pool = db_pool.clone();
                                    let apns_client = apns_client.clone();
                                    let fcm_client = fcm_client.clone();
                                    let apns_topic = apns_topic.clone();
                                    async move {
                                        let fut = handle_event(
                                            *event,
                                            db_pool,
                                            apns_client,
                                            fcm_client,
                                            apns_topic,
                                        );

                                        match tokio::time::timeout(Duration::from_secs(30), fut).await {
                                            Ok(Ok(_)) => {}
                                            Ok(Err(e)) => error!("Handle event error: {e}"),
                                            Err(_) => error!("Handle event timeout"),
                                        }
                                    }
                                });
                            }
                        }
                        RelayPoolNotification::Shutdown => {
                            warn!("Relay pool shutdown");
                            break;
                        }
                        RelayPoolNotification::Message { relay_url, message } => {
                            trace!(relay = %relay_url, ?message, "Relay message");
                        }
                    }
                }
                _ = receiver.changed() => {
                    info!("Group filter changed, reconnecting...");
                    break;
                }
            }
        }

        client.disconnect().await;
    }
}

async fn handle_event(
    event: Event,
    db_pool: Pool<ConnectionManager<PgConnection>>,
    apns_client: Option<Arc<ApnsClient>>,
    fcm_client: Option<Arc<FcmClient>>,
    apns_topic: String,
) -> anyhow::Result<()> {
    let mut conn = db_pool.get()?;

    let h_tags: Vec<String> = event
        .tags
        .iter()
        .filter_map(|tag| {
            if tag.kind() == TagKind::custom("h") {
                tag.content().map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    debug!(event_id = %event.id, ?h_tags, "Extracted #h tags");

    if h_tags.is_empty() {
        info!(event_id = %event.id, "No #h tags found, skipping");
        return Ok(());
    }

    let mut seen_ids = HashSet::new();
    let mut sub_infos = Vec::new();
    for group_id in &h_tags {
        let subs = SubscriptionInfo::find_by_group(&mut conn, group_id)?;
        debug!(group_id = %group_id, count = subs.len(), "Found subscriptions");
        for sub in subs {
            if seen_ids.insert(sub.id.clone()) {
                sub_infos.push(sub);
            }
        }
    }

    if sub_infos.is_empty() {
        info!(event_id = %event.id, ?h_tags, "No subscriptions found");
        return Ok(());
    }

    info!(
        event_id = %event.id,
        subscription_count = sub_infos.len(),
        ?h_tags,
        "Sending notifications"
    );

    // Serialize the event once for all notifications
    let event_json = serde_json::to_string(&event)?;

    for sub_info in &sub_infos {
        info!(
            platform = %sub_info.platform,
            subscription_id = %sub_info.id,
            device_token = %sub_info.device_token,
            ?h_tags,
            "NOTIFY"
        );

        match sub_info.platform.as_str() {
            "ios" => {
                if let Some(ref client) = apns_client {
                    let payload = EventNotificationPayload {
                        aps: APS {
                            alert: Some(APSAlert::Body("New message")),
                            sound: Some(a2::request::payload::APSSound::Sound("default")),
                            mutable_content: Some(1),
                            ..Default::default()
                        },
                        nostr_event: event_json.clone(),
                        options: NotificationOptions {
                            apns_topic: Some(&apns_topic),
                            ..Default::default()
                        },
                        device_token: &sub_info.device_token,
                    };
                    client.send(payload).await?;
                    info!(subscription_id = %sub_info.id, "APNs sent");
                } else {
                    info!(subscription_id = %sub_info.id, "APNs not configured, skipping send");
                }
            }
            "android" => {
                if let Some(ref client) = fcm_client {
                    let data = serde_json::json!({
                        "nostr_event": event_json,
                    });
                    let message = Message {
                        token: Some(sub_info.device_token.clone()),
                        notification: Some(FcmNotification {
                            title: Some("New message".to_string()),
                            body: Some("You have a new message".to_string()),
                        }),
                        data: Some(data),
                    };
                    client.send(message).await?;
                    info!(subscription_id = %sub_info.id, "FCM sent");
                } else {
                    info!(subscription_id = %sub_info.id, "FCM not configured, skipping send");
                }
            }
            other => {
                warn!(platform = %other, subscription_id = %sub_info.id, "Unknown platform");
            }
        }
    }

    Ok(())
}
