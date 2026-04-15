use std::path::Path;

use anyhow::Result;

pub use pika_mls::{
    IdentityFile, PikaMls, load_or_create_keys, load_processed_mls_event_ids,
    persist_processed_mls_event_ids,
};
pub use pikachat_sidecar::ingest_application_message;
pub use pikachat_sidecar::welcome::ingest_welcome_from_giftwrap;

pub fn open_mls(state_dir: &Path) -> Result<PikaMls> {
    pika_mls::open_unencrypted_mls(state_dir)
}

pub fn new_mls(state_dir: &Path, label: &str) -> Result<PikaMls> {
    pika_mls::new_unencrypted_mls(state_dir, label)
}
