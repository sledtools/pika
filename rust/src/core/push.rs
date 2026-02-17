// Push notification subscription management.

use std::collections::HashSet;
use std::path::Path;

use super::AppCore;

const DEFAULT_NOTIFICATION_URL: &str = "https://test.notifs.benthecarman.com";

impl AppCore {
    pub(super) fn load_or_create_push_device_id(data_dir: &str) -> String {
        let path = Path::new(data_dir).join("push_device_id.txt");
        if let Ok(id) = std::fs::read_to_string(&path) {
            let id = id.trim().to_string();
            if !id.is_empty() {
                return id;
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        let _ = std::fs::write(&path, &id);
        id
    }

    pub(super) fn load_push_subscriptions(data_dir: &str) -> HashSet<String> {
        let path = Path::new(data_dir).join("push_subscribed_chats.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(set) = serde_json::from_str::<HashSet<String>>(&data) {
                return set;
            }
        }
        HashSet::new()
    }

    fn save_push_subscriptions(&self) {
        let path = Path::new(&self.data_dir).join("push_subscribed_chats.json");
        if let Ok(json) = serde_json::to_string(&self.push_subscribed_chat_ids) {
            let _ = std::fs::write(&path, json);
        }
    }

    pub(super) fn notification_url(&self) -> String {
        if let Some(url) = &self.config.notification_url {
            if !url.is_empty() {
                return url.clone();
            }
        }
        if let Ok(url) = std::env::var("PIKA_NOTIFICATION_URL") {
            if !url.is_empty() {
                return url;
            }
        }
        DEFAULT_NOTIFICATION_URL.to_string()
    }

    pub(super) fn set_push_token(&mut self, token: String) {
        tracing::info!("push: APNs token received");
        self.push_apns_token = Some(token);
        self.register_push_device();
    }

    pub(super) fn register_push_device(&self) {
        let Some(token) = self.push_apns_token.clone() else {
            return;
        };
        let url = format!("{}/register", self.notification_url());
        let device_id = self.push_device_id.clone();
        let client = self.http_client.clone();

        self.runtime.spawn(async move {
            let body = serde_json::json!({
                "id": device_id,
                "device_token": token,
                "platform": "ios"
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    tracing::info!(status = %resp.status(), "push: registered device");
                }
                Err(e) => {
                    tracing::warn!(%e, "push: failed to register device");
                }
            }
        });
    }

    pub(super) fn sync_push_subscriptions(&mut self) {
        if self.push_apns_token.is_none() {
            return;
        }

        let current_ids: HashSet<String> = self
            .state
            .chat_list
            .iter()
            .map(|c| c.chat_id.clone())
            .collect();

        let to_subscribe: Vec<String> = current_ids
            .difference(&self.push_subscribed_chat_ids)
            .cloned()
            .collect();
        let to_unsubscribe: Vec<String> = self
            .push_subscribed_chat_ids
            .difference(&current_ids)
            .cloned()
            .collect();

        if to_subscribe.is_empty() && to_unsubscribe.is_empty() {
            return;
        }

        let base_url = self.notification_url();
        let device_id = self.push_device_id.clone();
        let client = self.http_client.clone();

        if !to_subscribe.is_empty() {
            let url = format!("{}/subscribe-groups", base_url);
            let client = client.clone();
            let device_id = device_id.clone();
            let groups = to_subscribe.clone();
            self.runtime.spawn(async move {
                let body = serde_json::json!({
                    "id": device_id,
                    "group_ids": groups
                });
                match client.post(&url).json(&body).send().await {
                    Ok(resp) => {
                        tracing::info!(status = %resp.status(), count = groups.len(), "push: subscribed to groups");
                    }
                    Err(e) => {
                        tracing::warn!(%e, "push: failed to subscribe to groups");
                    }
                }
            });
        }

        if !to_unsubscribe.is_empty() {
            let url = format!("{}/unsubscribe-groups", base_url);
            let groups = to_unsubscribe.clone();
            self.runtime.spawn(async move {
                let body = serde_json::json!({
                    "id": device_id,
                    "group_ids": groups
                });
                match client.post(&url).json(&body).send().await {
                    Ok(resp) => {
                        tracing::info!(status = %resp.status(), count = groups.len(), "push: unsubscribed from groups");
                    }
                    Err(e) => {
                        tracing::warn!(%e, "push: failed to unsubscribe from groups");
                    }
                }
            });
        }

        self.push_subscribed_chat_ids = current_ids;
        self.save_push_subscriptions();
    }

    pub(super) fn clear_push_subscriptions(&mut self) {
        let ids: Vec<String> = self.push_subscribed_chat_ids.drain().collect();
        if !ids.is_empty() {
            let url = format!("{}/unsubscribe-groups", self.notification_url());
            let device_id = self.push_device_id.clone();
            let client = self.http_client.clone();
            self.runtime.spawn(async move {
                let body = serde_json::json!({
                    "id": device_id,
                    "group_ids": ids
                });
                match client.post(&url).json(&body).send().await {
                    Ok(resp) => {
                        tracing::info!(status = %resp.status(), "push: cleared all subscriptions");
                    }
                    Err(e) => {
                        tracing::warn!(%e, "push: failed to clear subscriptions");
                    }
                }
            });
        }

        // Clear persisted file.
        let path = Path::new(&self.data_dir).join("push_subscribed_chats.json");
        let _ = std::fs::remove_file(&path);
    }
}
