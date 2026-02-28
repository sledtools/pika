use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use mdk_core::MDK;
use mdk_sqlite_storage::MdkSqliteStorage;
use nostr_sdk::prelude::*;
use pika_core::normalize_peer_key_package_event_for_mdk;
use pika_relay_profiles::app_default_message_relays;

fn looks_like_hex(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && s.len().is_multiple_of(2) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut args = std::env::args().skip(1);
    let peer = args
        .next()
        .ok_or_else(|| anyhow!("usage: kp_debug <peer_npub_or_hex> [relay_url...]"))?;
    let relays: Vec<String> = args.collect();

    let peer_pubkey = PublicKey::parse(&peer).context("parse peer pubkey")?;
    let relays: Vec<RelayUrl> = if relays.is_empty() {
        app_default_message_relays()
            .into_iter()
            .map(|url| RelayUrl::parse(&url))
            .collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        relays
            .into_iter()
            .map(|s| RelayUrl::parse(&s))
            .collect::<std::result::Result<Vec<_>, _>>()?
    };

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let keys = Keys::generate();
        let client = Client::new(keys);
        for r in relays.iter().cloned() {
            let _ = client.add_relay(r).await;
        }
        client.connect().await;
        client.wait_for_connection(Duration::from_secs(6)).await;

        let filter = Filter::new()
            .author(peer_pubkey)
            .kind(Kind::MlsKeyPackage)
            .limit(10);

        let events = client
            .fetch_events_from(relays.clone(), filter, Duration::from_secs(10))
            .await
            .context("fetch_events_from")?;

        let mut best: Option<Event> = None;
        for e in events.into_iter() {
            if best
                .as_ref()
                .map(|b| e.created_at > b.created_at)
                .unwrap_or(true)
            {
                best = Some(e);
            }
        }
        let Some(ev) = best else {
            return Err(anyhow!("no kind 443 key package events found for peer"));
        };

        println!("event id={}", ev.id.to_hex());
        println!("created_at={}", ev.created_at.as_secs());
        println!("kind={}", ev.kind.as_u16());
        println!("tags={}", ev.tags.as_slice().len());
        for t in ev.tags.iter() {
            println!("  tag: {:?}", t.as_slice());
        }
        println!("content_len={}", ev.content.len());
        let prefix = ev.content.chars().take(64).collect::<String>();
        println!("content_prefix={:?}", prefix);
        println!("content_looks_like_hex={}", looks_like_hex(&ev.content));

        // Open an MDK instance just to parse/validate the peer key package.
        let db_path = std::path::Path::new("/tmp")
            .join("pika_kp_debug")
            .join(format!("{}.sqlite", peer_pubkey.to_hex()));
        if let Some(p) = db_path.parent() {
            std::fs::create_dir_all(p).ok();
        }
        let storage = MdkSqliteStorage::new_unencrypted(db_path)
            .context("open unencrypted mdk sqlite storage")?;
        let mdk = MDK::new(storage);

        let r1 = mdk.parse_key_package(&ev);
        println!("mdk.parse_key_package(raw)={}", fmt_res(&r1));

        let normalized = normalize_peer_key_package_event_for_mdk(&ev);
        let r2 = mdk.parse_key_package(&normalized);
        println!("mdk.parse_key_package(normalized)={}", fmt_res(&r2));

        Ok::<(), anyhow::Error>(())
    })?;

    Ok(())
}

fn fmt_res<T, E: std::fmt::Display>(r: &std::result::Result<T, E>) -> String {
    match r {
        Ok(_) => "OK".to_string(),
        Err(e) => format!("ERR({e})"),
    }
}
