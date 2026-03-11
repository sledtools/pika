pub mod app_manager;

/// App version from the root VERSION file, embedded at compile time.
pub fn app_version() -> &'static str {
    include_str!("../../../VERSION").trim()
}

pub fn app_version_display() -> String {
    let version = app_version();
    if let Some(build) = option_env!("PIKA_BUILD_NUMBER") {
        format!("v{version} ({build})")
    } else {
        format!("v{version}")
    }
}
