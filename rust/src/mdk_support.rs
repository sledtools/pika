use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use mdk_core::{MdkConfig, MDK};
use mdk_sqlite_storage::{error::Error as MdkStorageError, MdkSqliteStorage};
use nostr_sdk::prelude::PublicKey;

pub type PikaMdk = MDK<MdkSqliteStorage>;

// Keep stable IDs; spec-v2 uses a reverse-DNS identifier.
pub const SERVICE_ID: &str = "com.pika.app";

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
    // IMPORTANT: `set_default_store` can only be called once per process.
    // We guard it via `OnceLock` above.
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

        // Prefer ndk-context if available. If the host app uses the Kotlin/JNI init hook,
        // this should be a no-op because the store is already set; however `set_default_store`
        // can only be called once, so we avoid calling it again here.
        //
        // We can't reliably detect whether a store is already set, so this path should only
        // be used when we can set it ourselves.
        let store = AndroidStore::from_ndk_context()
            .context("Android keyring store not initialized. Call Keyring.setAndroidKeyringCredentialBuilder(context) early in MainActivity, or use a framework that provides ndk-context.")?;
        keyring_core::set_default_store(store);
        return Ok(());
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        // Desktop/dev: mock store so keyring_core is initialized (the file-based key
        // path in `open_mdk` bypasses keyring entirely on desktop).
        keyring_core::set_default_store(
            keyring_core::mock::Store::new().context("failed to create mock keyring store")?,
        );
        Ok(())
    }
}

pub fn open_mdk(data_dir: &str, pubkey: &PublicKey, keychain_group: &str) -> Result<PikaMdk> {
    init_keyring_once(keychain_group)?;

    let pubkey_hex = pubkey.to_hex();
    let db_path = mdk_db_path(data_dir, &pubkey_hex);
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create mdk db dir: {}", parent.display()))?;
    }

    // On desktop (non-iOS, non-Android) always use a file-based key because the
    // mock keyring store is in-memory and keys are lost when the process exits.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        return open_mdk_desktop_file_key(data_dir, pubkey)
            .with_context(|| format!("open encrypted mdk sqlite db: {}", db_path.display()));
    }

    #[allow(unreachable_code)]
    let storage = match MdkSqliteStorage::new(&db_path, SERVICE_ID, &db_key_id(&pubkey_hex)) {
        Ok(storage) => storage,
        Err(e) => {
            // On iOS simulator, keychain operations can fail if the app is not provisioned
            // with the necessary entitlements. For dev/QA we fall back to an app-sandbox
            // file-based key to keep MLS state encrypted-at-rest without Keychain.
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

    Ok(MDK::builder(storage).with_config(mdk_config()).build())
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
            .downcast_ref::<MdkStorageError>()
            .map(|storage_err| matches!(storage_err, MdkStorageError::WrongEncryptionKey))
            .unwrap_or(false)
    })
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn remove_mdk_db_artifacts(db_path: &Path) {
    let _ = std::fs::remove_file(db_path);
    let _ = std::fs::remove_file(db_path.with_extension("sqlite3-shm"));
    let _ = std::fs::remove_file(db_path.with_extension("sqlite3-wal"));
}

/// Desktop: file-based encryption key stored next to the DB file.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn open_mdk_desktop_file_key(data_dir: &str, pubkey: &PublicKey) -> Result<PikaMdk> {
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
        use rand::rngs::OsRng;
        use rand::RngCore;

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

    // If a previous attempt created an empty DB file (e.g., keyring failure mid-init),
    // remove it so we can initialize encrypted storage cleanly.
    if let Ok(meta) = std::fs::metadata(&db_path) {
        if meta.len() == 0 {
            let _ = std::fs::remove_file(&db_path);
        }
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
                MDK::builder(storage)
                    .with_config(MdkConfig::default())
                    .build()
            })
    };

    match open() {
        Ok(mdk) => Ok(mdk),
        Err(err) => {
            // Legacy desktop builds could leave an encrypted DB without a persisted file key.
            // Only recover when an existing DB fails specifically with WrongEncryptionKey.
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
fn open_mdk_ios_file_key(data_dir: &str, pubkey: &PublicKey) -> Result<PikaMdk> {
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
        use rand::rngs::OsRng;
        use rand::RngCore;

        let mut k = [0u8; 32];
        OsRng.fill_bytes(&mut k);
        std::fs::write(&key_path, &k)
            .with_context(|| format!("write mdk file key: {}", key_path.display()))?;
        // Best-effort hardening; iOS sandbox is the primary boundary.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
        }
        k
    };

    // If a previous attempt created an empty DB file (e.g., keyring failure mid-init),
    // remove it so we can initialize encrypted storage cleanly.
    if let Ok(meta) = std::fs::metadata(&db_path) {
        if meta.len() == 0 {
            let _ = std::fs::remove_file(&db_path);
        }
    }

    let storage =
        MdkSqliteStorage::new_with_key(&db_path, mdk_sqlite_storage::EncryptionConfig::new(key))
            .with_context(|| {
                format!(
                    "open encrypted mdk sqlite db with file key: {}",
                    db_path.display()
                )
            })?;

    Ok(MDK::builder(storage).with_config(mdk_config()).build())
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use super::*;
    use nostr_sdk::prelude::Keys;
    use rand::rngs::OsRng;
    use rand::RngCore;
    use tempfile::tempdir;

    #[test]
    fn desktop_recovers_unreadable_legacy_db_when_key_file_missing() {
        let tmp = tempdir().expect("tempdir");
        let data_dir = tmp.path().to_string_lossy().to_string();
        let pubkey = Keys::generate().public_key();
        let pubkey_hex = pubkey.to_hex();
        let db_path = mdk_db_path(&data_dir, &pubkey_hex);
        let key_path = db_path.with_extension("key");

        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db dir");

        // Simulate a legacy encrypted DB created with an unknown keyring-backed key.
        let mut legacy_key = [0u8; 32];
        OsRng.fill_bytes(&mut legacy_key);
        let storage = MdkSqliteStorage::new_with_key(
            &db_path,
            mdk_sqlite_storage::EncryptionConfig::new(legacy_key),
        )
        .expect("create legacy db");
        let legacy = MDK::builder(storage)
            .with_config(MdkConfig::default())
            .build();
        drop(legacy);
        assert!(!key_path.exists(), "legacy setup should not have file key");

        // First open recreates unreadable DB and persists a new file key.
        let opened = open_mdk_desktop_file_key(&data_dir, &pubkey).expect("open with recovery");
        drop(opened);
        assert!(key_path.exists(), "file key should be persisted");

        // Subsequent open should succeed with the persisted key.
        let reopened = open_mdk_desktop_file_key(&data_dir, &pubkey).expect("reopen");
        drop(reopened);
    }

    #[test]
    fn desktop_does_not_delete_db_on_non_legacy_open_failure() {
        let tmp = tempdir().expect("tempdir");
        let data_dir = tmp.path().to_string_lossy().to_string();
        let pubkey = Keys::generate().public_key();
        let pubkey_hex = pubkey.to_hex();
        let db_path = mdk_db_path(&data_dir, &pubkey_hex);
        let key_path = db_path.with_extension("key");

        // Use a directory at the DB path to force an open failure that is not WrongEncryptionKey.
        std::fs::create_dir_all(&db_path).expect("create directory at db path");
        assert!(db_path.is_dir(), "fixture must stay a directory");

        let err = match open_mdk_desktop_file_key(&data_dir, &pubkey) {
            Ok(_) => panic!("open should fail"),
            Err(err) => err,
        };
        assert!(
            db_path.is_dir(),
            "non-legacy failures must not delete db artifacts"
        );
        assert!(key_path.exists(), "key file should still be persisted");

        let message = format!("{err:#}");
        assert!(
            message.contains("open encrypted mdk sqlite db with file key"),
            "error should surface context for troubleshooting"
        );
    }
}
