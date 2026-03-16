#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use nostr_connect::prelude::NostrConnect;
use nostr_sdk::prelude::{Keys, NostrSigner, Url};
use pika_core::{
    BunkerConnectError, BunkerConnectErrorKind, BunkerConnectOutput, BunkerSignerConnector,
    ExternalSignerBridge, ExternalSignerErrorKind, ExternalSignerHandshakeResult,
    ExternalSignerResult,
};

pub fn query_param(url: &str, key: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    parsed
        .query_pairs()
        .find_map(|(k, v)| if k == key { Some(v.into_owned()) } else { None })
}

pub fn nostrconnect_client_pubkey(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(ToString::to_string))
}

pub fn nostrconnect_metadata(url: &str) -> Option<serde_json::Value> {
    let raw = query_param(url, "metadata")?;
    serde_json::from_str(&raw).ok()
}

#[derive(Clone)]
pub struct MockExternalSignerBridge {
    handshake_result: Arc<Mutex<ExternalSignerHandshakeResult>>,
    last_hint: Arc<Mutex<Option<String>>>,
    open_url_result: Arc<Mutex<ExternalSignerResult>>,
    last_opened_url: Arc<Mutex<Option<String>>>,
    open_url_calls: Arc<Mutex<u64>>,
}

impl MockExternalSignerBridge {
    pub fn new(handshake_result: ExternalSignerHandshakeResult) -> Self {
        Self {
            handshake_result: Arc::new(Mutex::new(handshake_result)),
            last_hint: Arc::new(Mutex::new(None)),
            open_url_result: Arc::new(Mutex::new(ExternalSignerResult {
                ok: true,
                value: None,
                error_kind: None,
                error_message: None,
            })),
            last_opened_url: Arc::new(Mutex::new(None)),
            open_url_calls: Arc::new(Mutex::new(0)),
        }
    }

    pub fn last_hint(&self) -> Option<String> {
        self.last_hint.lock().unwrap().clone()
    }

    pub fn last_opened_url(&self) -> Option<String> {
        self.last_opened_url.lock().unwrap().clone()
    }

    pub fn open_url_calls(&self) -> u64 {
        *self.open_url_calls.lock().unwrap()
    }
}

impl ExternalSignerBridge for MockExternalSignerBridge {
    fn open_url(&self, url: String) -> ExternalSignerResult {
        *self.last_opened_url.lock().unwrap() = Some(url);
        let mut calls = self.open_url_calls.lock().unwrap();
        *calls += 1;
        self.open_url_result.lock().unwrap().clone()
    }

    fn request_public_key(
        &self,
        current_user_hint: Option<String>,
    ) -> ExternalSignerHandshakeResult {
        *self.last_hint.lock().unwrap() = current_user_hint;
        self.handshake_result.lock().unwrap().clone()
    }

    fn sign_event(
        &self,
        _signer_package: String,
        _current_user: String,
        _unsigned_event_json: String,
    ) -> ExternalSignerResult {
        ExternalSignerResult {
            ok: false,
            value: None,
            error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
            error_message: Some("signer unavailable".into()),
        }
    }

    fn nip44_encrypt(
        &self,
        _signer_package: String,
        _current_user: String,
        _peer_pubkey: String,
        _content: String,
    ) -> ExternalSignerResult {
        ExternalSignerResult {
            ok: false,
            value: None,
            error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
            error_message: Some("signer unavailable".into()),
        }
    }

    fn nip44_decrypt(
        &self,
        _signer_package: String,
        _current_user: String,
        _peer_pubkey: String,
        _payload: String,
    ) -> ExternalSignerResult {
        ExternalSignerResult {
            ok: false,
            value: None,
            error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
            error_message: Some("signer unavailable".into()),
        }
    }

    fn nip04_encrypt(
        &self,
        _signer_package: String,
        _current_user: String,
        _peer_pubkey: String,
        _content: String,
    ) -> ExternalSignerResult {
        ExternalSignerResult {
            ok: false,
            value: None,
            error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
            error_message: Some("signer unavailable".into()),
        }
    }

    fn nip04_decrypt(
        &self,
        _signer_package: String,
        _current_user: String,
        _peer_pubkey: String,
        _payload: String,
    ) -> ExternalSignerResult {
        ExternalSignerResult {
            ok: false,
            value: None,
            error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
            error_message: Some("signer unavailable".into()),
        }
    }
}

#[derive(Clone)]
pub struct MockBunkerSignerConnector {
    result: Arc<Mutex<Result<BunkerConnectOutput, BunkerConnectError>>>,
    last_bunker_uri: Arc<Mutex<Option<String>>>,
    last_client_pubkey: Arc<Mutex<Option<String>>>,
}

impl MockBunkerSignerConnector {
    pub fn success(canonical_bunker_uri: &str) -> (Self, String) {
        let signer_keys = Keys::generate();
        let user_pubkey = signer_keys.public_key();
        let output = BunkerConnectOutput {
            user_pubkey,
            canonical_bunker_uri: canonical_bunker_uri.to_string(),
            signer: Arc::new(signer_keys) as Arc<dyn NostrSigner>,
        };
        (
            Self {
                result: Arc::new(Mutex::new(Ok(output))),
                last_bunker_uri: Arc::new(Mutex::new(None)),
                last_client_pubkey: Arc::new(Mutex::new(None)),
            },
            user_pubkey.to_hex(),
        )
    }

    pub fn failure(kind: BunkerConnectErrorKind, message: &str) -> Self {
        Self {
            result: Arc::new(Mutex::new(Err(BunkerConnectError {
                kind,
                message: message.to_string(),
            }))),
            last_bunker_uri: Arc::new(Mutex::new(None)),
            last_client_pubkey: Arc::new(Mutex::new(None)),
        }
    }

    pub fn last_bunker_uri(&self) -> Option<String> {
        self.last_bunker_uri.lock().unwrap().clone()
    }

    pub fn last_client_pubkey(&self) -> Option<String> {
        self.last_client_pubkey.lock().unwrap().clone()
    }
}

impl BunkerSignerConnector for MockBunkerSignerConnector {
    fn connect(
        &self,
        _runtime: &tokio::runtime::Runtime,
        bunker_uri: &str,
        client_keys: Keys,
    ) -> Result<BunkerConnectOutput, BunkerConnectError> {
        *self.last_bunker_uri.lock().unwrap() = Some(bunker_uri.to_string());
        *self.last_client_pubkey.lock().unwrap() = Some(client_keys.public_key().to_hex());
        self.result.lock().unwrap().clone()
    }

    fn prepare(
        &self,
        _runtime: &tokio::runtime::Runtime,
        _bunker_uri: &str,
        _client_keys: Keys,
    ) -> Result<NostrConnect, BunkerConnectError> {
        Err(BunkerConnectError {
            kind: BunkerConnectErrorKind::Other,
            message: "mock: prepare not supported".to_string(),
        })
    }

    fn finish(
        &self,
        _runtime: &tokio::runtime::Runtime,
        _signer: NostrConnect,
    ) -> Result<BunkerConnectOutput, BunkerConnectError> {
        self.result.lock().unwrap().clone()
    }
}

#[derive(Clone)]
pub struct SequenceBunkerSignerConnector {
    results: Arc<Mutex<Vec<Result<BunkerConnectOutput, BunkerConnectError>>>>,
    seen_uris: Arc<Mutex<Vec<String>>>,
}

impl SequenceBunkerSignerConnector {
    pub fn new(results: Vec<Result<BunkerConnectOutput, BunkerConnectError>>) -> Self {
        Self {
            results: Arc::new(Mutex::new(results)),
            seen_uris: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn seen_uris(&self) -> Vec<String> {
        self.seen_uris.lock().unwrap().clone()
    }
}

impl BunkerSignerConnector for SequenceBunkerSignerConnector {
    fn connect(
        &self,
        _runtime: &tokio::runtime::Runtime,
        bunker_uri: &str,
        _client_keys: Keys,
    ) -> Result<BunkerConnectOutput, BunkerConnectError> {
        self.seen_uris.lock().unwrap().push(bunker_uri.to_string());
        let mut results = self.results.lock().unwrap();
        if results.is_empty() {
            return Err(BunkerConnectError {
                kind: BunkerConnectErrorKind::Other,
                message: "sequence connector exhausted".to_string(),
            });
        }
        results.remove(0)
    }

    fn prepare(
        &self,
        _runtime: &tokio::runtime::Runtime,
        _bunker_uri: &str,
        _client_keys: Keys,
    ) -> Result<NostrConnect, BunkerConnectError> {
        Err(BunkerConnectError {
            kind: BunkerConnectErrorKind::Other,
            message: "mock: prepare not supported".to_string(),
        })
    }

    fn finish(
        &self,
        _runtime: &tokio::runtime::Runtime,
        _signer: NostrConnect,
    ) -> Result<BunkerConnectOutput, BunkerConnectError> {
        Err(BunkerConnectError {
            kind: BunkerConnectErrorKind::Other,
            message: "mock: finish not supported".to_string(),
        })
    }
}
