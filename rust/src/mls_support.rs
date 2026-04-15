pub use pika_mls::{init_keyring_once, mls_state_path, PikaMls, SERVICE_ID};

use anyhow::Result;
use nostr_sdk::prelude::PublicKey;

pub fn open_mls(data_dir: &str, pubkey: &PublicKey, keychain_group: &str) -> Result<PikaMls> {
    pika_mls::open_secure_mls(data_dir, pubkey, keychain_group)
}
