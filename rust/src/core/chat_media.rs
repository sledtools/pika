use std::path::{Path, PathBuf};

use base64::Engine;
use mdk_core::encrypted_media::types::{MediaProcessingOptions, MediaReference};
use nostr_blossom::client::BlossomClient;
use sha2::{Digest, Sha256};

use crate::state::{ChatMediaAttachment, ChatMediaKind};

use super::chat_media_db::{self, ChatMediaRecord};
use super::*;

const MAX_CHAT_MEDIA_BYTES: usize = 32 * 1024 * 1024;

/// Map file extension to a MIME type that MDK's encrypted-media allowlist
/// accepts.  Types not on MDK's `SUPPORTED_MIME_TYPES` list must map to
/// `application/octet-stream` (MDK's escape-hatch type) so that arbitrary
/// files can be uploaded without validation errors.
fn mime_type_for_extension(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        // Image types (on MDK allowlist)
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "tiff" | "tif" => "image/tiff",
        "avif" => "image/avif",
        // Video types (on MDK allowlist)
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        // Audio types (on MDK allowlist)
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        // Document types (on MDK allowlist)
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        // Everything else → octet-stream (MDK escape hatch, skips validation)
        _ => "application/octet-stream",
    }
}

fn mime_type_for_filename(filename: &str) -> String {
    let ext = filename.rsplit('.').next().unwrap_or("");
    mime_type_for_extension(ext).to_string()
}

fn normalized_mime_type(mime_type: &str) -> String {
    mime_type.trim().to_ascii_lowercase()
}

fn is_voice_note_filename(filename: &str) -> bool {
    let normalized = filename.trim().to_ascii_lowercase();
    normalized.starts_with("voice_") && normalized.ends_with(".m4a")
}

fn infer_media_kind(mime_type: &str, filename: &str) -> ChatMediaKind {
    let normalized_mime = normalized_mime_type(mime_type);
    if normalized_mime.starts_with("image/") {
        return ChatMediaKind::Image;
    }
    if normalized_mime.starts_with("audio/") {
        return ChatMediaKind::VoiceNote;
    }

    if normalized_mime.is_empty() || normalized_mime == "application/octet-stream" {
        let inferred_mime = mime_type_for_filename(filename);
        if inferred_mime.starts_with("image/") {
            return ChatMediaKind::Image;
        }
        if inferred_mime.starts_with("audio/") {
            return ChatMediaKind::VoiceNote;
        }
    }

    if is_voice_note_filename(filename) {
        return ChatMediaKind::VoiceNote;
    }

    ChatMediaKind::File
}

fn sanitize_filename(filename: &str) -> String {
    let mut out = String::with_capacity(filename.len().min(120));
    for ch in filename.chars().take(120) {
        let allowed = ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_';
        out.push(if allowed { ch } else { '_' });
    }
    let trimmed = out.trim_matches('_').trim();
    if trimmed.is_empty() {
        "file.bin".to_string()
    } else {
        trimmed.to_string()
    }
}

fn media_root(data_dir: &str) -> PathBuf {
    Path::new(data_dir).join("chat_media")
}

fn media_file_path(
    data_dir: &str,
    account_pubkey: &str,
    chat_id: &str,
    original_hash_hex: &str,
    filename: &str,
) -> PathBuf {
    let name = sanitize_filename(filename);
    media_root(data_dir)
        .join(account_pubkey)
        .join(chat_id)
        .join(original_hash_hex)
        .join(name)
}

fn write_media_file(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create media dir failed: {e}"))?;
    }
    std::fs::write(path, data).map_err(|e| format!("write media file failed: {e}"))?;
    Ok(())
}

fn path_if_exists(path: &Path) -> Option<String> {
    if path.exists() {
        Some(path.to_string_lossy().to_string())
    } else {
        None
    }
}

pub(super) fn is_imeta_tag(tag: &Tag) -> bool {
    matches!(tag.kind(), TagKind::Custom(kind) if kind.as_ref() == "imeta")
}

impl AppCore {
    fn attachment_from_reference(
        &self,
        chat_id: &str,
        account_pubkey: &str,
        reference: &MediaReference,
        encrypted_hash_hex: Option<String>,
    ) -> ChatMediaAttachment {
        let original_hash_hex = hex::encode(reference.original_hash);
        let local_path = path_if_exists(&media_file_path(
            &self.data_dir,
            account_pubkey,
            chat_id,
            &original_hash_hex,
            &reference.filename,
        ));
        let (width, height) = reference
            .dimensions
            .map(|(w, h)| (Some(w), Some(h)))
            .unwrap_or((None, None));
        let normalized_mime = if reference.mime_type.trim().is_empty() {
            mime_type_for_filename(&reference.filename)
        } else {
            normalized_mime_type(&reference.mime_type)
        };
        let kind = infer_media_kind(&normalized_mime, &reference.filename);

        ChatMediaAttachment {
            original_hash_hex,
            encrypted_hash_hex,
            url: reference.url.clone(),
            mime_type: normalized_mime,
            filename: reference.filename.clone(),
            kind,
            width,
            height,
            nonce_hex: hex::encode(reference.nonce),
            scheme_version: reference.scheme_version.clone(),
            local_path,
        }
    }

    pub(super) fn chat_media_attachments_for_tags(
        &self,
        mdk: &PikaMdk,
        group_id: &GroupId,
        chat_id: &str,
        account_pubkey: &str,
        tags: &Tags,
    ) -> Vec<ChatMediaAttachment> {
        let manager = mdk.media_manager(group_id.clone());
        let mut out = Vec::new();

        for tag in tags.iter() {
            if !is_imeta_tag(tag) {
                continue;
            }

            let reference = match manager.parse_imeta_tag(tag) {
                Ok(reference) => reference,
                Err(e) => {
                    tracing::warn!(%e, "invalid imeta tag in chat message");
                    continue;
                }
            };

            let original_hash_hex = hex::encode(reference.original_hash);
            let encrypted_hash_hex = self.chat_media_db.as_ref().and_then(|conn| {
                chat_media_db::get_chat_media(conn, account_pubkey, chat_id, &original_hash_hex)
                    .map(|r| r.encrypted_hash_hex)
            });

            out.push(self.attachment_from_reference(
                chat_id,
                account_pubkey,
                &reference,
                encrypted_hash_hex,
            ));
        }

        out
    }

    pub(super) fn send_chat_media(
        &mut self,
        chat_id: String,
        data_base64: String,
        mime_type: String,
        filename: String,
        caption: String,
    ) {
        if !self.is_logged_in() {
            self.toast("Please log in first");
            return;
        }
        if !self.network_enabled() {
            self.toast("Network disabled");
            return;
        }

        let decoded = match base64::engine::general_purpose::STANDARD.decode(data_base64) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.toast(format!("Invalid media data: {e}"));
                return;
            }
        };
        if decoded.is_empty() {
            self.toast("Pick media first");
            return;
        }
        if decoded.len() > MAX_CHAT_MEDIA_BYTES {
            self.toast("Media too large (max 32 MB)");
            return;
        }

        let filename = filename.trim().to_string();
        if filename.is_empty() {
            self.toast("Filename is required");
            return;
        }

        let mime_type = if mime_type.trim().is_empty() {
            mime_type_for_filename(&filename)
        } else {
            normalized_mime_type(&mime_type)
        };

        let caption = caption.trim().to_string();

        let (
            request_id,
            encrypted_data,
            expected_hash_hex,
            upload_mime,
            signer_keys,
            blossom_servers,
        ) = {
            let Some(sess) = self.session.as_mut() else {
                return;
            };
            let Some(group) = sess.groups.get(&chat_id).cloned() else {
                self.toast("Chat not found");
                return;
            };
            let Some(local_keys) = sess.local_keys.clone() else {
                self.toast("Media upload requires local key signer");
                return;
            };

            let manager = sess.mdk.media_manager(group.mls_group_id.clone());
            let mut upload = match manager.encrypt_for_upload_with_options(
                &decoded,
                &mime_type,
                &filename,
                &MediaProcessingOptions::default(),
            ) {
                Ok(upload) => upload,
                Err(e) => {
                    self.toast(format!("Media encryption failed: {e}"));
                    return;
                }
            };

            let account_pubkey = sess.pubkey.to_hex();
            let original_hash_hex = hex::encode(upload.original_hash);
            let local_path = media_file_path(
                &self.data_dir,
                &account_pubkey,
                &chat_id,
                &original_hash_hex,
                &upload.filename,
            );
            if let Err(e) = write_media_file(&local_path, &decoded) {
                self.toast(format!("Media cache failed: {e}"));
                return;
            }

            let encrypted_data = std::mem::take(&mut upload.encrypted_data);
            let expected_hash_hex = hex::encode(upload.encrypted_hash);
            let upload_mime = upload.mime_type.clone();
            let request_id = uuid::Uuid::new_v4().to_string();

            self.pending_media_sends.insert(
                request_id.clone(),
                PendingMediaSend {
                    chat_id: chat_id.clone(),
                    caption,
                    upload,
                    account_pubkey,
                },
            );

            (
                request_id,
                encrypted_data,
                expected_hash_hex,
                upload_mime,
                local_keys,
                self.blossom_servers(),
            )
        };

        let tx = self.core_sender.clone();
        self.runtime.spawn(async move {
            let result = upload_to_blossom(
                &blossom_servers,
                encrypted_data,
                &upload_mime,
                &expected_hash_hex,
                &signer_keys,
            )
            .await;
            let event = match result {
                Ok((url, hash)) => InternalEvent::ChatMediaUploadCompleted {
                    request_id,
                    uploaded_url: Some(url),
                    descriptor_sha256_hex: Some(hash),
                    error: None,
                },
                Err(e) => InternalEvent::ChatMediaUploadCompleted {
                    request_id,
                    uploaded_url: None,
                    descriptor_sha256_hex: None,
                    error: Some(e),
                },
            };
            let _ = tx.send(CoreMsg::Internal(Box::new(event)));
        });
    }

    pub(super) fn handle_chat_media_upload_completed(
        &mut self,
        request_id: String,
        uploaded_url: Option<String>,
        descriptor_sha256_hex: Option<String>,
        error: Option<String>,
    ) {
        let Some(pending) = self.pending_media_sends.remove(&request_id) else {
            return;
        };

        if let Some(e) = error {
            self.toast(format!("Media upload failed: {e}"));
            return;
        }

        let Some(uploaded_url) = uploaded_url else {
            self.toast("Media upload failed: missing upload URL");
            return;
        };
        let Some(descriptor_hash) = descriptor_sha256_hex else {
            self.toast("Media upload failed: missing uploaded hash");
            return;
        };

        let expected_hash_hex = hex::encode(pending.upload.encrypted_hash);
        if !descriptor_hash.eq_ignore_ascii_case(&expected_hash_hex) {
            self.toast("Media upload failed: uploaded hash mismatch");
            return;
        }

        let Some(sess) = self.session.as_mut() else {
            return;
        };
        let Some(group) = sess.groups.get(&pending.chat_id).cloned() else {
            self.toast("Chat not found");
            return;
        };

        let manager = sess.mdk.media_manager(group.mls_group_id.clone());
        let imeta_tag = manager.create_imeta_tag(&pending.upload, &uploaded_url);
        let reference = manager.create_media_reference(&pending.upload, uploaded_url.clone());

        if let Some(conn) = self.chat_media_db.as_ref() {
            let record = ChatMediaRecord {
                account_pubkey: pending.account_pubkey.clone(),
                chat_id: pending.chat_id.clone(),
                original_hash_hex: hex::encode(pending.upload.original_hash),
                encrypted_hash_hex: expected_hash_hex.clone(),
                url: uploaded_url.clone(),
                mime_type: pending.upload.mime_type.clone(),
                filename: pending.upload.filename.clone(),
                nonce_hex: hex::encode(pending.upload.nonce),
                scheme_version: reference.scheme_version.clone(),
                created_at: now_seconds(),
            };
            if let Err(e) = chat_media_db::upsert_chat_media(conn, &record) {
                tracing::warn!(%e, "failed to persist chat media metadata");
            }
        }

        let media = vec![self.attachment_from_reference(
            &pending.chat_id,
            &pending.account_pubkey,
            &reference,
            Some(expected_hash_hex),
        )];

        self.publish_chat_message_with_tags(
            pending.chat_id,
            pending.caption,
            Kind::ChatMessage,
            vec![imeta_tag],
            None,
            media,
        );
    }

    pub(super) fn publish_chat_message_with_tags(
        &mut self,
        chat_id: String,
        content: String,
        kind: Kind,
        tags: Vec<Tag>,
        reply_to_message_id: Option<String>,
        media: Vec<ChatMediaAttachment>,
    ) {
        let network_enabled = self.network_enabled();
        let fallback_relays = self.default_relays();

        // Nostr timestamps are second-granularity; rapid sends can share the same second.
        // Keep outgoing timestamps monotonic to avoid tie-related paging nondeterminism.
        let ts = {
            let now = now_seconds();
            if now <= self.last_outgoing_ts {
                self.last_outgoing_ts += 1;
            } else {
                self.last_outgoing_ts = now;
            }
            self.last_outgoing_ts
        };

        let (client, wrapper, relays, rumor_id_hex) = {
            let Some(sess) = self.session.as_mut() else {
                return;
            };
            let Some(group) = sess.groups.get(&chat_id).cloned() else {
                self.toast("Chat not found");
                return;
            };

            let mut rumor = UnsignedEvent::new(
                sess.pubkey,
                Timestamp::from(ts as u64),
                kind,
                tags,
                content.clone(),
            );
            rumor.ensure_id();
            let rumor_id_hex = rumor.id().to_hex();

            self.delivery_overrides
                .entry(chat_id.clone())
                .or_default()
                .insert(rumor_id_hex.clone(), MessageDeliveryState::Pending);

            self.outbox_seq = self.outbox_seq.wrapping_add(1);
            let seq = self.outbox_seq;
            self.local_outbox
                .entry(chat_id.clone())
                .or_default()
                .insert(
                    rumor_id_hex.clone(),
                    LocalOutgoing {
                        content: content.clone(),
                        timestamp: ts,
                        sender_pubkey: sess.pubkey.to_hex(),
                        reply_to_message_id: reply_to_message_id.clone(),
                        seq,
                        media: media.clone(),
                        kind,
                    },
                );

            let wrapper = match sess.mdk.create_message(&group.mls_group_id, rumor) {
                Ok(e) => e,
                Err(e) => {
                    self.toast(format!("Encrypt failed: {e}"));
                    self.delivery_overrides
                        .entry(chat_id.clone())
                        .or_default()
                        .insert(
                            rumor_id_hex.clone(),
                            MessageDeliveryState::Failed {
                                reason: format!("encrypt failed: {e}"),
                            },
                        );
                    self.refresh_current_chat_if_open(&chat_id);
                    self.refresh_chat_list_from_storage();
                    return;
                }
            };

            self.pending_sends
                .entry(chat_id.clone())
                .or_default()
                .insert(
                    rumor_id_hex.clone(),
                    PendingSend {
                        wrapper_event: wrapper.clone(),
                        rumor_id_hex: rumor_id_hex.clone(),
                    },
                );

            let relays: Vec<RelayUrl> = if network_enabled {
                sess.mdk
                    .get_relays(&group.mls_group_id)
                    .ok()
                    .map(|s| s.into_iter().collect())
                    .filter(|v: &Vec<RelayUrl>| !v.is_empty())
                    .unwrap_or_else(|| fallback_relays.clone())
            } else {
                vec![]
            };

            (sess.client.clone(), wrapper, relays, rumor_id_hex)
        };

        self.prune_local_outbox(&chat_id);
        self.refresh_chat_list_from_storage();
        self.refresh_current_chat_if_open(&chat_id);

        if !network_enabled {
            let _ = self.core_sender.send(CoreMsg::Internal(Box::new(
                InternalEvent::PublishMessageResult {
                    chat_id,
                    rumor_id: rumor_id_hex,
                    ok: true,
                    error: None,
                },
            )));
            return;
        }

        let _ = self.core_sender.send(CoreMsg::Internal(Box::new(
            InternalEvent::PublishMessageResult {
                chat_id: chat_id.clone(),
                rumor_id: rumor_id_hex.clone(),
                ok: true,
                error: None,
            },
        )));

        let diag = diag_nostr_publish_enabled();
        let wrapper_id = wrapper.id.to_hex();
        let wrapper_kind = wrapper.kind.as_u16();
        let relay_list: Vec<String> = relays.iter().map(|r| r.to_string()).collect();
        self.runtime.spawn(async move {
            let out = client.send_event_to(relays, &wrapper).await;
            match out {
                Ok(output) => {
                    if diag {
                        tracing::info!(
                            target: "pika_core::nostr_publish",
                            context = "group_message",
                            rumor_id = %rumor_id_hex,
                            event_id = %wrapper_id,
                            kind = wrapper_kind,
                            relays = ?relay_list,
                            success = ?output.success,
                            failed = ?output.failed,
                        );
                    }
                }
                Err(e) => {
                    if diag {
                        tracing::info!(
                            target: "pika_core::nostr_publish",
                            context = "group_message",
                            rumor_id = %rumor_id_hex,
                            event_id = %wrapper_id,
                            kind = wrapper_kind,
                            relays = ?relay_list,
                            error = %e,
                        );
                    } else {
                        tracing::warn!(%e, "message broadcast failed");
                    }
                }
            }
        });
    }

    pub(super) fn download_chat_media(
        &mut self,
        chat_id: String,
        message_id: String,
        original_hash_hex: String,
    ) {
        if !self.is_logged_in() {
            self.toast("Please log in first");
            return;
        }

        let target_hash = original_hash_hex.trim().to_ascii_lowercase();
        if target_hash.len() != 64 || !target_hash.chars().all(|c| c.is_ascii_hexdigit()) {
            self.toast("Invalid media hash");
            return;
        }

        let (request_id, url) = {
            let Some(sess) = self.session.as_mut() else {
                return;
            };
            let Some(group) = sess.groups.get(&chat_id).cloned() else {
                self.toast("Chat not found");
                return;
            };

            let message_event_id = match EventId::parse(&message_id) {
                Ok(id) => id,
                Err(e) => {
                    self.toast(format!("Invalid message id: {e}"));
                    return;
                }
            };

            let message = match sess.mdk.get_message(&group.mls_group_id, &message_event_id) {
                Ok(Some(message)) => message,
                Ok(None) => {
                    self.toast("Message not found");
                    return;
                }
                Err(e) => {
                    self.toast(format!("Message lookup failed: {e}"));
                    return;
                }
            };

            let manager = sess.mdk.media_manager(group.mls_group_id.clone());
            let Some(reference) = message
                .tags
                .iter()
                .filter_map(|tag| {
                    if !is_imeta_tag(tag) {
                        return None;
                    }
                    manager.parse_imeta_tag(tag).ok()
                })
                .find(|reference| hex::encode(reference.original_hash) == target_hash)
            else {
                self.toast("Media reference not found");
                return;
            };

            let account_pubkey = sess.pubkey.to_hex();
            let local_path = media_file_path(
                &self.data_dir,
                &account_pubkey,
                &chat_id,
                &target_hash,
                &reference.filename,
            );
            if local_path.exists() {
                self.refresh_current_chat_if_open(&chat_id);
                return;
            }

            if !self.network_enabled() {
                self.toast("Network disabled");
                return;
            }

            let encrypted_hash_hex = self.chat_media_db.as_ref().and_then(|conn| {
                chat_media_db::get_chat_media(conn, &account_pubkey, &chat_id, &target_hash)
                    .map(|r| r.encrypted_hash_hex)
            });

            let request_id = uuid::Uuid::new_v4().to_string();
            self.pending_media_downloads.insert(
                request_id.clone(),
                PendingMediaDownload {
                    chat_id: chat_id.clone(),
                    account_pubkey,
                    group_id: group.mls_group_id.clone(),
                    reference: reference.clone(),
                    encrypted_hash_hex,
                },
            );

            (request_id, reference.url)
        };

        let tx = self.core_sender.clone();
        let client = self.http_client.clone();
        self.runtime.spawn(async move {
            let response = match client.get(&url).send().await {
                Ok(response) => response,
                Err(e) => {
                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::ChatMediaDownloadFetched {
                            request_id,
                            encrypted_data: None,
                            error: Some(format!("Media download failed: {e}")),
                        },
                    )));
                    return;
                }
            };

            if !response.status().is_success() {
                let _ = tx.send(CoreMsg::Internal(Box::new(
                    InternalEvent::ChatMediaDownloadFetched {
                        request_id,
                        encrypted_data: None,
                        error: Some(format!("Media download failed: HTTP {}", response.status())),
                    },
                )));
                return;
            }

            match response.bytes().await {
                Ok(bytes) => {
                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::ChatMediaDownloadFetched {
                            request_id,
                            encrypted_data: Some(bytes.to_vec()),
                            error: None,
                        },
                    )));
                }
                Err(e) => {
                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::ChatMediaDownloadFetched {
                            request_id,
                            encrypted_data: None,
                            error: Some(format!("Media download failed: {e}")),
                        },
                    )));
                }
            }
        });
    }

    pub(super) fn handle_chat_media_download_fetched(
        &mut self,
        request_id: String,
        encrypted_data: Option<Vec<u8>>,
        error: Option<String>,
    ) {
        let Some(pending) = self.pending_media_downloads.remove(&request_id) else {
            return;
        };

        if let Some(e) = error {
            self.toast(e);
            return;
        }

        let Some(encrypted_data) = encrypted_data else {
            self.toast("Media download failed: empty response");
            return;
        };

        if let Some(expected_hash_hex) = pending.encrypted_hash_hex.as_ref() {
            let actual_hash_hex = hex::encode(Sha256::digest(&encrypted_data));
            if !actual_hash_hex.eq_ignore_ascii_case(expected_hash_hex) {
                self.toast("Media download failed: ciphertext hash mismatch");
                return;
            }
        }

        let Some(sess) = self.session.as_mut() else {
            return;
        };

        let manager = sess.mdk.media_manager(pending.group_id.clone());
        let decrypted = match manager.decrypt_from_download(&encrypted_data, &pending.reference) {
            Ok(data) => data,
            Err(e) => {
                self.toast(format!("Media decrypt failed: {e}"));
                return;
            }
        };

        let original_hash_hex = hex::encode(pending.reference.original_hash);
        let local_path = media_file_path(
            &self.data_dir,
            &pending.account_pubkey,
            &pending.chat_id,
            &original_hash_hex,
            &pending.reference.filename,
        );
        if let Err(e) = write_media_file(&local_path, &decrypted) {
            self.toast(format!("Media cache failed: {e}"));
            return;
        }

        self.refresh_current_chat_if_open(&pending.chat_id);
    }
}

/// Upload data to the first available Blossom server, verifying the hash.
/// Returns `(uploaded_url, descriptor_hash_hex)` on success.
pub(super) async fn upload_to_blossom(
    servers: &[String],
    data: Vec<u8>,
    mime_type: &str,
    expected_hash_hex: &str,
    signer: &nostr_sdk::Keys,
) -> Result<(String, String), String> {
    if servers.is_empty() {
        return Err("No valid Blossom servers configured".to_string());
    }

    let mut last_error: Option<String> = None;
    for server in servers {
        let base_url = match Url::parse(server) {
            Ok(url) => url,
            Err(e) => {
                last_error = Some(format!("{server}: {e}"));
                continue;
            }
        };

        let blossom = BlossomClient::new(base_url);
        let descriptor = match blossom
            .upload_blob(
                data.clone(),
                Some(mime_type.to_string()),
                None,
                Some(signer),
            )
            .await
        {
            Ok(d) => d,
            Err(e) => {
                last_error = Some(format!("{server}: {e}"));
                continue;
            }
        };

        let descriptor_hash_hex = descriptor.sha256.to_string();
        if !descriptor_hash_hex.eq_ignore_ascii_case(expected_hash_hex) {
            last_error = Some(format!(
                "{server}: uploaded hash mismatch (expected {expected_hash_hex}, got {descriptor_hash_hex})"
            ));
            continue;
        }

        return Ok((descriptor.url.to_string(), descriptor_hash_hex));
    }

    Err(last_error.unwrap_or_else(|| "Blossom upload failed".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_media_kind_accepts_audio_mime_case_insensitively() {
        assert!(matches!(
            infer_media_kind("Audio/MP4", "clip.m4a"),
            ChatMediaKind::VoiceNote
        ));
    }

    #[test]
    fn infer_media_kind_treats_octet_stream_m4a_as_voice_note() {
        assert!(matches!(
            infer_media_kind("application/octet-stream", "voice_1700000000.m4a"),
            ChatMediaKind::VoiceNote
        ));
    }

    #[test]
    fn infer_media_kind_treats_empty_mime_m4a_as_voice_note() {
        assert!(matches!(
            infer_media_kind("", "voice_1700000001.m4a"),
            ChatMediaKind::VoiceNote
        ));
    }

    #[test]
    fn infer_media_kind_treats_unknown_pdf_as_file() {
        assert!(matches!(
            infer_media_kind("application/octet-stream", "doc.pdf"),
            ChatMediaKind::File
        ));
    }

    #[test]
    fn infer_media_kind_image_from_mime() {
        assert!(matches!(
            infer_media_kind("image/png", "file.bin"),
            ChatMediaKind::Image
        ));
    }

    #[test]
    fn infer_media_kind_file_for_unknown() {
        assert!(matches!(
            infer_media_kind("application/octet-stream", "data.bin"),
            ChatMediaKind::File
        ));
    }

    #[test]
    fn infer_media_kind_image_from_filename_when_mime_empty() {
        assert!(matches!(
            infer_media_kind("", "photo.jpg"),
            ChatMediaKind::Image
        ));
    }

    #[test]
    fn infer_media_kind_voice_note_from_filename_pattern() {
        assert!(matches!(
            infer_media_kind("application/octet-stream", "voice_1234567890.m4a"),
            ChatMediaKind::VoiceNote
        ));
    }

    // --- sanitize_filename tests ---

    #[test]
    fn sanitize_preserves_valid_chars() {
        assert_eq!(sanitize_filename("photo-2024_01.jpg"), "photo-2024_01.jpg");
    }

    #[test]
    fn sanitize_replaces_special_chars() {
        assert_eq!(sanitize_filename("my photo (1).jpg"), "my_photo__1_.jpg");
    }

    #[test]
    fn sanitize_truncates_at_120() {
        let long = "a".repeat(200) + ".jpg";
        let result = sanitize_filename(&long);
        assert!(
            result.len() <= 120,
            "expected <= 120 chars, got {}",
            result.len()
        );
    }

    #[test]
    fn sanitize_empty_returns_default() {
        assert_eq!(sanitize_filename(""), "file.bin");
    }

    #[test]
    fn sanitize_all_special_returns_default() {
        assert_eq!(sanitize_filename("@#$%^&*()"), "file.bin");
    }

    #[test]
    fn sanitize_trims_underscores() {
        assert_eq!(sanitize_filename("___photo.jpg___"), "photo.jpg");
    }

    // --- mime_type_for_extension tests ---

    #[test]
    fn mime_known_image_types() {
        assert_eq!(mime_type_for_extension("jpg"), "image/jpeg");
        assert_eq!(mime_type_for_extension("jpeg"), "image/jpeg");
        assert_eq!(mime_type_for_extension("png"), "image/png");
        assert_eq!(mime_type_for_extension("gif"), "image/gif");
        assert_eq!(mime_type_for_extension("webp"), "image/webp");
        assert_eq!(mime_type_for_extension("avif"), "image/avif");
    }

    #[test]
    fn mime_known_audio_types() {
        assert_eq!(mime_type_for_extension("mp3"), "audio/mpeg");
        assert_eq!(mime_type_for_extension("m4a"), "audio/mp4");
        assert_eq!(mime_type_for_extension("ogg"), "audio/ogg");
        assert_eq!(mime_type_for_extension("wav"), "audio/wav");
        assert_eq!(mime_type_for_extension("flac"), "audio/flac");
    }

    #[test]
    fn mime_known_video_types() {
        assert_eq!(mime_type_for_extension("mp4"), "video/mp4");
        assert_eq!(mime_type_for_extension("mov"), "video/quicktime");
        assert_eq!(mime_type_for_extension("mkv"), "video/x-matroska");
        assert_eq!(mime_type_for_extension("webm"), "video/webm");
    }

    #[test]
    fn mime_known_doc_types() {
        assert_eq!(mime_type_for_extension("pdf"), "application/pdf");
        assert_eq!(mime_type_for_extension("txt"), "text/plain");
    }

    #[test]
    fn mime_unknown_is_octet_stream() {
        assert_eq!(mime_type_for_extension("xyz"), "application/octet-stream");
        assert_eq!(mime_type_for_extension("docx"), "application/octet-stream");
    }

    #[test]
    fn mime_case_insensitive() {
        assert_eq!(mime_type_for_extension("JPG"), "image/jpeg");
        assert_eq!(mime_type_for_extension("PNG"), "image/png");
        assert_eq!(mime_type_for_extension("Mp4"), "video/mp4");
    }

    // --- mime_type_for_filename tests ---

    #[test]
    fn mime_from_filename_with_ext() {
        assert_eq!(mime_type_for_filename("photo.jpg"), "image/jpeg");
    }

    #[test]
    fn mime_from_filename_no_ext() {
        assert_eq!(mime_type_for_filename("README"), "application/octet-stream");
    }

    // --- normalized_mime_type tests ---

    #[test]
    fn normalized_lowercases_and_trims() {
        assert_eq!(normalized_mime_type(" Image/JPEG "), "image/jpeg");
        assert_eq!(normalized_mime_type("APPLICATION/PDF"), "application/pdf");
    }

    // --- is_voice_note_filename tests ---

    #[test]
    fn voice_note_matches_pattern() {
        assert!(is_voice_note_filename("voice_123.m4a"));
    }

    #[test]
    fn voice_note_case_insensitive() {
        assert!(is_voice_note_filename("VOICE_123.M4A"));
    }

    #[test]
    fn voice_note_rejects_non_voice() {
        assert!(!is_voice_note_filename("audio.m4a"));
    }

    #[test]
    fn voice_note_rejects_non_m4a() {
        assert!(!is_voice_note_filename("voice_123.mp3"));
    }

    // --- media_file_path tests ---

    #[test]
    fn media_path_constructs_hierarchy() {
        let path = media_file_path("/data", "acc", "chat", "hash", "photo.jpg");
        assert_eq!(
            path,
            PathBuf::from("/data/chat_media/acc/chat/hash/photo.jpg")
        );
    }

    #[test]
    fn media_path_sanitizes_filename() {
        let path = media_file_path("/data", "acc", "chat", "hash", "my photo (1).jpg");
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(
            !filename.contains(' '),
            "filename should be sanitized: {filename}"
        );
    }

    // --- is_imeta_tag tests ---

    #[test]
    fn imeta_tag_detected() {
        use nostr_sdk::prelude::*;
        let tag = Tag::parse(vec!["imeta", "url https://example.com/file.jpg"]).unwrap();
        assert!(is_imeta_tag(&tag));
    }

    #[test]
    fn non_imeta_tag_rejected() {
        use nostr_sdk::prelude::*;
        let tag = Tag::parse(vec!["e", "abc123"]).unwrap();
        assert!(!is_imeta_tag(&tag));
    }
}
