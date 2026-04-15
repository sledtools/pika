pub use pika_mls::{init_keyring_once, mdk_db_path, PikaMdk, SERVICE_ID};

use anyhow::Result;
use nostr_sdk::prelude::PublicKey;

pub fn open_mdk(data_dir: &str, pubkey: &PublicKey, keychain_group: &str) -> Result<PikaMdk> {
    pika_mls::open_secure_mls(data_dir, pubkey, keychain_group)
}
