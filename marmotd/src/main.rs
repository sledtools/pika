use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use ::rand::TryRngCore;
use anyhow::{Context, anyhow};
use clap::{Parser, Subcommand};
use mdk_core::prelude::*;
use mdk_sqlite_storage::MdkSqliteStorage;
use nostr_sdk::prelude::*;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::time::Instant;
use tracing::{Level, info, warn};

mod daemon;

#[derive(Debug, Parser)]
#[command(name = "marmotd")]
#[command(about = "Marmot interop lab harness (Rust track)")]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Scenario {
        #[command(subcommand)]
        scenario: ScenarioCommand,
    },
    /// Deterministic bot process that behaves like an OpenClaw-side fixture, but implemented in Rust.
    Bot {
        /// Relay websocket URL, e.g. ws://127.0.0.1:18080
        #[arg(long, default_value = "ws://127.0.0.1:18080")]
        relay: String,

        /// Folder-local state directory (will be created if missing)
        #[arg(long, default_value = ".state/openclaw-bot")]
        state_dir: PathBuf,

        /// Only accept welcomes and application prompts from this inviter pubkey (hex).
        ///
        /// If omitted, the bot will accept the first welcome it can decrypt and then treat that
        /// welcome sender as the inviter for the rest of the session.
        #[arg(long)]
        inviter_pubkey: Option<String>,

        /// Total timeout for each wait (welcome, prompt)
        #[arg(long, default_value_t = 120)]
        timeout_sec: u64,

        /// Giftwrap lookback window (NIP-59 backdates timestamps; use hours/days, not seconds)
        #[arg(long, default_value_t = 60 * 60 * 24 * 3)]
        giftwrap_lookback_sec: u64,
    },

    /// Long-running JSONL sidecar daemon intended to be embedded/invoked by OpenClaw.
    Daemon {
        /// Relay websocket URL(s), e.g. wss://relay.damus.io. Repeatable.
        #[arg(long, default_value = "ws://127.0.0.1:18080")]
        relay: Vec<String>,

        /// Folder-local state directory (will be created if missing)
        #[arg(long, default_value = ".state/marmotd")]
        state_dir: PathBuf,

        /// Giftwrap lookback window (NIP-59 backdates timestamps; use hours/days, not seconds)
        #[arg(long, default_value_t = 60 * 60 * 24 * 3)]
        giftwrap_lookback_sec: u64,

        /// Only accept welcomes and messages from these pubkeys (hex). Repeatable.
        /// If empty, all pubkeys are allowed (open mode).
        #[arg(long)]
        allow_pubkey: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
#[allow(clippy::enum_variant_names)]
enum ScenarioCommand {
    /// Phase 1: Rust <-> Rust over a local relay (no OpenClaw)
    InviteAndChat {
        /// Relay websocket URL, e.g. ws://127.0.0.1:18080
        #[arg(long, default_value = "ws://127.0.0.1:18080")]
        relay: String,

        /// Folder-local state directory (will be created if missing)
        #[arg(long, default_value = ".state")]
        state_dir: PathBuf,

        /// Total timeout for each wait (welcome, a->b, b->a)
        #[arg(long, default_value_t = 60)]
        timeout_sec: u64,

        /// Giftwrap lookback window (NIP-59 backdates timestamps; use hours/days, not seconds)
        #[arg(long, default_value_t = 60 * 60 * 24 * 3)]
        giftwrap_lookback_sec: u64,
    },

    /// Phase 2: Rust harness invites a deterministic Rust bot process (OpenClaw-side fixture)
    InviteAndChatRustBot {
        /// Relay websocket URL, e.g. ws://127.0.0.1:18080
        #[arg(long, default_value = "ws://127.0.0.1:18080")]
        relay: String,

        /// Folder-local state directory (will be created if missing)
        #[arg(long, default_value = ".state")]
        state_dir: PathBuf,

        /// Total timeout for each wait (bot ready, reply)
        #[arg(long, default_value_t = 90)]
        timeout_sec: u64,
    },

    /// Phase 3: Rust harness drives the JSONL daemon over stdio (OpenClaw integration surface).
    InviteAndChatDaemon {
        /// Relay websocket URL, e.g. ws://127.0.0.1:18080
        #[arg(long, default_value = "ws://127.0.0.1:18080")]
        relay: String,

        /// Folder-local state directory (will be created if missing)
        #[arg(long, default_value = ".state")]
        state_dir: PathBuf,

        /// Total timeout for each wait (daemon ready, welcome, reply)
        #[arg(long, default_value_t = 120)]
        timeout_sec: u64,

        /// Giftwrap lookback window (NIP-59 backdates timestamps; use hours/days, not seconds)
        #[arg(long, default_value_t = 60 * 60 * 24 * 3)]
        giftwrap_lookback_sec: u64,
    },

    /// Phase 4: Rust harness invites a peer pubkey (e.g. OpenClaw Marmot channel) and asserts a strict reply.
    InviteAndChatPeer {
        /// Relay websocket URL, e.g. ws://127.0.0.1:18080
        #[arg(long, default_value = "ws://127.0.0.1:18080")]
        relay: String,

        /// Folder-local state directory (will be created if missing)
        #[arg(long, default_value = ".state")]
        state_dir: PathBuf,

        /// Peer Nostr pubkey (hex) that publishes kind 443 KeyPackage events.
        #[arg(long)]
        peer_pubkey: String,

        /// Total timeout for each wait (keypackage, welcome, reply)
        #[arg(long, default_value_t = 120)]
        timeout_sec: u64,

        /// Giftwrap lookback window (NIP-59 backdates timestamps; use hours/days, not seconds)
        #[arg(long, default_value_t = 60 * 60 * 24 * 3)]
        giftwrap_lookback_sec: u64,
    },
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct IdentityFile {
    secret_key_hex: String,
    public_key_hex: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .without_time()
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Command::Scenario { scenario } => match scenario {
            ScenarioCommand::InviteAndChat {
                relay,
                state_dir,
                timeout_sec,
                giftwrap_lookback_sec,
            } => scenario_invite_and_chat(&relay, &state_dir, timeout_sec, giftwrap_lookback_sec)
                .await
                .context("scenario invite-and-chat failed"),
            ScenarioCommand::InviteAndChatRustBot {
                relay,
                state_dir,
                timeout_sec,
            } => scenario_invite_and_chat_rustbot(&relay, &state_dir, timeout_sec)
                .await
                .context("scenario invite-and-chat-rustbot failed"),
            ScenarioCommand::InviteAndChatDaemon {
                relay,
                state_dir,
                timeout_sec,
                giftwrap_lookback_sec,
            } => scenario_invite_and_chat_daemon(
                &relay,
                &state_dir,
                timeout_sec,
                giftwrap_lookback_sec,
            )
            .await
            .context("scenario invite-and-chat-daemon failed"),
            ScenarioCommand::InviteAndChatPeer {
                relay,
                state_dir,
                peer_pubkey,
                timeout_sec,
                giftwrap_lookback_sec,
            } => scenario_invite_and_chat_peer(
                &relay,
                &state_dir,
                &peer_pubkey,
                timeout_sec,
                giftwrap_lookback_sec,
            )
            .await
            .context("scenario invite-and-chat-peer failed"),
        },
        Command::Bot {
            relay,
            state_dir,
            inviter_pubkey,
            timeout_sec,
            giftwrap_lookback_sec,
        } => bot_main(
            &relay,
            &state_dir,
            inviter_pubkey.as_deref(),
            timeout_sec,
            giftwrap_lookback_sec,
        )
        .await
        .context("bot failed"),
        Command::Daemon {
            relay,
            state_dir,
            giftwrap_lookback_sec,
            allow_pubkey,
        } => daemon::daemon_main(&relay, &state_dir, giftwrap_lookback_sec, &allow_pubkey)
            .await
            .context("daemon failed"),
    }
}

async fn scenario_invite_and_chat_peer(
    relay: &str,
    state_dir: &Path,
    peer_pubkey_hex: &str,
    timeout_sec: u64,
    _giftwrap_lookback_sec: u64,
) -> anyhow::Result<()> {
    ensure_dir(state_dir).context("create state dir")?;
    let a_state = state_dir.join("a");
    ensure_dir(&a_state)?;

    let relay_url = RelayUrl::parse(relay).context("parse relay url")?;
    info!("[phase4] relay_url={relay}");

    check_relay_ready(relay, Duration::from_secs(90))
        .await
        .with_context(|| format!("relay readiness check failed for {relay}"))?;
    info!("[phase4] relay_ready=ok");

    let a_keys = load_or_create_keys(&a_state.join("identity.json"))?;
    info!(
        "[phase4] a_pubkey={}",
        a_keys.public_key().to_hex().to_lowercase()
    );

    let peer_pubkey = PublicKey::from_hex(peer_pubkey_hex.trim())
        .with_context(|| format!("parse peer_pubkey hex: {peer_pubkey_hex}"))?;
    info!(
        "[phase4] peer_pubkey={}",
        peer_pubkey.to_hex().to_lowercase()
    );

    let a_client = connect_client(&a_keys, relay).await?;
    let a_mdk = new_mdk(&a_state, "a")?;

    // Publish A keypackage too; not strictly required but keeps alignment with Phase 1/2.
    let _a_kp = publish_key_package(&a_client, &a_mdk, &a_keys, relay_url.clone()).await?;

    // Fetch peer keypackage from the relay (kind 443).
    let peer_kp = fetch_latest_key_package(&a_client, &peer_pubkey, relay_url.clone())
        .await
        .context("fetch peer keypackage")?;
    info!(
        "[phase4] fetched_peer_keypackage kind=443 id={} author={}",
        peer_kp.id.to_hex(),
        peer_kp.pubkey.to_hex().to_lowercase()
    );

    // Create group and invite the peer by keypackage event.
    let group_config = NostrGroupConfigData::new(
        "interop phase4".to_string(),
        "rust<->peer local relay".to_string(),
        None,
        None,
        None,
        vec![relay_url.clone()],
        vec![a_keys.public_key()],
    );

    let group_result = a_mdk
        .create_group(&a_keys.public_key(), vec![peer_kp.clone()], group_config)
        .context("create_group")?;

    let a_group = group_result.group;
    let mls_group_id = a_group.mls_group_id.clone();
    let nostr_group_id_hex = hex::encode(a_group.nostr_group_id);
    info!(
        "[phase4] group_created mls_group_id={} nostr_group_id={}",
        hex::encode(mls_group_id.as_slice()),
        nostr_group_id_hex
    );

    // Publish welcome giftwrap(s) (kind 1059, inner rumor kind 444).
    let welcome_rumors = group_result.welcome_rumors;
    if welcome_rumors.len() != 1 {
        return Err(anyhow!(
            "expected exactly 1 welcome rumor, got {}",
            welcome_rumors.len()
        ));
    }
    let mut welcome_rumor = welcome_rumors.into_iter().next().expect("checked len");
    info!(
        "[phase4] built_welcome_rumor kind={} id={}",
        welcome_rumor.kind.as_u16(),
        welcome_rumor.id().to_hex()
    );

    // Debug aid: capture the exact welcome rumor and giftwrap sent over the wire.
    let _ = std::fs::write(
        state_dir.join("phase4_welcome_rumor.json"),
        format!("{}\n", welcome_rumor.as_json()),
    );

    let giftwrap = EventBuilder::gift_wrap(&a_keys, &peer_pubkey, welcome_rumor, [])
        .await
        .context("build giftwrap")?;
    let _ = std::fs::write(
        state_dir.join("phase4_welcome_giftwrap.json"),
        format!("{}\n", giftwrap.as_json()),
    );
    publish_and_confirm(&a_client, relay_url.clone(), &giftwrap, "welcome_giftwrap").await?;

    // Subscribe for group messages and send the tokenized prompt.
    ingest_group_backlog(&a_mdk, &a_client, relay_url.clone(), &nostr_group_id_hex).await?;

    let mut a_rx = a_client.notifications();
    let a_sub = subscribe_group_msgs(&a_client, &nostr_group_id_hex).await?;

    let token = random_token();
    let prompt = format!("openclaw: reply exactly \"E2E_OK_{token}\"");
    let expected = format!("E2E_OK_{token}");
    info!("[phase4] prompt={prompt}");

    let a_rumor = EventBuilder::new(Kind::Custom(9), prompt).build(a_keys.public_key());
    let a_msg_event = a_mdk
        .create_message(&mls_group_id, a_rumor)
        .context("A create_message")?;
    publish_and_confirm(&a_client, relay_url.clone(), &a_msg_event, "a_to_peer").await?;

    let peer_received = wait_for_exact_application(
        "a_wait_peer_reply",
        &a_mdk,
        &mut a_rx,
        &a_sub,
        &peer_pubkey,
        &expected,
        Duration::from_secs(timeout_sec),
    )
    .await?;

    info!(
        "[phase4] a_received_ok pubkey={} content={}",
        peer_received.pubkey.to_hex().to_lowercase(),
        peer_received.content
    );

    a_client.unsubscribe_all().await;
    a_client.shutdown().await;

    info!("[phase4] ok token={token}");
    Ok(())
}

async fn scenario_invite_and_chat(
    relay: &str,
    state_dir: &Path,
    timeout_sec: u64,
    giftwrap_lookback_sec: u64,
) -> anyhow::Result<()> {
    ensure_dir(state_dir).context("create state dir")?;
    let a_state = state_dir.join("a");
    let b_state = state_dir.join("b");
    ensure_dir(&a_state)?;
    ensure_dir(&b_state)?;

    let relay_url = RelayUrl::parse(relay).context("parse relay url")?;
    info!("[phase1] relay_url={relay}");

    check_relay_ready(relay, Duration::from_secs(90))
        .await
        .with_context(|| format!("relay readiness check failed for {relay}"))?;
    info!("[phase1] relay_ready=ok");

    let a_keys = load_or_create_keys(&a_state.join("identity.json"))?;
    let b_keys = load_or_create_keys(&b_state.join("identity.json"))?;

    info!(
        "[phase1] a_pubkey={}",
        a_keys.public_key().to_hex().to_lowercase()
    );
    info!(
        "[phase1] b_pubkey={}",
        b_keys.public_key().to_hex().to_lowercase()
    );

    let a_client = connect_client(&a_keys, relay).await?;
    let b_client = connect_client(&b_keys, relay).await?;

    let a_mdk = new_mdk(&a_state, "a")?;
    let b_mdk = new_mdk(&b_state, "b")?;

    // 1) Publish key packages (kind 443)
    let _a_kp = publish_key_package(&a_client, &a_mdk, &a_keys, relay_url.clone()).await?;
    let b_kp = publish_key_package(&b_client, &b_mdk, &b_keys, relay_url.clone()).await?;

    // 2) Client A fetches B's key package from the relay (avoid "passed by reference" false pass)
    let fetched_b_kp = fetch_latest_key_package(&a_client, &b_keys.public_key(), relay_url.clone())
        .await
        .context("fetch B keypackage")?;
    if fetched_b_kp.id != b_kp.id {
        return Err(anyhow!(
            "unexpected B keypackage event id: fetched={} published={}",
            fetched_b_kp.id,
            b_kp.id
        ));
    }

    // 3) Create group and invite B by keypackage event
    let group_config = NostrGroupConfigData::new(
        "interop phase1".to_string(),
        "rust<->rust local relay".to_string(),
        None,
        None,
        None,
        vec![relay_url.clone()],
        vec![a_keys.public_key()],
    );

    let group_result = a_mdk
        .create_group(
            &a_keys.public_key(),
            vec![fetched_b_kp.clone()],
            group_config,
        )
        .context("create_group")?;

    let a_group = group_result.group;
    let mls_group_id = a_group.mls_group_id.clone();
    let nostr_group_id_hex = hex::encode(a_group.nostr_group_id);
    info!(
        "[phase1] group_created mls_group_id={} nostr_group_id={}",
        hex::encode(mls_group_id.as_slice()),
        nostr_group_id_hex
    );

    // 4) Publish welcome giftwrap(s) (kind 1059, inner rumor kind 444)
    // For Phase 1 we only invite B, so we expect exactly one welcome rumor.
    let welcome_rumors = group_result.welcome_rumors;
    if welcome_rumors.len() != 1 {
        return Err(anyhow!(
            "expected exactly 1 welcome rumor, got {}",
            welcome_rumors.len()
        ));
    }

    let mut welcome_rumor = welcome_rumors.into_iter().next().expect("checked len");

    info!(
        "[phase1] built_welcome_rumor kind={} id={}",
        welcome_rumor.kind.as_u16(),
        welcome_rumor.id().to_hex()
    );

    let giftwrap = EventBuilder::gift_wrap(&a_keys, &b_keys.public_key(), welcome_rumor, [])
        .await
        .context("build giftwrap")?;
    publish_and_confirm(&a_client, relay_url.clone(), &giftwrap, "welcome_giftwrap").await?;

    // 5) Client B subscribes/polls for giftwrap kind 1059, unwraps, asserts inner kind 444, joins
    let (wrapper_event_id, mut unwrapped_rumor) = wait_for_welcome_giftwrap(
        &b_client,
        &b_keys,
        &a_keys.public_key(),
        giftwrap_lookback_sec,
        Duration::from_secs(timeout_sec),
    )
    .await
    .context("wait for welcome giftwrap")?;

    info!(
        "[phase1] welcome_unwrapped wrapper_id={} rumor_kind={} rumor_id={} rumor_pubkey={}",
        wrapper_event_id.to_hex(),
        unwrapped_rumor.kind.as_u16(),
        unwrapped_rumor.id().to_hex(),
        unwrapped_rumor.pubkey.to_hex().to_lowercase()
    );

    b_mdk
        .process_welcome(&wrapper_event_id, &unwrapped_rumor)
        .context("mdk process_welcome")?;

    let pending = b_mdk
        .get_pending_welcomes(None)
        .context("get_pending_welcomes")?;
    if pending.len() != 1 {
        return Err(anyhow!("expected 1 pending welcome, got {}", pending.len()));
    }
    b_mdk
        .accept_welcome(&pending[0])
        .context("accept_welcome")?;

    let b_groups = b_mdk.get_groups().context("b get_groups")?;
    if b_groups.len() != 1 {
        return Err(anyhow!(
            "expected B to have 1 group, got {}",
            b_groups.len()
        ));
    }
    let b_group = &b_groups[0];
    let b_nostr_group_id_hex = hex::encode(b_group.nostr_group_id);
    if b_nostr_group_id_hex != nostr_group_id_hex {
        return Err(anyhow!(
            "group id mismatch: A={} B={}",
            nostr_group_id_hex,
            b_nostr_group_id_hex
        ));
    }
    info!("[phase1] b_joined_group nostr_group_id={b_nostr_group_id_hex}");

    // 6) Both sides ingest group backlog (kind 445, #h=<groupId>)
    // (New group should be empty, but do it anyway for correctness.)
    ingest_group_backlog(&a_mdk, &a_client, relay_url.clone(), &nostr_group_id_hex).await?;
    ingest_group_backlog(&b_mdk, &b_client, relay_url.clone(), &nostr_group_id_hex).await?;

    // 7) A -> B application message, strict match
    let token = random_token();
    let a_to_b = format!("HELLO_FROM_A_{token}");
    let b_to_a = format!("HELLO_FROM_B_{token}");

    let mut a_rx = a_client.notifications();
    let mut b_rx = b_client.notifications();

    let a_sub = subscribe_group_msgs(&a_client, &nostr_group_id_hex).await?;
    let b_sub = subscribe_group_msgs(&b_client, &nostr_group_id_hex).await?;

    let a_rumor = EventBuilder::new(Kind::Custom(9), a_to_b.clone()).build(a_keys.public_key());
    let a_msg_event = a_mdk
        .create_message(&mls_group_id, a_rumor)
        .context("A create_message")?;
    publish_and_confirm(&a_client, relay_url.clone(), &a_msg_event, "a_to_b").await?;

    let b_received = wait_for_exact_application(
        "b_wait_a_to_b",
        &b_mdk,
        &mut b_rx,
        &b_sub,
        &a_keys.public_key(),
        &a_to_b,
        Duration::from_secs(timeout_sec),
    )
    .await?;
    info!(
        "[phase1] b_received_ok pubkey={} content={}",
        b_received.pubkey.to_hex().to_lowercase(),
        b_received.content
    );

    let b_rumor = EventBuilder::new(Kind::Custom(9), b_to_a.clone()).build(b_keys.public_key());
    let b_msg_event = b_mdk
        .create_message(&mls_group_id, b_rumor)
        .context("B create_message")?;
    publish_and_confirm(&b_client, relay_url.clone(), &b_msg_event, "b_to_a").await?;

    let a_received = wait_for_exact_application(
        "a_wait_b_to_a",
        &a_mdk,
        &mut a_rx,
        &a_sub,
        &b_keys.public_key(),
        &b_to_a,
        Duration::from_secs(timeout_sec),
    )
    .await?;
    info!(
        "[phase1] a_received_ok pubkey={} content={}",
        a_received.pubkey.to_hex().to_lowercase(),
        a_received.content
    );

    // Best-effort cleanup
    a_client.unsubscribe_all().await;
    b_client.unsubscribe_all().await;
    a_client.shutdown().await;
    b_client.shutdown().await;

    info!("[phase1] ok token={token}");
    Ok(())
}

async fn scenario_invite_and_chat_rustbot(
    relay: &str,
    state_dir: &Path,
    timeout_sec: u64,
) -> anyhow::Result<()> {
    ensure_dir(state_dir).context("create state dir")?;
    let a_state = state_dir.join("a");
    let bot_state = state_dir.join("openclaw-bot");
    ensure_dir(&a_state)?;
    ensure_dir(&bot_state)?;

    let relay_url = RelayUrl::parse(relay).context("parse relay url")?;
    info!("[phase2] relay_url={relay}");

    check_relay_ready(relay, Duration::from_secs(90))
        .await
        .with_context(|| format!("relay readiness check failed for {relay}"))?;
    info!("[phase2] relay_ready=ok");

    let a_keys = load_or_create_keys(&a_state.join("identity.json"))?;
    info!(
        "[phase2] a_pubkey={}",
        a_keys.public_key().to_hex().to_lowercase()
    );

    // Start the bot fixture after we know A's pubkey, so the bot can be strict about the inviter.
    let (mut bot_child, bot_pubkey) = spawn_rust_bot(
        relay,
        &bot_state,
        &a_keys.public_key(),
        Duration::from_secs(timeout_sec),
    )
    .await
    .context("spawn rust bot")?;

    info!(
        "[phase2] bot_ready pubkey={}",
        bot_pubkey.to_hex().to_lowercase()
    );

    let a_client = connect_client(&a_keys, relay).await?;
    let a_mdk = new_mdk(&a_state, "a")?;

    // Publish A keypackage too; it isn't strictly required for this scenario but keeps us aligned with Phase 1.
    let _a_kp = publish_key_package(&a_client, &a_mdk, &a_keys, relay_url.clone()).await?;

    // Fetch bot keypackage from the relay (stronger than trusting bot stdout).
    let bot_kp = fetch_latest_key_package(&a_client, &bot_pubkey, relay_url.clone())
        .await
        .context("fetch bot keypackage")?;
    info!(
        "[phase2] fetched_bot_keypackage kind=443 id={} author={}",
        bot_kp.id.to_hex(),
        bot_kp.pubkey.to_hex().to_lowercase()
    );

    // Create group and invite the bot by keypackage event.
    let group_config = NostrGroupConfigData::new(
        "interop phase2".to_string(),
        "rust<->tsbot local relay".to_string(),
        None,
        None,
        None,
        vec![relay_url.clone()],
        vec![a_keys.public_key()],
    );

    let group_result = a_mdk
        .create_group(&a_keys.public_key(), vec![bot_kp.clone()], group_config)
        .context("create_group")?;

    let a_group = group_result.group;
    let mls_group_id = a_group.mls_group_id.clone();
    let nostr_group_id_hex = hex::encode(a_group.nostr_group_id);
    info!(
        "[phase2] group_created mls_group_id={} nostr_group_id={}",
        hex::encode(mls_group_id.as_slice()),
        nostr_group_id_hex
    );

    // Publish welcome giftwrap(s) (kind 1059, inner rumor kind 444)
    let welcome_rumors = group_result.welcome_rumors;
    if welcome_rumors.len() != 1 {
        // Best-effort cleanup of the bot process before returning.
        let _ = bot_child.kill().await;
        return Err(anyhow!(
            "expected exactly 1 welcome rumor, got {}",
            welcome_rumors.len()
        ));
    }
    let mut welcome_rumor = welcome_rumors.into_iter().next().expect("checked len");
    info!(
        "[phase2] built_welcome_rumor kind={} id={}",
        welcome_rumor.kind.as_u16(),
        welcome_rumor.id().to_hex()
    );

    // Debug aid: capture the exact welcome rumor (kind 444) before wrapping, so we can inspect
    // encoding/tags across implementations.
    let _ = std::fs::write(
        state_dir.join("phase2_welcome_rumor.json"),
        format!("{}\n", welcome_rumor.as_json()),
    );

    let giftwrap = EventBuilder::gift_wrap(&a_keys, &bot_pubkey, welcome_rumor, [])
        .await
        .context("build giftwrap")?;

    // Debug aid: capture the exact NIP-59 event sent over the wire for cross-impl investigation.
    // This stays folder-local under `.state/`.
    let json = giftwrap.as_json();
    let _ = std::fs::write(
        state_dir.join("phase2_welcome_giftwrap.json"),
        format!("{json}\n"),
    );

    publish_and_confirm(&a_client, relay_url.clone(), &giftwrap, "welcome_giftwrap").await?;

    // Subscribe for group messages and send the tokenized prompt.
    ingest_group_backlog(&a_mdk, &a_client, relay_url.clone(), &nostr_group_id_hex).await?;

    let mut a_rx = a_client.notifications();
    let a_sub = subscribe_group_msgs(&a_client, &nostr_group_id_hex).await?;

    let token = random_token();
    let prompt = format!("openclaw: reply exactly \"E2E_OK_{token}\"");
    let expected = format!("E2E_OK_{token}");
    info!("[phase2] prompt={prompt}");

    let a_rumor = EventBuilder::new(Kind::Custom(9), prompt).build(a_keys.public_key());
    let a_msg_event = a_mdk
        .create_message(&mls_group_id, a_rumor)
        .context("A create_message")?;
    publish_and_confirm(&a_client, relay_url.clone(), &a_msg_event, "a_to_bot").await?;

    let bot_received = wait_for_exact_application(
        "a_wait_bot_reply",
        &a_mdk,
        &mut a_rx,
        &a_sub,
        &bot_pubkey,
        &expected,
        Duration::from_secs(timeout_sec),
    )
    .await?;

    info!(
        "[phase2] a_received_ok pubkey={} content={}",
        bot_received.pubkey.to_hex().to_lowercase(),
        bot_received.content
    );

    // Best-effort cleanup: bot should exit on its own after replying.
    let _ = tokio::time::timeout(Duration::from_secs(10), bot_child.wait()).await;
    let _ = bot_child.kill().await;

    a_client.unsubscribe_all().await;
    a_client.shutdown().await;

    info!("[phase2] ok token={token}");
    Ok(())
}

async fn scenario_invite_and_chat_daemon(
    relay: &str,
    state_dir: &Path,
    timeout_sec: u64,
    giftwrap_lookback_sec: u64,
) -> anyhow::Result<()> {
    ensure_dir(state_dir).context("create state dir")?;
    let a_state = state_dir.join("a");
    let d_state = state_dir.join("marmotd");
    ensure_dir(&a_state)?;
    ensure_dir(&d_state)?;

    let relay_url = RelayUrl::parse(relay).context("parse relay url")?;
    info!("[phase3] relay_url={relay}");

    check_relay_ready(relay, Duration::from_secs(90))
        .await
        .with_context(|| format!("relay readiness check failed for {relay}"))?;
    info!("[phase3] relay_ready=ok");

    let a_keys = load_or_create_keys(&a_state.join("identity.json"))?;
    info!(
        "[phase3] a_pubkey={}",
        a_keys.public_key().to_hex().to_lowercase()
    );

    // Spawn daemon and speak JSONL over stdio.
    let exe = std::env::current_exe().context("resolve current exe")?;
    let mut cmd = tokio::process::Command::new(&exe);
    cmd.arg("daemon")
        .arg("--relay")
        .arg(relay)
        .arg("--state-dir")
        .arg(&d_state)
        .arg("--giftwrap-lookback-sec")
        .arg(giftwrap_lookback_sec.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().context("spawn daemon")?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("daemon stdin not captured"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("daemon stdout not captured"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("daemon stderr not captured"))?;

    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("[marmotd stderr] {line}");
        }
    });

    let mut out_lines = tokio::io::BufReader::new(stdout).lines();

    let deadline = Instant::now() + Duration::from_secs(timeout_sec);
    let mut daemon_pubkey: Option<PublicKey> = None;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let line = tokio::time::timeout(remaining, out_lines.next_line())
            .await
            .context("timeout waiting for daemon ready")?
            .context("daemon stdout closed")?;
        let Some(line) = line else {
            break;
        };
        let v: serde_json::Value =
            serde_json::from_str(&line).with_context(|| format!("decode daemon json: {line}"))?;
        if v.get("type").and_then(|t| t.as_str()) == Some("ready") {
            let pk = v
                .get("pubkey")
                .and_then(|p| p.as_str())
                .ok_or_else(|| anyhow!("daemon ready missing pubkey"))?;
            daemon_pubkey = Some(PublicKey::from_hex(pk).context("parse daemon pubkey")?);
            break;
        }
    }
    let daemon_pubkey = daemon_pubkey.ok_or_else(|| anyhow!("daemon did not emit ready"))?;
    info!(
        "[phase3] daemon_ready pubkey={}",
        daemon_pubkey.to_hex().to_lowercase()
    );

    async fn daemon_send(
        stdin: &mut tokio::process::ChildStdin,
        v: serde_json::Value,
    ) -> anyhow::Result<()> {
        let s = serde_json::to_string(&v).context("encode daemon cmd")?;
        stdin.write_all(s.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn daemon_wait_for<F>(
        lines: &mut tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
        timeout: Duration,
        mut pred: F,
    ) -> anyhow::Result<serde_json::Value>
    where
        F: FnMut(&serde_json::Value) -> bool,
    {
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(anyhow!("timeout waiting for daemon event"));
            }
            let remaining = deadline.duration_since(now);
            let line = tokio::time::timeout(remaining, lines.next_line())
                .await
                .context("daemon stdout recv timeout")?
                .context("daemon stdout closed")?;
            let Some(line) = line else {
                return Err(anyhow!("daemon stdout closed"));
            };
            let v: serde_json::Value = serde_json::from_str(&line)
                .with_context(|| format!("decode daemon json: {line}"))?;
            if pred(&v) {
                return Ok(v);
            }
        }
    }

    // Tell the daemon to publish its keypackage (kind 443).
    daemon_send(
        &mut stdin,
        serde_json::json!({"cmd":"publish_keypackage","request_id":"kp1","relays":[relay]}),
    )
    .await?;
    let _ = daemon_wait_for(&mut out_lines, Duration::from_secs(timeout_sec), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("ok")
            && v.get("request_id").and_then(|id| id.as_str()) == Some("kp1")
    })
    .await?;

    // Now run the same Rust->invite flow from A, but accept/join via daemon commands.
    let a_client = connect_client(&a_keys, relay).await?;
    let a_mdk = new_mdk(&a_state, "a")?;

    let _a_kp = publish_key_package(&a_client, &a_mdk, &a_keys, relay_url.clone()).await?;
    let daemon_kp = fetch_latest_key_package(&a_client, &daemon_pubkey, relay_url.clone())
        .await
        .context("fetch daemon keypackage")?;

    let group_config = NostrGroupConfigData::new(
        "interop phase3".to_string(),
        "rust<->daemon local relay".to_string(),
        None,
        None,
        None,
        vec![relay_url.clone()],
        vec![a_keys.public_key()],
    );
    let group_result = a_mdk
        .create_group(&a_keys.public_key(), vec![daemon_kp.clone()], group_config)
        .context("create_group")?;

    let a_group = group_result.group;
    let mls_group_id = a_group.mls_group_id.clone();
    let nostr_group_id_hex = hex::encode(a_group.nostr_group_id);
    info!(
        "[phase3] group_created mls_group_id={} nostr_group_id={}",
        hex::encode(mls_group_id.as_slice()),
        nostr_group_id_hex
    );

    let welcome_rumor = group_result
        .welcome_rumors
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("expected welcome rumor"))?;
    let giftwrap = EventBuilder::gift_wrap(&a_keys, &daemon_pubkey, welcome_rumor, [])
        .await
        .context("build giftwrap")?;
    publish_and_confirm(&a_client, relay_url.clone(), &giftwrap, "welcome_giftwrap").await?;

    // Wait for daemon to report the staged welcome, then accept it.
    let welcome = daemon_wait_for(&mut out_lines, Duration::from_secs(timeout_sec), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("welcome_received")
    })
    .await?;
    let wrapper_event_id = welcome
        .get("wrapper_event_id")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("welcome_received missing wrapper_event_id"))?
        .to_string();

    daemon_send(
        &mut stdin,
        serde_json::json!({"cmd":"accept_welcome","request_id":"acc1","wrapper_event_id":wrapper_event_id}),
    )
    .await?;
    let _ = daemon_wait_for(&mut out_lines, Duration::from_secs(timeout_sec), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("group_joined")
    })
    .await?;

    ingest_group_backlog(&a_mdk, &a_client, relay_url.clone(), &nostr_group_id_hex).await?;
    let mut a_rx = a_client.notifications();
    let a_sub = subscribe_group_msgs(&a_client, &nostr_group_id_hex).await?;

    let token = random_token();
    let prompt = format!("openclaw: reply exactly \"E2E_OK_{token}\"");
    let expected = format!("E2E_OK_{token}");
    info!("[phase3] prompt={prompt}");

    let a_rumor = EventBuilder::new(Kind::Custom(9), prompt.clone()).build(a_keys.public_key());
    let a_msg_event = a_mdk
        .create_message(&mls_group_id, a_rumor)
        .context("A create_message")?;
    publish_and_confirm(&a_client, relay_url.clone(), &a_msg_event, "a_to_daemon").await?;

    // When daemon reports it received the prompt, command it to send the reply.
    let _ = daemon_wait_for(&mut out_lines, Duration::from_secs(timeout_sec), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("message_received")
            && v.get("from_pubkey").and_then(|p| p.as_str())
                == Some(&a_keys.public_key().to_hex().to_lowercase())
            && v.get("content").and_then(|c| c.as_str()) == Some(&prompt)
    })
    .await?;

    daemon_send(
        &mut stdin,
        serde_json::json!({"cmd":"send_message","request_id":"send1","nostr_group_id":nostr_group_id_hex,"content":expected}),
    )
    .await?;
    let _ = daemon_wait_for(&mut out_lines, Duration::from_secs(timeout_sec), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("ok")
            && v.get("request_id").and_then(|id| id.as_str()) == Some("send1")
    })
    .await?;

    let received = wait_for_exact_application(
        "a_wait_daemon_reply",
        &a_mdk,
        &mut a_rx,
        &a_sub,
        &daemon_pubkey,
        &format!("E2E_OK_{token}"),
        Duration::from_secs(timeout_sec),
    )
    .await?;

    info!(
        "[phase3] a_received_ok pubkey={} content={}",
        received.pubkey.to_hex().to_lowercase(),
        received.content
    );

    // Shutdown daemon.
    let _ = daemon_send(
        &mut stdin,
        serde_json::json!({"cmd":"shutdown","request_id":"bye"}),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;

    a_client.unsubscribe_all().await;
    a_client.shutdown().await;
    info!("[phase3] ok token={token}");
    Ok(())
}

async fn bot_main(
    relay: &str,
    state_dir: &Path,
    inviter_pubkey_hex: Option<&str>,
    timeout_sec: u64,
    giftwrap_lookback_sec: u64,
) -> anyhow::Result<()> {
    ensure_dir(state_dir).context("create bot state dir")?;

    let relay_url = RelayUrl::parse(relay).context("parse relay url")?;
    check_relay_ready(relay, Duration::from_secs(90))
        .await
        .with_context(|| format!("relay readiness check failed for {relay}"))?;

    let expected_inviter = match inviter_pubkey_hex {
        Some(hex) => Some(PublicKey::from_hex(hex).context("parse inviter pubkey hex")?),
        None => None,
    };

    let keys = load_or_create_keys(&state_dir.join("identity.json"))?;

    // Parseable stdout line for the harness to latch onto.
    println!(
        "[openclaw_bot] ready pubkey={} npub={}",
        keys.public_key().to_hex().to_lowercase(),
        keys.public_key()
            .to_bech32()
            .unwrap_or_else(|_| "<npub_err>".to_string())
    );

    let client = connect_client(&keys, relay).await?;
    let mdk = new_mdk(state_dir, "bot")?;

    let kp_event = publish_key_package(&client, &mdk, &keys, relay_url.clone()).await?;
    println!(
        "[openclaw_bot] published kind={} id={} ok=true",
        kp_event.kind.as_u16(),
        kp_event.id.to_hex()
    );

    let (wrapper_event_id, mut unwrapped_rumor, inviter_pubkey) = match expected_inviter.as_ref() {
        Some(pk) => {
            let (wid, rumor) = wait_for_welcome_giftwrap(
                &client,
                &keys,
                pk,
                giftwrap_lookback_sec,
                Duration::from_secs(timeout_sec),
            )
            .await
            .context("wait for welcome giftwrap")?;
            (wid, rumor, *pk)
        }
        None => {
            let (wid, rumor, pk) = wait_for_welcome_giftwrap_any_sender(
                &client,
                &keys,
                giftwrap_lookback_sec,
                Duration::from_secs(timeout_sec),
            )
            .await
            .context("wait for welcome giftwrap")?;
            (wid, rumor, pk)
        }
    };

    println!(
        "[openclaw_bot] welcome_unwrapped wrapper_id={} rumor_kind={} rumor_id={} rumor_pubkey={}",
        wrapper_event_id.to_hex(),
        unwrapped_rumor.kind.as_u16(),
        unwrapped_rumor.id().to_hex(),
        unwrapped_rumor.pubkey.to_hex().to_lowercase()
    );

    mdk.process_welcome(&wrapper_event_id, &unwrapped_rumor)
        .context("mdk process_welcome")?;
    let pending = mdk
        .get_pending_welcomes(None)
        .context("get_pending_welcomes")?;
    if pending.len() != 1 {
        return Err(anyhow!("expected 1 pending welcome, got {}", pending.len()));
    }
    mdk.accept_welcome(&pending[0]).context("accept_welcome")?;

    let groups = mdk.get_groups().context("get_groups")?;
    if groups.len() != 1 {
        return Err(anyhow!(
            "expected bot to have 1 group, got {}",
            groups.len()
        ));
    }
    let group = &groups[0];
    let nostr_group_id_hex = hex::encode(group.nostr_group_id);
    let mls_group_id = group.mls_group_id.clone();
    println!("[openclaw_bot] joined_group nostr_group_id={nostr_group_id_hex}");

    let mut rx = client.notifications();
    let sub = subscribe_group_msgs(&client, &nostr_group_id_hex).await?;

    let deadline = Instant::now() + Duration::from_secs(timeout_sec);
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(anyhow!("timeout waiting for prompt (bot_wait_prompt)"));
        }
        let remaining = deadline.duration_since(now);

        let notification = tokio::time::timeout(remaining, rx.recv())
            .await
            .context("recv timeout (bot_wait_prompt)")?
            .context("notification channel closed (bot_wait_prompt)")?;

        let RelayPoolNotification::Event {
            subscription_id: sid,
            event,
            ..
        } = notification
        else {
            continue;
        };

        if sid != sub {
            continue;
        }

        let ev = *event;
        if ev.kind != Kind::MlsGroupMessage {
            continue;
        }

        match mdk.process_message(&ev) {
            Ok(MessageProcessingResult::ApplicationMessage(msg)) => {
                let from = msg.pubkey;
                if from != inviter_pubkey {
                    continue;
                }

                let Some(reply) = parse_openclaw_prompt(&msg.content) else {
                    continue;
                };

                println!("[openclaw_bot] replying content={reply}");
                let reply_rumor =
                    EventBuilder::new(Kind::Custom(9), reply.clone()).build(keys.public_key());
                let reply_event = mdk
                    .create_message(&mls_group_id, reply_rumor)
                    .context("bot create_message")?;
                publish_and_confirm(&client, relay_url.clone(), &reply_event, "bot_reply").await?;
                println!("[openclaw_bot] ok replied={reply}");

                client.unsubscribe_all().await;
                client.shutdown().await;
                return Ok(());
            }
            Ok(other) => {
                warn!(
                    "[openclaw_bot] inbound kind=445 id={} non_app={:?}",
                    ev.id.to_hex(),
                    other
                );
            }
            Err(err) => {
                warn!(
                    "[openclaw_bot] inbound kind=445 id={} MLS_DECODE_FAIL err={}",
                    ev.id.to_hex(),
                    err
                );
            }
        }
    }
}

fn parse_openclaw_prompt(content: &str) -> Option<String> {
    // Exact match required by the harness:
    // openclaw: reply exactly "E2E_OK_<hex token>"
    //
    // For easier device automation (adb/agent tooling), also accept:
    // - `ping` -> reply `pong`
    // - `openclaw: reply exactly E2E_OK_<hex token>` (no quotes)
    //
    // This keeps the strict quoted form (used by existing scenarios) while enabling
    // a simple manual E2E loop from mobile clients without having to type quotes.
    if content.trim() == "ping" {
        return Some("pong".to_string());
    }

    const PREFIX: &str = "openclaw: reply exactly \"";
    if !content.starts_with(PREFIX) || !content.ends_with('"') {
        const PREFIX_NO_QUOTES: &str = "openclaw: reply exactly ";
        if !content.starts_with(PREFIX_NO_QUOTES) {
            return None;
        }
        let inner = content[PREFIX_NO_QUOTES.len()..].trim();
        if !inner.starts_with("E2E_OK_") {
            return None;
        }
        let token = &inner["E2E_OK_".len()..];
        if token.is_empty() {
            return None;
        }
        if !token
            .as_bytes()
            .iter()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return None;
        }
        return Some(inner.to_string());
    }
    let inner = &content[PREFIX.len()..content.len() - 1];
    if !inner.starts_with("E2E_OK_") {
        return None;
    }
    let token = &inner["E2E_OK_".len()..];
    if token.is_empty() {
        return None;
    }
    if !token
        .as_bytes()
        .iter()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    Some(inner.to_string())
}

fn new_mdk(state_dir: &Path, label: &str) -> anyhow::Result<MDK<MdkSqliteStorage>> {
    let db_path = state_dir.join("mdk.sqlite");
    // Phase 1 uses unencrypted SQLite so all state stays inspectable under `.state/`.
    // (This is dev/test harness code, not production.)
    let _ = label; // keep the call-sites explicit; label helps when we switch to encrypted DBs.
    let storage = MdkSqliteStorage::new_unencrypted(db_path).context("open mdk sqlite storage")?;
    Ok(MDK::new(storage))
}

async fn connect_client(keys: &Keys, relay: &str) -> anyhow::Result<Client> {
    let client = Client::new(keys.clone());
    client.add_relay(relay).await.context("add relay")?;
    client.connect().await;
    Ok(client)
}

async fn publish_key_package(
    client: &Client,
    mdk: &MDK<MdkSqliteStorage>,
    keys: &Keys,
    relay_url: RelayUrl,
) -> anyhow::Result<Event> {
    let (kp_content, kp_tags) = mdk
        .create_key_package_for_event(&keys.public_key(), vec![relay_url.clone()])
        .context("create_key_package_for_event")?;

    let event = EventBuilder::new(Kind::MlsKeyPackage, kp_content)
        .tags(kp_tags)
        .sign_with_keys(keys)
        .context("sign keypackage event")?;

    publish_and_confirm(client, relay_url, &event, "keypackage").await?;

    // We confirm below with the relay in publish_and_confirm in normal flow; keep this log stable.
    info!(
        "[phase1] publish kind=443 id={} author={}",
        event.id.to_hex(),
        event.pubkey.to_hex().to_lowercase()
    );

    Ok(event)
}

async fn publish_and_confirm(
    client: &Client,
    relay_url: RelayUrl,
    event: &Event,
    label: &str,
) -> anyhow::Result<()> {
    let out = client
        .send_event_to([relay_url.clone()], event)
        .await
        .with_context(|| format!("send_event_to failed ({label})"))?;
    if out.success.is_empty() {
        return Err(anyhow!(
            "event publish had no successful relays ({label}): {out:?}"
        ));
    }

    info!(
        "[phase1] published label={label} kind={} id={}",
        event.kind.as_u16(),
        event.id.to_hex()
    );

    // Confirm we can fetch it back from the relay (stronger than only trusting client-side send result)
    let fetched = client
        .fetch_events_from(
            [relay_url.clone()],
            Filter::new().id(event.id),
            Duration::from_secs(5),
        )
        .await
        .with_context(|| format!("fetch_events_from failed ({label})"))?;

    if !fetched.iter().any(|e| e.id == event.id) {
        return Err(anyhow!(
            "published event not found on relay after send ({label}) id={}",
            event.id
        ));
    }

    Ok(())
}

async fn fetch_latest_key_package(
    client: &Client,
    author: &PublicKey,
    relay_url: RelayUrl,
) -> anyhow::Result<Event> {
    let filter = Filter::new()
        .kind(Kind::MlsKeyPackage)
        .author(*author)
        .limit(1);
    let events = client
        .fetch_events_from([relay_url], filter, Duration::from_secs(10))
        .await
        .context("fetch keypackage events")?;
    events
        .iter()
        .next()
        .cloned()
        .ok_or_else(|| anyhow!("no keypackage event found for author {}", author.to_hex()))
}

async fn wait_for_welcome_giftwrap(
    client: &Client,
    receiver_keys: &Keys,
    expected_sender: &PublicKey,
    lookback_sec: u64,
    timeout: Duration,
) -> anyhow::Result<(EventId, UnsignedEvent)> {
    let mut rx = client.notifications();

    let since = Timestamp::now() - Duration::from_secs(lookback_sec);
    let filter = Filter::new()
        .kind(Kind::GiftWrap)
        .pubkey(receiver_keys.public_key())
        .since(since)
        .limit(200);

    let sub = client
        .subscribe(filter, None)
        .await
        .context("subscribe giftwrap")?;

    let deadline = Instant::now() + timeout;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(anyhow!("timeout waiting for giftwrap welcome"));
        }
        let remaining = deadline.duration_since(now);

        let notification = tokio::time::timeout(remaining, rx.recv())
            .await
            .context("giftwrap recv timeout")?
            .context("giftwrap notification channel closed")?;

        let RelayPoolNotification::Event {
            subscription_id,
            event,
            ..
        } = notification
        else {
            continue;
        };

        if subscription_id != sub.val {
            continue;
        }

        let event = *event;
        if event.kind != Kind::GiftWrap {
            continue;
        }

        info!(
            "[phase1] observed kind=1059 id={} author={}",
            event.id.to_hex(),
            event.pubkey.to_hex().to_lowercase()
        );

        let unwrapped = nostr_sdk::nostr::nips::nip59::extract_rumor(receiver_keys, &event)
            .await
            .context("nip59 extract_rumor")?;

        if unwrapped.sender != *expected_sender {
            continue;
        }
        if unwrapped.rumor.kind != Kind::MlsWelcome {
            continue;
        }

        // Strong, Phase 1 correctness checks.
        if unwrapped.rumor.pubkey != *expected_sender {
            return Err(anyhow!(
                "welcome rumor pubkey mismatch: expected={} got={}",
                expected_sender.to_hex(),
                unwrapped.rumor.pubkey.to_hex()
            ));
        }

        client.unsubscribe(&sub.val).await;
        return Ok((event.id, unwrapped.rumor));
    }
}

async fn wait_for_welcome_giftwrap_any_sender(
    client: &Client,
    receiver_keys: &Keys,
    lookback_sec: u64,
    timeout: Duration,
) -> anyhow::Result<(EventId, UnsignedEvent, PublicKey)> {
    let mut rx = client.notifications();

    let since = Timestamp::now() - Duration::from_secs(lookback_sec);
    let filter = Filter::new()
        .kind(Kind::GiftWrap)
        .pubkey(receiver_keys.public_key())
        .since(since)
        .limit(200);

    let sub = client
        .subscribe(filter, None)
        .await
        .context("subscribe giftwrap")?;

    let deadline = Instant::now() + timeout;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(anyhow!("timeout waiting for giftwrap welcome"));
        }
        let remaining = deadline.duration_since(now);

        let notification = tokio::time::timeout(remaining, rx.recv())
            .await
            .context("giftwrap recv timeout")?
            .context("giftwrap notification channel closed")?;

        let RelayPoolNotification::Event {
            subscription_id,
            event,
            ..
        } = notification
        else {
            continue;
        };

        if subscription_id != sub.val {
            continue;
        }

        let event = *event;
        if event.kind != Kind::GiftWrap {
            continue;
        }

        info!(
            "[phase1] observed kind=1059 id={} author={}",
            event.id.to_hex(),
            event.pubkey.to_hex().to_lowercase()
        );

        let unwrapped = nostr_sdk::nostr::nips::nip59::extract_rumor(receiver_keys, &event)
            .await
            .context("nip59 extract_rumor")?;

        if unwrapped.rumor.kind != Kind::MlsWelcome {
            continue;
        }

        // Strong correctness check: the welcome rumor must be authored by the welcome sender.
        if unwrapped.rumor.pubkey != unwrapped.sender {
            return Err(anyhow!(
                "welcome rumor pubkey mismatch: sender={} rumor_pubkey={}",
                unwrapped.sender.to_hex(),
                unwrapped.rumor.pubkey.to_hex()
            ));
        }

        client.unsubscribe(&sub.val).await;
        return Ok((event.id, unwrapped.rumor, unwrapped.sender));
    }
}

async fn subscribe_group_msgs(
    client: &Client,
    nostr_group_id_hex: &str,
) -> anyhow::Result<SubscriptionId> {
    let mut filter = Filter::new()
        .kind(Kind::MlsGroupMessage)
        .custom_tag(SingleLetterTag::lowercase(Alphabet::H), nostr_group_id_hex)
        .limit(200);

    // Be generous about time skews; exact-token matching will disambiguate.
    filter = filter.since(Timestamp::now() - Duration::from_secs(60 * 60));

    let out = client.subscribe(filter, None).await?;
    Ok(out.val)
}

async fn ingest_group_backlog(
    mdk: &MDK<MdkSqliteStorage>,
    client: &Client,
    relay_url: RelayUrl,
    nostr_group_id_hex: &str,
) -> anyhow::Result<()> {
    let filter = Filter::new()
        .kind(Kind::MlsGroupMessage)
        .custom_tag(SingleLetterTag::lowercase(Alphabet::H), nostr_group_id_hex)
        .limit(200);

    let events = client
        .fetch_events_from([relay_url], filter, Duration::from_secs(10))
        .await
        .context("fetch group backlog")?;

    for ev in events.iter() {
        match mdk.process_message(ev) {
            Ok(res) => info!(
                "[phase1] ingest kind=445 id={} result={:?}",
                ev.id.to_hex(),
                res
            ),
            Err(err) => info!(
                "[phase1] ingest kind=445 id={} MLS_DECODE_FAIL err={}",
                ev.id.to_hex(),
                err
            ),
        }
    }

    Ok(())
}

async fn wait_for_exact_application(
    label: &str,
    mdk: &MDK<MdkSqliteStorage>,
    rx: &mut tokio::sync::broadcast::Receiver<RelayPoolNotification>,
    subscription_id: &SubscriptionId,
    expected_peer: &PublicKey,
    expected_content: &str,
    timeout: Duration,
) -> anyhow::Result<mdk_storage_traits::messages::types::Message> {
    let deadline = Instant::now() + timeout;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(anyhow!("timeout waiting for application message ({label})"));
        }
        let remaining = deadline.duration_since(now);

        let notification = tokio::time::timeout(remaining, rx.recv())
            .await
            .with_context(|| format!("recv timeout ({label})"))?
            .with_context(|| format!("notification channel closed ({label})"))?;

        let RelayPoolNotification::Event {
            subscription_id: sid,
            event,
            ..
        } = notification
        else {
            continue;
        };

        if &sid != subscription_id {
            continue;
        }

        let ev = *event;
        if ev.kind != Kind::MlsGroupMessage {
            continue;
        }

        match mdk.process_message(&ev) {
            Ok(MessageProcessingResult::ApplicationMessage(msg)) => {
                info!(
                    "[phase1] inbound kind=445 id={} decrypt=ok rumor_pubkey={} rumor_kind={} content_len={}",
                    ev.id.to_hex(),
                    msg.pubkey.to_hex().to_lowercase(),
                    msg.kind.as_u16(),
                    msg.content.len()
                );

                if msg.pubkey != *expected_peer {
                    continue;
                }
                if msg.content != expected_content {
                    continue;
                }
                return Ok(msg);
            }
            Ok(other) => {
                info!(
                    "[phase1] inbound kind=445 id={} decrypt=ok non_app={:?}",
                    ev.id.to_hex(),
                    other
                );
            }
            Err(err) => {
                info!(
                    "[phase1] inbound kind=445 id={} decrypt=fail MLS_DECODE_FAIL err={}",
                    ev.id.to_hex(),
                    err
                );
            }
        }
    }
}

async fn check_relay_ready(relay_url: &str, timeout: Duration) -> anyhow::Result<()> {
    let relay_url = RelayUrl::parse(relay_url).context("parse relay url")?;
    let deadline = Instant::now() + timeout;
    let mut attempt: usize = 0;
    let mut last_detail = String::new();

    loop {
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timeout waiting for relay websocket to become connected (attempts={attempt}, last={last_detail})"
            ));
        }

        attempt += 1;

        // Build a fresh nostr-sdk client each attempt. In CI we can hit a transient startup race
        // where the relay process is up but not yet ready for websocket handshakes; a single early
        // connect attempt may never transition to connected within the overall timeout.
        let client = Client::new(Keys::generate());
        match client.add_relay(relay_url.clone()).await {
            Ok(_) => {}
            Err(err) => {
                last_detail = format!("add_relay: {err}");
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
        }

        client.connect().await;
        let connect_deadline = Instant::now() + Duration::from_secs(3);
        let mut connected = false;
        while Instant::now() < connect_deadline {
            if let Ok(relay) = client.relay(relay_url.clone()).await
                && relay.is_connected()
            {
                connected = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        if connected {
            client.shutdown().await;
            return Ok(());
        }

        last_detail = "not connected yet".to_string();
        client.shutdown().await;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn load_or_create_keys(identity_path: &Path) -> anyhow::Result<Keys> {
    if let Ok(raw) = std::fs::read_to_string(identity_path) {
        let f: IdentityFile = serde_json::from_str(&raw).context("parse identity json")?;
        let keys = Keys::parse(&f.secret_key_hex).context("parse secret key hex")?;
        return Ok(keys);
    }

    let keys = Keys::generate();
    let secret = keys.secret_key().to_secret_hex();
    let pubkey = keys.public_key().to_hex().to_lowercase();
    let f = IdentityFile {
        secret_key_hex: secret,
        public_key_hex: pubkey,
    };
    std::fs::write(
        identity_path,
        format!("{}\n", serde_json::to_string_pretty(&f)?),
    )
    .context("write identity json")?;
    Ok(keys)
}

fn ensure_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("mkdir {dir:?}"))?;
    Ok(())
}

fn random_token() -> String {
    let mut bytes = [0u8; 8];
    let mut rng = ::rand::rngs::OsRng;
    rng.try_fill_bytes(&mut bytes)
        .expect("os rng should be available");
    hex::encode(bytes)
}

async fn spawn_rust_bot(
    relay: &str,
    bot_state_dir: &Path,
    inviter_pubkey: &PublicKey,
    timeout: Duration,
) -> anyhow::Result<(tokio::process::Child, PublicKey)> {
    ensure_dir(bot_state_dir).context("create bot state dir")?;

    let exe = std::env::current_exe().context("resolve current exe")?;
    let mut cmd = tokio::process::Command::new(&exe);
    cmd.arg("bot")
        .arg("--relay")
        .arg(relay)
        .arg("--state-dir")
        .arg(bot_state_dir)
        .arg("--inviter-pubkey")
        .arg(inviter_pubkey.to_hex().to_lowercase())
        .arg("--timeout-sec")
        .arg(timeout.as_secs().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().with_context(|| {
        format!(
            "spawn bot: {exe:?} bot --relay {relay} --state-dir {bot_state_dir:?} --inviter-pubkey {}",
            inviter_pubkey.to_hex().to_lowercase()
        )
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("bot stdout not captured"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("bot stderr not captured"))?;

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<PublicKey>();

    // Keep draining stdout/stderr for the lifetime of the child to avoid pipe backpressure
    // and to keep logs visible for debugging.
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        let mut ready_tx = Some(ready_tx);
        while let Ok(Some(line)) = lines.next_line().await {
            info!("[phase2] bot_stdout={line}");
            if ready_tx.is_some()
                && line.starts_with("[openclaw_bot] ready ")
                && let Some(hex) = line
                    .split("pubkey=")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                && let Ok(pk) = PublicKey::from_hex(hex)
            {
                let _ = ready_tx.take().unwrap().send(pk);
            }
        }
    });

    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("[rust_bot stderr] {line}");
        }
    });

    let deadline = Instant::now() + timeout;
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .unwrap_or(Duration::from_secs(0));

    tokio::select! {
        status = child.wait() => {
            let status = status.context("wait bot")?;
            Err(anyhow!("bot exited before ready: status={status}"))
        }
        ready = tokio::time::timeout(remaining, ready_rx) => {
            let pk = ready
                .context("timeout waiting for bot ready line")?
                .context("bot ready channel dropped")?;
            Ok((child, pk))
        }
    }
}
