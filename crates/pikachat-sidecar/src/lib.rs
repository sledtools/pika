//! Daemon-facing integration surface for Pika hosts.
//!
//! - [`protocol`] is the stable JSONL/socket contract external adapters target.
//! - [`daemon`] is the concrete runtime host that serves that contract over stdio/socket/exec.
//! - [`acp`] is the generic ACP backend/session bridge the daemon can host alongside
//!   the native protocol surface.

use anyhow::Context;

pub mod acp;
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

fn ensure_dir(dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("mkdir {}", dir.display()))?;
    Ok(())
}
