use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use nostr_blossom::client::BlossomClient;
use nostr_sdk::Keys;
use pika_marmot_runtime::media::mime_from_extension;
use pika_relay_profiles::blossom_servers_or_default;
use sha2::{Digest, Sha256};
use url::Url;

#[derive(Debug, Args)]
pub struct BlossomUploadArgs {
    /// Files to upload
    #[arg(required = true)]
    files: Vec<PathBuf>,

    /// Blossom server URL (repeatable; defaults to pikachat production servers)
    #[arg(long)]
    server: Vec<String>,
}

pub async fn run(args: BlossomUploadArgs) -> Result<()> {
    let servers = blossom_servers_or_default(&args.server);
    let keys = Keys::generate();

    let mut failed = Vec::new();

    for path in &args.files {
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;

        let mime_type = mime_from_extension(path)
            .unwrap_or("application/octet-stream")
            .to_string();

        let expected_hash = hex::encode(Sha256::digest(&data));

        match try_upload(&servers, data, &mime_type, &expected_hash, &keys).await {
            Ok(url) => println!("{url}"),
            Err(e) => {
                eprintln!("failed to upload {}: {e}", path.display());
                failed.push(path.display().to_string());
            }
        }
    }

    if !failed.is_empty() {
        anyhow::bail!("{} upload(s) failed: {}", failed.len(), failed.join(", "));
    }

    Ok(())
}

async fn try_upload(
    servers: &[String],
    data: Vec<u8>,
    mime_type: &str,
    expected_hash: &str,
    keys: &Keys,
) -> Result<String, String> {
    let mut last_error: Option<String> = None;

    for server in servers {
        let base_url = match Url::parse(server) {
            Ok(url) => url,
            Err(e) => {
                last_error = Some(format!("{server}: {e}"));
                continue;
            }
        };

        let blossom = BlossomClient::new(base_url);
        let descriptor = match blossom
            .upload_blob(data.clone(), Some(mime_type.to_string()), None, Some(keys))
            .await
        {
            Ok(d) => d,
            Err(e) => {
                last_error = Some(format!("{server}: {e}"));
                continue;
            }
        };

        let descriptor_hash = descriptor.sha256.to_string();
        if !descriptor_hash.eq_ignore_ascii_case(expected_hash) {
            last_error = Some(format!(
                "{server}: hash mismatch (expected {expected_hash}, got {descriptor_hash})"
            ));
            continue;
        }

        return Ok(descriptor.url.to_string());
    }

    Err(last_error.unwrap_or_else(|| "no servers configured".into()))
}
