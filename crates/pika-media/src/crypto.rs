use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes128Gcm, KeyInit, Nonce};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};

const FRAME_VERSION: u8 = 1;
const FLAGS_KEYFRAME: u8 = 0x01;
const HEADER_LEN: usize = 35;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    pub counter: u32,
    pub group_seq: u64,
    pub frame_idx: u32,
    pub keyframe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecryptedFrame {
    pub info: FrameInfo,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameKeyMaterial {
    pub key_id: u64,
    pub epoch: u64,
    pub generation: u8,
    pub track_label: String,
    pub group_root: [u8; 32],
    base_key: [u8; 32],
}

impl FrameKeyMaterial {
    pub fn from_base_key(
        base_key: [u8; 32],
        key_id: u64,
        epoch: u64,
        generation: u8,
        track_label: impl Into<String>,
        group_root: [u8; 32],
    ) -> Self {
        Self {
            key_id,
            epoch,
            generation,
            track_label: track_label.into(),
            group_root,
            base_key,
        }
    }

    pub fn from_fallback_context(
        shared_seed: &[u8],
        sender_id: &[u8],
        epoch: u64,
        generation: u8,
        track_label: impl Into<String>,
    ) -> Self {
        let base_key = hash32([b"pika.call.media.base.v1".as_slice(), shared_seed].as_slice());
        let group_root = hash32([b"pika.call.media.root.v1".as_slice(), shared_seed].as_slice());
        let key_id = {
            let digest = hash32([b"pika.call.media.keyid.v1".as_slice(), sender_id].as_slice());
            u64::from_be_bytes(digest[0..8].try_into().expect("hash width"))
        };
        Self::from_base_key(base_key, key_id, epoch, generation, track_label, group_root)
    }

    fn generation_keys(&self, generation: u8) -> Result<([u8; 16], [u8; 12]), FrameCryptoError> {
        let hk = Hkdf::<Sha256>::new(None, &self.base_key);
        let mut key = [0u8; 16];
        let mut nonce_salt = [0u8; 12];

        let mut key_info = Vec::with_capacity(10);
        key_info.push(b'k');
        key_info.push(generation);
        key_info.extend_from_slice(&self.key_id.to_be_bytes());
        hk.expand(&key_info, &mut key)
            .map_err(|_| FrameCryptoError::KdfExpandFailed)?;

        let mut nonce_info = Vec::with_capacity(10);
        nonce_info.push(b'n');
        nonce_info.push(generation);
        nonce_info.extend_from_slice(&self.key_id.to_be_bytes());
        hk.expand(&nonce_info, &mut nonce_salt)
            .map_err(|_| FrameCryptoError::KdfExpandFailed)?;

        Ok((key, nonce_salt))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameCryptoError {
    PayloadTooShort,
    UnsupportedVersion { got: u8 },
    KeyIdMismatch { expected: u64, got: u64 },
    EpochMismatch { expected: u64, got: u64 },
    GenerationMismatch { expected: u8, got: u8 },
    KdfExpandFailed,
    EncryptFailed,
    DecryptFailed,
}

impl std::fmt::Display for FrameCryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PayloadTooShort => write!(f, "encrypted frame payload too short"),
            Self::UnsupportedVersion { got } => {
                write!(f, "unsupported encrypted frame version: {got}")
            }
            Self::KeyIdMismatch { expected, got } => {
                write!(f, "key id mismatch: expected {expected}, got {got}")
            }
            Self::EpochMismatch { expected, got } => {
                write!(f, "epoch mismatch: expected {expected}, got {got}")
            }
            Self::GenerationMismatch { expected, got } => {
                write!(f, "generation mismatch: expected {expected}, got {got}")
            }
            Self::KdfExpandFailed => write!(f, "frame key derivation failed"),
            Self::EncryptFailed => write!(f, "frame encryption failed"),
            Self::DecryptFailed => write!(f, "frame decryption failed"),
        }
    }
}

impl std::error::Error for FrameCryptoError {}

pub fn encrypt_frame(
    payload: &[u8],
    keys: &FrameKeyMaterial,
    info: FrameInfo,
) -> Result<Vec<u8>, FrameCryptoError> {
    let generation = keys.generation;
    let (aead_key, nonce_salt) = keys.generation_keys(generation)?;
    let aad = frame_aad(keys, generation, info);
    let nonce = build_nonce(nonce_salt, info.counter);
    let cipher = Aes128Gcm::new_from_slice(&aead_key).expect("AES-128 key length");
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: payload,
                aad: &aad,
            },
        )
        .map_err(|_| FrameCryptoError::EncryptFailed)?;

    let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    out.push(FRAME_VERSION);
    out.extend_from_slice(&keys.key_id.to_be_bytes());
    out.push(generation);
    out.extend_from_slice(&keys.epoch.to_be_bytes());
    out.extend_from_slice(&info.counter.to_be_bytes());
    out.extend_from_slice(&info.group_seq.to_be_bytes());
    out.extend_from_slice(&info.frame_idx.to_be_bytes());
    out.push(if info.keyframe { FLAGS_KEYFRAME } else { 0 });
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub fn decrypt_frame(
    payload: &[u8],
    keys: &FrameKeyMaterial,
) -> Result<DecryptedFrame, FrameCryptoError> {
    if payload.len() < HEADER_LEN {
        return Err(FrameCryptoError::PayloadTooShort);
    }

    let version = payload[0];
    if version != FRAME_VERSION {
        return Err(FrameCryptoError::UnsupportedVersion { got: version });
    }
    let key_id = u64::from_be_bytes(payload[1..9].try_into().expect("header key id"));
    if key_id != keys.key_id {
        return Err(FrameCryptoError::KeyIdMismatch {
            expected: keys.key_id,
            got: key_id,
        });
    }

    let generation = payload[9];
    if generation != keys.generation {
        return Err(FrameCryptoError::GenerationMismatch {
            expected: keys.generation,
            got: generation,
        });
    }

    let epoch = u64::from_be_bytes(payload[10..18].try_into().expect("header epoch"));
    if epoch != keys.epoch {
        return Err(FrameCryptoError::EpochMismatch {
            expected: keys.epoch,
            got: epoch,
        });
    }

    let info = FrameInfo {
        counter: u32::from_be_bytes(payload[18..22].try_into().expect("header counter")),
        group_seq: u64::from_be_bytes(payload[22..30].try_into().expect("header group seq")),
        frame_idx: u32::from_be_bytes(payload[30..34].try_into().expect("header frame idx")),
        keyframe: (payload[34] & FLAGS_KEYFRAME) != 0,
    };
    let ciphertext = &payload[HEADER_LEN..];
    let aad = frame_aad(keys, generation, info);
    let (aead_key, nonce_salt) = keys.generation_keys(generation)?;
    let nonce = build_nonce(nonce_salt, info.counter);
    let cipher = Aes128Gcm::new_from_slice(&aead_key).expect("AES-128 key length");
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| FrameCryptoError::DecryptFailed)?;

    Ok(DecryptedFrame {
        info,
        payload: plaintext,
    })
}

pub fn opaque_participant_label(shared_seed: &[u8], participant_id: &[u8]) -> String {
    let digest = hash32(
        [
            b"pika.call.media.path.v1".as_slice(),
            shared_seed,
            participant_id,
        ]
        .as_slice(),
    );
    hex_lower(&digest)
}

fn frame_aad(keys: &FrameKeyMaterial, generation: u8, info: FrameInfo) -> Vec<u8> {
    let mut out = Vec::with_capacity(80 + keys.track_label.len());
    out.push(FRAME_VERSION);
    out.extend_from_slice(&keys.key_id.to_be_bytes());
    out.push(generation);
    out.extend_from_slice(&keys.epoch.to_be_bytes());
    out.extend_from_slice(&keys.group_root);
    out.extend_from_slice(&(keys.track_label.len() as u16).to_be_bytes());
    out.extend_from_slice(keys.track_label.as_bytes());
    out.extend_from_slice(&info.group_seq.to_be_bytes());
    out.extend_from_slice(&info.frame_idx.to_be_bytes());
    out.extend_from_slice(&info.counter.to_be_bytes());
    out.push(if info.keyframe { FLAGS_KEYFRAME } else { 0 });
    out
}

fn build_nonce(mut nonce_salt: [u8; 12], counter: u32) -> [u8; 12] {
    let ctr_bytes = counter.to_be_bytes();
    for (idx, b) in ctr_bytes.iter().enumerate() {
        nonce_salt[8 + idx] ^= b;
    }
    nonce_salt
}

fn hash32(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_keys(track: &str) -> FrameKeyMaterial {
        FrameKeyMaterial::from_fallback_context(b"call-shared-seed", b"sender-a", 42, 0, track)
    }

    #[test]
    fn roundtrip_encrypt_decrypt() {
        let keys = mk_keys("audio0");
        let info = FrameInfo {
            counter: 7,
            group_seq: 99,
            frame_idx: 0,
            keyframe: true,
        };
        let payload = b"opus-frame-payload";
        let sealed = encrypt_frame(payload, &keys, info).expect("encrypt");
        let opened = decrypt_frame(&sealed, &keys).expect("decrypt");
        assert_eq!(opened.info, info);
        assert_eq!(opened.payload, payload);
    }

    #[test]
    fn tamper_rejected() {
        let keys = mk_keys("audio0");
        let info = FrameInfo {
            counter: 11,
            group_seq: 5,
            frame_idx: 0,
            keyframe: false,
        };
        let mut sealed = encrypt_frame(b"hello", &keys, info).expect("encrypt");
        *sealed.last_mut().expect("has bytes") ^= 0x55;
        let err = decrypt_frame(&sealed, &keys).expect_err("tamper must fail");
        assert!(matches!(err, FrameCryptoError::DecryptFailed));
    }

    #[test]
    fn wrong_track_rejected() {
        let tx_keys = mk_keys("audio0");
        let rx_keys = mk_keys("audio1");
        let info = FrameInfo {
            counter: 1,
            group_seq: 1,
            frame_idx: 0,
            keyframe: true,
        };
        let sealed = encrypt_frame(b"hello", &tx_keys, info).expect("encrypt");
        let err = decrypt_frame(&sealed, &rx_keys).expect_err("track mismatch must fail");
        assert!(matches!(err, FrameCryptoError::DecryptFailed));
    }

    #[test]
    fn wrong_sender_key_id_rejected() {
        let tx_keys = mk_keys("audio0");
        let rx_keys = FrameKeyMaterial::from_fallback_context(
            b"call-shared-seed",
            b"sender-b",
            42,
            0,
            "audio0",
        );
        let info = FrameInfo {
            counter: 1,
            group_seq: 1,
            frame_idx: 0,
            keyframe: false,
        };
        let sealed = encrypt_frame(b"hello", &tx_keys, info).expect("encrypt");
        let err = decrypt_frame(&sealed, &rx_keys).expect_err("sender mismatch must fail");
        assert!(matches!(err, FrameCryptoError::KeyIdMismatch { .. }));
    }

    #[test]
    fn nonce_uses_full_counter_width() {
        let salt = [0u8; 12];
        let low = build_nonce(salt, 1);
        let high = build_nonce(salt, 0x0100_0001);
        assert_ne!(low, high, "high counter bits must affect nonce");
    }
}
