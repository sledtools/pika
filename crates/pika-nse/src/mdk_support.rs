pub use pika_mls::PikaMdk;

use anyhow::Result;
use nostr::PublicKey;

pub fn open_mdk(data_dir: &str, pubkey: &PublicKey, keychain_group: &str) -> Result<PikaMdk> {
    pika_mls::open_secure_mls(data_dir, pubkey, keychain_group)
}
