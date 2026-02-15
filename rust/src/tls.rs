use std::sync::Once;

// rustls 0.23 selects a process-level CryptoProvider. If more than one provider
// feature is present in the dependency graph (common with quinn/aws-lc-rs plus
// nostr-sdk/ring), rustls requires an explicit choice.
static INIT: Once = Once::new();

pub fn init_rustls_crypto_provider() {
    INIT.call_once(|| {
        // Keep the historical behavior but delegate to the shared helper so
        // other crates (e.g. pika-media) can do the same thing.
        pika_tls::init_rustls_crypto_provider();
    });
}
