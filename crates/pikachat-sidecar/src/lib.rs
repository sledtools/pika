use anyhow::Context;

mod call_audio;
mod call_tts;
pub mod daemon;
mod relay;

pub use pika_marmot_runtime::{
    IdentityFile, PikaMdk, ingest_application_message, ingest_welcome_from_giftwrap,
    load_or_create_keys, new_mdk, open_mdk,
};
pub use relay::{check_relay_ready, connect_client, subscribe_group_msgs};

fn ensure_dir(dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("mkdir {}", dir.display()))?;
    Ok(())
}
