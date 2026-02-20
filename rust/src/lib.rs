mod actions;
mod core;
mod logging;
mod mdk_support;
mod route_projection;
mod state;
mod tls;
mod updates;

#[cfg(target_os = "android")]
mod android_keyring;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;

use flume::{Receiver, Sender};

pub use actions::AppAction;
pub use route_projection::*;
pub use state::*;
pub use updates::*;

// Not exposed over UniFFI; used by binaries/tests to avoid rustls provider ambiguity when
// multiple crypto backends are enabled in the dependency graph.
pub fn init_rustls_crypto_provider() {
    tls::init_rustls_crypto_provider();
}

/// Load all cached profiles from the on-disk database and return them as
/// `FollowListEntry` values.  Synchronous read intended for populating the
/// new-chat contact list immediately at startup.
pub fn load_cached_profiles(data_dir: &str) -> Vec<FollowListEntry> {
    core::load_cached_profiles(data_dir)
}

/// Return the default `pika_config.json` payload used when no config file exists.
pub fn default_config_json() -> String {
    core::default_app_config_json()
}

/// Reset only relay-related config keys to defaults while preserving unrelated keys.
pub fn reset_relay_config_json(existing_json: Option<String>) -> String {
    core::relay_reset_config_json(existing_json.as_deref())
}

uniffi::setup_scaffolding!();

#[uniffi::export(callback_interface)]
pub trait AppReconciler: Send + Sync + 'static {
    fn reconcile(&self, update: AppUpdate);
}

#[derive(uniffi::Object)]
pub struct FfiApp {
    core_tx: Sender<CoreMsg>,
    update_rx: Receiver<AppUpdate>,
    listening: AtomicBool,
    shared_state: Arc<RwLock<AppState>>,
}

#[uniffi::export]
impl FfiApp {
    #[uniffi::constructor]
    pub fn new(data_dir: String, keychain_group: String) -> Arc<Self> {
        // Must run before any rustls users (nostr-sdk, moq/quinn, etc) initialize.
        tls::init_rustls_crypto_provider();
        logging::init_logging(&data_dir);
        tracing::info!(data_dir = %data_dir, keychain_group = %keychain_group, "FfiApp::new() starting");

        let (update_tx, update_rx) = flume::unbounded();
        let (core_tx, core_rx) = flume::unbounded::<CoreMsg>();
        let shared_state = Arc::new(RwLock::new(AppState::empty()));

        // Actor loop thread (single threaded "app actor").
        let core_tx_for_core = core_tx.clone();
        let shared_for_core = shared_state.clone();
        thread::spawn(move || {
            let mut core = crate::core::AppCore::new(
                update_tx,
                core_tx_for_core,
                data_dir,
                keychain_group,
                shared_for_core,
            );
            while let Ok(msg) = core_rx.recv() {
                core.handle_message(msg);
            }
        });

        Arc::new(Self {
            core_tx,
            update_rx,
            listening: AtomicBool::new(false),
            shared_state,
        })
    }

    pub fn state(&self) -> AppState {
        match self.shared_state.read() {
            Ok(g) => g.clone(),
            Err(poison) => poison.into_inner().clone(),
        }
    }

    pub fn dispatch(&self, action: AppAction) {
        // Contract: never block caller.
        let _ = self.core_tx.send(CoreMsg::Action(action));
    }

    pub fn listen_for_updates(&self, reconciler: Box<dyn AppReconciler>) {
        if self
            .listening
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            // Avoid multiple listeners that would split messages.
            return;
        }

        let rx = self.update_rx.clone();
        thread::spawn(move || {
            while let Ok(update) = rx.recv() {
                reconciler.reconcile(update);
            }
        });
    }
}
