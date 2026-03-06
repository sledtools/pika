mod mdk_support;

use mdk_core::prelude::MessageProcessingResult;
use nostr::{Event, Kind, TagKind};

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
    /// Decrypted image bytes for rich notification thumbnails, if available.
    pub image_data: Option<Vec<u8>>,
}

#[derive(uniffi::Enum)]
pub enum PushNotificationResult {
    /// Decrypted successfully — show the notification.
    Content { content: PushNotificationContent },
    /// Incoming call invite — show call notification.
    CallInvite {
        chat_id: String,
        call_id: String,
        caller_name: String,
        caller_picture_url: Option<String>,
        is_video: bool,
    },
    /// Something went wrong during decryption / processing.
    Error { message: String },
}

#[derive(serde::Deserialize)]
struct CallProbe {
    #[serde(rename = "type")]
    msg_type: String,
    call_id: String,
    #[serde(default)]
    body: Option<CallProbeBody>,
}

#[derive(serde::Deserialize)]
struct CallProbeBody {
    #[serde(default)]
    tracks: Vec<CallProbeTrack>,
}

#[derive(serde::Deserialize)]
struct CallProbeTrack {
    name: String,
}

#[uniffi::export]
pub fn decrypt_push_notification(
    data_dir: String,
    nsec: String,
    event_json: String,
    keychain_group: String,
) -> Option<PushNotificationResult> {
    pika_tls::init_rustls_crypto_provider();

    let keys = match nostr::Keys::parse(&nsec) {
        Ok(k) => k,
        Err(e) => {
            return Some(PushNotificationResult::Error {
                message: format!("failed to parse nsec: {e}"),
            })
        }
    };
    let pubkey = keys.public_key();

    let mdk = match mdk_support::open_mdk(&data_dir, &pubkey, &keychain_group) {
        Ok(m) => m,
        Err(e) => {
            return Some(PushNotificationResult::Error {
                message: format!("failed to open mdk: {e}"),
            })
        }
    };

    let event: Event = match serde_json::from_str(&event_json) {
        Ok(e) => e,
        Err(e) => {
            return Some(PushNotificationResult::Error {
                message: format!("failed to parse event json: {e}"),
            })
        }
    };

    let result = match mdk.process_message(&event) {
        Ok(r) => r,
        Err(e) => {
            return Some(PushNotificationResult::Error {
                message: format!("failed to process message: {e}"),
            })
        }
    };

    let msg = match result {
        MessageProcessingResult::ApplicationMessage(msg) => msg,
        _ => return None,
    };

    // Don't notify for self-messages.
    if msg.pubkey == pubkey {
        return None;
    }

    // Helper: fetch group only in branches that need it.
    let get_group = || match mdk.get_group(&msg.mls_group_id) {
        Ok(Some(g)) => Ok(g),
        Ok(None) => Err("group not found".to_string()),
        Err(e) => Err(format!("failed to get group: {e}")),
    };

    match msg.kind {
        Kind::ChatMessage | Kind::Reaction => {
            let group = match get_group() {
                Ok(g) => g,
                Err(e) => {
                    return Some(PushNotificationResult::Error { message: e });
                }
            };
            let chat_id = hex::encode(group.nostr_group_id);

            let media = match msg.kind {
                Kind::ChatMessage => notif_media(&msg.tags),
                _ => None,
            };

            let content = match msg.kind {
                Kind::ChatMessage => {
                    if let Some(ref media) = media {
                        if msg.content.is_empty() {
                            media.kind.label().to_string()
                        } else {
                            format!("{} {}", media.kind.emoji(), msg.content)
                        }
                    } else if msg.content.is_empty() {
                        return None;
                    } else {
                        msg.content
                    }
                }
                Kind::Reaction => {
                    let emoji = if msg.content.is_empty() || msg.content == "+" {
                        "\u{2764}\u{FE0F}".to_string()
                    } else {
                        msg.content
                    };
                    format!("Reacted {emoji}")
                }
                _ => unreachable!(),
            };

            // Try to download and decrypt the image for rich notification thumbnails.
            let image_data = media
                .filter(|m| m.kind == NotifMediaKind::Image)
                .and_then(|m| download_and_decrypt_image(&mdk, &msg.mls_group_id, m.tag));

            let group_name = if group.name != "DM" && !group.name.is_empty() {
                Some(group.name.clone())
            } else {
                None
            };

            let members = mdk.get_members(&msg.mls_group_id).unwrap_or_default();
            let other_count = members.iter().filter(|p| *p != &pubkey).count();
            let is_group = group_name.is_some() || other_count > 1;

            let sender_hex = msg.pubkey.to_hex();
            let (sender_name, sender_picture_url) =
                resolve_sender_profile(&data_dir, &sender_hex, Some(&chat_id));

            Some(PushNotificationResult::Content {
                content: PushNotificationContent {
                    chat_id,
                    sender_pubkey: sender_hex,
                    sender_name,
                    sender_picture_url,
                    content,
                    is_group,
                    group_name,
                    image_data,
                },
            })
        }
        Kind::Custom(10) => {
            let probe: CallProbe = match serde_json::from_str(&msg.content) {
                Ok(p) => p,
                Err(e) => {
                    return Some(PushNotificationResult::Error {
                        message: format!("failed to parse call probe: {e}"),
                    })
                }
            };
            if probe.msg_type != "call.invite" {
                return None;
            }
            let group = match get_group() {
                Ok(g) => g,
                Err(e) => {
                    return Some(PushNotificationResult::Error { message: e });
                }
            };
            let chat_id = hex::encode(group.nostr_group_id);
            let is_video = probe
                .body
                .as_ref()
                .map(|b| b.tracks.iter().any(|t| t.name == "video0"))
                .unwrap_or(false);
            let sender_hex = msg.pubkey.to_hex();
            let (caller_name, caller_picture_url) =
                resolve_sender_profile(&data_dir, &sender_hex, Some(&chat_id));
            Some(PushNotificationResult::CallInvite {
                chat_id,
                call_id: probe.call_id,
                caller_name,
                caller_picture_url,
                is_video,
            })
        }
        _ => None,
    }
}

/// Broad media category inferred from the first `imeta` tag's MIME type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotifMediaKind {
    Image,
    Video,
    Audio,
    File,
}

impl NotifMediaKind {
    fn label(&self) -> &'static str {
        match self {
            Self::Image => "Sent a photo",
            Self::Video => "Sent a video",
            Self::Audio => "Sent a voice message",
            Self::File => "Sent a file",
        }
    }

    fn emoji(&self) -> &'static str {
        match self {
            Self::Image => "\u{1F4F7}", // 📷
            Self::Video => "\u{1F3AC}", // 🎬
            Self::Audio => "\u{1F3A4}", // 🎤
            Self::File => "\u{1F4CE}",  // 📎
        }
    }
}

/// Parsed media info from the first `imeta` tag.
struct NotifMedia<'a> {
    kind: NotifMediaKind,
    tag: &'a nostr::Tag,
}

/// Detect the media kind from the first `imeta` tag, if any.
fn notif_media(tags: &nostr::Tags) -> Option<NotifMedia<'_>> {
    for tag in tags.iter() {
        if !matches!(tag.kind(), TagKind::Custom(ref k) if k.as_ref() == "imeta") {
            continue;
        }
        let mime = tag
            .as_slice()
            .iter()
            .skip(1)
            .find_map(|e| e.strip_prefix("m "))
            .unwrap_or("");
        let kind = if mime.starts_with("image/") {
            NotifMediaKind::Image
        } else if mime.starts_with("video/") {
            NotifMediaKind::Video
        } else if mime.starts_with("audio/") {
            NotifMediaKind::Audio
        } else {
            NotifMediaKind::File
        };
        return Some(NotifMedia { kind, tag });
    }
    None
}

/// Max encrypted download size for NSE image thumbnails (10 MB).
const MAX_NSE_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

/// Download encrypted image from the URL in the imeta tag and decrypt it via MDK.
/// Returns `None` on any failure so the notification still shows with text only.
fn download_and_decrypt_image(
    mdk: &mdk_support::PikaMdk,
    mls_group_id: &mdk_storage_traits::GroupId,
    imeta_tag: &nostr::Tag,
) -> Option<Vec<u8>> {
    let manager = mdk.media_manager(mls_group_id.clone());
    let reference = manager.parse_imeta_tag(imeta_tag).ok()?;

    let agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(std::time::Duration::from_secs(8)))
        .build()
        .new_agent();

    let response = agent.get(&reference.url).call().ok()?;

    // Bail if the server reports a size larger than our cap.
    if let Some(len) = response.headers().get("content-length") {
        if let Ok(n) = len.to_str().unwrap_or("0").parse::<u64>() {
            if n > MAX_NSE_IMAGE_BYTES {
                return None;
            }
        }
    }

    let encrypted = response
        .into_body()
        .with_config()
        .limit(MAX_NSE_IMAGE_BYTES)
        .read_to_vec()
        .ok()?;

    manager.decrypt_from_download(&encrypted, &reference).ok()
}

/// Look up display name and picture URL from the SQLite profile cache.
/// If `chat_id` is provided, checks for a group-specific profile first,
/// falling back to the global profile.
fn resolve_sender_profile(
    data_dir: &str,
    pubkey_hex: &str,
    chat_id: Option<&str>,
) -> (String, Option<String>) {
    let fallback = (format!("{}...", &pubkey_hex[..8]), None);

    let db_path = std::path::Path::new(data_dir).join("profiles.sqlite3");
    let conn = match rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(_) => return fallback,
    };

    // Try group profile first, then fall back to global profile.
    let row: Option<(Option<String>, Option<String>, Option<String>)> = chat_id
        .and_then(|cid| {
            conn.query_row(
                "SELECT metadata->>'display_name', metadata->>'name', metadata->>'picture'
                 FROM profiles WHERE pubkey = ?1 AND chat_id = ?2",
                rusqlite::params![pubkey_hex, cid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok()
            .filter(|(dn, n, _): &(Option<String>, Option<String>, _)| {
                dn.as_ref().is_some_and(|s| !s.is_empty())
                    || n.as_ref().is_some_and(|s| !s.is_empty())
            })
        })
        .or_else(|| {
            conn.query_row(
                "SELECT metadata->>'display_name', metadata->>'name', metadata->>'picture'
                 FROM profiles WHERE pubkey = ?1 AND chat_id IS NULL",
                [pubkey_hex],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok()
        });

    let Some((display_name, name_field, picture)) = row else {
        return fallback;
    };

    let name = display_name
        .filter(|s| !s.is_empty())
        .or(name_field.filter(|s| !s.is_empty()))
        .unwrap_or_else(|| format!("{}...", &pubkey_hex[..8]));

    let picture_url = picture.filter(|s| !s.is_empty()).map(|url| {
        // Prefer locally cached profile picture if available.
        // Check group-specific cache first, then global cache.
        let base = std::path::Path::new(data_dir).join("profile_pics");
        let group_cached = chat_id
            .map(|cid| base.join(format!("group_{cid}")).join(pubkey_hex))
            .filter(|p| p.exists());
        let global_cached = base.join(pubkey_hex);
        if let Some(path) = group_cached {
            format!("file://{}", path.display())
        } else if global_cached.exists() {
            format!("file://{}", global_cached.display())
        } else {
            url
        }
    });

    (name, picture_url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{Tag, Tags};

    fn imeta_tag(mime: &str) -> Tag {
        Tag::parse(vec![
            "imeta",
            "url https://example.com/file",
            &format!("m {mime}"),
        ])
        .unwrap()
    }

    fn tags_from(v: Vec<Tag>) -> Tags {
        v.into_iter().collect()
    }

    #[test]
    fn media_kind_image() {
        let tags = tags_from(vec![imeta_tag("image/jpeg")]);
        let m = notif_media(&tags).unwrap();
        assert_eq!(m.kind.label(), "Sent a photo");
        assert_eq!(m.kind.emoji(), "\u{1F4F7}");
    }

    #[test]
    fn media_kind_video() {
        let tags = tags_from(vec![imeta_tag("video/mp4")]);
        let m = notif_media(&tags).unwrap();
        assert_eq!(m.kind.label(), "Sent a video");
        assert_eq!(m.kind.emoji(), "\u{1F3AC}");
    }

    #[test]
    fn media_kind_audio() {
        let tags = tags_from(vec![imeta_tag("audio/mp4")]);
        let m = notif_media(&tags).unwrap();
        assert_eq!(m.kind.label(), "Sent a voice message");
        assert_eq!(m.kind.emoji(), "\u{1F3A4}");
    }

    #[test]
    fn media_kind_unknown_mime() {
        let tags = tags_from(vec![imeta_tag("application/pdf")]);
        let m = notif_media(&tags).unwrap();
        assert_eq!(m.kind.label(), "Sent a file");
        assert_eq!(m.kind.emoji(), "\u{1F4CE}");
    }

    #[test]
    fn media_kind_no_mime() {
        let tag = Tag::parse(vec!["imeta", "url https://example.com/file"]).unwrap();
        let tags = tags_from(vec![tag]);
        let m = notif_media(&tags).unwrap();
        assert_eq!(m.kind.label(), "Sent a file");
    }

    #[test]
    fn media_kind_no_imeta_tags() {
        let tag = Tag::parse(vec!["e", "abc123"]).unwrap();
        let tags = tags_from(vec![tag]);
        assert!(notif_media(&tags).is_none());
    }

    #[test]
    fn media_kind_empty_tags() {
        let tags = tags_from(vec![]);
        assert!(notif_media(&tags).is_none());
    }
}
