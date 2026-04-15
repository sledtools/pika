//! Daemon-facing integration surface for Pika hosts.
//!
//! - [`protocol`] is the stable JSONL/socket contract external adapters target.
//! - [`daemon`] is the concrete runtime host that serves that contract over stdio/socket/exec.
//! - the daemon only serves the native protocol surface; no secondary ACP backend bridge
//!   is hosted here anymore.

extern crate self as pika_marmot_runtime;

use anyhow::Context;
use anyhow::Result;
use nostr_sdk::prelude::{Event, EventId};
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub mod call;
mod call_audio;
pub mod call_runtime;
mod call_tts;
pub mod conversation;
pub mod daemon;
pub mod key_package;
pub mod media;
pub mod membership;
pub mod message;
pub mod outbound;
pub mod protocol;
pub mod relay;
pub mod runtime;
pub mod welcome;

pub use pika_mls::{
    IdentityFile, PROCESSED_MLS_EVENT_IDS_FILE, PROCESSED_MLS_EVENT_IDS_MAX, PikaMdk,
    load_or_create_keys, load_processed_mls_event_ids, new_unencrypted_mls as new_mdk,
    open_unencrypted_mls as open_mdk, persist_processed_mls_event_ids,
    processed_mls_event_ids_path,
};
pub use protocol::{DaemonCmd, InCmd, OutMsg};
pub use relay::{check_relay_ready, connect_client, subscribe_group_msgs};
pub use welcome::ingest_welcome_from_giftwrap;

const MAX_UNIX_SOCKET_PATH_BYTES: usize = 100;

fn ensure_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("mkdir {}", dir.display()))?;
    Ok(())
}

pub fn resolve_daemon_socket_path(state_dir: &Path) -> PathBuf {
    let preferred = state_dir.join("daemon.sock");
    if preferred.as_os_str().to_string_lossy().len() <= MAX_UNIX_SOCKET_PATH_BYTES {
        return preferred;
    }

    let mut hasher = DefaultHasher::new();
    state_dir.hash(&mut hasher);
    std::env::temp_dir().join(format!("pikachat-daemon-{:016x}.sock", hasher.finish()))
}

pub fn ingest_application_message(
    mdk: &PikaMdk,
    event: &Event,
) -> Result<Option<pika_mls::storage_traits::messages::types::Message>> {
    match runtime::MarmotRuntime::new(mdk).process_event(event)? {
        Some(conversation::ConversationEvent::Application(message)) => Ok(Some(message.message)),
        _ => Ok(None),
    }
}

pub async fn ingest_group_backlog(
    mdk: &PikaMdk,
    client: &nostr_sdk::Client,
    relay_urls: &[nostr_sdk::RelayUrl],
    nostr_group_id_hex: &str,
    seen: &mut HashSet<EventId>,
    limit: usize,
) -> Result<Vec<pika_mls::storage_traits::messages::types::Message>> {
    runtime::MarmotRuntime::with_client(mdk, client)
        .ingest_group_backlog(relay_urls, nostr_group_id_hex, seen, limit)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn daemon_socket_path_falls_back_for_long_state_dirs() {
        let long_state_dir = Path::new(
            "/var/folders/fj/g0fl0k296k52j6vk64bf_c8w0000gn/T/pikahut-openclaw-gateway-e2e-8gEPa6/cli/pikachat/default",
        );
        let socket_path = resolve_daemon_socket_path(long_state_dir);
        assert!(
            socket_path.starts_with(std::env::temp_dir()),
            "long state dir should use temp-dir socket fallback: {}",
            socket_path.display()
        );
        assert!(
            socket_path.as_os_str().to_string_lossy().len() <= 100,
            "fallback socket path should stay under the Unix socket limit: {}",
            socket_path.display()
        );
    }

    #[test]
    fn identity_file_round_trip() {
        let f = IdentityFile {
            secret_key_hex: "abcd".to_string(),
            public_key_hex: "1234".to_string(),
        };
        let json = serde_json::to_string(&f).unwrap();
        let parsed: IdentityFile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.secret_key_hex, "abcd");
        assert_eq!(parsed.public_key_hex, "1234");
    }

    #[test]
    fn load_or_create_keys_creates_new_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.json");
        assert!(!path.exists());

        let keys = load_or_create_keys(&path).unwrap();
        assert!(path.exists());

        let raw: IdentityFile =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw.public_key_hex, keys.public_key().to_hex());
    }

    #[test]
    fn load_or_create_keys_reloads_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.json");

        let keys1 = load_or_create_keys(&path).unwrap();
        let keys2 = load_or_create_keys(&path).unwrap();
        assert_eq!(keys1.public_key(), keys2.public_key());
    }

    #[test]
    fn processed_ids_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path();

        let empty = load_processed_mls_event_ids(state_dir);
        assert!(empty.is_empty());

        let mut ids = HashSet::new();
        ids.insert(EventId::from_hex(&"a".repeat(64)).unwrap());
        ids.insert(EventId::from_hex(&"b".repeat(64)).unwrap());
        persist_processed_mls_event_ids(state_dir, &ids).unwrap();

        let loaded = load_processed_mls_event_ids(state_dir);
        assert_eq!(loaded, ids);
    }

    #[test]
    fn processed_ids_bounded_to_max() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path();

        let mut ids = HashSet::new();
        for i in 0..(PROCESSED_MLS_EVENT_IDS_MAX + 100) {
            let hex = format!("{:064x}", i);
            ids.insert(EventId::from_hex(&hex).unwrap());
        }
        persist_processed_mls_event_ids(state_dir, &ids).unwrap();

        let loaded = load_processed_mls_event_ids(state_dir);
        assert_eq!(loaded.len(), PROCESSED_MLS_EVENT_IDS_MAX);
    }
}
