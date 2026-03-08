use std::path::Path;

use anyhow::{Context, Result};
use mdk_core::encrypted_media::types::{
    EncryptedMediaUpload, MediaProcessingOptions, MediaReference,
};
use mdk_storage_traits::{GroupId, messages::types::Message};
use nostr_blossom::client::BlossomClient;
use nostr_sdk::prelude::{NostrSigner, Tag, TagKind, Url};
use sha2::{Digest, Sha256};

use crate::PikaMdk;

pub const MAX_CHAT_MEDIA_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeMediaAttachment {
    pub url: String,
    pub mime_type: String,
    pub filename: String,
    pub original_hash_hex: String,
    pub encrypted_hash_hex: Option<String>,
    pub nonce_hex: String,
    pub scheme_version: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ParsedMediaAttachment {
    pub attachment: RuntimeMediaAttachment,
    pub reference: MediaReference,
}

#[derive(Debug, Clone)]
pub struct PreparedMediaUpload {
    pub upload: EncryptedMediaUpload,
    pub encrypted_data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct UploadedBlob {
    pub blossom_server: String,
    pub uploaded_url: String,
    pub descriptor_sha256_hex: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeMediaUploadResult {
    pub attachment: RuntimeMediaAttachment,
    pub reference: MediaReference,
    pub imeta_tag: Tag,
    pub uploaded_blob: UploadedBlob,
}

#[derive(Debug, Clone)]
pub struct RuntimeDownloadedMedia {
    pub attachment: RuntimeMediaAttachment,
    pub decrypted_data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUploadMetadata {
    pub filename: String,
    pub mime_type: String,
}

pub struct MediaRuntime<'a> {
    mdk: &'a PikaMdk,
}

impl<'a> MediaRuntime<'a> {
    pub fn new(mdk: &'a PikaMdk) -> Self {
        Self { mdk }
    }

    pub fn prepare_upload(
        &self,
        mls_group_id: &GroupId,
        bytes: &[u8],
        mime_type: Option<&str>,
        filename: Option<&str>,
    ) -> Result<PreparedMediaUpload> {
        if bytes.is_empty() {
            anyhow::bail!("media file is empty");
        }
        if bytes.len() > MAX_CHAT_MEDIA_BYTES {
            anyhow::bail!("media too large (max 32 MB)");
        }

        let resolved_filename = filename
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("file.bin");
        let resolved_mime = normalize_mime_type(
            mime_type
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("application/octet-stream"),
        );

        let manager = self.mdk.media_manager(mls_group_id.clone());
        let mut upload = manager
            .encrypt_for_upload_with_options(
                bytes,
                &resolved_mime,
                resolved_filename,
                &MediaProcessingOptions::default(),
            )
            .context("encrypt media for upload")?;
        let encrypted_data = std::mem::take(&mut upload.encrypted_data);

        Ok(PreparedMediaUpload {
            upload,
            encrypted_data,
        })
    }

    pub fn finish_upload(
        &self,
        mls_group_id: &GroupId,
        upload: &EncryptedMediaUpload,
        uploaded_blob: UploadedBlob,
    ) -> RuntimeMediaUploadResult {
        let manager = self.mdk.media_manager(mls_group_id.clone());
        let imeta_tag = manager.create_imeta_tag(upload, &uploaded_blob.uploaded_url);
        let reference = manager.create_media_reference(upload, uploaded_blob.uploaded_url.clone());
        let attachment =
            attachment_from_reference(&reference, Some(hex::encode(upload.encrypted_hash)));

        RuntimeMediaUploadResult {
            attachment,
            reference,
            imeta_tag,
            uploaded_blob,
        }
    }

    pub async fn upload_media<T>(
        &self,
        signer: &T,
        mls_group_id: &GroupId,
        bytes: &[u8],
        mime_type: Option<&str>,
        filename: Option<&str>,
        blossom_servers: &[String],
    ) -> Result<RuntimeMediaUploadResult>
    where
        T: NostrSigner,
    {
        let prepared = self.prepare_upload(mls_group_id, bytes, mime_type, filename)?;
        let uploaded_blob = upload_encrypted_blob(
            signer,
            prepared.encrypted_data,
            &prepared.upload.mime_type,
            &hex::encode(prepared.upload.encrypted_hash),
            blossom_servers,
        )
        .await?;
        Ok(self.finish_upload(mls_group_id, &prepared.upload, uploaded_blob))
    }

    pub fn parse_message_attachments(&self, message: &Message) -> Vec<ParsedMediaAttachment> {
        self.parse_attachments_from_tags(&message.mls_group_id, message.tags.iter())
    }

    pub fn parse_attachments_from_tags<'b, I>(
        &self,
        mls_group_id: &GroupId,
        tags: I,
    ) -> Vec<ParsedMediaAttachment>
    where
        I: IntoIterator<Item = &'b Tag>,
    {
        let manager = self.mdk.media_manager(mls_group_id.clone());
        tags.into_iter()
            .filter(|tag| is_imeta_tag(tag))
            .filter_map(|tag| manager.parse_imeta_tag(tag).ok())
            .map(|reference| ParsedMediaAttachment {
                attachment: attachment_from_reference(&reference, None),
                reference,
            })
            .collect()
    }

    pub async fn download_media(
        &self,
        mls_group_id: &GroupId,
        reference: &MediaReference,
        expected_encrypted_hash_hex: Option<&str>,
    ) -> Result<RuntimeDownloadedMedia> {
        let response = reqwest::Client::new()
            .get(reference.url.as_str())
            .send()
            .await
            .with_context(|| format!("download encrypted media from {}", reference.url))?;
        if !response.status().is_success() {
            anyhow::bail!("download failed: HTTP {}", response.status());
        }
        let encrypted_data = response.bytes().await.context("read media response body")?;
        self.decrypt_downloaded_media(
            mls_group_id,
            reference,
            &encrypted_data,
            expected_encrypted_hash_hex,
        )
    }

    pub fn decrypt_downloaded_media(
        &self,
        mls_group_id: &GroupId,
        reference: &MediaReference,
        encrypted_data: &[u8],
        expected_encrypted_hash_hex: Option<&str>,
    ) -> Result<RuntimeDownloadedMedia> {
        if let Some(expected_hash_hex) = expected_encrypted_hash_hex {
            let actual_hash_hex = hex::encode(Sha256::digest(encrypted_data));
            if !actual_hash_hex.eq_ignore_ascii_case(expected_hash_hex) {
                anyhow::bail!(
                    "ciphertext hash mismatch (expected {expected_hash_hex}, got {actual_hash_hex})"
                );
            }
        }

        let manager = self.mdk.media_manager(mls_group_id.clone());
        let decrypted_data = manager
            .decrypt_from_download(encrypted_data, reference)
            .context("decrypt downloaded media")?;

        let original_hash_hex = hex::encode(reference.original_hash);
        let decrypted_hash_hex = hex::encode(Sha256::digest(&decrypted_data));
        if !decrypted_hash_hex.eq_ignore_ascii_case(&original_hash_hex) {
            anyhow::bail!(
                "decrypted hash mismatch (expected {original_hash_hex}, got {decrypted_hash_hex})"
            );
        }

        Ok(RuntimeDownloadedMedia {
            attachment: attachment_from_reference(
                reference,
                expected_encrypted_hash_hex.map(ToOwned::to_owned),
            ),
            decrypted_data,
        })
    }
}

pub async fn upload_encrypted_blob<T>(
    signer: &T,
    encrypted_data: Vec<u8>,
    mime_type: &str,
    expected_hash_hex: &str,
    blossom_servers: &[String],
) -> Result<UploadedBlob>
where
    T: NostrSigner,
{
    if blossom_servers.is_empty() {
        anyhow::bail!("no valid Blossom servers configured");
    }

    let mut last_error: Option<String> = None;
    for server in blossom_servers {
        let base_url = match Url::parse(server) {
            Ok(url) => url,
            Err(err) => {
                last_error = Some(format!("{server}: {err}"));
                continue;
            }
        };

        let blossom = BlossomClient::new(base_url);
        let descriptor = match blossom
            .upload_blob(
                encrypted_data.clone(),
                Some(mime_type.to_string()),
                None,
                Some(signer),
            )
            .await
        {
            Ok(descriptor) => descriptor,
            Err(err) => {
                last_error = Some(format!("{server}: {err}"));
                continue;
            }
        };

        let descriptor_sha256_hex = descriptor.sha256.to_string();
        if !descriptor_sha256_hex.eq_ignore_ascii_case(expected_hash_hex) {
            last_error = Some(format!(
                "{server}: uploaded hash mismatch (expected {expected_hash_hex}, got {descriptor_sha256_hex})"
            ));
            continue;
        }

        return Ok(UploadedBlob {
            blossom_server: server.clone(),
            uploaded_url: descriptor.url.to_string(),
            descriptor_sha256_hex,
        });
    }

    anyhow::bail!(
        "blossom upload failed: {}",
        last_error.unwrap_or_else(|| "unknown error".to_string())
    )
}

pub fn is_imeta_tag(tag: &Tag) -> bool {
    matches!(tag.kind(), TagKind::Custom(kind) if kind.as_ref() == "imeta")
}

pub fn mime_from_extension(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    mime_from_extension_str(&ext)
}

pub fn resolve_upload_metadata(
    path: &Path,
    mime_type: Option<&str>,
    filename: Option<&str>,
) -> ResolvedUploadMetadata {
    let filename = filename
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            path.file_name()
                .and_then(|f| f.to_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "file.bin".to_string());
    let mime_type = mime_type
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(normalize_mime_type)
        .unwrap_or_else(|| {
            mime_from_extension(path)
                .unwrap_or("application/octet-stream")
                .into()
        });
    ResolvedUploadMetadata {
        filename,
        mime_type,
    }
}

fn attachment_from_reference(
    reference: &MediaReference,
    encrypted_hash_hex: Option<String>,
) -> RuntimeMediaAttachment {
    let (width, height) = reference
        .dimensions
        .map(|(width, height)| (Some(width), Some(height)))
        .unwrap_or((None, None));
    RuntimeMediaAttachment {
        url: reference.url.clone(),
        mime_type: normalize_mime_type(&reference.mime_type),
        filename: reference.filename.clone(),
        original_hash_hex: hex::encode(reference.original_hash),
        encrypted_hash_hex,
        nonce_hex: hex::encode(reference.nonce),
        scheme_version: reference.scheme_version.clone(),
        width,
        height,
    }
}

fn normalize_mime_type(mime_type: &str) -> String {
    mime_type.trim().to_ascii_lowercase()
}

fn mime_from_extension_str(ext: &str) -> Option<&'static str> {
    match ext {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "ico" => Some("image/x-icon"),
        "tiff" | "tif" => Some("image/tiff"),
        "avif" => Some("image/avif"),
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "mkv" => Some("video/x-matroska"),
        "webm" => Some("video/webm"),
        "avi" => Some("video/x-msvideo"),
        "ogg" => Some("audio/ogg"),
        "flac" => Some("audio/flac"),
        "aac" => Some("audio/aac"),
        "m4a" => Some("audio/mp4"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "heic" => Some("image/heic"),
        "svg" => Some("image/svg+xml"),
        "pdf" => Some("application/pdf"),
        "txt" | "md" => Some("text/plain"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::open_mdk;
    use nostr_sdk::prelude::{EventBuilder, Keys, Kind, RelayUrl};

    #[test]
    fn is_imeta_tag_matches() {
        let tag = Tag::parse(["imeta", "url https://example.com/img.jpg"]).unwrap();
        assert!(is_imeta_tag(&tag));
    }

    #[test]
    fn is_imeta_tag_rejects_other_tags() {
        let tag = Tag::parse(["p", "deadbeef"]).unwrap();
        assert!(!is_imeta_tag(&tag));
    }

    #[test]
    fn mime_common_types() {
        assert_eq!(
            mime_from_extension(Path::new("photo.jpg")),
            Some("image/jpeg")
        );
        assert_eq!(
            mime_from_extension(Path::new("photo.JPEG")),
            Some("image/jpeg")
        );
        assert_eq!(
            mime_from_extension(Path::new("video.mp4")),
            Some("video/mp4")
        );
        assert_eq!(
            mime_from_extension(Path::new("doc.pdf")),
            Some("application/pdf")
        );
    }

    #[test]
    fn mime_unknown_extension_defaults_to_octet_stream() {
        assert_eq!(
            mime_from_extension(Path::new("file.xyz")),
            Some("application/octet-stream")
        );
    }

    #[test]
    fn mime_no_extension() {
        assert_eq!(mime_from_extension(Path::new("README")), None);
    }

    #[test]
    fn resolve_upload_metadata_prefers_explicit_values() {
        let resolved = resolve_upload_metadata(
            Path::new("/tmp/photo.jpg"),
            Some(" Image/JPEG "),
            Some("custom-name.JPG"),
        );
        assert_eq!(resolved.filename, "custom-name.JPG");
        assert_eq!(resolved.mime_type, "image/jpeg");
    }

    #[test]
    fn resolve_upload_metadata_falls_back_to_path() {
        let resolved = resolve_upload_metadata(Path::new("/tmp/photo.jpg"), None, None);
        assert_eq!(resolved.filename, "photo.jpg");
        assert_eq!(resolved.mime_type, "image/jpeg");
    }

    fn make_key_package_event(mdk: &PikaMdk, keys: &Keys) -> nostr_sdk::prelude::Event {
        let relay = RelayUrl::parse("wss://test.relay").expect("relay url");
        let (content, tags, _hash_ref) = mdk
            .create_key_package_for_event(&keys.public_key(), vec![relay])
            .expect("create key package");
        EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign key package")
    }

    #[test]
    fn prepare_upload_and_parse_message_attachments_share_runtime_surface() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);

        let config = mdk_core::prelude::NostrGroupConfigData::new(
            "media runtime".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let runtime = MediaRuntime::new(&inviter_mdk);
        let prepared = runtime
            .prepare_upload(
                &created.group.mls_group_id,
                b"hello media",
                Some("text/plain"),
                Some("note.txt"),
            )
            .expect("prepare upload");
        let uploaded = runtime.finish_upload(
            &created.group.mls_group_id,
            &prepared.upload,
            UploadedBlob {
                blossom_server: "https://example.com".to_string(),
                uploaded_url: "https://example.com/blob".to_string(),
                descriptor_sha256_hex: hex::encode(prepared.upload.encrypted_hash),
            },
        );

        let attachments = runtime.parse_attachments_from_tags(
            &created.group.mls_group_id,
            std::iter::once(&uploaded.imeta_tag),
        );

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].attachment.filename, "note.txt");
        assert_eq!(attachments[0].attachment.mime_type, "text/plain");
        assert_eq!(
            attachments[0].attachment.original_hash_hex,
            uploaded.attachment.original_hash_hex
        );
    }

    #[test]
    fn decrypt_downloaded_media_rejects_ciphertext_hash_mismatch() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = mdk_core::prelude::NostrGroupConfigData::new(
            "runtime decrypt".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let mls_group_id = created.group.mls_group_id;

        let runtime = MediaRuntime::new(&inviter_mdk);
        let prepared = runtime
            .prepare_upload(
                &mls_group_id,
                b"secret media",
                Some("text/plain"),
                Some("x.txt"),
            )
            .expect("prepare");
        let uploaded = runtime.finish_upload(
            &mls_group_id,
            &prepared.upload,
            UploadedBlob {
                blossom_server: "https://example.com".to_string(),
                uploaded_url: "https://example.com/blob".to_string(),
                descriptor_sha256_hex: hex::encode(prepared.upload.encrypted_hash),
            },
        );
        let err = runtime
            .decrypt_downloaded_media(
                &mls_group_id,
                &uploaded.reference,
                &prepared.encrypted_data,
                Some("deadbeef"),
            )
            .expect_err("ciphertext hash mismatch should fail");
        assert!(err.to_string().contains("ciphertext hash mismatch"));
    }
}
