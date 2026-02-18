mod mdk_support;

use mdk_core::prelude::MessageProcessingResult;
use nostr::Event;

uniffi::setup_scaffolding!();

#[derive(uniffi::Record)]
pub struct PushNotificationContent {
    pub chat_id: String,
    pub sender_pubkey: String,
    pub sender_name: String,
    pub sender_picture_url: Option<String>,
    pub content: String,
    pub is_group: bool,
    pub group_name: Option<String>,
}

#[derive(uniffi::Enum)]
pub enum PushNotificationResult {
    /// Decrypted successfully — show the notification.
    Content { content: PushNotificationContent },
    /// Recognised but should not alert (self-message, call signal, etc.).
    Suppress,
}

#[uniffi::export]
pub fn decrypt_push_notification(
    data_dir: String,
    nsec: String,
    event_json: String,
    keychain_group: String,
) -> Option<PushNotificationResult> {
    pika_tls::init_rustls_crypto_provider();

    let keys = nostr::Keys::parse(&nsec).ok()?;
    let pubkey = keys.public_key();

    let mdk = mdk_support::open_mdk(&data_dir, &pubkey, &keychain_group).ok()?;

    let event: Event = serde_json::from_str(&event_json).ok()?;

    let result = mdk.process_message(&event).ok()?;

    let msg = match result {
        MessageProcessingResult::ApplicationMessage(msg) => msg,
        _ => return None,
    };

    // Don't notify for self-messages.
    if msg.pubkey == pubkey {
        return Some(PushNotificationResult::Suppress);
    }

    // Don't notify for call signals.
    if is_call_signal_payload(&msg.content) {
        return Some(PushNotificationResult::Suppress);
    }

    let group = mdk.get_group(&msg.mls_group_id).ok()??;
    let chat_id = hex::encode(group.nostr_group_id);

    let all_groups = mdk.get_groups().ok()?;
    let group_info = all_groups
        .iter()
        .find(|g| g.mls_group_id == msg.mls_group_id);

    let group_name = group_info.and_then(|g| {
        if g.name != "DM" && !g.name.is_empty() {
            Some(g.name.clone())
        } else {
            None
        }
    });

    let members = mdk.get_members(&msg.mls_group_id).unwrap_or_default();
    let other_count = members.iter().filter(|p| *p != &pubkey).count();
    let is_group = other_count > 1 || (group_name.is_some() && other_count > 0);

    let sender_hex = msg.pubkey.to_hex();
    let (sender_name, sender_picture_url) = resolve_sender_profile(&data_dir, &sender_hex);

    Some(PushNotificationResult::Content {
        content: PushNotificationContent {
            chat_id,
            sender_pubkey: sender_hex,
            sender_name,
            sender_picture_url,
            content: msg.content,
            is_group,
            group_name,
        },
    })
}

/// Look up display name and picture URL from the profiles cache on disk.
fn resolve_sender_profile(data_dir: &str, pubkey_hex: &str) -> (String, Option<String>) {
    let path = std::path::Path::new(data_dir).join("profiles_cache.json");
    if let Ok(data) = std::fs::read_to_string(&path) {
        if let Ok(map) =
            serde_json::from_str::<std::collections::HashMap<String, ProfileCacheEntry>>(&data)
        {
            if let Some(entry) = map.get(pubkey_hex) {
                let name = entry
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("{}...", &pubkey_hex[..8]));
                return (name, entry.picture.clone());
            }
        }
    }
    // Truncated hex fallback.
    (format!("{}...", &pubkey_hex[..8]), None)
}

#[derive(serde::Deserialize)]
struct ProfileCacheEntry {
    name: Option<String>,
    picture: Option<String>,
}

/// Minimal check for call signal payloads without pulling in heavy call_control deps.
/// Matches `{"v":1,"ns":"pika.call",...}`.
fn is_call_signal_payload(content: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct Probe {
        v: u8,
        ns: String,
    }
    serde_json::from_str::<Probe>(content)
        .map(|p| p.v == 1 && p.ns == "pika.call")
        .unwrap_or(false)
}
