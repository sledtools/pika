use anyhow::{Context, Result};
use mdk_core::encrypted_media::types::{
    EncryptedMediaUpload, MediaProcessingOptions, MediaReference,
};
use nostr_blossom::client::BlossomClient;
use nostr_sdk::prelude::{NostrSigner, Tag, Url};
use sha2::{Digest, Sha256};

use crate::mdk_support::PikaMdk;
use mdk_storage_traits::GroupId;

pub(crate) const MAX_CHAT_MEDIA_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MediaAttachmentDescriptor {
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
pub(crate) struct PreparedMediaUpload {
    pub upload: EncryptedMediaUpload,
    pub encrypted_data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(crate) struct UploadedBlob {
    pub uploaded_url: String,
    pub descriptor_sha256_hex: String,
}

#[derive(Debug, Clone)]
pub(crate) struct MediaUploadResult {
    pub attachment: MediaAttachmentDescriptor,
    pub reference: MediaReference,
    pub imeta_tag: Tag,
    pub uploaded_blob: UploadedBlob,
}

#[derive(Debug, Clone)]
pub(crate) struct DownloadedMedia {
    pub attachment: MediaAttachmentDescriptor,
    pub decrypted_data: Vec<u8>,
}

pub(crate) fn prepare_upload(
    mdk: &PikaMdk,
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

    let manager = mdk.media_manager(mls_group_id.clone());
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

pub(crate) fn finish_upload(
    mdk: &PikaMdk,
    mls_group_id: &GroupId,
    upload: &EncryptedMediaUpload,
    uploaded_blob: UploadedBlob,
) -> MediaUploadResult {
    let manager = mdk.media_manager(mls_group_id.clone());
    let imeta_tag = manager.create_imeta_tag(upload, &uploaded_blob.uploaded_url);
    let reference = manager.create_media_reference(upload, uploaded_blob.uploaded_url.clone());
    let attachment =
        attachment_from_reference(&reference, Some(hex::encode(upload.encrypted_hash)));

    MediaUploadResult {
        attachment,
        reference,
        imeta_tag,
        uploaded_blob,
    }
}

pub(crate) fn decrypt_downloaded_media(
    mdk: &PikaMdk,
    mls_group_id: &GroupId,
    reference: &MediaReference,
    encrypted_data: &[u8],
    expected_encrypted_hash_hex: Option<&str>,
) -> Result<DownloadedMedia> {
    if let Some(expected_hash_hex) = expected_encrypted_hash_hex {
        let actual_hash_hex = hex::encode(Sha256::digest(encrypted_data));
        if !actual_hash_hex.eq_ignore_ascii_case(expected_hash_hex) {
            anyhow::bail!(
                "ciphertext hash mismatch (expected {expected_hash_hex}, got {actual_hash_hex})"
            );
        }
    }

    let manager = mdk.media_manager(mls_group_id.clone());
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

    Ok(DownloadedMedia {
        attachment: attachment_from_reference(
            reference,
            expected_encrypted_hash_hex.map(ToOwned::to_owned),
        ),
        decrypted_data,
    })
}

pub(crate) async fn upload_encrypted_blob<T>(
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
            uploaded_url: descriptor.url.to_string(),
            descriptor_sha256_hex,
        });
    }

    anyhow::bail!(
        "blossom upload failed: {}",
        last_error.unwrap_or_else(|| "unknown error".to_string())
    )
}

fn attachment_from_reference(
    reference: &MediaReference,
    encrypted_hash_hex: Option<String>,
) -> MediaAttachmentDescriptor {
    let (width, height) = reference
        .dimensions
        .map(|(width, height)| (Some(width), Some(height)))
        .unwrap_or((None, None));
    MediaAttachmentDescriptor {
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
