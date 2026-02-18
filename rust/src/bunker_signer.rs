use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use nostr_connect::error::Error as NostrConnectError;
use nostr_connect::prelude::{NostrConnect, NostrConnectURI};
use nostr_sdk::prelude::{Keys, NostrSigner, PublicKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BunkerConnectErrorKind {
    InvalidUri,
    Rejected,
    Timeout,
    SignerUnavailable,
    Other,
}

#[derive(Debug, Clone)]
pub struct BunkerConnectError {
    pub kind: BunkerConnectErrorKind,
    pub message: String,
}

impl BunkerConnectError {
    pub fn user_visible_message(&self) -> Option<&'static str> {
        match self.kind {
            BunkerConnectErrorKind::InvalidUri => Some("Invalid bunker URI"),
            BunkerConnectErrorKind::Rejected => Some("Bunker request rejected"),
            BunkerConnectErrorKind::Timeout => Some("Bunker request timed out"),
            BunkerConnectErrorKind::SignerUnavailable => Some("Bunker signer unavailable"),
            BunkerConnectErrorKind::Other => None,
        }
    }
}

impl fmt::Display for BunkerConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for BunkerConnectError {}

#[derive(Clone)]
pub struct BunkerConnectOutput {
    pub user_pubkey: PublicKey,
    pub canonical_bunker_uri: String,
    pub signer: Arc<dyn NostrSigner>,
}

pub trait BunkerSignerConnector: Send + Sync + 'static {
    fn connect(
        &self,
        runtime: &tokio::runtime::Runtime,
        bunker_uri: &str,
        client_keys: Keys,
    ) -> Result<BunkerConnectOutput, BunkerConnectError>;
}

pub type SharedBunkerSignerConnector = Arc<RwLock<Arc<dyn BunkerSignerConnector>>>;

#[derive(Debug, Clone)]
pub struct NostrConnectBunkerSignerConnector {
    timeout: Duration,
}

impl Default for NostrConnectBunkerSignerConnector {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(90),
        }
    }
}

impl NostrConnectBunkerSignerConnector {
    fn map_nostr_connect_error(err: NostrConnectError) -> BunkerConnectError {
        match err {
            NostrConnectError::Timeout => BunkerConnectError {
                kind: BunkerConnectErrorKind::Timeout,
                message: "bunker request timed out".to_string(),
            },
            NostrConnectError::SignerPublicKeyNotFound => BunkerConnectError {
                kind: BunkerConnectErrorKind::SignerUnavailable,
                message: "bunker signer public key not found".to_string(),
            },
            NostrConnectError::Response(msg) => {
                let lower = msg.to_lowercase();
                if lower.contains("reject") || lower.contains("denied") {
                    BunkerConnectError {
                        kind: BunkerConnectErrorKind::Rejected,
                        message: msg,
                    }
                } else {
                    BunkerConnectError {
                        kind: BunkerConnectErrorKind::Other,
                        message: msg,
                    }
                }
            }
            NostrConnectError::UnexpectedUri
            | NostrConnectError::PublicKeyNotMatchAppKeys
            | NostrConnectError::NIP46(_) => BunkerConnectError {
                kind: BunkerConnectErrorKind::InvalidUri,
                message: "invalid bunker URI".to_string(),
            },
            other => BunkerConnectError {
                kind: BunkerConnectErrorKind::Other,
                message: format!("{other}"),
            },
        }
    }

    fn map_signer_error(message: String) -> BunkerConnectError {
        let lower = message.to_lowercase();
        if lower.contains("timeout") {
            return BunkerConnectError {
                kind: BunkerConnectErrorKind::Timeout,
                message,
            };
        }
        if lower.contains("reject") || lower.contains("denied") {
            return BunkerConnectError {
                kind: BunkerConnectErrorKind::Rejected,
                message,
            };
        }
        if lower.contains("signer public key not found")
            || lower.contains("signer unavailable")
            || lower.contains("not found")
        {
            return BunkerConnectError {
                kind: BunkerConnectErrorKind::SignerUnavailable,
                message,
            };
        }
        BunkerConnectError {
            kind: BunkerConnectErrorKind::Other,
            message,
        }
    }
}

impl BunkerSignerConnector for NostrConnectBunkerSignerConnector {
    fn connect(
        &self,
        runtime: &tokio::runtime::Runtime,
        bunker_uri: &str,
        client_keys: Keys,
    ) -> Result<BunkerConnectOutput, BunkerConnectError> {
        let trimmed = bunker_uri.trim();
        if trimmed.is_empty() {
            return Err(BunkerConnectError {
                kind: BunkerConnectErrorKind::InvalidUri,
                message: "enter bunker URI".to_string(),
            });
        }

        let parsed = NostrConnectURI::parse(trimmed).map_err(|_| BunkerConnectError {
            kind: BunkerConnectErrorKind::InvalidUri,
            message: "invalid bunker URI".to_string(),
        })?;

        if !parsed.is_bunker() {
            return Err(BunkerConnectError {
                kind: BunkerConnectErrorKind::InvalidUri,
                message: "invalid bunker URI: expected bunker://".to_string(),
            });
        }

        if parsed.relays().is_empty() {
            return Err(BunkerConnectError {
                kind: BunkerConnectErrorKind::InvalidUri,
                message: "invalid bunker URI: missing relay".to_string(),
            });
        }

        let signer = NostrConnect::new(parsed, client_keys, self.timeout, None)
            .map_err(Self::map_nostr_connect_error)?;

        let user_pubkey = runtime
            .block_on(async { signer.get_public_key().await })
            .map_err(|e| Self::map_signer_error(format!("{e}")))?;

        let canonical_bunker_uri = runtime
            .block_on(async { signer.bunker_uri().await })
            .map_err(Self::map_nostr_connect_error)?
            .to_string();

        Ok(BunkerConnectOutput {
            user_pubkey,
            canonical_bunker_uri,
            signer: Arc::new(signer),
        })
    }
}
