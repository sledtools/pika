pub mod call;
pub mod key_package;
pub mod media;
pub mod message;
pub mod relay;

use std::collections::HashSet;
use std::future::Future;
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

#[derive(Debug, Clone)]
pub struct AcceptedWelcome {
    pub wrapper_event_id: EventId,
    pub welcome_event_id: EventId,
    pub nostr_group_id_hex: String,
    pub mls_group_id: mdk_storage_traits::GroupId,
    pub group_name: String,
    pub ingested_messages: Vec<mdk_storage_traits::messages::types::Message>,
}

/// Unwrap and process a gift-wrapped MLS welcome into MDK pending-welcome
/// storage. This intentionally does not accept the welcome; hosts decide
/// whether to stage, auto-accept, subscribe, or backfill after ingest. MDK may
/// already expose a pending group row before accept.
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

/// Accept a known pending welcome, optionally let the host run a narrow
/// post-accept hook, then backfill recent group messages through the shared
/// backlog ingest path.
///
/// Hosts still own policy. They choose when to call this, which relays to use
/// for catch-up, and what to do in the `after_accept` hook (for example daemon
/// subscription bookkeeping before backlog fetch).
pub async fn accept_welcome_and_catch_up<F, Fut>(
    mdk: &PikaMdk,
    client: &nostr_sdk::Client,
    relay_urls: &[nostr_sdk::RelayUrl],
    welcome: &mdk_storage_traits::welcomes::types::Welcome,
    seen: &mut HashSet<EventId>,
    limit: usize,
    after_accept: F,
) -> Result<AcceptedWelcome>
where
    F: FnOnce(&AcceptedWelcome) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let mut accepted = AcceptedWelcome {
        wrapper_event_id: welcome.wrapper_event_id,
        welcome_event_id: welcome.id,
        nostr_group_id_hex: hex::encode(welcome.nostr_group_id),
        mls_group_id: welcome.mls_group_id.clone(),
        group_name: welcome.group_name.clone(),
        ingested_messages: Vec::new(),
    };

    mdk.accept_welcome(welcome).context("accept welcome")?;
    after_accept(&accepted).await?;

    if !relay_urls.is_empty() {
        accepted.ingested_messages = ingest_group_backlog(
            mdk,
            client,
            relay_urls,
            &accepted.nostr_group_id_hex,
            seen,
            limit,
        )
        .await
        .context("ingest accepted welcome backlog")?;
    }

    Ok(accepted)
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

/// Fetch recent group messages from relays and process them through
/// `ingest_application_message` so the local MLS epoch is up-to-date.
///
/// Returns the messages that were successfully ingested.
/// Errors on individual events are silently ignored (expected for
/// own messages bouncing back, already-processed events, etc.).
pub async fn ingest_group_backlog(
    mdk: &PikaMdk,
    client: &nostr_sdk::Client,
    relay_urls: &[nostr_sdk::RelayUrl],
    nostr_group_id_hex: &str,
    seen: &mut HashSet<EventId>,
    limit: usize,
) -> Result<Vec<mdk_storage_traits::messages::types::Message>> {
    use nostr_sdk::prelude::{Alphabet, Filter, SingleLetterTag};

    let filter = Filter::new()
        .kind(Kind::MlsGroupMessage)
        .custom_tag(SingleLetterTag::lowercase(Alphabet::H), nostr_group_id_hex)
        .limit(limit);

    let events = client
        .fetch_events_from(
            relay_urls.to_vec(),
            filter,
            std::time::Duration::from_secs(10),
        )
        .await
        .context("fetch group backlog")?;

    let mut messages = Vec::new();
    for ev in events.iter() {
        if !seen.insert(ev.id) {
            continue;
        }
        if let Ok(Some(msg)) = ingest_application_message(mdk, ev) {
            messages.push(msg);
        }
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    use mdk_core::prelude::NostrGroupConfigData;
    use nostr_sdk::prelude::{EventBuilder, RelayUrl};

    fn make_key_package_event(mdk: &PikaMdk, keys: &Keys) -> Event {
        let relay = RelayUrl::parse("wss://test.relay").expect("relay url");
        let (content, tags, _hash_ref) = mdk
            .create_key_package_for_event(&keys.public_key(), vec![relay])
            .expect("create key package");
        EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign key package")
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

    #[test]
    fn accept_welcome_and_catch_up_accepts_and_returns_group_ids_without_relays() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let invitee_client = nostr_sdk::Client::builder()
            .signer(invitee_keys.clone())
            .build();

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime accept test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let group_result = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let welcome_rumor = group_result
            .welcome_rumors
            .into_iter()
            .next()
            .expect("welcome rumor");
        let wrapper = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                EventBuilder::gift_wrap(
                    &inviter_keys,
                    &invitee_keys.public_key(),
                    welcome_rumor,
                    [],
                )
                .await
                .expect("build giftwrap")
            });

        tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                ingest_welcome_from_giftwrap(&invitee_mdk, &invitee_keys, &wrapper, |_| true)
                    .await
                    .expect("ingest welcome")
                    .expect("welcome should ingest");
            });

        let pending = invitee_mdk
            .get_pending_welcomes(None)
            .expect("get pending welcomes");
        let welcome = pending.first().expect("pending welcome");
        let mut seen = HashSet::new();
        let accepted = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                accept_welcome_and_catch_up(
                    &invitee_mdk,
                    &invitee_client,
                    &[],
                    welcome,
                    &mut seen,
                    200,
                    |_| async { Ok(()) },
                )
                .await
                .expect("accept welcome and catch up")
            });

        assert_eq!(accepted.wrapper_event_id, wrapper.id);
        assert_eq!(
            accepted.nostr_group_id_hex,
            hex::encode(group_result.group.nostr_group_id)
        );
        assert_eq!(accepted.group_name, "Runtime accept test");
        assert!(
            accepted.ingested_messages.is_empty(),
            "empty relay list should preserve manual/narrow host behavior"
        );
        assert!(
            invitee_mdk
                .get_pending_welcomes(None)
                .expect("get pending welcomes")
                .is_empty(),
            "accept should clear the pending welcome"
        );
    }

    #[test]
    fn ingest_welcome_from_giftwrap_stages_pending_welcome_without_joining() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime ingest test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let group_result = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let mut welcome_rumor = group_result
            .welcome_rumors
            .into_iter()
            .next()
            .expect("welcome rumor");
        let welcome_event_id = welcome_rumor.id();

        let wrapper = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                EventBuilder::gift_wrap(
                    &inviter_keys,
                    &invitee_keys.public_key(),
                    welcome_rumor,
                    [],
                )
                .await
                .expect("build giftwrap")
            });

        let ingested = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                ingest_welcome_from_giftwrap(&invitee_mdk, &invitee_keys, &wrapper, |_| true)
                    .await
                    .expect("ingest welcome")
                    .expect("welcome should be accepted for ingest")
            });

        assert_eq!(ingested.wrapper_event_id, wrapper.id);
        assert_eq!(ingested.welcome_event_id, welcome_event_id);
        assert_eq!(
            ingested.nostr_group_id_hex,
            hex::encode(group_result.group.nostr_group_id)
        );
        assert_eq!(ingested.group_name, "Runtime ingest test");

        let pending = invitee_mdk
            .get_pending_welcomes(None)
            .expect("get pending welcomes");
        assert_eq!(pending.len(), 1, "ingest should stage exactly one welcome");
        assert_eq!(
            pending[0].wrapper_event_id, wrapper.id,
            "staged welcome should keep the wrapper id for explicit accept flows"
        );
        let groups = invitee_mdk.get_groups().expect("get groups");
        assert_eq!(
            groups.len(),
            1,
            "shared ingest already surfaces a pending group before accept"
        );
        assert_eq!(
            hex::encode(groups[0].nostr_group_id),
            ingested.nostr_group_id_hex,
            "pending group should line up with the staged welcome metadata"
        );
    }
}
