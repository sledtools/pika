use crate::models::group_subscription::GroupSubscription;
use crate::models::subscription_info::SubscriptionInfo;
use crate::State;
use a2::{DefaultNotificationBuilder, NotificationBuilder, NotificationOptions};
use axum::http::StatusCode;
use axum::{Extension, Json};
use fcm_rs::models::{Message, Notification as FcmNotification};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub id: String,
    pub device_token: String,
    pub platform: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeGroupsRequest {
    pub id: String,
    pub group_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsubscribeGroupsRequest {
    pub id: String,
    pub group_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadcastNotification {
    pub title: String,
    pub body: String,
    pub icon: Option<String>,
}

async fn register_impl(state: &State, payload: RegisterRequest) -> anyhow::Result<String> {
    debug!(
        "register: id={} platform={} token={}",
        payload.id, payload.platform, payload.device_token
    );
    let mut conn = state.db_pool.get()?;
    let id = SubscriptionInfo::register(
        &mut conn,
        &payload.id,
        &payload.device_token,
        &payload.platform,
    )?;

    info!(
        "Registered subscription: id={id} platform={}",
        payload.platform
    );

    Ok(id)
}

pub async fn register(
    Extension(state): Extension<State>,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<String>, (StatusCode, String)> {
    match register_impl(&state, payload).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("register", e)),
    }
}

async fn subscribe_groups_impl(
    state: &State,
    payload: SubscribeGroupsRequest,
) -> anyhow::Result<()> {
    let mut conn = state.db_pool.get()?;
    GroupSubscription::subscribe(&mut conn, &payload.id, &payload.group_ids)?;

    // notify new group subscriptions
    let filter_info = state.channel.lock().await;
    filter_info.send_if_modified(|current| {
        let mut changed = false;
        for group_id in &payload.group_ids {
            if !current.group_ids.contains(group_id) {
                current.group_ids.push(group_id.clone());
                changed = true;
            }
        }
        changed
    });

    info!(
        "Subscribed id={} to {} group(s): {:?}",
        payload.id,
        payload.group_ids.len(),
        payload.group_ids
    );

    Ok(())
}

pub async fn subscribe_groups(
    Extension(state): Extension<State>,
    Json(payload): Json<SubscribeGroupsRequest>,
) -> Result<Json<()>, (StatusCode, String)> {
    match subscribe_groups_impl(&state, payload).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("subscribe_groups", e)),
    }
}

async fn unsubscribe_groups_impl(
    state: &State,
    payload: UnsubscribeGroupsRequest,
) -> anyhow::Result<()> {
    let mut conn = state.db_pool.get()?;
    GroupSubscription::unsubscribe(&mut conn, &payload.id, &payload.group_ids)?;

    // Refresh the listener filter — groups with no remaining subscribers should be removed.
    let updated = GroupSubscription::get_filter_info(&mut conn)?;
    let filter_info = state.channel.lock().await;
    filter_info.send_replace(updated);

    info!(
        "Unsubscribed id={} from {} group(s): {:?}",
        payload.id,
        payload.group_ids.len(),
        payload.group_ids
    );

    Ok(())
}

pub async fn unsubscribe_groups(
    Extension(state): Extension<State>,
    Json(payload): Json<UnsubscribeGroupsRequest>,
) -> Result<Json<()>, (StatusCode, String)> {
    match unsubscribe_groups_impl(&state, payload).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("unsubscribe_groups", e)),
    }
}

async fn broadcast_individual(
    state: &State,
    sub_info: &SubscriptionInfo,
    notification: &BroadcastNotification,
) -> anyhow::Result<()> {
    info!(
        "BROADCAST [{}] id={} token={}",
        sub_info.platform, sub_info.id, sub_info.device_token
    );

    match sub_info.platform.as_str() {
        "ios" => {
            if let Some(ref client) = state.apns_client {
                let builder = DefaultNotificationBuilder::new()
                    .set_title(&notification.title)
                    .set_body(&notification.body);
                let payload = builder.build(
                    &sub_info.device_token,
                    NotificationOptions {
                        apns_topic: Some(&state.apns_topic),
                        ..Default::default()
                    },
                );
                client.send(payload).await?;
            } else {
                info!("  -> APNs not configured, skipping actual send");
            }
        }
        "android" => {
            if let Some(ref client) = state.fcm_client {
                let message = Message {
                    token: Some(sub_info.device_token.clone()),
                    notification: Some(FcmNotification {
                        title: Some(notification.title.clone()),
                        body: Some(notification.body.clone()),
                    }),
                    data: None,
                };
                client.send(message).await?;
            } else {
                info!("  -> FCM not configured, skipping actual send");
            }
        }
        other => {
            anyhow::bail!("Unknown platform: {other}");
        }
    }
    Ok(())
}

async fn broadcast_impl(state: &State, notification: BroadcastNotification) -> anyhow::Result<()> {
    let mut conn = state.db_pool.get()?;
    let all = SubscriptionInfo::get_all(&mut conn)?;

    let mut futures = Vec::with_capacity(all.len());
    for item in &all {
        let fut = broadcast_individual(state, item, &notification);
        futures.push(fut);
    }
    futures::future::try_join_all(futures).await?;

    Ok(())
}

pub async fn broadcast(
    Extension(state): Extension<State>,
    Json(notification): Json<BroadcastNotification>,
) -> Result<Json<()>, (StatusCode, String)> {
    match broadcast_impl(&state, notification).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("broadcast", e)),
    }
}

pub async fn health_check() -> Result<Json<()>, (StatusCode, String)> {
    Ok(Json(()))
}

pub(crate) fn handle_anyhow_error(function: &str, err: anyhow::Error) -> (StatusCode, String) {
    error!("Error in {function}: {err:?}");
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{err}"))
}
