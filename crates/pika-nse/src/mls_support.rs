pub use pika_mls::PikaMls;

use anyhow::Result;
use nostr::PublicKey;

pub fn open_mls(data_dir: &str, pubkey: &PublicKey, keychain_group: &str) -> Result<PikaMls> {
    pika_mls::open_secure_mls(data_dir, pubkey, keychain_group)
}
