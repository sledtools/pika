use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mdk_core::MDK;
use mdk_core::prelude::MessageProcessingResult;
use mdk_sqlite_storage::MdkSqliteStorage;
use nostr_sdk::prelude::{Event, EventId, Keys, Kind, PublicKey};
use serde::{Deserialize, Serialize};

pub type PikaMdk = MDK<MdkSqliteStorage>;

pub const PROCESSED_MLS_EVENT_IDS_FILE: &str = "processed_mls_event_ids_v1.txt";
pub const PROCESSED_MLS_EVENT_IDS_MAX: usize = 8192;

#[derive(Debug, Serialize, Deserialize)]
pub struct IdentityFile {
    pub secret_key_hex: String,
    pub public_key_hex: String,
}

pub fn load_or_create_keys(identity_path: &Path) -> Result<Keys> {
    if let Ok(raw) = std::fs::read_to_string(identity_path) {
        let f: IdentityFile = serde_json::from_str(&raw).context("parse identity json")?;
        let keys = Keys::parse(&f.secret_key_hex).context("parse secret key hex")?;
        return Ok(keys);
    }

    let keys = Keys::generate();
    let secret = keys.secret_key().to_secret_hex();
    let pubkey = keys.public_key().to_hex();
    let f = IdentityFile {
        secret_key_hex: secret,
        public_key_hex: pubkey,
    };

    if let Some(parent) = identity_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }

    std::fs::write(
        identity_path,
        format!("{}\n", serde_json::to_string_pretty(&f)?),
    )
    .context("write identity json")?;
    Ok(keys)
}

pub fn open_mdk(state_dir: &Path) -> Result<PikaMdk> {
    let db_path = state_dir.join("mdk.sqlite");
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    // Unencrypted for dev/test usage.
    let storage = MdkSqliteStorage::new_unencrypted(&db_path)
        .with_context(|| format!("open mdk sqlite: {}", db_path.display()))?;
    Ok(MDK::new(storage))
}

pub fn new_mdk(state_dir: &Path, _label: &str) -> Result<PikaMdk> {
    open_mdk(state_dir)
}

pub fn processed_mls_event_ids_path(state_dir: &Path) -> PathBuf {
    state_dir.join(PROCESSED_MLS_EVENT_IDS_FILE)
}

pub fn load_processed_mls_event_ids(state_dir: &Path) -> HashSet<EventId> {
    let path = processed_mls_event_ids_path(state_dir);
    let Ok(raw) = std::fs::read_to_string(path) else {
        return HashSet::new();
    };
    raw.lines()
        .filter_map(|line| EventId::from_hex(line.trim()).ok())
        .collect()
}

pub fn persist_processed_mls_event_ids(
    state_dir: &Path,
    event_ids: &HashSet<EventId>,
) -> Result<()> {
    let mut ids: Vec<String> = event_ids.iter().map(|id| id.to_hex()).collect();
    ids.sort_unstable();
    if ids.len() > PROCESSED_MLS_EVENT_IDS_MAX {
        ids = ids.split_off(ids.len() - PROCESSED_MLS_EVENT_IDS_MAX);
    }
    let mut body = ids.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    let path = processed_mls_event_ids_path(state_dir);
    std::fs::write(&path, body)
        .with_context(|| format!("persist processed MLS event ids to {}", path.display()))
}

#[derive(Debug, Clone)]
pub struct IngestedWelcome {
    pub wrapper_event_id: EventId,
    pub welcome_event_id: EventId,
    pub sender: PublicKey,
    pub sender_hex: String,
    pub nostr_group_id_hex: String,
    pub group_name: String,
}

pub async fn ingest_welcome_from_giftwrap<F>(
    mdk: &PikaMdk,
    keys: &Keys,
    event: &Event,
    sender_allowed: F,
) -> Result<Option<IngestedWelcome>>
where
    F: Fn(&str) -> bool,
{
    if event.kind != Kind::GiftWrap {
        return Ok(None);
    }

    let unwrapped = nostr_sdk::nostr::nips::nip59::extract_rumor(keys, event)
        .await
        .context("unwrap giftwrap rumor")?;
    if unwrapped.rumor.kind != Kind::MlsWelcome {
        return Ok(None);
    }

    let sender_hex = unwrapped.sender.to_hex().to_lowercase();
    if !sender_allowed(&sender_hex) {
        return Ok(None);
    }

    let mut rumor = unwrapped.rumor;
    mdk.process_welcome(&event.id, &rumor)
        .context("process welcome rumor")?;

    let pending = mdk
        .get_pending_welcomes(None)
        .context("get pending welcomes")?;
    let stored = pending.into_iter().find(|w| w.wrapper_event_id == event.id);
    let (nostr_group_id_hex, group_name) = match stored {
        Some(w) => (hex::encode(w.nostr_group_id), w.group_name),
        None => (String::new(), String::new()),
    };

    Ok(Some(IngestedWelcome {
        wrapper_event_id: event.id,
        welcome_event_id: rumor.id(),
        sender: unwrapped.sender,
        sender_hex,
        nostr_group_id_hex,
        group_name,
    }))
}

pub fn ingest_application_message(
    mdk: &PikaMdk,
    event: &Event,
) -> Result<Option<mdk_storage_traits::messages::types::Message>> {
    if event.kind != Kind::MlsGroupMessage {
        return Ok(None);
    }
    match mdk
        .process_message(event)
        .context("process group message")?
    {
        MessageProcessingResult::ApplicationMessage(msg) => Ok(Some(msg)),
        _ => Ok(None),
    }
}
