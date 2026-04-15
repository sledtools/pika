use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use nostr::{Event, EventId, Keys, PublicKey, RelayUrl, UnsignedEvent};
use serde::{Deserialize, Serialize};

pub mod conversation;
pub mod membership;
pub mod welcome;

pub use mdk_core::{self, MdkConfig, encrypted_media, prelude};
pub use mdk_sqlite_storage::{self, MdkSqliteStorage};
pub use mdk_storage_traits::{self as storage_traits};

type RawMls = mdk_core::MDK<MdkSqliteStorage>;

pub struct PikaMls {
    inner: RawMls,
}

pub type PikaMdk = PikaMls;

impl PikaMls {
    fn from_raw(inner: RawMls) -> Self {
        Self { inner }
    }

    pub fn as_raw(&self) -> &RawMls {
        &self.inner
    }

    pub fn create_group(
        &self,
        creator_public_key: &PublicKey,
        member_key_package_events: Vec<Event>,
        config: prelude::NostrGroupConfigData,
    ) -> std::result::Result<prelude::GroupResult, prelude::Error> {
        self.inner
            .create_group(creator_public_key, member_key_package_events, config)
    }

    pub fn add_members(
        &self,
        group_id: &storage_traits::GroupId,
        key_package_events: &[Event],
    ) -> std::result::Result<prelude::UpdateGroupResult, prelude::Error> {
        self.inner.add_members(group_id, key_package_events)
    }

    pub fn remove_members(
        &self,
        group_id: &storage_traits::GroupId,
        pubkeys: &[PublicKey],
    ) -> std::result::Result<prelude::UpdateGroupResult, prelude::Error> {
        self.inner.remove_members(group_id, pubkeys)
    }

    pub fn update_group_data(
        &self,
        group_id: &storage_traits::GroupId,
        update: prelude::NostrGroupDataUpdate,
    ) -> std::result::Result<prelude::UpdateGroupResult, prelude::Error> {
        self.inner.update_group_data(group_id, update)
    }

    pub fn leave_group(
        &self,
        group_id: &storage_traits::GroupId,
    ) -> std::result::Result<prelude::UpdateGroupResult, prelude::Error> {
        self.inner.leave_group(group_id)
    }

    pub fn merge_pending_commit(
        &self,
        group_id: &storage_traits::GroupId,
    ) -> std::result::Result<(), prelude::Error> {
        self.inner.merge_pending_commit(group_id)
    }

    pub fn clear_pending_commit(
        &self,
        group_id: &storage_traits::GroupId,
    ) -> std::result::Result<(), prelude::Error> {
        self.inner.clear_pending_commit(group_id)
    }

    pub fn create_message(
        &self,
        mls_group_id: &storage_traits::GroupId,
        rumor: UnsignedEvent,
    ) -> std::result::Result<Event, prelude::Error> {
        self.inner.create_message(mls_group_id, rumor)
    }

    pub fn process_message(
        &self,
        event: &Event,
    ) -> std::result::Result<prelude::MessageProcessingResult, prelude::Error> {
        self.inner.process_message(event)
    }

    pub fn process_welcome(
        &self,
        wrapper_event_id: &EventId,
        rumor_event: &UnsignedEvent,
    ) -> std::result::Result<storage_traits::welcomes::types::Welcome, prelude::Error> {
        self.inner.process_welcome(wrapper_event_id, rumor_event)
    }

    pub fn accept_welcome(
        &self,
        welcome: &storage_traits::welcomes::types::Welcome,
    ) -> std::result::Result<(), prelude::Error> {
        self.inner.accept_welcome(welcome)
    }

    pub fn get_group(
        &self,
        group_id: &storage_traits::GroupId,
    ) -> std::result::Result<Option<storage_traits::groups::types::Group>, prelude::Error> {
        self.inner.get_group(group_id)
    }

    pub fn get_groups(
        &self,
    ) -> std::result::Result<Vec<storage_traits::groups::types::Group>, prelude::Error> {
        self.inner.get_groups()
    }

    pub fn get_members(
        &self,
        group_id: &storage_traits::GroupId,
    ) -> std::result::Result<BTreeSet<PublicKey>, prelude::Error> {
        self.inner.get_members(group_id)
    }

    pub fn get_relays(
        &self,
        group_id: &storage_traits::GroupId,
    ) -> std::result::Result<BTreeSet<RelayUrl>, prelude::Error> {
        self.inner.get_relays(group_id)
    }

    pub fn get_message(
        &self,
        group_id: &storage_traits::GroupId,
        message_id: &EventId,
    ) -> std::result::Result<Option<storage_traits::messages::types::Message>, prelude::Error> {
        self.inner.get_message(group_id, message_id)
    }

    pub fn get_pending_welcomes(
        &self,
        pagination: Option<storage_traits::welcomes::Pagination>,
    ) -> std::result::Result<Vec<storage_traits::welcomes::types::Welcome>, prelude::Error> {
        self.inner.get_pending_welcomes(pagination)
    }

    pub fn parse_key_package(&self, event: &Event) -> std::result::Result<(), prelude::Error> {
        self.inner.parse_key_package(event).map(|_| ())
    }

    pub fn media_manager(
        &self,
        group_id: storage_traits::GroupId,
    ) -> encrypted_media::manager::EncryptedMediaManager<'_, MdkSqliteStorage> {
        self.inner.media_manager(group_id)
    }
}

impl std::fmt::Debug for PikaMls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PikaMls").finish_non_exhaustive()
    }
}

impl std::ops::Deref for PikaMls {
    type Target = RawMls;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

pub const SERVICE_ID: &str = "com.pika.app";
pub const PROCESSED_MLS_EVENT_IDS_FILE: &str = "processed_mls_event_ids_v1.txt";
pub const PROCESSED_MLS_EVENT_IDS_MAX: usize = 8192;

#[derive(Debug, Serialize, Deserialize)]
pub struct IdentityFile {
    pub secret_key_hex: String,
    pub public_key_hex: String,
}

pub fn mdk_db_path(data_dir: &str, pubkey_hex: &str) -> PathBuf {
    Path::new(data_dir)
        .join("mls")
        .join(pubkey_hex)
        .join("mdk.sqlite3")
}

pub fn db_key_id(pubkey_hex: &str) -> String {
    format!("mdk.db.key.{pubkey_hex}")
}

pub fn init_keyring_once(#[allow(unused)] keychain_group: &str) -> Result<()> {
    static INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    match INIT.get_or_init(|| init_keyring_inner(keychain_group).map_err(|e| e.to_string())) {
        Ok(()) => Ok(()),
        Err(e) => Err(anyhow!(e.clone())),
    }
}

fn init_keyring_inner(#[allow(unused)] keychain_group: &str) -> Result<()> {
    #[cfg(target_os = "ios")]
    {
        let mut config = std::collections::HashMap::new();
        config.insert("access-group", keychain_group);
        let store = apple_native_keyring_store::protected::Store::new_with_configuration(&config)
            .context(
            "failed to create Apple protected keyring store with shared access group",
        )?;
        keyring_core::set_default_store(store);
        return Ok(());
    }

    #[cfg(target_os = "android")]
    {
        use android_native_keyring_store::credential::AndroidStore;

        let store = AndroidStore::from_ndk_context()
            .context("Android keyring store not initialized. Call Keyring.setAndroidKeyringCredentialBuilder(context) early in MainActivity, or use a framework that provides ndk-context.")?;
        keyring_core::set_default_store(store);
        return Ok(());
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        keyring_core::set_default_store(
            keyring_core::mock::Store::new().context("failed to create mock keyring store")?,
        );
        Ok(())
    }
}

pub fn open_secure_mls(
    data_dir: &str,
    pubkey: &PublicKey,
    keychain_group: &str,
) -> Result<PikaMls> {
    init_keyring_once(keychain_group)?;

    let pubkey_hex = pubkey.to_hex();
    let db_path = mdk_db_path(data_dir, &pubkey_hex);
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create mdk db dir: {}", parent.display()))?;
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        return open_mdk_desktop_file_key(data_dir, pubkey)
            .with_context(|| format!("open encrypted mdk sqlite db: {}", db_path.display()));
    }

    #[allow(unreachable_code)]
    let storage = match MdkSqliteStorage::new(&db_path, SERVICE_ID, &db_key_id(&pubkey_hex)) {
        Ok(storage) => storage,
        Err(e) => {
            #[cfg(all(target_os = "ios", target_env = "sim"))]
            {
                use mdk_sqlite_storage::error::Error as MdkErr;
                if matches!(e, MdkErr::Keyring(_) | MdkErr::KeyringNotInitialized(_)) {
                    tracing::warn!(
                        "mdk keyring-backed storage failed on iOS; falling back to file key: {e}"
                    );
                    return open_mdk_ios_file_key(data_dir, pubkey).with_context(|| {
                        format!("open encrypted mdk sqlite db: {}", db_path.display())
                    });
                }
            }

            Err(e)
                .with_context(|| format!("open encrypted mdk sqlite db: {}", db_path.display()))?
        }
    };

    Ok(PikaMls::from_raw(
        mdk_core::MDK::builder(storage)
            .with_config(mdk_config())
            .build(),
    ))
}

pub fn open_unencrypted_mls(state_dir: &Path) -> Result<PikaMls> {
    let db_path = state_dir.join("mdk.sqlite");
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let storage = MdkSqliteStorage::new_unencrypted(&db_path)
        .with_context(|| format!("open mdk sqlite: {}", db_path.display()))?;
    Ok(PikaMls::from_raw(mdk_core::MDK::new(storage)))
}

pub fn new_unencrypted_mls(state_dir: &Path, _label: &str) -> Result<PikaMls> {
    open_unencrypted_mls(state_dir)
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

fn mdk_config() -> MdkConfig {
    MdkConfig {
        ..Default::default()
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn is_legacy_missing_file_key_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<mdk_sqlite_storage::error::Error>()
            .map(|storage_err| {
                matches!(
                    storage_err,
                    mdk_sqlite_storage::error::Error::WrongEncryptionKey
                )
            })
            .unwrap_or(false)
    })
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn remove_mdk_db_artifacts(db_path: &Path) {
    let _ = std::fs::remove_file(db_path);
    let _ = std::fs::remove_file(db_path.with_extension("sqlite3-shm"));
    let _ = std::fs::remove_file(db_path.with_extension("sqlite3-wal"));
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn open_mdk_desktop_file_key(data_dir: &str, pubkey: &PublicKey) -> Result<PikaMls> {
    let pubkey_hex = pubkey.to_hex();
    let db_path = mdk_db_path(data_dir, &pubkey_hex);
    let key_path = db_path.with_extension("key");
    let had_existing_db = db_path.exists();

    if let Some(parent) = key_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create mdk key dir: {}", parent.display()))?;
    }

    let (key, created_key): ([u8; 32], bool) = if key_path.exists() {
        let bytes = std::fs::read(&key_path)
            .with_context(|| format!("read mdk file key: {}", key_path.display()))?;
        let key = bytes.as_slice().try_into().map_err(|_| {
            anyhow!(
                "invalid mdk file key length: expected 32 bytes, got {}",
                bytes.len()
            )
        })?;
        (key, false)
    } else {
        use rand::RngCore;
        use rand::rngs::OsRng;

        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        std::fs::write(&key_path, key)
            .with_context(|| format!("write mdk file key: {}", key_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
        }
        (key, true)
    };

    if let Ok(meta) = std::fs::metadata(&db_path)
        && meta.len() == 0
    {
        let _ = std::fs::remove_file(&db_path);
    }

    let open = || {
        MdkSqliteStorage::new_with_key(&db_path, mdk_sqlite_storage::EncryptionConfig::new(key))
            .with_context(|| {
                format!(
                    "open encrypted mdk sqlite db with file key: {}",
                    db_path.display()
                )
            })
            .map(|storage| {
                PikaMls::from_raw(
                    mdk_core::MDK::builder(storage)
                        .with_config(MdkConfig::default())
                        .build(),
                )
            })
    };

    match open() {
        Ok(mdk) => Ok(mdk),
        Err(err) => {
            if created_key && had_existing_db && is_legacy_missing_file_key_error(&err) {
                tracing::warn!(
                    error = %err,
                    path = %db_path.display(),
                    "desktop mdk key missing for existing db; recreating local encrypted db"
                );
                remove_mdk_db_artifacts(&db_path);
                open()
            } else {
                Err(err)
            }
        }
    }
}

#[cfg(all(target_os = "ios", target_env = "sim"))]
fn open_mdk_ios_file_key(data_dir: &str, pubkey: &PublicKey) -> Result<PikaMls> {
    let pubkey_hex = pubkey.to_hex();
    let db_path = mdk_db_path(data_dir, &pubkey_hex);
    let key_path = Path::new(data_dir)
        .join("mls")
        .join(&pubkey_hex)
        .join("mdk.db.key");

    if let Some(parent) = key_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create mdk key dir: {}", parent.display()))?;
    }

    let key: [u8; 32] = if key_path.exists() {
        let bytes = std::fs::read(&key_path)
            .with_context(|| format!("read mdk file key: {}", key_path.display()))?;
        bytes.as_slice().try_into().map_err(|_| {
            anyhow!(
                "invalid mdk file key length: expected 32 bytes, got {}",
                bytes.len()
            )
        })?
    } else {
        use rand::RngCore;
        use rand::rngs::OsRng;

        let mut k = [0u8; 32];
        OsRng.fill_bytes(&mut k);
        std::fs::write(&key_path, &k)
            .with_context(|| format!("write mdk file key: {}", key_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
        }
        k
    };

    let storage =
        MdkSqliteStorage::new_with_key(&db_path, mdk_sqlite_storage::EncryptionConfig::new(key))
            .with_context(|| {
                format!(
                    "open encrypted mdk sqlite db with iOS simulator file key: {}",
                    db_path.display()
                )
            })?;
    Ok(PikaMls::from_raw(
        mdk_core::MDK::builder(storage)
            .with_config(mdk_config())
            .build(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
