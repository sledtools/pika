/// Platform-native logging initialization.
///
/// - iOS: tracing-oslog → Apple unified logging (os_log) + file fallback
/// - Android: paranoid-android → logcat
/// - Tests / desktop: tracing-subscriber::fmt → stderr
///
/// Called once at the start of `FfiApp::new()`, before anything else.
///
/// On iOS the file fallback writes to `<data_dir>/pika.log` so logs are always
/// retrievable from the simulator filesystem even if os_log filtering hides them.
pub fn init_logging(#[allow(unused)] data_dir: &str) {
    #[cfg(target_os = "ios")]
    {
        use tracing_subscriber::prelude::*;

        let os_log = tracing_oslog::OsLogger::new("com.pika.app", "default");

        // Also write to a file inside the app data dir for easy retrieval.
        let log_path = std::path::Path::new(data_dir).join("pika.log");
        let _ = std::fs::create_dir_all(data_dir);
        let env_filter = tracing_subscriber::EnvFilter::new(
            "pika_core=debug,mdk_core=info,openmls=warn,nostr_relay_pool=info,info",
        );

        let file_layer = if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            Some(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::sync::Mutex::new(file))
                    .with_ansi(false)
                    .with_target(true),
            )
        } else {
            None
        };

        let _ = tracing_subscriber::registry()
            .with(env_filter)
            .with(os_log)
            .with(file_layer)
            .try_init();
    }

    #[cfg(target_os = "android")]
    {
        use tracing_subscriber::prelude::*;

        let android_layer = paranoid_android::layer("pika")
            .with_filter(tracing_subscriber::EnvFilter::new(
                "pika_core=debug,mdk_core=info,openmls=warn,nostr_relay_pool=info,info",
            ));

        let _ = tracing_subscriber::registry()
            .with(android_layer)
            .try_init();
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "pika_core=debug,info".into()),
            )
            .try_init();
    }
}
