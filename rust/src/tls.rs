use std::sync::Once;

// rustls 0.23 selects a process-level CryptoProvider. If more than one provider
// feature is present in the dependency graph (common with quinn/aws-lc-rs plus
// nostr-sdk/ring), rustls requires an explicit choice.
static INIT: Once = Once::new();

pub fn init_rustls_crypto_provider() {
    INIT.call_once(|| {
        // Prefer ring for compatibility with nostr-sdk.
        // If another provider was already installed, ignore the error.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
