mod actions;
mod bunker_signer;
mod core;
mod external_signer;
mod logging;
mod mdk_support;
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
pub use bunker_signer::*;
pub use external_signer::*;
pub use state::*;
pub use updates::*;

// Not exposed over UniFFI; used by binaries/tests to avoid rustls provider ambiguity when
// multiple crypto backends are enabled in the dependency graph.
pub fn init_rustls_crypto_provider() {
    tls::init_rustls_crypto_provider();
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
    external_signer_bridge: SharedExternalSignerBridge,
    bunker_signer_connector: SharedBunkerSignerConnector,
}

#[uniffi::export]
impl FfiApp {
    #[uniffi::constructor]
    pub fn new(data_dir: String) -> Arc<Self> {
        // Must run before any rustls users (nostr-sdk, moq/quinn, etc) initialize.
        tls::init_rustls_crypto_provider();
        logging::init_logging(&data_dir);
        tracing::info!(data_dir = %data_dir, "FfiApp::new() starting");

        let (update_tx, update_rx) = flume::unbounded();
        let (core_tx, core_rx) = flume::unbounded::<CoreMsg>();
        let shared_state = Arc::new(RwLock::new(AppState::empty()));
        let external_signer_bridge: SharedExternalSignerBridge = Arc::new(RwLock::new(None));
        let bunker_signer_connector: SharedBunkerSignerConnector = Arc::new(RwLock::new(Arc::new(
            NostrConnectBunkerSignerConnector::default(),
        )));

        // Actor loop thread (single threaded "app actor").
        let core_tx_for_core = core_tx.clone();
        let shared_for_core = shared_state.clone();
        let signer_bridge_for_core = external_signer_bridge.clone();
        let bunker_connector_for_core = bunker_signer_connector.clone();
        thread::spawn(move || {
            let mut core = crate::core::AppCore::new(
                update_tx,
                core_tx_for_core,
                data_dir,
                shared_for_core,
                signer_bridge_for_core,
                bunker_connector_for_core,
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
            external_signer_bridge,
            bunker_signer_connector,
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

    pub fn set_external_signer_bridge(&self, bridge: Box<dyn ExternalSignerBridge>) {
        let bridge: Arc<dyn ExternalSignerBridge> = Arc::from(bridge);
        match self.external_signer_bridge.write() {
            Ok(mut slot) => {
                *slot = Some(bridge);
            }
            Err(poison) => {
                *poison.into_inner() = Some(bridge);
            }
        }
    }
}

impl FfiApp {
    pub fn set_bunker_signer_connector_for_tests(&self, connector: Arc<dyn BunkerSignerConnector>) {
        match self.bunker_signer_connector.write() {
            Ok(mut slot) => {
                *slot = connector;
            }
            Err(poison) => {
                *poison.into_inner() = connector;
            }
        }
    }
}
