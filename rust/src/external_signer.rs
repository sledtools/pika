use std::sync::{Arc, RwLock};

use nostr_sdk::prelude::{
    BoxedFuture, Event, JsonUtil, NostrSigner, PublicKey, SignerBackend, SignerError, UnsignedEvent,
};

#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum ExternalSignerErrorKind {
    Rejected,
    Canceled,
    Timeout,
    SignerUnavailable,
    PackageMismatch,
    InvalidResponse,
    Other,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ExternalSignerResult {
    pub ok: bool,
    pub value: Option<String>,
    pub error_kind: Option<ExternalSignerErrorKind>,
    pub error_message: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ExternalSignerHandshakeResult {
    pub ok: bool,
    pub pubkey: Option<String>,
    pub signer_package: Option<String>,
    pub current_user: Option<String>,
    pub error_kind: Option<ExternalSignerErrorKind>,
    pub error_message: Option<String>,
}

#[uniffi::export(callback_interface)]
pub trait ExternalSignerBridge: Send + Sync + 'static {
    fn open_url(&self, url: String) -> ExternalSignerResult;
    fn request_public_key(
        &self,
        current_user_hint: Option<String>,
    ) -> ExternalSignerHandshakeResult;
    fn sign_event(
        &self,
        signer_package: String,
        current_user: String,
        unsigned_event_json: String,
    ) -> ExternalSignerResult;
    fn nip44_encrypt(
        &self,
        signer_package: String,
        current_user: String,
        peer_pubkey: String,
        content: String,
    ) -> ExternalSignerResult;
    fn nip44_decrypt(
        &self,
        signer_package: String,
        current_user: String,
        peer_pubkey: String,
        payload: String,
    ) -> ExternalSignerResult;
    fn nip04_encrypt(
        &self,
        signer_package: String,
        current_user: String,
        peer_pubkey: String,
        content: String,
    ) -> ExternalSignerResult;
    fn nip04_decrypt(
        &self,
        signer_package: String,
        current_user: String,
        peer_pubkey: String,
        payload: String,
    ) -> ExternalSignerResult;
}

pub type SharedExternalSignerBridge = Arc<RwLock<Option<Arc<dyn ExternalSignerBridge>>>>;

#[derive(Clone)]
pub struct ExternalSignerBridgeSigner {
    expected_pubkey: PublicKey,
    signer_package: String,
    current_user: String,
    bridge: Arc<dyn ExternalSignerBridge>,
}

impl std::fmt::Debug for ExternalSignerBridgeSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalSignerBridgeSigner")
            .field("expected_pubkey", &self.expected_pubkey.to_hex())
            .finish_non_exhaustive()
    }
}

impl ExternalSignerBridgeSigner {
    pub fn new(
        expected_pubkey: PublicKey,
        signer_package: String,
        current_user: String,
        bridge: Arc<dyn ExternalSignerBridge>,
    ) -> Self {
        Self {
            expected_pubkey,
            signer_package,
            current_user,
            bridge,
        }
    }

    fn error_prefix(kind: Option<ExternalSignerErrorKind>) -> &'static str {
        match kind {
            Some(ExternalSignerErrorKind::Rejected) => "rejected",
            Some(ExternalSignerErrorKind::Canceled) => "canceled",
            Some(ExternalSignerErrorKind::Timeout) => "timeout",
            Some(ExternalSignerErrorKind::SignerUnavailable) => "signer unavailable",
            Some(ExternalSignerErrorKind::PackageMismatch) => "package mismatch",
            Some(ExternalSignerErrorKind::InvalidResponse) => "invalid response",
            Some(ExternalSignerErrorKind::Other) | None => "external signer error",
        }
    }

    fn into_signer_error(result: ExternalSignerResult) -> SignerError {
        let prefix = Self::error_prefix(result.error_kind);
        let msg = result
            .error_message
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| prefix.to_string());
        SignerError::from(format!("{prefix}: {msg}"))
    }

    fn expect_value(result: ExternalSignerResult) -> Result<String, SignerError> {
        if !result.ok {
            return Err(Self::into_signer_error(result));
        }
        result
            .value
            .filter(|s| !s.is_empty())
            .ok_or_else(|| SignerError::from("invalid response: empty value"))
    }
}

impl NostrSigner for ExternalSignerBridgeSigner {
    fn backend(&self) -> SignerBackend<'_> {
        SignerBackend::Custom("android-amber-bridge".into())
    }

    fn get_public_key(&self) -> BoxedFuture<'_, Result<PublicKey, SignerError>> {
        let pk = self.expected_pubkey;
        Box::pin(async move { Ok(pk) })
    }

    fn sign_event(&self, unsigned: UnsignedEvent) -> BoxedFuture<'_, Result<Event, SignerError>> {
        let bridge = self.bridge.clone();
        let signer_package = self.signer_package.clone();
        let current_user = self.current_user.clone();
        let expected_pubkey = self.expected_pubkey;
        Box::pin(async move {
            let unsigned_json = serde_json::to_string(&unsigned)
                .map_err(|e| SignerError::from(format!("invalid response: {e}")))?;
            let signed_json =
                Self::expect_value(bridge.sign_event(signer_package, current_user, unsigned_json))?;
            let event = Event::from_json(signed_json)
                .map_err(|e| SignerError::from(format!("invalid response: {e}")))?;
            event
                .verify()
                .map_err(|e| SignerError::from(format!("invalid event signature: {e}")))?;
            if event.pubkey != expected_pubkey {
                return Err(SignerError::from(
                    "package mismatch: signed pubkey mismatch",
                ));
            }
            Ok(event)
        })
    }

    fn nip04_encrypt<'a>(
        &'a self,
        public_key: &'a PublicKey,
        content: &'a str,
    ) -> BoxedFuture<'a, Result<String, SignerError>> {
        let bridge = self.bridge.clone();
        let signer_package = self.signer_package.clone();
        let current_user = self.current_user.clone();
        let peer = public_key.to_hex();
        let body = content.to_string();
        Box::pin(async move {
            Self::expect_value(bridge.nip04_encrypt(signer_package, current_user, peer, body))
        })
    }

    fn nip04_decrypt<'a>(
        &'a self,
        public_key: &'a PublicKey,
        encrypted_content: &'a str,
    ) -> BoxedFuture<'a, Result<String, SignerError>> {
        let bridge = self.bridge.clone();
        let signer_package = self.signer_package.clone();
        let current_user = self.current_user.clone();
        let peer = public_key.to_hex();
        let body = encrypted_content.to_string();
        Box::pin(async move {
            Self::expect_value(bridge.nip04_decrypt(signer_package, current_user, peer, body))
        })
    }

    fn nip44_encrypt<'a>(
        &'a self,
        public_key: &'a PublicKey,
        content: &'a str,
    ) -> BoxedFuture<'a, Result<String, SignerError>> {
        let bridge = self.bridge.clone();
        let signer_package = self.signer_package.clone();
        let current_user = self.current_user.clone();
        let peer = public_key.to_hex();
        let body = content.to_string();
        Box::pin(async move {
            Self::expect_value(bridge.nip44_encrypt(signer_package, current_user, peer, body))
        })
    }

    fn nip44_decrypt<'a>(
        &'a self,
        public_key: &'a PublicKey,
        payload: &'a str,
    ) -> BoxedFuture<'a, Result<String, SignerError>> {
        let bridge = self.bridge.clone();
        let signer_package = self.signer_package.clone();
        let current_user = self.current_user.clone();
        let peer = public_key.to_hex();
        let body = payload.to_string();
        Box::pin(async move {
            Self::expect_value(bridge.nip44_decrypt(signer_package, current_user, peer, body))
        })
    }
}

pub fn user_visible_signer_error(err: &str) -> Option<&'static str> {
    let lower = err.to_lowercase();
    if lower.contains("rejected") {
        return Some("Signing request rejected");
    }
    if lower.contains("canceled") {
        return Some("Signing request canceled");
    }
    if lower.contains("timeout") {
        return Some("Signing request timed out");
    }
    if lower.contains("signer unavailable") {
        return Some("External signer unavailable");
    }
    if lower.contains("package mismatch") {
        return Some("External signer package mismatch");
    }
    None
}

pub fn user_visible_signer_error_kind(
    kind: Option<ExternalSignerErrorKind>,
) -> Option<&'static str> {
    match kind {
        Some(ExternalSignerErrorKind::Rejected) => Some("Signing request rejected"),
        Some(ExternalSignerErrorKind::Canceled) => Some("Signing request canceled"),
        Some(ExternalSignerErrorKind::Timeout) => Some("Signing request timed out"),
        Some(ExternalSignerErrorKind::SignerUnavailable) => Some("External signer unavailable"),
        Some(ExternalSignerErrorKind::PackageMismatch) => Some("External signer package mismatch"),
        Some(ExternalSignerErrorKind::InvalidResponse) => {
            Some("External signer returned an invalid response")
        }
        Some(ExternalSignerErrorKind::Other) | None => None,
    }
}
