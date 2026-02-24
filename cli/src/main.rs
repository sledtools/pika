mod agent;
mod fly_machines;
mod harness;
mod mdk_util;
mod microvm_spawner;
mod relay_util;
mod workers_agents;

use std::collections::{HashMap, HashSet};
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

// Same defaults as the Pika app (rust/src/core/config.rs).
const DEFAULT_RELAY_URLS: &[&str] = &[
    "wss://us-east.nostr.pikachat.org",
    "wss://eu.nostr.pikachat.org",
];

// Key packages (kind 443) are NIP-70 "protected" — many popular relays reject them.
// These relays are known to accept protected kind 443 publishes.
const DEFAULT_KP_RELAY_URLS: &[&str] = &[
    "wss://nostr-pub.wellorder.net",
    "wss://nostr-01.yakihonne.com",
    "wss://nostr-02.yakihonne.com",
];
const PROCESSED_MLS_EVENT_IDS_FILE: &str = "processed_mls_event_ids_v1.txt";
const PROCESSED_MLS_EVENT_IDS_MAX: usize = 8192;

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

    /// Relay websocket URLs (default: us-east.nostr.pikachat.org, eu.nostr.pikachat.org)
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

    /// Manage AI agents (`fly`, `workers`, or `microvm`)
    Agent {
        #[command(subcommand)]
        cmd: AgentCommand,
    },
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// Create a new pi agent and start chatting
    New {
        /// Agent name (default: agent-<random>)
        #[arg(long)]
        name: Option<String>,

        /// Runtime provider (`fly` keeps existing behavior)
        #[arg(long, value_enum, default_value_t = AgentProvider::Fly, env = "PIKA_AGENT_PROVIDER")]
        provider: AgentProvider,

        /// Brain mode for provider backends that support multiple brains
        #[arg(long, value_enum, env = "PIKA_AGENT_BRAIN")]
        brain: Option<AgentBrain>,

        #[command(flatten)]
        microvm: AgentNewMicrovmArgs,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum AgentProvider {
    Fly,
    Workers,
    Microvm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum AgentBrain {
    Stub,
    Pi,
}

impl std::fmt::Display for AgentBrain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Stub => "stub",
            Self::Pi => "pi",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Debug, Args, Default)]
struct AgentNewMicrovmArgs {
    /// MicroVM spawner base URL
    #[arg(long, env = "PIKA_MICROVM_SPAWNER_URL")]
    spawner_url: Option<String>,

    /// Spawn variant for vm-spawner
    #[arg(long, value_enum)]
    spawn_variant: Option<MicrovmSpawnVariant>,

    /// Nix flake reference used by vm-spawner when building/running guest commands
    #[arg(long)]
    flake_ref: Option<String>,

    /// Nix dev shell name used for guest command execution
    #[arg(long)]
    dev_shell: Option<String>,

    /// vCPU count for the VM
    #[arg(long)]
    cpu: Option<u32>,

    /// VM memory in MB
    #[arg(long)]
    memory_mb: Option<u32>,

    /// VM time-to-live in seconds
    #[arg(long)]
    ttl_seconds: Option<u64>,

    /// Keep VM running after CLI exit (skip auto-delete)
    #[arg(long)]
    keep: bool,
}

impl AgentNewMicrovmArgs {
    fn provided_flag_names(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.spawner_url.is_some() {
            out.push("--spawner-url");
        }
        if self.spawn_variant.is_some() {
            out.push("--spawn-variant");
        }
        if self.flake_ref.is_some() {
            out.push("--flake-ref");
        }
        if self.dev_shell.is_some() {
            out.push("--dev-shell");
        }
        if self.cpu.is_some() {
            out.push("--cpu");
        }
        if self.memory_mb.is_some() {
            out.push("--memory-mb");
        }
        if self.ttl_seconds.is_some() {
            out.push("--ttl-seconds");
        }
        if self.keep {
            out.push("--keep");
        }
        out
    }

    fn ensure_provider_compatible(&self, provider: AgentProvider) -> anyhow::Result<()> {
        if provider == AgentProvider::Microvm {
            return Ok(());
        }
        let flags = self.provided_flag_names();
        if flags.is_empty() {
            return Ok(());
        }
        anyhow::bail!(
            "microvm options {} require --provider microvm",
            flags.join(", ")
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum MicrovmSpawnVariant {
    Prebuilt,
    #[value(name = "prebuilt-cow")]
    PrebuiltCow,
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
            AgentCommand::New {
                name,
                provider,
                brain,
                microvm,
            } => cmd_agent_new(&cli, name.as_deref(), *provider, *brain, microvm).await,
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

fn processed_mls_event_ids_path(state_dir: &Path) -> PathBuf {
    state_dir.join(PROCESSED_MLS_EVENT_IDS_FILE)
}

fn load_processed_mls_event_ids(state_dir: &Path) -> HashSet<EventId> {
    let path = processed_mls_event_ids_path(state_dir);
    let Ok(raw) = std::fs::read_to_string(path) else {
        return HashSet::new();
    };
    raw.lines()
        .filter_map(|line| EventId::from_hex(line.trim()).ok())
        .collect()
}

fn persist_processed_mls_event_ids(
    state_dir: &Path,
    event_ids: &HashSet<EventId>,
) -> anyhow::Result<()> {
    let mut ids: Vec<String> = event_ids.iter().map(|id| id.to_hex()).collect();
    ids.sort_unstable();
    if ids.len() > PROCESSED_MLS_EVENT_IDS_MAX {
        ids = ids.split_off(ids.len() - PROCESSED_MLS_EVENT_IDS_MAX);
    }
    let mut body = ids.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    let path = processed_mls_event_ids_path(state_dir);
    std::fs::write(&path, body)
        .with_context(|| format!("persist processed MLS event ids to {}", path.display()))
}

/// Fetch recent group messages from the relay and feed them through
/// `mdk.process_message` so the local MLS epoch is up-to-date before we
/// attempt to create a new message.
async fn ingest_group_backlog(
    mdk: &mdk_util::PikaMdk,
    client: &Client,
    relay_urls: &[RelayUrl],
    nostr_group_id_hex: &str,
    seen_mls_event_ids: &mut HashSet<EventId>,
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
        if !seen_mls_event_ids.insert(ev.id) {
            continue;
        }
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
    let mut seen_mls_event_ids = load_processed_mls_event_ids(&cli.state_dir);

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
    ingest_group_backlog(&mdk, &client, &relays, &ngid, &mut seen_mls_event_ids).await?;

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
    persist_processed_mls_event_ids(&cli.state_dir, &seen_mls_event_ids)?;

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

async fn cmd_agent_new(
    cli: &Cli,
    name: Option<&str>,
    provider: AgentProvider,
    brain: Option<AgentBrain>,
    microvm: &AgentNewMicrovmArgs,
) -> anyhow::Result<()> {
    let resolved_brain = resolve_agent_new_brain(provider, brain, microvm)?;
    match (provider, resolved_brain) {
        (AgentProvider::Fly, AgentBrain::Stub) => cmd_agent_new_fly(cli, name).await,
        (AgentProvider::Workers, AgentBrain::Pi) => cmd_agent_new_workers(cli, name).await,
        (AgentProvider::Microvm, AgentBrain::Pi) => cmd_agent_new_microvm(cli, name, microvm).await,
        _ => unreachable!("provider/brain validation must run before dispatch"),
    }
}

fn resolve_agent_new_brain(
    provider: AgentProvider,
    brain: Option<AgentBrain>,
    microvm: &AgentNewMicrovmArgs,
) -> anyhow::Result<AgentBrain> {
    microvm.ensure_provider_compatible(provider)?;
    let resolved = match provider {
        AgentProvider::Fly => brain.unwrap_or(AgentBrain::Stub),
        AgentProvider::Workers | AgentProvider::Microvm => brain.unwrap_or(AgentBrain::Pi),
    };
    match provider {
        AgentProvider::Fly if resolved != AgentBrain::Stub => anyhow::bail!(
            "--brain {} is not supported with --provider fly (current Fly path is already pi-backed)",
            resolved
        ),
        AgentProvider::Workers if resolved != AgentBrain::Pi => {
            anyhow::bail!("--provider workers only supports --brain pi")
        }
        AgentProvider::Microvm if resolved != AgentBrain::Pi => {
            anyhow::bail!("--provider microvm only supports --brain pi")
        }
        _ => Ok(resolved),
    }
}

async fn cmd_agent_new_microvm(
    cli: &Cli,
    _name: Option<&str>,
    microvm: &AgentNewMicrovmArgs,
) -> anyhow::Result<()> {
    let resolved = agent::microvm::resolve_args(microvm);
    let relay_urls = resolve_relays(cli);
    let relays = relay_util::parse_relay_urls(&relay_urls)?;
    let (keys, mdk) = open(cli)?;
    eprintln!("Your pubkey: {}", keys.public_key().to_hex());

    let bot_keys = Keys::generate();
    let bot_pubkey = bot_keys.public_key();
    let bot_secret_hex = bot_keys.secret_key().to_secret_hex();
    eprintln!("Bot pubkey: {}", bot_pubkey.to_hex());

    let spawner = microvm_spawner::MicrovmSpawnerClient::new(resolved.spawner_url.clone());
    let create_vm_req = agent::microvm::build_create_vm_request(
        &resolved,
        keys.public_key(),
        &relay_urls,
        &bot_secret_hex,
        &bot_pubkey.to_hex(),
    );

    eprint!("Creating microVM via {}...", spawner.base_url());
    std::io::stderr().flush().ok();
    let vm = spawner
        .create_vm(&create_vm_req)
        .await
        .map_err(|err| agent::microvm::spawner_create_error(spawner.base_url(), err))?;
    eprintln!(" done ({}, ip={})", vm.id, vm.ip);

    let session_result = async {
        let client = client_all(cli, &keys).await?;
        let run_result = async {
            let bot_kp = agent::session::wait_for_latest_key_package(
                &client,
                bot_pubkey,
                &relays,
                agent::provider::KeyPackageWaitPlan {
                    progress_message: "Waiting for microVM bot key package",
                    timeout: Duration::from_secs(120),
                    fetch_timeout: Duration::from_secs(5),
                    retry_delay: Duration::from_secs(2),
                },
            )
            .await?;

            let created_group = agent::session::create_group_and_publish_welcomes(
                &keys,
                &mdk,
                &client,
                &relays,
                bot_kp,
                bot_pubkey,
                agent::provider::GroupCreatePlan {
                    progress_message: "Creating MLS group and inviting microVM bot...",
                    create_group_context: "create microvm MLS group",
                    build_welcome_context: "build microvm welcome giftwrap",
                    welcome_publish_label: "microvm welcome",
                },
            )
            .await?;

            let bot_npub = bot_pubkey
                .to_bech32()
                .unwrap_or_else(|_| bot_pubkey.to_hex().to_string());
            eprintln!();
            eprintln!("Connected to microVM agent {} ({bot_npub})", vm.id);
            eprintln!("Type messages below. Ctrl-C to exit.");
            eprintln!();

            agent::session::run_interactive_chat_loop(agent::session::ChatLoopContext {
                keys: &keys,
                mdk: &mdk,
                send_client: &client,
                listen_client: &client,
                relays: &relays,
                bot_pubkey,
                mls_group_id: &created_group.mls_group_id,
                nostr_group_id_hex: &created_group.nostr_group_id_hex,
                plan: agent::provider::ChatLoopPlan {
                    outbound_publish_label: "microvm chat user",
                    wait_for_pending_replies_on_eof: false,
                    eof_reply_timeout: Duration::from_secs(0),
                },
                seen_mls_event_ids: None,
            })
            .await
        }
        .await;

        client.unsubscribe_all().await;
        client.shutdown().await;
        run_result
    }
    .await;

    let mut teardown_error: Option<anyhow::Error> = None;
    match agent::microvm::teardown_policy(resolved.keep) {
        agent::microvm::MicrovmTeardownPolicy::DeleteOnExit => {
            eprint!("Deleting microVM {}...", vm.id);
            std::io::stderr().flush().ok();
            match spawner.delete_vm(&vm.id).await {
                Ok(()) => eprintln!(" done"),
                Err(err) => {
                    eprintln!(" failed");
                    teardown_error = Some(err);
                }
            }
        }
        agent::microvm::MicrovmTeardownPolicy::KeepVm => {
            let base = spawner.base_url().trim_end_matches('/');
            eprintln!();
            eprintln!("Keeping microVM {} alive (--keep).", vm.id);
            eprintln!("List VMs:   curl {base}/vms");
            eprintln!("Delete VM:  curl -X DELETE {base}/vms/{}", vm.id);
        }
    }

    if let Some(err) = teardown_error {
        let base = spawner.base_url().trim_end_matches('/');
        let delete_hint = format!(
            "failed to delete microvm {} via {}: {err:#}\nmanual cleanup: curl -X DELETE {base}/vms/{}",
            vm.id,
            spawner.base_url(),
            vm.id
        );
        if let Err(session_err) = session_result {
            return Err(anyhow!("{session_err:#}\n{delete_hint}"));
        }
        return Err(anyhow!(delete_hint));
    }

    session_result
}

async fn cmd_agent_new_fly(cli: &Cli, name: Option<&str>) -> anyhow::Result<()> {
    let fly = fly_machines::FlyClient::from_env()?;
    let anthropic_key =
        std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY must be set")?;
    let openai_key = std::env::var("OPENAI_API_KEY").ok();
    let pi_model = std::env::var("PI_MODEL")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let relays = resolve_relays(cli);

    let (keys, mdk) = open(cli)?;
    eprintln!("Your pubkey: {}", keys.public_key().to_hex());

    let bot_keys = Keys::generate();
    let bot_pubkey = bot_keys.public_key();
    let bot_secret_hex = bot_keys.secret_key().to_secret_hex();
    eprintln!("Bot pubkey: {}", bot_pubkey.to_hex());

    let suffix = format!("{:08x}", rand::random::<u32>());
    let volume_name = format!("agent_{suffix}");
    let machine_name = name
        .map(std::string::ToString::to_string)
        .unwrap_or_else(|| format!("agent-{suffix}"));

    eprint!("Creating Fly volume...");
    std::io::stderr().flush().ok();
    let volume = fly.create_volume(&volume_name).await?;
    eprintln!(" done ({})", volume.id);

    let mut env = HashMap::new();
    env.insert("STATE_DIR".to_string(), "/app/state".to_string());
    env.insert("NOSTR_SECRET_KEY".to_string(), bot_secret_hex);
    env.insert("ANTHROPIC_API_KEY".to_string(), anthropic_key);
    if let Some(openai) = openai_key {
        env.insert("OPENAI_API_KEY".to_string(), openai);
    }
    if let Some(model) = pi_model {
        env.insert("PI_MODEL".to_string(), model);
    }

    eprint!("Creating Fly machine...");
    std::io::stderr().flush().ok();
    let machine = fly.create_machine(&machine_name, &volume.id, env).await?;
    eprintln!(" done ({})", machine.id);

    let client = client_all(cli, &keys).await?;
    let relays = relay_util::parse_relay_urls(&relays)?;

    let bot_kp = agent::session::wait_for_latest_key_package(
        &client,
        bot_pubkey,
        &relays,
        agent::provider::KeyPackageWaitPlan {
            progress_message: "Waiting for bot to publish key package",
            timeout: Duration::from_secs(120),
            fetch_timeout: Duration::from_secs(5),
            retry_delay: Duration::from_secs(3),
        },
    )
    .await?;

    let created_group = agent::session::create_group_and_publish_welcomes(
        &keys,
        &mdk,
        &client,
        &relays,
        bot_kp,
        bot_pubkey,
        agent::provider::GroupCreatePlan {
            progress_message: "Creating MLS group and inviting bot...",
            create_group_context: "create group for bot",
            build_welcome_context: "build welcome giftwrap",
            welcome_publish_label: "welcome",
        },
    )
    .await?;

    let bot_npub = bot_pubkey
        .to_bech32()
        .unwrap_or_else(|_| bot_pubkey.to_hex().to_string());
    eprintln!();
    eprintln!("Connected to pi agent ({bot_npub})");
    eprintln!("Type messages below. Ctrl-C to exit.");
    eprintln!();

    agent::session::run_interactive_chat_loop(agent::session::ChatLoopContext {
        keys: &keys,
        mdk: &mdk,
        send_client: &client,
        listen_client: &client,
        relays: &relays,
        bot_pubkey,
        mls_group_id: &created_group.mls_group_id,
        nostr_group_id_hex: &created_group.nostr_group_id_hex,
        plan: agent::provider::ChatLoopPlan {
            outbound_publish_label: "chat",
            wait_for_pending_replies_on_eof: false,
            eof_reply_timeout: Duration::from_secs(0),
        },
        seen_mls_event_ids: None,
    })
    .await?;

    client.unsubscribe_all().await;
    client.shutdown().await;
    eprintln!();
    eprintln!("Machine {} is still running.", machine.id);
    eprintln!(
        "Stop with: fly machine stop {} -a {}",
        machine.id,
        fly.app_name()
    );
    Ok(())
}

async fn cmd_agent_new_workers(cli: &Cli, name: Option<&str>) -> anyhow::Result<()> {
    let workers = workers_agents::WorkersClient::from_env()?;
    let relay_urls = resolve_relays(cli);
    let relays = relay_util::parse_relay_urls(&relay_urls)?;
    let (keys, mdk) = open(cli)?;
    let mut seen_mls_event_ids = load_processed_mls_event_ids(&cli.state_dir);
    let client = client_all(cli, &keys).await?;
    let listener_client = client_all(cli, &keys).await?;
    eprintln!("Your pubkey: {}", keys.public_key().to_hex());

    let agent_name = name
        .map(str::to_owned)
        .unwrap_or_else(|| format!("agent-{:08x}", rand::random::<u32>()));
    let bot_keys = Keys::generate();
    let bot_pubkey = bot_keys.public_key();

    eprint!("Creating Workers agent...");
    std::io::stderr().flush().ok();
    let mut status = workers
        .create_agent(&workers_agents::CreateAgentRequest {
            name: Some(agent_name.clone()),
            brain: "pi".to_string(),
            relay_urls: relay_urls.clone(),
            bot_secret_key_hex: Some(bot_keys.secret_key().to_secret_hex()),
        })
        .await?;
    let expected_bot_pubkey_hex = bot_pubkey.to_hex();
    if status.bot_pubkey.trim().to_lowercase() != expected_bot_pubkey_hex {
        anyhow::bail!(
            "workers bot pubkey mismatch: expected {}, got {}",
            expected_bot_pubkey_hex,
            status.bot_pubkey
        );
    }
    eprintln!(" done ({})", status.id);
    eprintln!("Bot pubkey: {}", status.bot_pubkey);

    if let Some(probe) = &status.relay_probe {
        if probe.ok {
            eprintln!(
                "Relay probe ok: {}{}",
                probe.relay,
                probe
                    .status_code
                    .map(|code| format!(" (HTTP {code})"))
                    .unwrap_or_default()
            );
        } else {
            eprintln!(
                "Relay probe failed: {}{}",
                probe.relay,
                probe
                    .error
                    .as_deref()
                    .map(|err| format!(" ({err})"))
                    .unwrap_or_default()
            );
        }
    }

    eprint!("Waiting for bot to publish key package");
    std::io::stderr().flush().ok();
    let start = tokio::time::Instant::now();
    let timeout = Duration::from_secs(120);
    while status.key_package_published_at_ms.is_none() {
        if start.elapsed() >= timeout {
            anyhow::bail!(
                "timed out waiting for workers bot key package after {}s",
                timeout.as_secs()
            );
        }
        eprint!(".");
        std::io::stderr().flush().ok();
        tokio::time::sleep(Duration::from_millis(900)).await;
        status = workers.get_agent(&status.id).await?;
    }
    eprintln!(" done");
    if let Some(ts) = status.key_package_published_at_ms {
        eprintln!("Bot key package published at {} ms", ts);
    }

    let bot_kp = agent::session::wait_for_latest_key_package(
        &client,
        bot_pubkey,
        &relays,
        agent::provider::KeyPackageWaitPlan {
            progress_message: "Fetching bot key package from relay",
            timeout: Duration::from_secs(120),
            fetch_timeout: Duration::from_secs(5),
            retry_delay: Duration::from_millis(700),
        },
    )
    .await?;

    let created_group = agent::session::create_group_and_publish_welcomes(
        &keys,
        &mdk,
        &client,
        &relays,
        bot_kp,
        bot_pubkey,
        agent::provider::GroupCreatePlan {
            progress_message: "Creating MLS group and inviting workers bot...",
            create_group_context: "create workers MLS group",
            build_welcome_context: "build workers welcome giftwrap",
            welcome_publish_label: "workers welcome",
        },
    )
    .await?;

    for welcome in &created_group.published_welcomes {
        workers
            .runtime_process_welcome_event_json(
                &status.id,
                &created_group.nostr_group_id_hex,
                Some(&welcome.wrapper_event_id_hex),
                Some(&welcome.rumor_json),
            )
            .await
            .context("process workers runtime welcome")?;
    }

    eprintln!();
    eprintln!("Connected to workers agent {} ({})", status.id, status.name);
    eprintln!("Type messages below. Ctrl-C to exit.");
    eprintln!();

    agent::session::run_interactive_chat_loop(agent::session::ChatLoopContext {
        keys: &keys,
        mdk: &mdk,
        send_client: &client,
        listen_client: &listener_client,
        relays: &relays,
        bot_pubkey,
        mls_group_id: &created_group.mls_group_id,
        nostr_group_id_hex: &created_group.nostr_group_id_hex,
        plan: agent::provider::ChatLoopPlan {
            outbound_publish_label: "workers chat user",
            wait_for_pending_replies_on_eof: true,
            eof_reply_timeout: Duration::from_secs(20),
        },
        seen_mls_event_ids: Some(&mut seen_mls_event_ids),
    })
    .await?;

    listener_client.unsubscribe_all().await;
    listener_client.shutdown().await;
    client.unsubscribe_all().await;
    client.shutdown().await;
    persist_processed_mls_event_ids(&cli.state_dir, &seen_mls_event_ids)?;

    eprintln!();
    eprintln!("Workers agent {} is still active.", status.id);
    eprintln!("Inspect with: {}/agents/{}", workers.base_url(), status.id);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_agent_new(args: &[&str]) -> (AgentProvider, Option<AgentBrain>, AgentNewMicrovmArgs) {
        let cli = Cli::try_parse_from(args).expect("parse args");
        match cli.cmd {
            Command::Agent {
                cmd:
                    AgentCommand::New {
                        provider,
                        brain,
                        microvm,
                        ..
                    },
            } => (provider, brain, microvm),
            _ => panic!("expected agent new command"),
        }
    }

    #[test]
    fn agent_new_microvm_flags_parse() {
        let (provider, brain, microvm) = parse_agent_new(&[
            "pikachat",
            "agent",
            "new",
            "--provider",
            "microvm",
            "--spawner-url",
            "http://127.0.0.1:8080",
            "--spawn-variant",
            "prebuilt-cow",
            "--flake-ref",
            ".#nixpi",
            "--dev-shell",
            "default",
            "--cpu",
            "1",
            "--memory-mb",
            "1024",
            "--ttl-seconds",
            "600",
            "--keep",
        ]);
        assert_eq!(provider, AgentProvider::Microvm);
        assert_eq!(brain, None);
        assert_eq!(
            microvm.spawner_url.as_deref(),
            Some("http://127.0.0.1:8080")
        );
        assert_eq!(
            microvm.spawn_variant,
            Some(MicrovmSpawnVariant::PrebuiltCow)
        );
        assert_eq!(microvm.flake_ref.as_deref(), Some(".#nixpi"));
        assert_eq!(microvm.dev_shell.as_deref(), Some("default"));
        assert_eq!(microvm.cpu, Some(1));
        assert_eq!(microvm.memory_mb, Some(1024));
        assert_eq!(microvm.ttl_seconds, Some(600));
        assert!(microvm.keep);
    }

    #[test]
    fn agent_new_existing_fly_and_workers_parse_unchanged() {
        let (fly_provider, fly_brain, fly_microvm) =
            parse_agent_new(&["pikachat", "agent", "new", "--provider", "fly"]);
        assert_eq!(fly_provider, AgentProvider::Fly);
        assert_eq!(fly_brain, None);
        assert!(fly_microvm.provided_flag_names().is_empty());

        let (workers_provider, workers_brain, workers_microvm) = parse_agent_new(&[
            "pikachat",
            "agent",
            "new",
            "--provider",
            "workers",
            "--brain",
            "pi",
        ]);
        assert_eq!(workers_provider, AgentProvider::Workers);
        assert_eq!(workers_brain, Some(AgentBrain::Pi));
        assert!(workers_microvm.provided_flag_names().is_empty());
    }

    #[test]
    fn microvm_flags_rejected_for_non_microvm_provider() {
        let (provider, brain, microvm) = parse_agent_new(&[
            "pikachat",
            "agent",
            "new",
            "--provider",
            "fly",
            "--spawner-url",
            "http://127.0.0.1:8080",
        ]);
        let err = resolve_agent_new_brain(provider, brain, &microvm).expect_err("should fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("--spawner-url"));
        assert!(msg.contains("--provider microvm"));
    }
}
