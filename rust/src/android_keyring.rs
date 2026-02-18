//! Android-only early init hook for keyring-core and ndk-context.
//!
//! This project uses UniFFI via JNA on Android, so we don't get `ndk-glue` to
//! initialize `ndk-context`. MDK's encrypted SQLite backend needs `keyring-core`
//! to have an Android keystore-backed default store and (for that crate) an
//! initialized ndk-context.
//!
//! Kotlin calls `com.pika.app.Keyring.init(Context)` very early in `MainActivity`.

#![cfg(target_os = "android")]

use std::ffi::c_void;
use std::sync::OnceLock;

use jni::objects::{JClass, JObject};
use jni::JNIEnv;

static INIT: OnceLock<()> = OnceLock::new();

#[no_mangle]
pub extern "system" fn Java_com_pika_app_Keyring_init(
    env: JNIEnv,
    _class: JClass,
    context: JObject,
) {
    // Idempotent: if called multiple times, keep the first initialization.
    if INIT.set(()).is_err() {
        return;
    }

    // Promote the context to a global ref so it stays valid after this JNI call returns.
    // We intentionally leak it for process lifetime; ndk-context expects a stable pointer.
    let global_ctx = match env.new_global_ref(context) {
        Ok(g) => g,
        Err(_) => return,
    };

    let vm = match env.get_java_vm() {
        Ok(vm) => vm,
        Err(_) => return,
    };

    unsafe {
        ndk_context::initialize_android_context(
            vm.get_java_vm_pointer().cast::<c_void>(),
            global_ctx.as_obj().as_raw().cast::<c_void>(),
        );
    }

    // Leak the global ref to keep the raw pointer valid for the rest of the process.
    std::mem::forget(global_ctx);

    // Initialize keyring-core default store for encrypted MDK sqlite.
    // Android doesn't use keychain access groups, so pass an empty string.
    let _ = crate::mdk_support::init_keyring_once("");
}
