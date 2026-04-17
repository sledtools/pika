use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use nostr::{Tag, TagKind};
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

use crate::storage_traits::GroupId;

pub mod types {
    pub const MAX_FILE_SIZE: usize = 100 * 1024 * 1024;
    pub const MAX_FILENAME_LENGTH: usize = 210;
    pub const MAX_IMAGE_DIMENSION: u32 = 16384;

    #[derive(Debug, Clone)]
    pub struct MediaProcessingOptions {
        pub sanitize_exif: bool,
        pub generate_blurhash: bool,
        pub max_dimension: Option<u32>,
        pub max_file_size: Option<usize>,
        pub max_filename_length: Option<usize>,
    }

    impl Default for MediaProcessingOptions {
        fn default() -> Self {
            Self {
                sanitize_exif: true,
                generate_blurhash: true,
                max_dimension: Some(MAX_IMAGE_DIMENSION),
                max_file_size: Some(MAX_FILE_SIZE),
                max_filename_length: Some(MAX_FILENAME_LENGTH),
            }
        }
    }

    impl MediaProcessingOptions {
        pub fn validation_only() -> Self {
            Self {
                sanitize_exif: false,
                generate_blurhash: false,
                ..Default::default()
            }
        }
    }

    #[derive(Debug, Clone)]
    pub struct EncryptedMediaUpload {
        pub encrypted_data: Vec<u8>,
        pub original_hash: [u8; 32],
        pub encrypted_hash: [u8; 32],
        pub mime_type: String,
        pub filename: String,
        pub original_size: u64,
        pub encrypted_size: u64,
        pub dimensions: Option<(u32, u32)>,
        pub blurhash: Option<String>,
        pub nonce: [u8; 12],
    }

    #[derive(Debug, Clone)]
    pub struct MediaReference {
        pub url: String,
        pub original_hash: [u8; 32],
        pub mime_type: String,
        pub filename: String,
        pub dimensions: Option<(u32, u32)>,
        pub scheme_version: String,
        pub nonce: [u8; 12],
    }

    #[derive(Debug, thiserror::Error)]
    pub enum EncryptedMediaError {
        #[error("media processing failed: {reason}")]
        MediaProcessing { reason: String },
        #[error("unsupported MIME type: {mime_type}")]
        UnsupportedMimeType { mime_type: String },
        #[error("encryption failed: {reason}")]
        EncryptionFailed { reason: String },
        #[error("decryption failed: {reason}")]
        DecryptionFailed { reason: String },
        #[error("hash verification failed")]
        HashVerificationFailed,
        #[error("MLS group not found")]
        GroupNotFound,
        #[error("invalid encryption nonce")]
        InvalidNonce,
        #[error("invalid IMETA tag format: {reason}")]
        InvalidImetaTag { reason: String },
        #[error("no exporter secret found for epoch {0}")]
        NoExporterSecretForEpoch(u64),
        #[error("unknown encryption scheme version: {0}")]
        UnknownSchemeVersion(String),
    }
}

pub mod crypto {
    use super::*;
    use types::EncryptedMediaError;

    pub const DEFAULT_SCHEME_VERSION: &str = "pika-media-v2";

    pub fn derive_encryption_key(
        group_id: &GroupId,
        key_context: &[u8; 32],
        scheme_version: &str,
        original_hash: &[u8; 32],
        mime_type: &str,
        filename: &str,
    ) -> Result<[u8; 32], EncryptedMediaError> {
        let mut hasher = Sha256::new();
        hasher.update(b"pika-media-key-v1");
        hasher.update(group_id.as_slice());
        hasher.update(key_context);
        hasher.update(scheme_version.as_bytes());
        hasher.update(original_hash);
        hasher.update(mime_type.as_bytes());
        hasher.update(filename.as_bytes());
        Ok(hasher.finalize().into())
    }

    pub(super) fn encrypt_bytes(
        data: &[u8],
        key: &[u8; 32],
        nonce: &[u8; 12],
        aad: &[u8],
    ) -> Result<Vec<u8>, EncryptedMediaError> {
        let cipher =
            Aes256Gcm::new_from_slice(key).map_err(|_| EncryptedMediaError::EncryptionFailed {
                reason: "invalid key length".to_string(),
            })?;
        cipher
            .encrypt(Nonce::from_slice(nonce), Payload { msg: data, aad })
            .map_err(|_| EncryptedMediaError::EncryptionFailed {
                reason: "AES-GCM seal failed".to_string(),
            })
    }

    pub(super) fn decrypt_bytes(
        data: &[u8],
        key: &[u8; 32],
        nonce: &[u8; 12],
        aad: &[u8],
    ) -> Result<Vec<u8>, EncryptedMediaError> {
        let cipher =
            Aes256Gcm::new_from_slice(key).map_err(|_| EncryptedMediaError::DecryptionFailed {
                reason: "invalid key length".to_string(),
            })?;
        cipher
            .decrypt(Nonce::from_slice(nonce), Payload { msg: data, aad })
            .map_err(|_| EncryptedMediaError::DecryptionFailed {
                reason: "AES-GCM open failed".to_string(),
            })
    }
}

pub mod manager {
    use super::*;
    use crypto::{DEFAULT_SCHEME_VERSION, decrypt_bytes, derive_encryption_key, encrypt_bytes};
    use types::{
        EncryptedMediaError, EncryptedMediaUpload, MediaProcessingOptions, MediaReference,
    };

    pub struct EncryptedMediaManager {
        group_id: GroupId,
        key_context: [u8; 32],
    }

    impl EncryptedMediaManager {
        pub(crate) fn new(group_id: GroupId, key_context: [u8; 32]) -> Self {
            Self {
                group_id,
                key_context,
            }
        }

        pub fn encrypt_for_upload_with_options(
            &self,
            bytes: &[u8],
            mime_type: &str,
            filename: &str,
            options: &MediaProcessingOptions,
        ) -> Result<EncryptedMediaUpload, EncryptedMediaError> {
            let max_size = options.max_file_size.unwrap_or(types::MAX_FILE_SIZE);
            if bytes.len() > max_size {
                return Err(EncryptedMediaError::MediaProcessing {
                    reason: format!("file too large: {} bytes", bytes.len()),
                });
            }
            let max_filename = options
                .max_filename_length
                .unwrap_or(types::MAX_FILENAME_LENGTH);
            if filename.len() > max_filename {
                return Err(EncryptedMediaError::MediaProcessing {
                    reason: format!("filename too long: {} bytes", filename.len()),
                });
            }

            let original_hash: [u8; 32] = Sha256::digest(bytes).into();
            let mut nonce = [0u8; 12];
            OsRng.fill_bytes(&mut nonce);
            let key = derive_encryption_key(
                &self.group_id,
                &self.key_context,
                DEFAULT_SCHEME_VERSION,
                &original_hash,
                mime_type,
                filename,
            )?;
            let encrypted_data = encrypt_bytes(
                bytes,
                &key,
                &nonce,
                media_aad(&self.group_id, DEFAULT_SCHEME_VERSION, mime_type, filename).as_bytes(),
            )?;
            let encrypted_hash: [u8; 32] = Sha256::digest(&encrypted_data).into();
            Ok(EncryptedMediaUpload {
                encrypted_size: encrypted_data.len() as u64,
                original_size: bytes.len() as u64,
                encrypted_data,
                original_hash,
                encrypted_hash,
                mime_type: mime_type.to_ascii_lowercase(),
                filename: filename.to_string(),
                dimensions: None,
                blurhash: None,
                nonce,
            })
        }

        pub fn decrypt_from_download(
            &self,
            encrypted_data: &[u8],
            reference: &MediaReference,
        ) -> Result<Vec<u8>, EncryptedMediaError> {
            if reference.scheme_version != DEFAULT_SCHEME_VERSION {
                return Err(EncryptedMediaError::UnknownSchemeVersion(
                    reference.scheme_version.clone(),
                ));
            }
            let key = derive_encryption_key(
                &self.group_id,
                &self.key_context,
                &reference.scheme_version,
                &reference.original_hash,
                &reference.mime_type,
                &reference.filename,
            )?;
            let plain = decrypt_bytes(
                encrypted_data,
                &key,
                &reference.nonce,
                media_aad(
                    &self.group_id,
                    &reference.scheme_version,
                    &reference.mime_type,
                    &reference.filename,
                )
                .as_bytes(),
            )?;
            let actual: [u8; 32] = Sha256::digest(&plain).into();
            if actual != reference.original_hash {
                return Err(EncryptedMediaError::HashVerificationFailed);
            }
            Ok(plain)
        }

        pub fn create_imeta_tag(&self, upload: &EncryptedMediaUpload, uploaded_url: &str) -> Tag {
            let mut values = vec![
                format!("url {uploaded_url}"),
                format!("m {}", upload.mime_type),
                format!("filename {}", upload.filename),
                format!("x {}", hex::encode(upload.original_hash)),
                format!("n {}", hex::encode(upload.nonce)),
                format!("v {}", DEFAULT_SCHEME_VERSION),
            ];
            if let Some((width, height)) = upload.dimensions {
                values.push(format!("dim {width}x{height}"));
            }
            if let Some(blurhash) = &upload.blurhash {
                values.push(format!("blurhash {blurhash}"));
            }
            Tag::custom(TagKind::custom("imeta"), values)
        }

        pub fn create_media_reference(
            &self,
            upload: &EncryptedMediaUpload,
            uploaded_url: String,
        ) -> MediaReference {
            MediaReference {
                url: uploaded_url,
                original_hash: upload.original_hash,
                mime_type: upload.mime_type.clone(),
                filename: upload.filename.clone(),
                dimensions: upload.dimensions,
                scheme_version: DEFAULT_SCHEME_VERSION.to_string(),
                nonce: upload.nonce,
            }
        }

        pub fn parse_imeta_tag(&self, tag: &Tag) -> Result<MediaReference, EncryptedMediaError> {
            if tag.kind() != TagKind::custom("imeta") {
                return Err(EncryptedMediaError::InvalidImetaTag {
                    reason: "not an imeta tag".to_string(),
                });
            }

            let mut url = None;
            let mut mime_type = None;
            let mut filename = None;
            let mut original_hash = None;
            let mut nonce = None;
            let mut dimensions = None;
            let mut version = None;

            for item in tag.clone().to_vec().iter().skip(1) {
                let Some((key, value)) = item.split_once(' ') else {
                    continue;
                };
                match key {
                    "url" => url = Some(value.to_string()),
                    "m" => mime_type = Some(value.to_ascii_lowercase()),
                    "filename" => filename = Some(value.to_string()),
                    "x" => {
                        let bytes = hex::decode(value).map_err(|_| {
                            EncryptedMediaError::InvalidImetaTag {
                                reason: "invalid original hash".to_string(),
                            }
                        })?;
                        original_hash = Some(bytes.as_slice().try_into().map_err(|_| {
                            EncryptedMediaError::InvalidImetaTag {
                                reason: "original hash must be 32 bytes".to_string(),
                            }
                        })?);
                    }
                    "n" => {
                        let bytes =
                            hex::decode(value).map_err(|_| EncryptedMediaError::InvalidNonce)?;
                        nonce = Some(
                            bytes
                                .as_slice()
                                .try_into()
                                .map_err(|_| EncryptedMediaError::InvalidNonce)?,
                        );
                    }
                    "dim" => {
                        if let Some((width, height)) = value.split_once('x')
                            && let (Ok(width), Ok(height)) =
                                (width.parse::<u32>(), height.parse::<u32>())
                        {
                            dimensions = Some((width, height));
                        }
                    }
                    "v" => version = Some(value.to_string()),
                    _ => {}
                }
            }

            Ok(MediaReference {
                url: url.ok_or_else(|| EncryptedMediaError::InvalidImetaTag {
                    reason: "missing url".to_string(),
                })?,
                original_hash: original_hash.ok_or_else(|| {
                    EncryptedMediaError::InvalidImetaTag {
                        reason: "missing original hash".to_string(),
                    }
                })?,
                mime_type: mime_type.ok_or_else(|| EncryptedMediaError::InvalidImetaTag {
                    reason: "missing MIME type".to_string(),
                })?,
                filename: filename.ok_or_else(|| EncryptedMediaError::InvalidImetaTag {
                    reason: "missing filename".to_string(),
                })?,
                dimensions,
                scheme_version: version.ok_or_else(|| EncryptedMediaError::InvalidImetaTag {
                    reason: "missing scheme version".to_string(),
                })?,
                nonce: nonce.ok_or(EncryptedMediaError::InvalidNonce)?,
            })
        }
    }

    fn media_aad(
        group_id: &GroupId,
        scheme_version: &str,
        mime_type: &str,
        filename: &str,
    ) -> String {
        format!(
            "pika-media-aad-v1:{}:{scheme_version}:{}:{filename}",
            hex::encode(group_id.as_slice()),
            mime_type.to_ascii_lowercase()
        )
    }
}
