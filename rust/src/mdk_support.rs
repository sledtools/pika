use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use mdk_core::{MdkConfig, MDK};
use mdk_sqlite_storage::MdkSqliteStorage;
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
        // Desktop/dev fallback: keep encrypted storage working for local iteration.
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

    Ok(MDK::builder(storage)
        .with_config(MdkConfig::default())
        .build())
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

    Ok(MDK::builder(storage)
        .with_config(MdkConfig::default())
        .build())
}
