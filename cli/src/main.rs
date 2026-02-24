mod fly_machines;
mod harness;
mod mdk_util;
mod microvm_spawner;
mod relay_util;

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow};
use clap::{Args, Parser, Subcommand, ValueEnum};
use mdk_core::encrypted_media::types::{MediaProcessingOptions, MediaReference};
use mdk_core::prelude::*;
use nostr_blossom::client::BlossomClient;
use nostr_sdk::prelude::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::AsyncBufReadExt;

// Same defaults as the Pika app (rust/src/core/config.rs).
const DEFAULT_RELAY_URLS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://relay.primal.net",
    "wss://nos.lol",
];

// Key packages (kind 443) are NIP-70 "protected" — many popular relays reject them.
// These relays are known to accept protected kind 443 publishes.
const DEFAULT_KP_RELAY_URLS: &[&str] = &[
    "wss://nostr-pub.wellorder.net",
    "wss://nostr-01.yakihonne.com",
    "wss://nostr-02.yakihonne.com",
];

fn default_state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_STATE_HOME") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir).join("pikachat");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("pikachat");
        }
    }
    PathBuf::from(".pikachat")
}

const PI_BRIDGE_PY: &str = include_str!("../../bots/pi-bridge.py");
const PI_BRIDGE_SH: &str = include_str!("../../bots/pi-bridge.sh");

#[derive(Debug, Parser)]
#[command(name = "pikachat")]
#[command(version, propagate_version = true)]
#[command(about = "Pikachat — encrypted messaging over Nostr + MLS")]
#[command(after_help = "\x1b[1mQuickstart:\x1b[0m
  1. pikachat init
  2. pikachat update-profile --name \"Alice\"
  3. pikachat send --to npub1... --content \"hello!\"
  4. pikachat listen")]
struct Cli {
    /// State directory (identity + MLS database persist here between runs)
    #[arg(long, global = true, default_value_os_t = default_state_dir())]
    state_dir: PathBuf,

    /// Relay websocket URLs (default: relay.damus.io, relay.primal.net, nos.lol)
    #[arg(long, global = true)]
    relay: Vec<String>,

    /// Key-package relay URLs (default: wellorder.net, yakihonne x2)
    #[arg(long, global = true)]
    kp_relay: Vec<String>,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Interop lab scenarios (ported from the legacy daemon harness)
    Scenario {
        #[command(subcommand)]
        scenario: harness::ScenarioCommand,
    },

    /// Deterministic bot process that behaves like an OpenClaw-side fixture, but implemented in Rust
    Bot {
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

    /// Initialize your identity and publish a key package so peers can invite you
    #[command(after_help = "Examples:
  pikachat init
  pikachat init --nsec nsec1abc...
  pikachat init --nsec <64-char-hex>")]
    Init {
        /// Nostr secret key to import (nsec1... or hex). Omit to generate a fresh keypair.
        #[arg(long)]
        nsec: Option<String>,
    },

    /// Show (or create) identity for this state dir
    #[command(after_help = "Example:
  pikachat identity")]
    Identity,

    /// Publish a key package (kind 443) so peers can invite you
    #[command(after_help = "Example:
  pikachat publish-kp

Note: 'pikachat init' publishes a key package automatically.
You only need this command to refresh an expired key package.")]
    PublishKp,

    /// Create a group with a peer and send them a welcome
    #[command(after_help = "Examples:
  pikachat invite --peer npub1xyz...
  pikachat invite --peer <hex-pubkey> --name \"Book Club\"

Tip: 'pikachat send --to npub1...' does this automatically for 1:1 DMs.")]
    Invite {
        /// Peer public key (hex or npub)
        #[arg(long)]
        peer: String,

        /// Group name
        #[arg(long, default_value = "DM")]
        name: String,
    },

    /// List pending welcome invitations
    #[command(after_help = "Example:
  pikachat welcomes")]
    Welcomes,

    /// Accept a pending welcome and join the group
    #[command(after_help = "Example:
  pikachat welcomes   # find the wrapper_event_id
  pikachat accept-welcome --wrapper-event-id abc123...")]
    AcceptWelcome {
        /// Wrapper event ID (hex) from the welcomes list
        #[arg(long)]
        wrapper_event_id: String,
    },

    /// List groups you are a member of
    #[command(after_help = "Example:
  pikachat groups")]
    Groups,

    /// Send a message (with optional media) to a group or a peer
    #[command(after_help = "Examples:
  pikachat send --to npub1xyz... --content \"hey!\"
  pikachat send --group <hex-group-id> --content \"hello\"
  pikachat send --to npub1xyz... --media photo.jpg
  pikachat send --group <hex-group-id> --media doc.pdf --mime-type application/pdf
  pikachat send --group <hex-group-id> --media pic.png --content \"check this out\"

When using --to, pikachat searches your groups for an existing 1:1 DM.
If none exists, it automatically creates one and sends your message.

When --media is provided, the file is encrypted and uploaded to a Blossom
server, and --content becomes the caption (optional).")]
    Send {
        /// Nostr group ID (hex) — send directly to this group
        #[arg(long, conflicts_with = "to")]
        group: Option<String>,

        /// Peer public key (npub or hex) — find or create a 1:1 DM with this peer
        #[arg(long, conflicts_with = "group")]
        to: Option<String>,

        /// Message content (or caption when --media is used)
        #[arg(long, default_value = "")]
        content: String,

        /// Local file to encrypt, upload, and attach
        #[arg(long)]
        media: Option<PathBuf>,

        /// MIME type for --media (defaults to application/octet-stream)
        #[arg(long, requires = "media")]
        mime_type: Option<String>,

        /// Override filename stored in media metadata
        #[arg(long, requires = "media")]
        filename: Option<String>,

        /// Blossom server URL (repeatable; defaults to blossom.yakihonne.com)
        #[arg(long = "blossom", requires = "media")]
        blossom_servers: Vec<String>,
    },

    /// Download and decrypt a media attachment from a message
    #[command(after_help = "Examples:
  pikachat download-media <message-id>
  pikachat download-media <message-id> --output photo.jpg

The message ID is shown in `pikachat messages` output.
If --output is omitted, the original filename from the sender is used.")]
    DownloadMedia {
        /// Message ID (hex) containing the media attachment
        message_id: String,

        /// Output file path (defaults to the original filename)
        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// Fetch and decrypt recent messages from a group
    #[command(after_help = "Example:
  pikachat messages --group <hex-group-id>
  pikachat messages --group <hex-group-id> --limit 10")]
    Messages {
        /// Nostr group ID (hex)
        #[arg(long)]
        group: String,

        /// Max messages to return
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },

    /// View your Nostr profile (kind-0 metadata)
    #[command(after_help = "Example:
  pikachat profile")]
    Profile,

    /// Update your Nostr profile (kind-0 metadata)
    #[command(after_help = "Examples:
  pikachat update-profile --name \"Alice\"
  pikachat update-profile --picture ./avatar.jpg
  pikachat update-profile --name \"Alice\" --picture ./avatar.jpg")]
    UpdateProfile {
        /// Set display name
        #[arg(long)]
        name: Option<String>,

        /// Upload a profile picture from a local file (JPEG/PNG, max 8 MB)
        #[arg(long)]
        picture: Option<PathBuf>,
    },

    /// Listen for incoming messages (runs until interrupted or --timeout)
    #[command(after_help = "Examples:
  pikachat listen                    # listen for 60 seconds
  pikachat listen --timeout 0        # listen forever (ctrl-c to stop)
  pikachat listen --timeout 300      # listen for 5 minutes")]
    Listen {
        /// Timeout in seconds (0 = run forever)
        #[arg(long, default_value_t = 60)]
        timeout: u64,

        /// Giftwrap lookback in seconds
        #[arg(long, default_value_t = 86400)]
        lookback: u64,
    },

    /// Long-running JSONL sidecar daemon intended to be embedded/invoked by OpenClaw
    Daemon {
        /// Giftwrap lookback window (NIP-59 backdates timestamps; use hours/days, not seconds)
        #[arg(long, default_value_t = 60 * 60 * 24 * 3)]
        giftwrap_lookback_sec: u64,

        /// Only accept welcomes and messages from these pubkeys (hex). Repeatable.
        /// If empty, all pubkeys are allowed (open mode).
        #[arg(long)]
        allow_pubkey: Vec<String>,

        /// Automatically accept incoming MLS welcomes (group invitations).
        #[arg(long, default_value_t = false)]
        auto_accept_welcomes: bool,

        /// Spawn a child process and bridge its stdio to the pikachat JSONL protocol.
        /// pikachat OutMsg lines are written to the child's stdin; the child's stdout
        /// lines are parsed as pikachat InCmd and executed. This turns pikachat into a
        /// self-contained bot runtime.
        #[arg(long)]
        exec: Option<String>,
    },

    /// Manage AI agents
    Agent {
        #[command(subcommand)]
        cmd: AgentCommand,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AgentProviderArg {
    Fly,
    Microvm,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SpawnVariantArg {
    Prebuilt,
    #[value(name = "prebuilt-cow")]
    PrebuiltCow,
    Legacy,
}

impl SpawnVariantArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Prebuilt => "prebuilt",
            Self::PrebuiltCow => "prebuilt-cow",
            Self::Legacy => "legacy",
        }
    }
}

#[derive(Debug, Args)]
struct AgentNewArgs {
    /// Runtime provider
    #[arg(long, value_enum, default_value_t = AgentProviderArg::Fly)]
    provider: AgentProviderArg,

    /// Machine name hint (Fly only; default: agent-<random>)
    #[arg(long)]
    name: Option<String>,

    /// vm-spawner base URL (microvm only)
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    spawner_url: String,

    /// vm-spawner variant (microvm only)
    #[arg(long, value_enum, default_value_t = SpawnVariantArg::PrebuiltCow)]
    spawn_variant: SpawnVariantArg,

    /// Flake ref used in the guest (microvm only)
    #[arg(long, default_value = "github:sledtools/pika")]
    flake_ref: String,

    /// Dev shell used in the guest (microvm only)
    #[arg(long, default_value = "default")]
    dev_shell: String,

    /// vCPU count (microvm only)
    #[arg(long, default_value_t = 1)]
    cpu: u32,

    /// Memory in MiB (microvm only)
    #[arg(long, default_value_t = 1024)]
    memory_mb: u32,

    /// VM TTL in seconds (microvm only)
    #[arg(long, default_value_t = 7200)]
    ttl_seconds: u64,

    /// Keep VM running on exit (microvm only)
    #[arg(long, default_value_t = false)]
    keep: bool,
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// Create a new pi agent and start chatting
    New(AgentNewArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Both `ring` and `aws-lc-rs` are in the dep tree (nostr-sdk uses ring,
    // quinn/moq-native uses aws-lc-rs). Rustls cannot auto-select when both
    // are present, so we explicitly install ring as the default provider.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("install rustls CryptoProvider");

    let cli = Cli::parse();

    let default_filter = match &cli.cmd {
        Command::Daemon { .. }
        | Command::Scenario { .. }
        | Command::Bot { .. }
        | Command::Agent { .. } => "info",
        _ => "warn",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter)),
        )
        .with_target(false)
        .without_time()
        .with_writer(std::io::stderr)
        .init();
    std::fs::create_dir_all(&cli.state_dir)
        .with_context(|| format!("create state dir {}", cli.state_dir.display()))?;

    match &cli.cmd {
        Command::Scenario { scenario } => harness::cmd_scenario(&cli, scenario).await,
        Command::Bot {
            inviter_pubkey,
            timeout_sec,
            giftwrap_lookback_sec,
        } => {
            harness::cmd_bot(
                &cli,
                inviter_pubkey.as_deref(),
                *timeout_sec,
                *giftwrap_lookback_sec,
            )
            .await
        }
        Command::Init { nsec } => cmd_init(&cli, nsec.as_deref()).await,
        Command::Identity => cmd_identity(&cli),
        Command::PublishKp => cmd_publish_kp(&cli).await,
        Command::Invite { peer, name } => cmd_invite(&cli, peer, name).await,
        Command::Welcomes => cmd_welcomes(&cli),
        Command::AcceptWelcome { wrapper_event_id } => cmd_accept_welcome(&cli, wrapper_event_id),
        Command::Groups => cmd_groups(&cli),
        Command::Send {
            group,
            to,
            content,
            media,
            mime_type,
            filename,
            blossom_servers,
        } => {
            cmd_send(
                &cli,
                group.as_deref(),
                to.as_deref(),
                content,
                media.as_deref(),
                mime_type.as_deref(),
                filename.as_deref(),
                blossom_servers,
            )
            .await
        }
        Command::DownloadMedia { message_id, output } => {
            cmd_download_media(&cli, message_id, output.as_deref()).await
        }
        Command::Messages { group, limit } => cmd_messages(&cli, group, *limit),
        Command::Profile => cmd_profile(&cli).await,
        Command::UpdateProfile { name, picture } => {
            cmd_update_profile(&cli, name.as_deref(), picture.as_deref()).await
        }
        Command::Listen { timeout, lookback } => cmd_listen(&cli, *timeout, *lookback).await,
        Command::Daemon {
            giftwrap_lookback_sec,
            allow_pubkey,
            auto_accept_welcomes,
            exec,
        } => {
            cmd_daemon(
                &cli,
                *giftwrap_lookback_sec,
                allow_pubkey,
                *auto_accept_welcomes,
                exec.as_deref(),
            )
            .await
        }
        Command::Agent { cmd } => match cmd {
            AgentCommand::New(args) => cmd_agent_new(&cli, args).await,
        },
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn open(cli: &Cli) -> anyhow::Result<(Keys, mdk_util::PikaMdk)> {
    let keys = mdk_util::load_or_create_keys(&cli.state_dir.join("identity.json"))?;
    let mdk = mdk_util::open_mdk(&cli.state_dir)?;
    Ok((keys, mdk))
}

/// Resolve message relay URLs: use --relay if provided, otherwise defaults.
fn resolve_relays(cli: &Cli) -> Vec<String> {
    if cli.relay.is_empty() {
        DEFAULT_RELAY_URLS.iter().map(|s| s.to_string()).collect()
    } else {
        cli.relay.clone()
    }
}

/// Resolve key-package relay URLs: use --kp-relay if provided, otherwise defaults.
fn resolve_kp_relays(cli: &Cli) -> Vec<String> {
    if cli.kp_relay.is_empty() {
        DEFAULT_KP_RELAY_URLS
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        cli.kp_relay.clone()
    }
}

/// Union of message + key-package relays (deduped).
fn resolve_all_relays(cli: &Cli) -> Vec<String> {
    let mut all = resolve_relays(cli);
    for kp in resolve_kp_relays(cli) {
        if !all.contains(&kp) {
            all.push(kp);
        }
    }
    all
}

async fn client(cli: &Cli, keys: &Keys) -> anyhow::Result<Client> {
    let relays = resolve_relays(cli);
    relay_util::connect_client(keys, &relays).await
}

/// Connect to both message and key-package relays.
async fn client_all(cli: &Cli, keys: &Keys) -> anyhow::Result<Client> {
    let relays = resolve_all_relays(cli);
    relay_util::connect_client(keys, &relays).await
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
        .ok_or_else(|| {
            anyhow!(
                "no group with ID {nostr_group_id_hex}. Run 'pikachat groups' to list your groups."
            )
        })
}

fn print(v: serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(&v).expect("json encode"));
}

/// Fetch recent group messages from the relay and feed them through
/// `mdk.process_message` so the local MLS epoch is up-to-date before we
/// attempt to create a new message.
async fn ingest_group_backlog(
    mdk: &mdk_util::PikaMdk,
    client: &Client,
    relay_urls: &[RelayUrl],
    nostr_group_id_hex: &str,
) -> anyhow::Result<()> {
    let filter = Filter::new()
        .kind(Kind::MlsGroupMessage)
        .custom_tag(SingleLetterTag::lowercase(Alphabet::H), nostr_group_id_hex)
        .limit(200);

    let events = client
        .fetch_events_from(relay_urls.to_vec(), filter, Duration::from_secs(10))
        .await
        .context("fetch group backlog")?;

    for ev in events.iter() {
        // Errors are expected (own messages bouncing back, already-processed
        // events, etc.) — the important thing is that commits get applied.
        let _ = mdk.process_message(ev);
    }

    Ok(())
}

const MAX_CHAT_MEDIA_BYTES: usize = 32 * 1024 * 1024;

fn is_imeta_tag(tag: &Tag) -> bool {
    matches!(tag.kind(), TagKind::Custom(kind) if kind.as_ref() == "imeta")
}

fn mime_from_extension(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "heic" => Some("image/heic"),
        "svg" => Some("image/svg+xml"),
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "webm" => Some("video/webm"),
        "mp3" => Some("audio/mpeg"),
        "ogg" => Some("audio/ogg"),
        "wav" => Some("audio/wav"),
        "pdf" => Some("application/pdf"),
        "txt" | "md" => Some("text/plain"),
        _ => None,
    }
}

fn blossom_servers_or_default(values: &[String]) -> Vec<String> {
    let parsed: Vec<String> = values
        .iter()
        .filter_map(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return None;
            }
            Url::parse(trimmed).ok().map(|_| trimmed.to_string())
        })
        .collect();
    if !parsed.is_empty() {
        return parsed;
    }
    vec![DEFAULT_BLOSSOM_SERVER.to_string()]
}

fn message_media_refs(
    mdk: &mdk_util::PikaMdk,
    group_id: &GroupId,
    tags: &Tags,
) -> Vec<serde_json::Value> {
    let manager = mdk.media_manager(group_id.clone());
    tags.iter()
        .filter(|tag| is_imeta_tag(tag))
        .filter_map(|tag| manager.parse_imeta_tag(tag).ok())
        .map(media_ref_to_json)
        .collect()
}

fn media_ref_to_json(reference: MediaReference) -> serde_json::Value {
    let (width, height) = reference
        .dimensions
        .map(|(w, h)| (Some(w), Some(h)))
        .unwrap_or((None, None));
    json!({
        "original_hash_hex": hex::encode(reference.original_hash),
        "url": reference.url,
        "mime_type": reference.mime_type,
        "filename": reference.filename,
        "width": width,
        "height": height,
        "nonce_hex": hex::encode(reference.nonce),
        "scheme_version": reference.scheme_version,
    })
}

// ── Commands ────────────────────────────────────────────────────────────────

async fn cmd_init(cli: &Cli, nsec: Option<&str>) -> anyhow::Result<()> {
    let identity_path = cli.state_dir.join("identity.json");
    let db_path = cli.state_dir.join("mdk.sqlite");

    // Resolve or generate keys.
    let keys = match nsec {
        Some(s) => Keys::parse(s.trim())
            .context("invalid nsec — expected nsec1... (bech32) or 64-char hex secret key")?,
        None => Keys::generate(),
    };

    let new_pubkey = keys.public_key().to_hex();

    // Check for conflicts with existing state.
    let mut warnings: Vec<String> = Vec::new();

    if identity_path.exists() {
        let raw = std::fs::read_to_string(&identity_path).context("read existing identity.json")?;
        let existing: mdk_util::IdentityFile =
            serde_json::from_str(&raw).context("parse existing identity.json")?;

        if existing.public_key_hex == new_pubkey {
            eprintln!("[pikachat] identity.json already matches this pubkey — no changes needed.");
            // Still publish key package (idempotent).
        } else {
            warnings.push(format!(
                "identity.json exists with a DIFFERENT pubkey (existing={}, new={})",
                existing.public_key_hex, new_pubkey,
            ));
        }
    }

    if db_path.exists() {
        warnings.push(format!(
            "mdk.sqlite exists at {}; it may contain MLS state from a previous identity. \
             Consider removing it if you are switching keys.",
            db_path.display(),
        ));
    }

    // Prompt for confirmation if there are warnings.
    if !warnings.is_empty() {
        for w in &warnings {
            eprintln!("[pikachat] WARNING: {w}");
        }
        eprint!("[pikachat] Continue anyway? (yes/abort): ");
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .context("read user input")?;
        if answer.trim().to_lowercase() != "yes" {
            anyhow::bail!("aborted by user");
        }
    }

    // Write identity.json.
    let id_file = mdk_util::IdentityFile {
        secret_key_hex: keys.secret_key().to_secret_hex(),
        public_key_hex: new_pubkey.clone(),
    };
    std::fs::write(
        &identity_path,
        format!("{}\n", serde_json::to_string_pretty(&id_file)?),
    )
    .context("write identity.json")?;

    // Publish a key package so the user is immediately invitable.
    let mdk = mdk_util::open_mdk(&cli.state_dir)?;
    let kp_relays_str = resolve_kp_relays(cli);
    let kp_relays = relay_util::parse_relay_urls(&kp_relays_str)?;
    let client = relay_util::connect_client(&keys, &kp_relays_str).await?;

    let (content, tags, _hash_ref) = mdk
        .create_key_package_for_event(&keys.public_key(), kp_relays.clone())
        .context("create key package")?;

    let tags: Tags = tags
        .into_iter()
        .filter(|t: &Tag| !matches!(t.kind(), TagKind::Protected))
        .collect();

    let event = EventBuilder::new(Kind::MlsKeyPackage, content)
        .tags(tags)
        .sign_with_keys(&keys)
        .context("sign key package event")?;

    relay_util::publish_and_confirm(&client, &kp_relays, &event, "keypackage").await?;
    client.shutdown().await;

    print(json!({
        "pubkey": keys.public_key().to_hex(),
        "npub": keys.public_key().to_bech32().unwrap_or_default(),
        "key_package_event_id": event.id.to_hex(),
    }));
    Ok(())
}

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
    let kp_relays_str = resolve_kp_relays(cli);
    let client = relay_util::connect_client(&keys, &kp_relays_str).await?;
    let relays = relay_util::parse_relay_urls(&kp_relays_str)?;

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
    let client = client_all(cli, &keys).await?;
    let relays = relay_util::parse_relay_urls(&resolve_relays(cli))?;
    let kp_relays = relay_util::parse_relay_urls(&resolve_kp_relays(cli))?;

    let peer_pubkey =
        PublicKey::parse(peer_str.trim()).with_context(|| format!("parse peer key: {peer_str}"))?;

    // Fetch peer key package from key-package relays.
    let peer_kp = relay_util::fetch_latest_key_package(
        &client,
        &peer_pubkey,
        &kp_relays,
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

/// Encrypt and upload a media file to Blossom, returning the imeta tag.
async fn upload_media(
    keys: &Keys,
    mdk: &mdk_util::PikaMdk,
    mls_group_id: &GroupId,
    file: &Path,
    mime_type: Option<&str>,
    filename: Option<&str>,
    blossom_servers: &[String],
) -> anyhow::Result<(Tag, serde_json::Value)> {
    let bytes =
        std::fs::read(file).with_context(|| format!("read media file {}", file.display()))?;
    if bytes.is_empty() {
        anyhow::bail!("media file is empty");
    }
    if bytes.len() > MAX_CHAT_MEDIA_BYTES {
        anyhow::bail!("media too large (max 32 MB)");
    }

    let resolved_filename = filename
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            file.file_name()
                .and_then(|f| f.to_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "file.bin".to_string());
    let resolved_mime = mime_type
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| mime_from_extension(file))
        .unwrap_or("application/octet-stream")
        .to_string();

    let manager = mdk.media_manager(mls_group_id.clone());
    let mut upload = manager
        .encrypt_for_upload_with_options(
            &bytes,
            &resolved_mime,
            &resolved_filename,
            &MediaProcessingOptions::default(),
        )
        .context("encrypt media for upload")?;
    let encrypted_data = std::mem::take(&mut upload.encrypted_data);
    let expected_hash_hex = hex::encode(upload.encrypted_hash);

    let upload_servers = blossom_servers_or_default(blossom_servers);

    let mut uploaded_url: Option<String> = None;
    let mut used_server: Option<String> = None;
    let mut descriptor_sha256_hex: Option<String> = None;
    let mut last_error: Option<String> = None;
    for server in &upload_servers {
        let base_url = match Url::parse(server) {
            Ok(url) => url,
            Err(e) => {
                last_error = Some(format!("{server}: {e}"));
                continue;
            }
        };
        let blossom = BlossomClient::new(base_url);
        let descriptor = match blossom
            .upload_blob(
                encrypted_data.clone(),
                Some(upload.mime_type.clone()),
                None,
                Some(keys),
            )
            .await
        {
            Ok(descriptor) => descriptor,
            Err(e) => {
                last_error = Some(format!("{server}: {e}"));
                continue;
            }
        };

        let descriptor_hash_hex = descriptor.sha256.to_string();
        if !descriptor_hash_hex.eq_ignore_ascii_case(&expected_hash_hex) {
            last_error = Some(format!(
                "{server}: uploaded hash mismatch (expected {expected_hash_hex}, got {descriptor_hash_hex})"
            ));
            continue;
        }

        uploaded_url = Some(descriptor.url.to_string());
        used_server = Some(server.clone());
        descriptor_sha256_hex = Some(descriptor_hash_hex);
        break;
    }

    let Some(uploaded_url) = uploaded_url else {
        anyhow::bail!(
            "blossom upload failed: {}",
            last_error.unwrap_or_else(|| "unknown error".to_string())
        );
    };

    let imeta_tag = manager.create_imeta_tag(&upload, &uploaded_url);
    let media_json = json!({
        "blossom_server": used_server,
        "uploaded_url": uploaded_url,
        "original_hash_hex": hex::encode(upload.original_hash),
        "encrypted_hash_hex": expected_hash_hex,
        "descriptor_sha256_hex": descriptor_sha256_hex,
        "mime_type": upload.mime_type,
        "filename": upload.filename,
        "bytes": bytes.len(),
    });
    Ok((imeta_tag, media_json))
}

#[allow(clippy::too_many_arguments)]
async fn cmd_send(
    cli: &Cli,
    group_hex: Option<&str>,
    to_str: Option<&str>,
    content: &str,
    media: Option<&Path>,
    mime_type: Option<&str>,
    filename: Option<&str>,
    blossom_servers: &[String],
) -> anyhow::Result<()> {
    if group_hex.is_none() && to_str.is_none() {
        anyhow::bail!(
            "either --group or --to is required.\n\
             Use --group <HEX> to send to a known group, or --to <NPUB> to send to a peer."
        );
    }
    if media.is_none() && content.is_empty() {
        anyhow::bail!("--content is required (or use --media to send a file)");
    }

    let (keys, mdk) = open(cli)?;

    // ── Resolve target group ────────────────────────────────────────────
    struct ResolvedTarget {
        group: mdk_storage_traits::groups::types::Group,
        auto_created: bool,
    }

    let relays = relay_util::parse_relay_urls(&resolve_relays(cli))?;

    let (resolved, client) = match (group_hex, to_str) {
        (Some(gid), _) => {
            let group = find_group(&mdk, gid)?;
            let c = client(cli, &keys).await?;
            (
                ResolvedTarget {
                    group,
                    auto_created: false,
                },
                c,
            )
        }
        (_, Some(peer_str)) => {
            let peer_pubkey = PublicKey::parse(peer_str.trim())
                .with_context(|| format!("parse peer key: {peer_str}"))?;
            let my_pubkey = keys.public_key();

            // Search for an existing 1:1 DM with this peer.
            let groups = mdk.get_groups().context("get groups")?;
            let found = groups.into_iter().find(|g| {
                let members = mdk.get_members(&g.mls_group_id).unwrap_or_default();
                let others: Vec<_> = members.iter().filter(|p| *p != &my_pubkey).collect();
                others.len() == 1 && *others[0] == peer_pubkey
            });

            if let Some(group) = found {
                let c = client(cli, &keys).await?;
                (
                    ResolvedTarget {
                        group,
                        auto_created: false,
                    },
                    c,
                )
            } else {
                // Auto-create a DM group.
                let c = client_all(cli, &keys).await?;
                let kp_relays = relay_util::parse_relay_urls(&resolve_kp_relays(cli))?;

                let peer_kp = relay_util::fetch_latest_key_package(
                    &c,
                    &peer_pubkey,
                    &kp_relays,
                    Duration::from_secs(10),
                )
                .await
                .context("fetch peer key package — has the peer run `pikachat init`?")?;

                let config = NostrGroupConfigData::new(
                    "DM".to_string(),
                    String::new(),
                    None,
                    None,
                    None,
                    relays.clone(),
                    vec![my_pubkey, peer_pubkey],
                );

                let result = mdk
                    .create_group(&my_pubkey, vec![peer_kp], config)
                    .context("create group")?;

                for rumor in result.welcome_rumors {
                    let giftwrap = EventBuilder::gift_wrap(&keys, &peer_pubkey, rumor, [])
                        .await
                        .context("build giftwrap")?;
                    relay_util::publish_and_confirm(&c, &relays, &giftwrap, "welcome").await?;
                }

                (
                    ResolvedTarget {
                        group: result.group,
                        auto_created: true,
                    },
                    c,
                )
            }
        }
        _ => unreachable!(),
    };

    let ngid = hex::encode(resolved.group.nostr_group_id);

    // ── Catch up: process any pending group messages from the relay ─────
    // Without this, sending twice without running `listen` in between can
    // leave the local MLS epoch stale, producing ciphertext that peers
    // (who are on a newer epoch) cannot decrypt.
    ingest_group_backlog(&mdk, &client, &relays, &ngid).await?;

    // ── Upload media (if any) ───────────────────────────────────────────
    let mut tags: Vec<Tag> = Vec::new();
    let mut media_json: Option<serde_json::Value> = None;

    if let Some(file) = media {
        let (imeta_tag, mj) = upload_media(
            &keys,
            &mdk,
            &resolved.group.mls_group_id,
            file,
            mime_type,
            filename,
            blossom_servers,
        )
        .await?;
        tags.push(imeta_tag);
        media_json = Some(mj);
    }

    // ── Build and send MLS message ──────────────────────────────────────
    let rumor = EventBuilder::new(Kind::ChatMessage, content)
        .tags(tags)
        .build(keys.public_key());
    let msg_event = mdk
        .create_message(&resolved.group.mls_group_id, rumor)
        .context("create message")?;
    relay_util::publish_and_confirm(&client, &relays, &msg_event, "send_message").await?;
    client.shutdown().await;

    let mut out = json!({
        "event_id": msg_event.id.to_hex(),
        "nostr_group_id": ngid,
    });
    if resolved.auto_created {
        out["auto_created_group"] = json!(true);
    }
    if let Some(mj) = media_json {
        out["media"] = mj;
    }
    print(out);
    Ok(())
}

async fn cmd_download_media(
    cli: &Cli,
    message_id_hex: &str,
    output_path: Option<&Path>,
) -> anyhow::Result<()> {
    let (_keys, mdk) = open(cli)?;
    let message_id = EventId::from_hex(message_id_hex.trim()).context("parse message id")?;

    // Scan groups to find the one containing this message.
    let groups = mdk.get_groups().context("get groups")?;
    let mut found = None;
    for g in &groups {
        if let Ok(Some(msg)) = mdk.get_message(&g.mls_group_id, &message_id) {
            found = Some((g.mls_group_id.clone(), msg));
            break;
        }
    }
    let (mls_group_id, message) =
        found.ok_or_else(|| anyhow!("message {message_id_hex} not found in any group"))?;

    let manager = mdk.media_manager(mls_group_id);
    let media_ref = message
        .tags
        .iter()
        .filter(|tag| is_imeta_tag(tag))
        .filter_map(|tag| manager.parse_imeta_tag(tag).ok())
        .next()
        .ok_or_else(|| anyhow!("message has no media attachments"))?;

    let response = reqwest::Client::new()
        .get(media_ref.url.as_str())
        .send()
        .await
        .with_context(|| format!("download encrypted media from {}", media_ref.url))?;
    if !response.status().is_success() {
        anyhow::bail!("download failed: HTTP {}", response.status());
    }
    let encrypted_data = response.bytes().await.context("read media response body")?;
    let decrypted = manager
        .decrypt_from_download(&encrypted_data, &media_ref)
        .context("decrypt downloaded media")?;

    let original_hash_hex = hex::encode(media_ref.original_hash);
    let decrypted_hash_hex = hex::encode(Sha256::digest(&decrypted));
    if !decrypted_hash_hex.eq_ignore_ascii_case(&original_hash_hex) {
        anyhow::bail!(
            "decrypted hash mismatch (expected {original_hash_hex}, got {decrypted_hash_hex})"
        );
    }

    // Resolve output path: explicit --output > original filename > fallback
    let default_name = if media_ref.filename.is_empty() {
        "download.bin"
    } else {
        &media_ref.filename
    };
    let resolved_output = match output_path {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from(default_name),
    };

    if let Some(parent) = resolved_output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create output dir {}", parent.display()))?;
    }
    std::fs::write(&resolved_output, &decrypted)
        .with_context(|| format!("write decrypted media to {}", resolved_output.display()))?;

    print(json!({
        "message_id": message_id.to_hex(),
        "original_hash_hex": original_hash_hex,
        "mime_type": media_ref.mime_type,
        "filename": media_ref.filename,
        "url": media_ref.url.to_string(),
        "output_path": resolved_output,
        "bytes": decrypted.len(),
    }));
    Ok(())
}

#[derive(Debug)]
enum AgentRuntimeHandle {
    Fly {
        machine_id: String,
        app_name: String,
    },
    Microvm {
        spawner_url: String,
        vm_id: String,
        ip: String,
    },
}

#[derive(Debug, Clone)]
struct AgentSpawnConfig {
    bot_secret_hex: String,
    anthropic_key: String,
    openai_key: Option<String>,
    pi_model: Option<String>,
    relays: Vec<String>,
}

struct FlyProvider {
    client: fly_machines::FlyClient,
}

struct MicrovmProvider {
    client: microvm_spawner::MicrovmSpawnerClient,
}

enum AgentProvider {
    Fly(FlyProvider),
    Microvm(MicrovmProvider),
}

impl AgentProvider {
    async fn spawn(
        &self,
        args: &AgentNewArgs,
        spawn: &AgentSpawnConfig,
    ) -> anyhow::Result<AgentRuntimeHandle> {
        match self {
            AgentProvider::Fly(provider) => provider.spawn(args.name.as_deref(), spawn).await,
            AgentProvider::Microvm(provider) => provider.spawn(args, spawn).await,
        }
    }

    async fn teardown(&self, runtime: &AgentRuntimeHandle) -> anyhow::Result<()> {
        match (self, runtime) {
            (AgentProvider::Fly(_), AgentRuntimeHandle::Fly { .. }) => Ok(()),
            (AgentProvider::Microvm(provider), AgentRuntimeHandle::Microvm { vm_id, .. }) => {
                provider.client.delete_vm(vm_id).await
            }
            _ => Ok(()),
        }
    }

    fn keypackage_timeout(&self) -> Duration {
        match self {
            AgentProvider::Fly(_) => Duration::from_secs(120),
            AgentProvider::Microvm(_) => Duration::from_secs(180),
        }
    }

    fn keypackage_fetch_timeout(&self) -> Duration {
        match self {
            AgentProvider::Fly(_) => Duration::from_secs(5),
            AgentProvider::Microvm(_) => Duration::from_secs(2),
        }
    }

    fn keypackage_retry_interval(&self) -> Duration {
        match self {
            AgentProvider::Fly(_) => Duration::from_secs(3),
            AgentProvider::Microvm(_) => Duration::from_millis(250),
        }
    }

    fn keypackage_primary_only_window(&self) -> Option<Duration> {
        match self {
            AgentProvider::Fly(_) => None,
            AgentProvider::Microvm(_) => Some(Duration::from_secs(20)),
        }
    }
}

impl FlyProvider {
    async fn spawn(
        &self,
        name_hint: Option<&str>,
        spawn: &AgentSpawnConfig,
    ) -> anyhow::Result<AgentRuntimeHandle> {
        let suffix = format!("{:08x}", rand::random::<u32>());
        let volume_name = format!("agent_{suffix}");
        let machine_name = name_hint
            .map(std::string::ToString::to_string)
            .unwrap_or_else(|| format!("agent-{suffix}"));

        eprint!("Creating Fly volume...");
        std::io::stderr().flush().ok();
        let volume = self.client.create_volume(&volume_name).await?;
        eprintln!(" done ({})", volume.id);

        let mut env = HashMap::new();
        env.insert("STATE_DIR".to_string(), "/app/state".to_string());
        env.insert("NOSTR_SECRET_KEY".to_string(), spawn.bot_secret_hex.clone());
        env.insert("ANTHROPIC_API_KEY".to_string(), spawn.anthropic_key.clone());
        if let Some(openai) = &spawn.openai_key {
            env.insert("OPENAI_API_KEY".to_string(), openai.clone());
        }
        if let Some(model) = &spawn.pi_model {
            env.insert("PI_MODEL".to_string(), model.clone());
        }

        eprint!("Creating Fly machine...");
        std::io::stderr().flush().ok();
        let machine = self
            .client
            .create_machine(&machine_name, &volume.id, env)
            .await?;
        eprintln!(" done ({})", machine.id);

        Ok(AgentRuntimeHandle::Fly {
            machine_id: machine.id,
            app_name: self.client.app_name().to_string(),
        })
    }
}

impl MicrovmProvider {
    async fn spawn(
        &self,
        args: &AgentNewArgs,
        spawn: &AgentSpawnConfig,
    ) -> anyhow::Result<AgentRuntimeHandle> {
        let request = microvm_spawner::CreateVmRequest {
            flake_ref: Some(args.flake_ref.clone()),
            dev_shell: Some(args.dev_shell.clone()),
            cpu: Some(args.cpu),
            memory_mb: Some(args.memory_mb),
            ttl_seconds: Some(args.ttl_seconds),
            spawn_variant: Some(args.spawn_variant.as_str().to_string()),
            guest_autostart: Some(build_microvm_guest_autostart(spawn)),
        };

        eprint!("Spawning microVM...");
        std::io::stderr().flush().ok();
        let vm = self
            .client
            .create_vm(&request)
            .await
            .with_context(|| {
                format!(
                    "spawn microvm via {} (if this is a 5xx, check vm-spawner/dnsmasq on pika-build and verify SSH tunnel: just -f infra/justfile build-vmspawner-tunnel)",
                    self.client.base_url()
                )
            })?;
        eprintln!(" done ({} @ {})", vm.id, vm.ip);

        Ok(AgentRuntimeHandle::Microvm {
            spawner_url: self.client.base_url().to_string(),
            vm_id: vm.id,
            ip: vm.ip,
        })
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn build_microvm_guest_autostart(
    spawn: &AgentSpawnConfig,
) -> microvm_spawner::GuestAutostartRequest {
    let relay_flags = spawn
        .relays
        .iter()
        .map(|relay| format!("--relay {}", shell_quote(relay)))
        .collect::<Vec<_>>()
        .join(" ");

    let run_script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
if [ -f /run/agent-meta/env ]; then
  set -a
  . /run/agent-meta/env
  set +a
fi
if [ -f /run/agent-meta/autostart.env ]; then
  set -a
  . /run/agent-meta/autostart.env
  set +a
fi
STATE_DIR=/workspace/pika-agent/state
mkdir -p "$STATE_DIR"

PI_RUNTIME_DIR="${{PIKA_RUNTIME_ARTIFACTS_GUEST:-/opt/runtime-artifacts}}"
PI_RUNTIME_BIN="$PI_RUNTIME_DIR/pi/bin/pi"
if [ -x "$PI_RUNTIME_BIN" ]; then
  export PATH="$PI_RUNTIME_DIR/pi/bin:$PATH"
elif [ -z "${{PI_CMD:-}}" ]; then
  echo "[agent-autostart] missing pi runtime at $PI_RUNTIME_BIN" >&2
  exit 1
fi
if [ -z "${{PI_CMD:-}}" ]; then
  export PI_CMD="pi --mode rpc --no-session --provider anthropic"
fi

BRIDGE_CMD="bash /workspace/pika-agent/pi-bridge.sh"
if command -v python3 >/dev/null 2>&1; then
  BRIDGE_CMD="python3 /workspace/pika-agent/pi-bridge.py"
fi

if [ -z "${{PIKA_MARMOTD_BIN:-}}" ] || [ ! -x "${{PIKA_MARMOTD_BIN}}" ]; then
  echo "[agent-autostart] missing packaged marmotd at ${{PIKA_MARMOTD_BIN:-<unset>}}" >&2
  exit 1
fi

"${{PIKA_MARMOTD_BIN}}" init --nsec "${{NOSTR_SECRET_KEY}}" --state-dir "$STATE_DIR" >/dev/null
exec "${{PIKA_MARMOTD_BIN}}" daemon {} --state-dir "$STATE_DIR" --auto-accept-welcomes --exec "$BRIDGE_CMD"
    "#,
        relay_flags
    );
    let relays_json = serde_json::to_string(&spawn.relays).unwrap_or_else(|_| "[]".to_string());

    let mut env = std::collections::BTreeMap::new();
    env.insert("NOSTR_SECRET_KEY".to_string(), spawn.bot_secret_hex.clone());
    env.insert("ANTHROPIC_API_KEY".to_string(), spawn.anthropic_key.clone());
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/workspace/pika-agent/target".to_string(),
    );
    env.insert("PI_RELAYS_JSON".to_string(), relays_json);
    env.insert(
        "MARMOTD_SKIP_RELAY_READY_CHECK".to_string(),
        "1".to_string(),
    );
    env.insert("PI_BRIDGE_ENABLE_CHAT".to_string(), "1".to_string());
    if let Some(openai) = &spawn.openai_key {
        env.insert("OPENAI_API_KEY".to_string(), openai.clone());
    }
    if let Some(model) = &spawn.pi_model {
        env.insert("PI_MODEL".to_string(), model.clone());
    }

    let mut files = std::collections::BTreeMap::new();
    files.insert(
        "workspace/pika-agent/pi-bridge.py".to_string(),
        PI_BRIDGE_PY.to_string(),
    );
    files.insert(
        "workspace/pika-agent/pi-bridge.sh".to_string(),
        PI_BRIDGE_SH.to_string(),
    );
    files.insert("workspace/pika-agent/run-agent.sh".to_string(), run_script);

    microvm_spawner::GuestAutostartRequest {
        command: "bash /workspace/pika-agent/run-agent.sh".to_string(),
        env,
        files,
    }
}

async fn cmd_agent_new(cli: &Cli, args: &AgentNewArgs) -> anyhow::Result<()> {
    let anthropic_key =
        std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY must be set")?;
    let openai_key = std::env::var("OPENAI_API_KEY").ok();
    let pi_model = std::env::var("PI_MODEL")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let relays = resolve_relays(cli);

    let provider = match args.provider {
        AgentProviderArg::Fly => AgentProvider::Fly(FlyProvider {
            client: fly_machines::FlyClient::from_env()?,
        }),
        AgentProviderArg::Microvm => AgentProvider::Microvm(MicrovmProvider {
            client: microvm_spawner::MicrovmSpawnerClient::new(args.spawner_url.clone()),
        }),
    };

    if matches!(args.provider, AgentProviderArg::Fly) && args.keep {
        eprintln!("--keep has no effect for --provider fly.");
    }

    let (keys, mdk) = open(cli)?;
    eprintln!("Your pubkey: {}", keys.public_key().to_hex());

    let bot_keys = if matches!(args.provider, AgentProviderArg::Microvm)
        && microvm_reuse_bot_identity_enabled()
    {
        load_or_create_microvm_bot_keys(&cli.state_dir)?
    } else {
        Keys::generate()
    };
    let bot_pubkey = bot_keys.public_key();
    let bot_secret_hex = bot_keys.secret_key().to_secret_hex();
    eprintln!("Bot pubkey: {}", bot_pubkey.to_hex());

    let spawn = AgentSpawnConfig {
        bot_secret_hex,
        anthropic_key,
        openai_key,
        pi_model,
        relays: relays.clone(),
    };

    let runtime = provider.spawn(args, &spawn).await?;
    let should_teardown = matches!(runtime, AgentRuntimeHandle::Microvm { .. }) && !args.keep;
    let keypackage_timeout = provider.keypackage_timeout();
    let keypackage_fetch_timeout = provider.keypackage_fetch_timeout();
    let keypackage_retry_interval = provider.keypackage_retry_interval();
    let keypackage_primary_only_window = provider.keypackage_primary_only_window();

    let session_result = async {
        let client = client_all(cli, &keys).await?;
        let relays = relay_util::parse_relay_urls(&relays)?;
        let mut primary_relay_only = Vec::new();
        if let Some(primary) = relays.first() {
            primary_relay_only.push(primary.clone());
        }

        eprint!("Waiting for bot to publish key package");
        std::io::stderr().flush().ok();
        let start = tokio::time::Instant::now();
        let bot_kp = loop {
            let fetch_relays: &[RelayUrl] = if let Some(window) = keypackage_primary_only_window {
                if start.elapsed() < window && !primary_relay_only.is_empty() {
                    &primary_relay_only
                } else {
                    &relays
                }
            } else {
                &relays
            };
            match relay_util::fetch_latest_key_package(
                &client,
                &bot_pubkey,
                fetch_relays,
                keypackage_fetch_timeout,
            )
            .await
            {
                Ok(kp) => break kp,
                Err(err) => {
                    if start.elapsed() >= keypackage_timeout {
                        client.shutdown().await;
                        anyhow::bail!(
                            "timed out waiting for bot key package after {}s: {err}",
                            keypackage_timeout.as_secs()
                        );
                    }
                    eprint!(".");
                    std::io::stderr().flush().ok();
                    tokio::time::sleep(keypackage_retry_interval).await;
                }
            }
        };
        eprintln!(" done");

        eprint!("Creating MLS group and inviting bot...");
        std::io::stderr().flush().ok();
        let config = NostrGroupConfigData::new(
            "Agent Chat".to_string(),
            String::new(),
            None,
            None,
            None,
            relays.clone(),
            vec![keys.public_key(), bot_pubkey],
        );
        let result = mdk
            .create_group(&keys.public_key(), vec![bot_kp], config)
            .context("create group for bot")?;
        let mls_group_id = result.group.mls_group_id.clone();
        let nostr_group_id_hex = hex::encode(result.group.nostr_group_id);

        for rumor in result.welcome_rumors {
            let giftwrap = EventBuilder::gift_wrap(&keys, &bot_pubkey, rumor, [])
                .await
                .context("build welcome giftwrap")?;
            relay_util::publish_and_confirm(&client, &relays, &giftwrap, "welcome").await?;
        }
        eprintln!(" done");

        let group_filter = Filter::new()
            .kind(Kind::MlsGroupMessage)
            .custom_tag(SingleLetterTag::lowercase(Alphabet::H), &nostr_group_id_hex)
            .since(Timestamp::now());
        let sub = client.subscribe(group_filter, None).await?;
        let mut rx = client.notifications();

        let bot_npub = bot_pubkey
            .to_bech32()
            .unwrap_or_else(|_| bot_pubkey.to_hex().to_string());
        eprintln!();
        eprintln!("Connected to pi agent ({bot_npub})");
        eprintln!("Type messages below. Ctrl-C to exit.");
        eprintln!();

        let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();
        eprint!("you> ");
        std::io::stderr().flush().ok();

        loop {
            tokio::select! {
                line = stdin.next_line() => {
                    let Some(line) = line? else { break };
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        eprint!("you> ");
                        std::io::stderr().flush().ok();
                        continue;
                    }

                    let rumor = EventBuilder::new(Kind::ChatMessage, &line).build(keys.public_key());
                    let msg_event = mdk.create_message(&mls_group_id, rumor).context("create chat message")?;
                    relay_util::publish_and_confirm(&client, &relays, &msg_event, "chat").await?;
                    eprint!("you> ");
                    std::io::stderr().flush().ok();
                }
                notification = rx.recv() => {
                    let notification = match notification {
                        Ok(n) => n,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    };
                    let RelayPoolNotification::Event { subscription_id, event, .. } = notification else { continue };
                    if subscription_id != sub.val {
                        continue;
                    }
                    let event = *event;
                    if event.kind != Kind::MlsGroupMessage {
                        continue;
                    }
                    if let Ok(MessageProcessingResult::ApplicationMessage(msg)) =
                        mdk.process_message(&event)
                        && msg.pubkey == bot_pubkey
                    {
                        eprint!("\r");
                        println!("pi> {}", msg.content);
                        println!();
                        eprint!("you> ");
                        std::io::stderr().flush().ok();
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    break;
                }
            }
        }

        client.unsubscribe_all().await;
        client.shutdown().await;
        Ok::<(), anyhow::Error>(())
    }
    .await;

    let mut teardown_error: Option<anyhow::Error> = None;
    if should_teardown {
        eprint!("Deleting microVM...");
        std::io::stderr().flush().ok();
        match provider.teardown(&runtime).await {
            Ok(()) => eprintln!(" done"),
            Err(err) => {
                eprintln!(" failed");
                eprintln!("Teardown error: {err}");
                teardown_error = Some(err);
            }
        }
    }

    eprintln!();
    match &runtime {
        AgentRuntimeHandle::Fly {
            machine_id,
            app_name,
        } => {
            eprintln!("Machine {machine_id} is still running.");
            eprintln!("Stop with: fly machine stop {machine_id} -a {app_name}");
        }
        AgentRuntimeHandle::Microvm {
            spawner_url,
            vm_id,
            ip,
        } => {
            if args.keep {
                eprintln!("microVM {vm_id} is still running at {ip}.");
                eprintln!("Inspect: curl {spawner_url}/vms/{vm_id}");
                eprintln!("Delete:  curl -X DELETE {spawner_url}/vms/{vm_id}");
            } else {
                eprintln!("microVM {vm_id} has been deleted.");
            }
        }
    }

    session_result?;
    if let Some(err) = teardown_error {
        return Err(err);
    }
    Ok(())
}

fn load_or_create_microvm_bot_keys(state_dir: &Path) -> anyhow::Result<Keys> {
    let path = state_dir.join("agent-microvm-bot-secret.hex");
    if let Ok(raw) = fs::read_to_string(&path) {
        let secret = raw.trim();
        if !secret.is_empty() {
            return Keys::parse(secret)
                .with_context(|| format!("parse bot key from {}", path.display()));
        }
    }

    fs::create_dir_all(state_dir)
        .with_context(|| format!("create state dir {}", state_dir.display()))?;
    let keys = Keys::generate();
    fs::write(&path, format!("{}\n", keys.secret_key().to_secret_hex()))
        .with_context(|| format!("write bot key to {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path)
            .with_context(|| format!("stat bot key {}", path.display()))?
            .permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&path, perms)
            .with_context(|| format!("chmod 600 {}", path.display()))?;
    }
    Ok(keys)
}

fn microvm_reuse_bot_identity_enabled() -> bool {
    match std::env::var("PIKA_MICROVM_REUSE_BOT_IDENTITY") {
        Ok(raw) => matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
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
                "media": message_media_refs(&mdk, &group.mls_group_id, &m.tags),
            })
        })
        .collect();
    print(json!({ "messages": out }));
    Ok(())
}

const DEFAULT_BLOSSOM_SERVER: &str = "https://us-east.nostr.pikachat.org";
const MAX_PROFILE_IMAGE_BYTES: usize = 8 * 1024 * 1024;

async fn cmd_profile(cli: &Cli) -> anyhow::Result<()> {
    let (keys, _mdk) = open(cli)?;
    let client = client(cli, &keys).await?;

    client.wait_for_connection(Duration::from_secs(4)).await;
    let metadata = client
        .fetch_metadata(keys.public_key(), Duration::from_secs(8))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    client.shutdown().await;

    print(json!({
        "pubkey": keys.public_key().to_hex(),
        "npub": keys.public_key().to_bech32().unwrap_or_default(),
        "name": metadata.name,
        "about": metadata.about,
        "picture_url": metadata.picture,
    }));
    Ok(())
}

async fn cmd_update_profile(
    cli: &Cli,
    name: Option<&str>,
    picture: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    if name.is_none() && picture.is_none() {
        anyhow::bail!(
            "at least one of --name or --picture is required.\n\
             Use 'pikachat profile' to view your current profile."
        );
    }

    let (keys, _mdk) = open(cli)?;
    let client = client(cli, &keys).await?;

    // Fetch current metadata to preserve fields we don't edit.
    client.wait_for_connection(Duration::from_secs(4)).await;
    let mut metadata = client
        .fetch_metadata(keys.public_key(), Duration::from_secs(8))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    // Apply name update.
    if let Some(n) = name {
        let trimmed = n.trim();
        if trimmed.is_empty() {
            metadata.name = None;
            metadata.display_name = None;
        } else {
            metadata.name = Some(trimmed.to_string());
            metadata.display_name = Some(trimmed.to_string());
        }
    }

    // Upload picture if provided.
    if let Some(path) = picture {
        let image_bytes =
            std::fs::read(path).with_context(|| format!("read image file: {}", path.display()))?;
        if image_bytes.is_empty() {
            anyhow::bail!("image file is empty");
        }
        if image_bytes.len() > MAX_PROFILE_IMAGE_BYTES {
            anyhow::bail!("image too large ({} bytes, max 8 MB)", image_bytes.len());
        }

        // Infer MIME type from extension.
        let mime_type = match path.extension().and_then(|e| e.to_str()) {
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("png") => "image/png",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => "image/jpeg", // default fallback
        };

        let base_url =
            nostr_sdk::Url::parse(DEFAULT_BLOSSOM_SERVER).context("parse blossom server URL")?;
        let blossom = nostr_blossom::client::BlossomClient::new(base_url);
        let descriptor = blossom
            .upload_blob(image_bytes, Some(mime_type.to_string()), None, Some(&keys))
            .await
            .context("blossom upload failed — is the server reachable?")?;
        metadata.picture = Some(descriptor.url.to_string());
    }

    // Publish updated metadata.
    let output = client
        .set_metadata(&metadata)
        .await
        .context("publish metadata")?;
    if output.success.is_empty() {
        let reasons: Vec<String> = output.failed.values().cloned().collect();
        anyhow::bail!("no relay accepted profile update: {reasons:?}");
    }
    client.shutdown().await;

    print(json!({
        "pubkey": keys.public_key().to_hex(),
        "npub": keys.public_key().to_bech32().unwrap_or_default(),
        "name": metadata.name,
        "about": metadata.about,
        "picture_url": metadata.picture,
    }));
    Ok(())
}

/// Listen for new incoming messages and welcomes. Prints each as a JSON line to stdout.
/// This is the one subcommand that *does* stay running — it's an event tail.
async fn cmd_listen(cli: &Cli, timeout_sec: u64, lookback_sec: u64) -> anyhow::Result<()> {
    let (keys, mdk) = open(cli)?;
    let client = client(cli, &keys).await?;

    let mut rx = client.notifications();

    // Subscribe to giftwrap (welcomes).
    // NIP-59 randomises the outer created_at to ±48 h, so the lookback for
    // giftwraps must be at least 2 days regardless of the caller's --lookback.
    let gift_lookback = lookback_sec.max(2 * 86400);
    let gift_since = Timestamp::now() - Duration::from_secs(gift_lookback);
    // Giftwraps are authored by the sender; recipients are indicated via the `p` tag.
    // Filtering by `pubkey(...)` would only match events *we* authored and would miss inbound invites.
    let gift_filter = Filter::new()
        .kind(Kind::GiftWrap)
        .custom_tag(
            SingleLetterTag::lowercase(Alphabet::P),
            keys.public_key().to_hex(),
        )
        .since(gift_since)
        .limit(200);
    let gift_sub = client.subscribe(gift_filter, None).await?;

    // Subscribe to all known groups.
    let mut group_subs = std::collections::HashMap::<SubscriptionId, (String, GroupId)>::new();
    if let Ok(groups) = mdk.get_groups() {
        for g in &groups {
            let ngid = hex::encode(g.nostr_group_id);
            let filter = Filter::new()
                .kind(Kind::MlsGroupMessage)
                .custom_tag(SingleLetterTag::lowercase(Alphabet::H), &ngid)
                .since(Timestamp::now() - Duration::from_secs(lookback_sec))
                .limit(200);
            if let Ok(out) = client.subscribe(filter, None).await {
                group_subs.insert(out.val, (ngid, g.mls_group_id.clone()));
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
        if event.kind == Kind::MlsGroupMessage
            && group_subs.contains_key(&subscription_id)
            && let Ok(MessageProcessingResult::ApplicationMessage(msg)) =
                mdk.process_message(&event)
        {
            let Some((ngid, mls_group_id)) = group_subs.get(&subscription_id).cloned() else {
                continue;
            };
            let line = json!({
                "type": "message",
                "nostr_group_id": ngid,
                "from_pubkey": msg.pubkey.to_hex(),
                "content": msg.content,
                "created_at": msg.created_at.as_secs(),
                "message_id": msg.id.to_hex(),
                "media": message_media_refs(&mdk, &mls_group_id, &msg.tags),
            });
            println!("{}", serde_json::to_string(&line).unwrap());
        }
    }

    client.unsubscribe_all().await;
    client.shutdown().await;
    Ok(())
}

async fn cmd_daemon(
    cli: &Cli,
    giftwrap_lookback_sec: u64,
    allow_pubkey: &[String],
    auto_accept_welcomes: bool,
    exec_cmd: Option<&str>,
) -> anyhow::Result<()> {
    let relay_urls = resolve_relays(cli);
    pikachat_sidecar::daemon::daemon_main(
        &relay_urls,
        &cli.state_dir,
        giftwrap_lookback_sec,
        allow_pubkey,
        auto_accept_welcomes,
        exec_cmd,
    )
    .await
    .context("pikachat daemon failed")
}
