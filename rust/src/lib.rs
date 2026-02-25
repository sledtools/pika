mod actions;
mod bunker_signer;
mod core;
mod external_signer;
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

use crate::bunker_signer::{
    BunkerSignerConnector as BunkerSignerConnectorTrait,
    NostrConnectBunkerSignerConnector as NostrConnectBunkerSignerConnectorImpl,
    SharedBunkerSignerConnector as SharedBunkerSignerConnectorType,
};
use crate::external_signer::{
    ExternalSignerBridge as ExternalSignerBridgeTrait,
    SharedExternalSignerBridge as SharedExternalSignerBridgeType,
};
use flume::{Receiver, Sender};

pub use actions::AppAction;
pub use bunker_signer::*;
pub use external_signer::*;
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

/// Platform-side callback for receiving decoded video frames from remote peers.
/// Called from the video worker thread at ~30fps during active video calls.
#[uniffi::export(callback_interface)]
pub trait VideoFrameReceiver: Send + Sync + 'static {
    fn on_video_frame(&self, call_id: String, payload: Vec<u8>);
}

/// Platform-side callback for receiving avatar animation frames from remote peers.
/// Called from the avatar worker thread at ~30fps during avatar calls.
#[uniffi::export(callback_interface)]
pub trait AvatarFrameReceiver: Send + Sync + 'static {
    fn on_avatar_frame(&self, call_id: String, payload: Vec<u8>);
}

#[derive(uniffi::Object)]
pub struct FfiApp {
    core_tx: Sender<CoreMsg>,
    update_rx: Receiver<AppUpdate>,
    listening: AtomicBool,
    shared_state: Arc<RwLock<AppState>>,
    external_signer_bridge: SharedExternalSignerBridgeType,
    bunker_signer_connector: SharedBunkerSignerConnectorType,
    video_frame_receiver: Arc<RwLock<Option<Arc<dyn VideoFrameReceiver>>>>,
    avatar_frame_receiver: Arc<RwLock<Option<Arc<dyn AvatarFrameReceiver>>>>,
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
        let external_signer_bridge: SharedExternalSignerBridgeType = Arc::new(RwLock::new(None));
        let bunker_signer_connector: SharedBunkerSignerConnectorType = Arc::new(RwLock::new(
            Arc::new(NostrConnectBunkerSignerConnectorImpl::default()),
        ));
        let video_frame_receiver: Arc<RwLock<Option<Arc<dyn VideoFrameReceiver>>>> =
            Arc::new(RwLock::new(None));
        let avatar_frame_receiver: Arc<RwLock<Option<Arc<dyn AvatarFrameReceiver>>>> =
            Arc::new(RwLock::new(None));

        // Actor loop thread (single threaded "app actor").
        let core_tx_for_core = core_tx.clone();
        let shared_for_core = shared_state.clone();
        let signer_bridge_for_core = external_signer_bridge.clone();
        let bunker_connector_for_core = bunker_signer_connector.clone();
        let video_receiver_for_core = video_frame_receiver.clone();
        let avatar_receiver_for_core = avatar_frame_receiver.clone();
        thread::spawn(move || {
            let mut core = crate::core::AppCore::new(
                update_tx,
                core_tx_for_core,
                data_dir,
                keychain_group,
                shared_for_core,
                signer_bridge_for_core,
                bunker_connector_for_core,
            );
            core.set_video_frame_receiver(video_receiver_for_core);
            core.set_avatar_frame_receiver(avatar_receiver_for_core);
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
            video_frame_receiver,
            avatar_frame_receiver,
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

    pub fn set_video_frame_receiver(&self, receiver: Box<dyn VideoFrameReceiver>) {
        let receiver: Arc<dyn VideoFrameReceiver> = Arc::from(receiver);
        match self.video_frame_receiver.write() {
            Ok(mut slot) => {
                *slot = Some(receiver);
            }
            Err(poison) => {
                *poison.into_inner() = Some(receiver);
            }
        }
    }

    pub fn set_avatar_frame_receiver(&self, receiver: Box<dyn AvatarFrameReceiver>) {
        let receiver: Arc<dyn AvatarFrameReceiver> = Arc::from(receiver);
        match self.avatar_frame_receiver.write() {
            Ok(mut slot) => {
                *slot = Some(receiver);
            }
            Err(poison) => {
                *poison.into_inner() = Some(receiver);
            }
        }
    }

    pub fn send_video_frame(&self, payload: Vec<u8>) {
        let _ = self.core_tx.send(CoreMsg::Internal(Box::new(
            InternalEvent::VideoFrameFromPlatform { payload },
        )));
    }

    pub fn set_external_signer_bridge(&self, bridge: Box<dyn ExternalSignerBridgeTrait>) {
        let bridge: Arc<dyn ExternalSignerBridgeTrait> = Arc::from(bridge);
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
    pub fn set_bunker_signer_connector_for_tests(
        &self,
        connector: Arc<dyn BunkerSignerConnectorTrait>,
    ) {
        match self.bunker_signer_connector.write() {
            Ok(mut slot) => {
                *slot = connector;
            }
            Err(poison) => {
                *poison.into_inner() = connector;
            }
        }
    }

    pub fn inject_nostr_connect_connect_response_for_tests(&self, remote_signer_pubkey: String) {
        let _ = self.core_tx.send(CoreMsg::Internal(Box::new(
            InternalEvent::NostrConnectInjectConnectResponseForTests {
                remote_signer_pubkey,
            },
        )));
    }
}
