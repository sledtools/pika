mod mdk_util;
mod relay_util;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context};
use clap::{Parser, Subcommand};
use mdk_core::prelude::*;
use nostr_sdk::prelude::*;
use serde_json::json;

#[derive(Debug, Parser)]
#[command(name = "pika-cli")]
#[command(about = "Marmot protocol CLI for testing and agent automation")]
struct Cli {
    /// State directory (identity + MLS database persist here between runs)
    #[arg(long, default_value = ".pika-cli")]
    state_dir: PathBuf,

    /// Relay websocket URLs
    #[arg(long, required = true)]
    relay: Vec<String>,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show (or create) identity for this state dir
    Identity,

    /// Publish a key package (kind 443) so peers can invite you
    PublishKp,

    /// Create a group with a peer and send them a welcome
    Invite {
        /// Peer public key (hex or npub)
        #[arg(long)]
        peer: String,

        /// Group name
        #[arg(long, default_value = "DM")]
        name: String,
    },

    /// List pending welcome invitations
    Welcomes,

    /// Accept a pending welcome and join the group
    AcceptWelcome {
        /// Wrapper event ID (hex) from the welcomes list
        #[arg(long)]
        wrapper_event_id: String,
    },

    /// List groups
    Groups,

    /// Send a message to a group
    Send {
        /// Nostr group ID (hex)
        #[arg(long)]
        group: String,

        /// Message content
        #[arg(long)]
        content: String,
    },

    /// Fetch and decrypt recent messages from a group
    Messages {
        /// Nostr group ID (hex)
        #[arg(long)]
        group: String,

        /// Max messages to return
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },

    /// Listen for incoming messages (runs until interrupted or --timeout)
    Listen {
        /// Timeout in seconds (0 = run forever)
        #[arg(long, default_value_t = 60)]
        timeout: u64,

        /// Giftwrap lookback in seconds
        #[arg(long, default_value_t = 86400)]
        lookback: u64,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .without_time()
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.state_dir)
        .with_context(|| format!("create state dir {}", cli.state_dir.display()))?;

    match &cli.cmd {
        Command::Identity => cmd_identity(&cli),
        Command::PublishKp => cmd_publish_kp(&cli).await,
        Command::Invite { peer, name } => cmd_invite(&cli, peer, name).await,
        Command::Welcomes => cmd_welcomes(&cli),
        Command::AcceptWelcome { wrapper_event_id } => cmd_accept_welcome(&cli, wrapper_event_id),
        Command::Groups => cmd_groups(&cli),
        Command::Send { group, content } => cmd_send(&cli, group, content).await,
        Command::Messages { group, limit } => cmd_messages(&cli, group, *limit),
        Command::Listen { timeout, lookback } => cmd_listen(&cli, *timeout, *lookback).await,
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn open(cli: &Cli) -> anyhow::Result<(Keys, mdk_util::PikaMdk)> {
    let keys = mdk_util::load_or_create_keys(&cli.state_dir.join("identity.json"))?;
    let mdk = mdk_util::open_mdk(&cli.state_dir)?;
    Ok((keys, mdk))
}

async fn client(cli: &Cli, keys: &Keys) -> anyhow::Result<Client> {
    relay_util::connect_client(keys, &cli.relay).await
}

fn find_group(
    mdk: &mdk_util::PikaMdk,
    nostr_group_id_hex: &str,
) -> anyhow::Result<mdk_storage_traits::groups::types::Group> {
    let gid_bytes = hex::decode(nostr_group_id_hex).context("decode group id hex")?;
    let groups = mdk.get_groups().context("get_groups")?;
    groups
        .into_iter()
        .find(|g| g.nostr_group_id.as_slice() == gid_bytes.as_slice())
        .ok_or_else(|| anyhow!("group not found: {nostr_group_id_hex}"))
}

fn print(v: serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(&v).expect("json encode"));
}

// ── Commands ────────────────────────────────────────────────────────────────

fn cmd_identity(cli: &Cli) -> anyhow::Result<()> {
    let keys = mdk_util::load_or_create_keys(&cli.state_dir.join("identity.json"))?;
    print(json!({
        "pubkey": keys.public_key().to_hex(),
        "npub": keys.public_key().to_bech32().unwrap_or_default(),
    }));
    Ok(())
}

async fn cmd_publish_kp(cli: &Cli) -> anyhow::Result<()> {
    let (keys, mdk) = open(cli)?;
    let client = client(cli, &keys).await?;
    let relays = relay_util::parse_relay_urls(&cli.relay)?;

    let (content, tags, _hash_ref) = mdk
        .create_key_package_for_event(&keys.public_key(), relays.clone())
        .context("create key package")?;

    // Strip NIP-70 "protected" tag — many popular relays reject protected events.
    let tags: Tags = tags
        .into_iter()
        .filter(|t: &Tag| !matches!(t.kind(), TagKind::Protected))
        .collect();

    let event = EventBuilder::new(Kind::MlsKeyPackage, content)
        .tags(tags)
        .sign_with_keys(&keys)
        .context("sign key package event")?;

    relay_util::publish_and_confirm(&client, &relays, &event, "keypackage").await?;
    client.shutdown().await;

    print(json!({
        "event_id": event.id.to_hex(),
        "kind": 443,
    }));
    Ok(())
}

async fn cmd_invite(cli: &Cli, peer_str: &str, group_name: &str) -> anyhow::Result<()> {
    let (keys, mdk) = open(cli)?;
    let client = client(cli, &keys).await?;
    let relays = relay_util::parse_relay_urls(&cli.relay)?;

    let peer_pubkey =
        PublicKey::parse(peer_str.trim()).with_context(|| format!("parse peer key: {peer_str}"))?;

    // Fetch peer key package.
    let peer_kp = relay_util::fetch_latest_key_package(
        &client,
        &peer_pubkey,
        &relays,
        Duration::from_secs(10),
    )
    .await
    .context("fetch peer key package — has the peer run `publish-kp`?")?;

    // Create group.
    let config = NostrGroupConfigData::new(
        group_name.to_string(),
        String::new(),
        None,
        None,
        None,
        relays.clone(),
        vec![keys.public_key(), peer_pubkey],
    );

    let result = mdk
        .create_group(&keys.public_key(), vec![peer_kp], config)
        .context("create group")?;

    let ngid = hex::encode(result.group.nostr_group_id);

    // Send welcome giftwraps.
    for rumor in result.welcome_rumors {
        let giftwrap = EventBuilder::gift_wrap(&keys, &peer_pubkey, rumor, [])
            .await
            .context("build giftwrap")?;
        relay_util::publish_and_confirm(&client, &relays, &giftwrap, "welcome").await?;
    }

    client.shutdown().await;

    print(json!({
        "nostr_group_id": ngid,
        "mls_group_id": hex::encode(result.group.mls_group_id.as_slice()),
        "peer_pubkey": peer_pubkey.to_hex(),
    }));
    Ok(())
}

fn cmd_welcomes(cli: &Cli) -> anyhow::Result<()> {
    let (_keys, mdk) = open(cli)?;
    let pending = mdk
        .get_pending_welcomes(None)
        .context("get pending welcomes")?;
    let out: Vec<serde_json::Value> = pending
        .iter()
        .map(|w| {
            json!({
                "wrapper_event_id": w.wrapper_event_id.to_hex(),
                "from_pubkey": w.welcomer.to_hex(),
                "nostr_group_id": hex::encode(w.nostr_group_id),
                "group_name": w.group_name,
            })
        })
        .collect();
    print(json!({ "welcomes": out }));
    Ok(())
}

fn cmd_accept_welcome(cli: &Cli, wrapper_event_id_hex: &str) -> anyhow::Result<()> {
    let (_keys, mdk) = open(cli)?;
    let wrapper_id = EventId::from_hex(wrapper_event_id_hex).context("parse wrapper event id")?;

    let pending = mdk
        .get_pending_welcomes(None)
        .context("get pending welcomes")?;
    let welcome = pending
        .into_iter()
        .find(|w| w.wrapper_event_id == wrapper_id)
        .ok_or_else(|| anyhow!("no pending welcome with that wrapper_event_id"))?;

    let ngid = hex::encode(welcome.nostr_group_id);
    let mls_gid = hex::encode(welcome.mls_group_id.as_slice());

    mdk.accept_welcome(&welcome).context("accept welcome")?;

    print(json!({
        "nostr_group_id": ngid,
        "mls_group_id": mls_gid,
    }));
    Ok(())
}

fn cmd_groups(cli: &Cli) -> anyhow::Result<()> {
    let (_keys, mdk) = open(cli)?;
    let groups = mdk.get_groups().context("get groups")?;
    let out: Vec<serde_json::Value> = groups
        .iter()
        .map(|g| {
            json!({
                "nostr_group_id": hex::encode(g.nostr_group_id),
                "mls_group_id": hex::encode(g.mls_group_id.as_slice()),
                "name": g.name,
                "description": g.description,
            })
        })
        .collect();
    print(json!({ "groups": out }));
    Ok(())
}

async fn cmd_send(cli: &Cli, nostr_group_id_hex: &str, content: &str) -> anyhow::Result<()> {
    let (keys, mdk) = open(cli)?;
    let client = client(cli, &keys).await?;
    let relays = relay_util::parse_relay_urls(&cli.relay)?;

    let group = find_group(&mdk, nostr_group_id_hex)?;
    let rumor = EventBuilder::new(Kind::Custom(9), content).build(keys.public_key());
    let msg_event = mdk
        .create_message(&group.mls_group_id, rumor)
        .context("create message")?;

    relay_util::publish_and_confirm(&client, &relays, &msg_event, "send_message").await?;
    client.shutdown().await;

    print(json!({
        "event_id": msg_event.id.to_hex(),
    }));
    Ok(())
}

fn cmd_messages(cli: &Cli, nostr_group_id_hex: &str, limit: usize) -> anyhow::Result<()> {
    let (_keys, mdk) = open(cli)?;
    let group = find_group(&mdk, nostr_group_id_hex)?;

    let pagination = mdk_storage_traits::groups::Pagination::new(Some(limit), None);
    let msgs = mdk
        .get_messages(&group.mls_group_id, Some(pagination))
        .context("get messages")?;

    let out: Vec<serde_json::Value> = msgs
        .iter()
        .map(|m| {
            json!({
                "message_id": m.id.to_hex(),
                "from_pubkey": m.pubkey.to_hex(),
                "content": m.content,
                "created_at": m.created_at.as_secs(),
            })
        })
        .collect();
    print(json!({ "messages": out }));
    Ok(())
}

/// Listen for new incoming messages and welcomes. Prints each as a JSON line to stdout.
/// This is the one subcommand that *does* stay running — it's an event tail.
async fn cmd_listen(cli: &Cli, timeout_sec: u64, lookback_sec: u64) -> anyhow::Result<()> {
    let (keys, mdk) = open(cli)?;
    let client = client(cli, &keys).await?;

    let mut rx = client.notifications();

    // Subscribe to giftwrap (welcomes).
    let since = Timestamp::now() - Duration::from_secs(lookback_sec);
    // Giftwraps are authored by the sender; recipients are indicated via the `p` tag.
    // Filtering by `pubkey(...)` would only match events *we* authored and would miss inbound invites.
    let gift_filter = Filter::new()
        .kind(Kind::GiftWrap)
        .custom_tag(
            SingleLetterTag::lowercase(Alphabet::P),
            keys.public_key().to_hex(),
        )
        .since(since)
        .limit(200);
    let gift_sub = client.subscribe(gift_filter, None).await?;

    // Subscribe to all known groups.
    let mut group_subs = std::collections::HashMap::<SubscriptionId, String>::new();
    if let Ok(groups) = mdk.get_groups() {
        for g in &groups {
            let ngid = hex::encode(g.nostr_group_id);
            let filter = Filter::new()
                .kind(Kind::MlsGroupMessage)
                .custom_tag(SingleLetterTag::lowercase(Alphabet::H), &ngid)
                .since(Timestamp::now() - Duration::from_secs(lookback_sec))
                .limit(200);
            if let Ok(out) = client.subscribe(filter, None).await {
                group_subs.insert(out.val, ngid);
            }
        }
    }

    let mut seen = std::collections::HashSet::<EventId>::new();

    let deadline = if timeout_sec == 0 {
        None
    } else {
        Some(tokio::time::Instant::now() + Duration::from_secs(timeout_sec))
    };

    loop {
        let recv_fut = rx.recv();
        let notification = if let Some(dl) = deadline {
            let remaining = dl.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, recv_fut).await {
                Ok(Ok(n)) => n,
                Ok(Err(_)) => break,
                Err(_) => break, // timeout
            }
        } else {
            match recv_fut.await {
                Ok(n) => n,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        };

        let RelayPoolNotification::Event {
            subscription_id,
            event,
            ..
        } = notification
        else {
            continue;
        };
        let event = *event;

        if !seen.insert(event.id) {
            continue;
        }

        // Welcome.
        if subscription_id == gift_sub.val && event.kind == Kind::GiftWrap {
            let unwrapped = match nostr_sdk::nostr::nips::nip59::extract_rumor(&keys, &event).await
            {
                Ok(u) => u,
                Err(_) => continue,
            };
            if unwrapped.rumor.kind != Kind::MlsWelcome {
                continue;
            }
            let rumor = unwrapped.rumor;
            if mdk.process_welcome(&event.id, &rumor).is_err() {
                continue;
            }
            let (ngid, group_name) = match mdk.get_pending_welcomes(None) {
                Ok(list) => list
                    .into_iter()
                    .find(|w| w.wrapper_event_id == event.id)
                    .map(|w| (hex::encode(w.nostr_group_id), w.group_name))
                    .unwrap_or_default(),
                Err(_) => (String::new(), String::new()),
            };
            let line = json!({
                "type": "welcome",
                "wrapper_event_id": event.id.to_hex(),
                "from_pubkey": unwrapped.sender.to_hex(),
                "nostr_group_id": ngid,
                "group_name": group_name,
            });
            println!("{}", serde_json::to_string(&line).unwrap());
            continue;
        }

        // Group message.
        if event.kind == Kind::MlsGroupMessage && group_subs.contains_key(&subscription_id) {
            if let Ok(MessageProcessingResult::ApplicationMessage(msg)) =
                mdk.process_message(&event)
            {
                let ngid = group_subs
                    .get(&subscription_id)
                    .cloned()
                    .unwrap_or_default();
                let line = json!({
                    "type": "message",
                    "nostr_group_id": ngid,
                    "from_pubkey": msg.pubkey.to_hex(),
                    "content": msg.content,
                    "created_at": msg.created_at.as_secs(),
                    "message_id": msg.id.to_hex(),
                });
                println!("{}", serde_json::to_string(&line).unwrap());
            }
        }
    }

    client.unsubscribe_all().await;
    client.shutdown().await;
    Ok(())
}
