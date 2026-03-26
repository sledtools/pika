//! Daemon-facing integration surface for Pika hosts.
//!
//! - [`protocol`] is the stable JSONL/socket contract external adapters target.
//! - [`daemon`] is the concrete runtime host that serves that contract over stdio/socket/exec.
//! - the daemon only serves the native protocol surface; no secondary ACP backend bridge
//!   is hosted here anymore.

use anyhow::Context;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

mod call_audio;
mod call_tts;
pub mod daemon;
pub mod protocol;
mod relay;

pub use pika_marmot_runtime::welcome::ingest_welcome_from_giftwrap;
pub use pika_marmot_runtime::{
    IdentityFile, PikaMdk, ingest_application_message, load_or_create_keys, new_mdk, open_mdk,
};
pub use protocol::{DaemonCmd, InCmd, OutMsg};
pub use relay::{check_relay_ready, connect_client, subscribe_group_msgs};

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

#[cfg(test)]
mod tests {
    use super::resolve_daemon_socket_path;
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
}
