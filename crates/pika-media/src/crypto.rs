#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameKeyMaterial {
    pub key_id: u64,
}

pub fn encrypt_frame(payload: &[u8], _keys: &FrameKeyMaterial) -> Vec<u8> {
    // Phase-6 placeholder. Real MLS-derived AEAD lands in encryption hardening phase.
    payload.to_vec()
}

pub fn decrypt_frame(payload: &[u8], _keys: &FrameKeyMaterial) -> Vec<u8> {
    // Phase-6 placeholder. Real MLS-derived AEAD lands in encryption hardening phase.
    payload.to_vec()
}
