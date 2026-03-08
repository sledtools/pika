mod agent;
mod harness;
mod mdk_util;
mod relay_util;
mod remote;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow};
use base64::Engine;
use clap::{Args, Parser, Subcommand, ValueEnum};
use hypernote_protocol as hn;
use mdk_core::encrypted_media::types::{MediaProcessingOptions, MediaReference};
use mdk_core::prelude::*;
use nostr_blossom::client::BlossomClient;
use nostr_sdk::prelude::*;
use pika_marmot_runtime::key_package::normalize_peer_key_package_event_for_mdk;
use pika_relay_profiles::{
    default_key_package_relays, default_message_relays, default_primary_blossom_server,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

const CHANNELS_MANIFEST_JSON: &str = include_str!("../../config/channels.json");
const AGENT_API_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const AGENT_API_TIMEOUT: Duration = Duration::from_secs(30);

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

    /// Relay websocket URLs (default: shared app/CLI profile from pika-relay-profiles)
    #[arg(long, global = true)]
    relay: Vec<String>,

    /// Key-package relay URLs (default: wellorder.net, yakihonne x2)
    #[arg(long, global = true)]
    kp_relay: Vec<String>,

    /// Connect to a running daemon instead of opening MLS state directly
    #[arg(long, global = true)]
    remote: bool,

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

    /// Print a QR code for your identity that deep-links into the Pika app
    #[command(after_help = "Example:
  pikachat qr
  pikachat qr --channel test
  pikachat qr --channel dev --scheme customscheme

Prints a QR code to the terminal. When scanned, it opens Pika and starts a 1:1 chat.")]
    Qr {
        /// Channel id used to resolve deep-link scheme from config/channels.json.
        #[arg(long, value_enum, default_value_t = QrChannel::Prod, env = "PIKA_CHANNEL")]
        channel: QrChannel,

        /// Explicit URL scheme override (bypasses channel mapping).
        #[arg(long)]
        scheme: Option<String>,
    },

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

        /// Blossom server URL (repeatable; defaults to shared profile blossom servers)
        #[arg(long = "blossom", requires = "media")]
        blossom_servers: Vec<String>,
    },

    /// Send a hypernote (MDX content with optional interactive components) to a group or peer
    #[command(after_help = "Examples:
  pikachat send-hypernote --group <hex-group-id> --content '# Hello\\n\\n<Card><Heading>Test</Heading></Card>'
  pikachat send-hypernote --to <npub> --file note.hnmd
  pikachat send-hypernote --group <hex-group-id> --content '# Poll\\n\\n<SubmitButton action=\"yes\">Yes</SubmitButton>'

A .hnmd file can include a JSON frontmatter block with title and state:

  ```hnmd
  {\"title\": \"My Note\", \"state\": {\"name\": \"Alice\"}}
  ```
  # Content starts here")]
    SendHypernote {
        /// Nostr group ID (hex) — send directly to this group
        #[arg(long, conflicts_with = "to")]
        group: Option<String>,

        /// Peer public key (npub or hex) — find or create a 1:1 DM with this peer
        #[arg(long, conflicts_with = "group")]
        to: Option<String>,

        /// Hypernote MDX content (mutually exclusive with --file)
        #[arg(long, conflicts_with = "file")]
        content: Option<String>,

        /// Path to a .hnmd file (mutually exclusive with --content)
        #[arg(long, conflicts_with = "content")]
        file: Option<std::path::PathBuf>,

        /// Hypernote title
        #[arg(long)]
        title: Option<String>,

        /// JSON-encoded default state for interactive components
        #[arg(long)]
        state: Option<String>,
    },

    /// Print the canonical hypernote component/action catalog
    HypernoteCatalog {
        /// Compact JSON output (single line)
        #[arg(long, default_value_t = false)]
        compact: bool,
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

    /// Set a per-group profile (name/about published as kind-0 inside the MLS group)
    #[command(after_help = "Examples:
  pikachat update-group-profile --group <HEX> --name \"Alice in Wonderland\"
  pikachat update-group-profile --group <HEX> --name \"Alice\" --about \"group bio\"")]
    UpdateGroupProfile {
        /// Nostr group ID (hex)
        #[arg(long)]
        group: String,

        /// Display name for this group
        #[arg(long)]
        name: Option<String>,

        /// About text for this group
        #[arg(long)]
        about: Option<String>,
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

    /// Manage AI agents (HTTP control plane)
    Agent {
        #[command(subcommand)]
        cmd: AgentCommand,
    },
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// Ensure an agent exists for the configured Nostr signer
    New {
        #[command(flatten)]
        http: AgentHttpArgs,
    },

    /// Fetch the current agent state for the configured Nostr signer
    Me {
        #[command(flatten)]
        http: AgentHttpArgs,
    },

    /// Ask the server to recover the current agent VM for the configured signer
    Recover {
        #[command(flatten)]
        http: AgentHttpArgs,
    },

    /// Ensure/reuse your personal agent, send one message, and optionally listen for replies
    #[command(after_help = "Behavior:
  - if --listen-timeout is 0, the command only sends the message
  - if listening times out without any reply message, the command exits with `no_reply_within_timeout`")]
    Chat {
        #[command(flatten)]
        http: AgentHttpArgs,

        /// Message content to send to your personal agent
        message: String,

        /// Listen duration (seconds) after sending (0 disables listening)
        #[arg(long, default_value_t = 120)]
        listen_timeout: u64,

        /// Max status poll attempts while waiting for ready
        #[arg(long, default_value_t = 45)]
        poll_attempts: u32,

        /// Delay between status polls (seconds)
        #[arg(long, default_value_t = 2)]
        poll_delay_sec: u64,

        /// Trigger recover after this many creating-state polls (0 disables)
        #[arg(long, default_value_t = 20)]
        recover_after_attempt: u32,
    },
}

const DEFAULT_AGENT_API_BASE_URL: &str = "http://127.0.0.1:8080";
const AGENT_API_ENSURE_PATH: &str = "/v1/agents/ensure";
const AGENT_API_ME_PATH: &str = "/v1/agents/me";
const AGENT_API_RECOVER_PATH: &str = "/v1/agents/me/recover";

#[derive(Clone, Debug, Args)]
struct AgentHttpArgs {
    /// Base URL for pika-server HTTP control plane
    #[arg(long, env = "PIKA_AGENT_API_BASE_URL", default_value = DEFAULT_AGENT_API_BASE_URL)]
    api_base_url: String,

    /// Nostr secret key (nsec1... or hex) used for NIP-98 request signing
    #[arg(long, env = "PIKA_AGENT_API_NSEC")]
    nsec: Option<String>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum QrChannel {
    Prod,
    Dev,
    Test,
}

impl QrChannel {
    fn as_id(self) -> &'static str {
        match self {
            Self::Prod => "prod",
            Self::Dev => "dev",
            Self::Test => "test",
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChannelManifest {
    channels: Vec<ChannelConfig>,
}

#[derive(Debug, Deserialize)]
struct ChannelConfig {
    id: String,
    url_scheme: String,
}

async fn handle_remote(cli: &Cli) -> anyhow::Result<()> {
    let cmd_json = match &cli.cmd {
        Command::Groups => {
            serde_json::json!({"cmd": "list_groups"})
        }
        Command::Welcomes => {
            serde_json::json!({"cmd": "list_pending_welcomes"})
        }
        Command::AcceptWelcome { wrapper_event_id } => {
            serde_json::json!({"cmd": "accept_welcome", "wrapper_event_id": wrapper_event_id})
        }
        Command::Messages { group, limit } => {
            serde_json::json!({"cmd": "get_messages", "nostr_group_id": group, "limit": limit})
        }
        Command::Send {
            group,
            to,
            content,
            media,
            mime_type,
            filename,
            blossom_servers,
        } => {
            if to.is_some() {
                anyhow::bail!("--remote --to not yet supported; use --group with the hex group id");
            }
            let group = group
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("--group is required in --remote mode"))?;
            if let Some(media_path) = media {
                serde_json::json!({
                    "cmd": "send_media",
                    "nostr_group_id": group,
                    "file_path": media_path.display().to_string(),
                    "caption": content,
                    "mime_type": mime_type,
                    "filename": filename,
                    "blossom_servers": blossom_servers,
                })
            } else {
                serde_json::json!({
                    "cmd": "send_message",
                    "nostr_group_id": group,
                    "content": content,
                })
            }
        }
        Command::SendHypernote {
            group,
            to,
            content,
            file,
            title,
            state,
        } => {
            if to.is_some() {
                anyhow::bail!("--remote --to not yet supported; use --group with the hex group id");
            }
            let group = group
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("--group is required in --remote mode"))?;
            let (body, file_title, file_state): HnmdParts = match (&content, &file) {
                (Some(c), None) => (c.clone(), None, None),
                (None, Some(path)) => parse_hnmd_file(path)?,
                (None, None) => anyhow::bail!("either --content or --file is required"),
                _ => unreachable!(),
            };
            serde_json::json!({
                "cmd": "send_hypernote",
                "nostr_group_id": group,
                "content": body,
                "title": title.as_deref().or(file_title.as_deref()),
                "state": state.as_deref().or(file_state.as_deref()),
            })
        }
        Command::Invite { peer, name } => {
            serde_json::json!({
                "cmd": "init_group",
                "peer_pubkey": peer,
                "group_name": name,
            })
        }
        Command::PublishKp => {
            serde_json::json!({"cmd": "publish_keypackage"})
        }
        _ => {
            anyhow::bail!(
                "command not supported in --remote mode; supported: groups, welcomes, accept-welcome, messages, send, send-hypernote, invite, publish-kp"
            );
        }
    };
    let result = remote::remote_call(&cli.state_dir, cmd_json).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
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

    if cli.remote {
        return handle_remote(&cli).await;
    }

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
        Command::Qr { channel, scheme } => cmd_qr(&cli, *channel, scheme.as_deref()),
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
        Command::SendHypernote {
            group,
            to,
            content,
            file,
            title,
            state,
        } => {
            let (content, file_title, file_state): HnmdParts = match (&content, &file) {
                (Some(c), None) => (c.clone(), None, None),
                (None, Some(path)) => parse_hnmd_file(path)?,
                (None, None) => anyhow::bail!("either --content or --file is required"),
                _ => unreachable!(), // conflicts_with prevents this
            };
            cmd_send_hypernote(
                &cli,
                group.as_deref(),
                to.as_deref(),
                &content,
                title.as_deref().or(file_title.as_deref()),
                state.as_deref().or(file_state.as_deref()),
            )
            .await
        }
        Command::HypernoteCatalog { compact } => cmd_hypernote_catalog(*compact),
        Command::DownloadMedia { message_id, output } => {
            cmd_download_media(&cli, message_id, output.as_deref()).await
        }
        Command::Messages { group, limit } => cmd_messages(&cli, group, *limit),
        Command::Profile => cmd_profile(&cli).await,
        Command::UpdateProfile { name, picture } => {
            cmd_update_profile(&cli, name.as_deref(), picture.as_deref()).await
        }
        Command::UpdateGroupProfile { group, name, about } => {
            cmd_update_group_profile(&cli, group, name.as_deref(), about.as_deref()).await
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
            AgentCommand::New { http } => cmd_agent_new(http).await,
            AgentCommand::Me { http } => cmd_agent_me(http).await,
            AgentCommand::Recover { http } => cmd_agent_recover(http).await,
            AgentCommand::Chat {
                http,
                message,
                listen_timeout,
                poll_attempts,
                poll_delay_sec,
                recover_after_attempt,
            } => {
                cmd_agent_chat(
                    &cli,
                    http,
                    message,
                    *listen_timeout,
                    *poll_attempts,
                    *poll_delay_sec,
                    *recover_after_attempt,
                )
                .await
            }
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
        default_message_relays()
    } else {
        cli.relay.clone()
    }
}

/// Resolve key-package relay URLs: use --kp-relay if provided, otherwise defaults.
fn resolve_kp_relays(cli: &Cli) -> Vec<String> {
    if cli.kp_relay.is_empty() {
        default_key_package_relays()
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
/// `ingest_application_message` so the local MLS epoch is up-to-date
/// before we attempt to create a new message.
async fn ingest_group_backlog(
    mdk: &mdk_util::PikaMdk,
    client: &Client,
    relay_urls: &[RelayUrl],
    nostr_group_id_hex: &str,
    seen_mls_event_ids: &mut HashSet<EventId>,
) -> anyhow::Result<()> {
    pika_marmot_runtime::ingest_group_backlog(
        mdk,
        client,
        relay_urls,
        nostr_group_id_hex,
        seen_mls_event_ids,
        200,
    )
    .await?;
    Ok(())
}

const MAX_CHAT_MEDIA_BYTES: usize = 32 * 1024 * 1024;

use pika_marmot_runtime::media::{is_imeta_tag, mime_from_extension};

fn blossom_servers_or_default(values: &[String]) -> Vec<String> {
    pika_relay_profiles::blossom_servers_or_default(values)
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

fn cmd_qr(cli: &Cli, channel: QrChannel, scheme_override: Option<&str>) -> anyhow::Result<()> {
    let keys = mdk_util::load_or_create_keys(&cli.state_dir.join("identity.json"))?;
    let npub = keys.public_key().to_bech32().context("encode npub")?;
    let scheme = resolve_qr_scheme(channel, scheme_override)?;
    let deep_link = format!("{scheme}://chat/{npub}");

    qr2term::print_qr(&deep_link).context("render QR code")?;
    eprintln!();
    eprintln!("  npub: {npub}");
    eprintln!("  link: {deep_link}");
    Ok(())
}

fn resolve_qr_scheme(channel: QrChannel, scheme_override: Option<&str>) -> anyhow::Result<String> {
    if let Some(raw) = scheme_override {
        let scheme = raw.trim().to_ascii_lowercase();
        if scheme.is_empty() {
            anyhow::bail!("--scheme must not be empty");
        }
        return Ok(scheme);
    }

    let manifest: ChannelManifest =
        serde_json::from_str(CHANNELS_MANIFEST_JSON).context("parse config/channels.json")?;
    let channel_id = channel.as_id();
    let scheme = manifest
        .channels
        .iter()
        .find(|entry| entry.id == channel_id)
        .map(|entry| entry.url_scheme.as_str())
        .with_context(|| format!("missing channel {channel_id} in config/channels.json"))?;
    Ok(scheme.to_string())
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
    let peer_kp = relay_util::fetch_latest_key_package_for_mdk(
        &client,
        &peer_pubkey,
        &kp_relays,
        Duration::from_secs(10),
    )
    .await
    .context("fetch peer key package — has the peer run `publish-kp`?")?;
    let peer_kp = normalize_peer_key_package_event_for_mdk(&peer_kp);

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

    // CLI invite waits for welcome delivery before returning, but it does not
    // subscribe or backfill here; later commands do catch-up on demand.
    let created = create_group_and_publish_welcomes_for_invite(
        &keys,
        &mdk,
        &client,
        &relays,
        peer_kp,
        peer_pubkey,
        config,
    )
    .await?;

    let ngid = hex::encode(created.group.nostr_group_id);

    client.shutdown().await;

    print(json!({
        "nostr_group_id": ngid,
        "mls_group_id": hex::encode(created.group.mls_group_id.as_slice()),
        "peer_pubkey": peer_pubkey.to_hex(),
    }));
    Ok(())
}

async fn create_group_and_publish_welcomes_for_invite(
    keys: &Keys,
    mdk: &mdk_util::PikaMdk,
    client: &Client,
    relays: &[RelayUrl],
    peer_kp: Event,
    peer_pubkey: PublicKey,
    config: NostrGroupConfigData,
) -> anyhow::Result<pika_marmot_runtime::welcome::CreatedGroup> {
    create_group_and_publish_welcomes_for_invite_with_publisher(
        keys,
        mdk,
        peer_kp,
        peer_pubkey,
        config,
        |_, giftwrap| {
            let client = client.clone();
            let relays = relays.to_vec();
            async move { relay_util::publish_and_confirm(&client, &relays, &giftwrap, "welcome").await }
        },
    )
    .await
}

async fn create_group_and_publish_welcomes_for_invite_with_publisher<F, Fut>(
    keys: &Keys,
    mdk: &mdk_util::PikaMdk,
    peer_kp: Event,
    peer_pubkey: PublicKey,
    config: NostrGroupConfigData,
    publish_giftwrap: F,
) -> anyhow::Result<pika_marmot_runtime::welcome::CreatedGroup>
where
    F: FnMut(PublicKey, Event) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    pika_marmot_runtime::welcome::create_group_and_publish_welcomes(
        keys,
        mdk,
        vec![peer_kp],
        config,
        &[peer_pubkey],
        vec![],
        publish_giftwrap,
    )
    .await
    .context("create group and publish welcomes")
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
    let target_id =
        EventId::from_hex(wrapper_event_id_hex).context("parse pending welcome event id")?;

    let pending = mdk
        .get_pending_welcomes(None)
        .context("get pending welcomes")?;
    let welcome = find_pending_welcome_for_accept(&pending, &target_id)
        .ok_or_else(|| anyhow!("no pending welcome with that event id"))?;

    let ngid = hex::encode(welcome.nostr_group_id);
    let mls_gid = hex::encode(welcome.mls_group_id.as_slice());

    mdk.accept_welcome(welcome).context("accept welcome")?;

    // CLI accept is intentionally narrow today: it joins locally but does not
    // subscribe or backfill here. Later `messages`, `send`, and listener flows
    // do their own catch-up on demand.

    print(json!({
        "nostr_group_id": ngid,
        "mls_group_id": mls_gid,
    }));
    Ok(())
}

fn find_pending_welcome_for_accept<'a>(
    pending: &'a [mdk_storage_traits::welcomes::types::Welcome],
    target_id: &EventId,
) -> Option<&'a mdk_storage_traits::welcomes::types::Welcome> {
    pika_marmot_runtime::welcome::find_pending_welcome(pending, target_id)
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
    let mut seen_mls_event_ids = mdk_util::load_processed_mls_event_ids(&cli.state_dir);

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
                let peer_kp = match relay_util::fetch_latest_key_package_for_mdk(
                    &c,
                    &peer_pubkey,
                    &kp_relays,
                    Duration::from_secs(10),
                )
                .await
                {
                    Ok(kp) => kp,
                    Err(primary_err) => {
                        // If kp relays are defaults, retry on active message relays.
                        if cli.kp_relay.is_empty() && kp_relays != relays {
                            relay_util::fetch_latest_key_package_for_mdk(
                                &c,
                                &peer_pubkey,
                                &relays,
                                Duration::from_secs(10),
                            )
                            .await
                            .with_context(|| {
                                format!(
                                    "fetch peer key package failed on default kp relays and message relays; has the peer run `pikachat init`? primary={primary_err}"
                                )
                            })?
                        } else {
                            return Err(primary_err).context(
                                "fetch peer key package — has the peer run `pikachat init`?",
                            );
                        }
                    }
                };
                let peer_kp = normalize_peer_key_package_event_for_mdk(&peer_kp);
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
    mdk_util::persist_processed_mls_event_ids(&cli.state_dir, &seen_mls_event_ids)?;

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

/// (content, title, state)
type HnmdParts = (String, Option<String>, Option<String>);

/// Parse a `.hnmd` file into (content, title, state).
///
/// The file may optionally start with a JSON frontmatter block:
/// ````
/// ```hnmd
/// {"title": "...", "state": {...}}
/// ```
/// # MDX content here
/// ````
fn parse_hnmd_file(path: &std::path::Path) -> anyhow::Result<HnmdParts> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("read file: {}", path.display()))?;

    let trimmed = raw.trim_start();

    // Check for ```hnmd frontmatter block.
    if let Some(after_open) = trimmed.strip_prefix("```hnmd") {
        let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
        if let Some(close_pos) = after_open.find("\n```") {
            let json_str = &after_open[..close_pos];
            let body = after_open[close_pos + 4..].trim_start_matches('\n');

            let meta: serde_json::Value = serde_json::from_str(json_str)
                .with_context(|| "invalid JSON in ```hnmd frontmatter")?;

            let title = meta.get("title").and_then(|v| v.as_str()).map(String::from);
            let state = meta.get("state").map(|v| v.to_string());

            return Ok((body.to_string(), title, state));
        }
        anyhow::bail!("unclosed ```hnmd frontmatter block in {}", path.display());
    }

    // No frontmatter — entire file is content.
    Ok((raw, None, None))
}

async fn cmd_send_hypernote(
    cli: &Cli,
    group_hex: Option<&str>,
    to_str: Option<&str>,
    content: &str,
    title: Option<&str>,
    state: Option<&str>,
) -> anyhow::Result<()> {
    if group_hex.is_none() && to_str.is_none() {
        anyhow::bail!(
            "either --group or --to is required.\n\
             Use --group <HEX> to send to a known group, or --to <NPUB> to send to a peer."
        );
    }
    if content.is_empty() {
        anyhow::bail!("--content is required");
    }

    let (keys, mdk) = open(cli)?;
    let mut seen_mls_event_ids = mdk_util::load_processed_mls_event_ids(&cli.state_dir);
    let relays = relay_util::parse_relay_urls(&resolve_relays(cli))?;

    // Resolve target group (reuse the same logic as cmd_send for --group / --to).
    let (group, client) = match (group_hex, to_str) {
        (Some(gid), _) => {
            let group = find_group(&mdk, gid)?;
            let c = client(cli, &keys).await?;
            (group, c)
        }
        (_, Some(peer_str)) => {
            let peer_pubkey = PublicKey::parse(peer_str.trim())
                .with_context(|| format!("parse peer key: {peer_str}"))?;
            let my_pubkey = keys.public_key();
            let groups = mdk.get_groups().context("get groups")?;
            let found = groups.into_iter().find(|g| {
                let members = mdk.get_members(&g.mls_group_id).unwrap_or_default();
                let others: Vec<_> = members.iter().filter(|p| *p != &my_pubkey).collect();
                others.len() == 1 && *others[0] == peer_pubkey
            });
            if let Some(group) = found {
                let c = client(cli, &keys).await?;
                (group, c)
            } else {
                let c = client_all(cli, &keys).await?;
                let kp_relays = relay_util::parse_relay_urls(&resolve_kp_relays(cli))?;
                let peer_kp = match relay_util::fetch_latest_key_package_for_mdk(
                    &c,
                    &peer_pubkey,
                    &kp_relays,
                    Duration::from_secs(10),
                )
                .await
                {
                    Ok(kp) => kp,
                    Err(primary_err) => {
                        if cli.kp_relay.is_empty() && kp_relays != relays {
                            relay_util::fetch_latest_key_package_for_mdk(
                                &c,
                                &peer_pubkey,
                                &relays,
                                Duration::from_secs(10),
                            )
                            .await
                            .with_context(|| {
                                format!(
                                    "fetch peer key package failed on default kp relays and message relays; has the peer run `pikachat init`? primary={primary_err}"
                                )
                            })?
                        } else {
                            return Err(primary_err).context(
                                "fetch peer key package — has the peer run `pikachat init`?",
                            );
                        }
                    }
                };
                let peer_kp = normalize_peer_key_package_event_for_mdk(&peer_kp);
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
                (result.group, c)
            }
        }
        _ => unreachable!(),
    };

    let ngid = hex::encode(group.nostr_group_id);
    ingest_group_backlog(&mdk, &client, &relays, &ngid, &mut seen_mls_event_ids).await?;

    // Build tags.
    let mut tags: Vec<Tag> = Vec::new();
    if let Some(t) = title {
        tags.push(Tag::custom(TagKind::custom("title"), vec![t.to_string()]));
    }
    if let Some(s) = state {
        tags.push(Tag::custom(TagKind::custom("state"), vec![s.to_string()]));
    }

    // Build and send MLS message with hypernote kind.
    let rumor = EventBuilder::new(Kind::Custom(hn::HYPERNOTE_KIND), content)
        .tags(tags)
        .build(keys.public_key());
    let msg_event = mdk
        .create_message(&group.mls_group_id, rumor)
        .context("create message")?;
    relay_util::publish_and_confirm(&client, &relays, &msg_event, "send_hypernote").await?;
    client.shutdown().await;
    mdk_util::persist_processed_mls_event_ids(&cli.state_dir, &seen_mls_event_ids)?;

    print(json!({
        "event_id": msg_event.id.to_hex(),
        "nostr_group_id": ngid,
    }));
    Ok(())
}

fn cmd_hypernote_catalog(compact: bool) -> anyhow::Result<()> {
    if compact {
        print(hn::hypernote_catalog_value());
    } else {
        println!("{}", hn::hypernote_catalog_json());
    }
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

async fn cmd_agent_new(http: &AgentHttpArgs) -> anyhow::Result<()> {
    let ensured = ensure_agent_idempotent(http).await?;
    print(json!({
        "operation": "ensure",
        "created": ensured.created,
        "agent": ensured.agent,
    }));
    Ok(())
}

async fn cmd_agent_me(http: &AgentHttpArgs) -> anyhow::Result<()> {
    let agent = call_agent_api(http, reqwest::Method::GET, AGENT_API_ME_PATH).await?;
    print(json!({
        "operation": "me",
        "agent": agent,
    }));
    Ok(())
}

async fn cmd_agent_recover(http: &AgentHttpArgs) -> anyhow::Result<()> {
    let agent = call_agent_api(http, reqwest::Method::POST, AGENT_API_RECOVER_PATH).await?;
    print(json!({
        "operation": "recover",
        "agent": agent,
    }));
    Ok(())
}

#[derive(Debug)]
struct EnsureAgentResult {
    agent: serde_json::Value,
    created: bool,
}

async fn ensure_agent_idempotent(http: &AgentHttpArgs) -> anyhow::Result<EnsureAgentResult> {
    let method = reqwest::Method::POST;
    let (status, body) = call_agent_api_raw(http, method.clone(), AGENT_API_ENSURE_PATH).await?;
    if status.is_success() {
        return Ok(EnsureAgentResult {
            agent: parse_agent_api_response_json(&body, AGENT_API_ENSURE_PATH)?,
            created: true,
        });
    }

    if status == reqwest::StatusCode::CONFLICT
        && agent_api_error_code(&body).as_deref() == Some("agent_exists")
    {
        return Ok(EnsureAgentResult {
            agent: call_agent_api(http, reqwest::Method::GET, AGENT_API_ME_PATH).await?,
            created: false,
        });
    }

    Err(agent_api_http_error(
        &method,
        AGENT_API_ENSURE_PATH,
        status,
        &body,
    ))
}

fn parse_agent_fields(agent: &serde_json::Value) -> anyhow::Result<(String, String)> {
    let agent_id = agent
        .get("agent_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("agent response missing agent_id"))?
        .to_string();
    let state = agent
        .get("state")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("agent response missing state"))?
        .to_string();
    Ok((agent_id, state))
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ChatSendOutcome {
    ReplyReceived,
    NoReplyWithinTimeout,
    ListenDisabled,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
struct ListenSummary {
    saw_matching_message: bool,
}

fn find_direct_group_with_peer(
    mdk: &mdk_util::PikaMdk,
    my_pubkey: &PublicKey,
    peer_pubkey: &PublicKey,
) -> anyhow::Result<Option<mdk_storage_traits::groups::types::Group>> {
    let groups = mdk.get_groups().context("get groups")?;
    Ok(groups.into_iter().find(|g| {
        let members = mdk.get_members(&g.mls_group_id).unwrap_or_default();
        let others: Vec<_> = members.iter().filter(|p| *p != my_pubkey).collect();
        others.len() == 1 && *others[0] == *peer_pubkey
    }))
}

async fn send_to_agent_and_optionally_listen(
    chat_cli: &Cli,
    agent_npub: &str,
    message: &str,
    listen_timeout: u64,
) -> anyhow::Result<ChatSendOutcome> {
    let send_started_at = Timestamp::now().as_secs();
    cmd_send(
        chat_cli,
        None,
        Some(agent_npub),
        message,
        None,
        None,
        None,
        &[],
    )
    .await?;
    if listen_timeout > 0 {
        let (keys, mdk) = open(chat_cli)?;
        let agent_pubkey = PublicKey::parse(agent_npub).context("parse agent npub")?;
        let expected_group_id =
            find_direct_group_with_peer(&mdk, &keys.public_key(), &agent_pubkey)?
                .map(|group| hex::encode(group.nostr_group_id));
        let summary = listen_for_incoming(
            chat_cli,
            listen_timeout,
            listen_timeout.max(5),
            Some(agent_pubkey),
            expected_group_id.as_deref(),
            Some(send_started_at),
        )
        .await?;
        return Ok(if summary.saw_matching_message {
            ChatSendOutcome::ReplyReceived
        } else {
            ChatSendOutcome::NoReplyWithinTimeout
        });
    }
    Ok(ChatSendOutcome::ListenDisabled)
}

fn finish_chat_send(outcome: ChatSendOutcome, listen_timeout: u64) -> anyhow::Result<()> {
    match outcome {
        ChatSendOutcome::ReplyReceived | ChatSendOutcome::ListenDisabled => Ok(()),
        ChatSendOutcome::NoReplyWithinTimeout => anyhow::bail!(
            "no_reply_within_timeout: sent message but no reply arrived within {}s",
            listen_timeout
        ),
    }
}

fn ensure_identity_for_state_dir(state_dir: &Path, keys: &Keys) -> anyhow::Result<()> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("create state dir {}", state_dir.display()))?;
    let identity_path = state_dir.join("identity.json");
    let identity = mdk_util::IdentityFile {
        secret_key_hex: keys.secret_key().to_secret_hex(),
        public_key_hex: keys.public_key().to_hex(),
    };
    std::fs::write(
        &identity_path,
        format!("{}\n", serde_json::to_string_pretty(&identity)?),
    )
    .with_context(|| format!("write {}", identity_path.display()))?;
    let _ = mdk_util::open_mdk(state_dir)?;
    Ok(())
}

async fn cmd_agent_chat(
    cli: &Cli,
    http: &AgentHttpArgs,
    message: &str,
    listen_timeout: u64,
    poll_attempts: u32,
    poll_delay_sec: u64,
    recover_after_attempt: u32,
) -> anyhow::Result<()> {
    let message = message.trim();
    anyhow::ensure!(!message.is_empty(), "message cannot be empty");
    anyhow::ensure!(poll_attempts > 0, "--poll-attempts must be > 0");
    let nsec = agent_api_nsec_value(http.nsec.as_deref())?;
    let owner_keys = Keys::parse(&nsec).context("parse agent api nsec")?;
    let owner_chat_state_dir = cli
        .state_dir
        .join("agent-chat")
        .join(owner_keys.public_key().to_hex());
    ensure_identity_for_state_dir(&owner_chat_state_dir, &owner_keys)?;
    let chat_cli = Cli {
        state_dir: owner_chat_state_dir,
        relay: cli.relay.clone(),
        kp_relay: cli.kp_relay.clone(),
        remote: false,
        cmd: Command::Identity,
    };

    let ensured = ensure_agent_idempotent(http).await?;
    let mut tried_optimistic_send = false;
    let mut recovered_stalled_creating = false;
    let poll_delay = Duration::from_secs(poll_delay_sec);
    let mut last_state = String::new();
    let mut last_agent_npub = String::new();

    for attempt in 1..=poll_attempts {
        let me = call_agent_api(http, reqwest::Method::GET, AGENT_API_ME_PATH).await?;
        let (agent_npub, state) = parse_agent_fields(&me)?;
        last_state = state.clone();
        last_agent_npub = agent_npub.clone();

        if state == "ready" {
            let outcome = send_to_agent_and_optionally_listen(
                &chat_cli,
                &agent_npub,
                message,
                listen_timeout,
            )
            .await?;
            return finish_chat_send(outcome, listen_timeout);
        }

        if state == "creating" && !ensured.created && !tried_optimistic_send {
            match send_to_agent_and_optionally_listen(
                &chat_cli,
                &agent_npub,
                message,
                listen_timeout,
            )
            .await
            {
                Ok(outcome) => return finish_chat_send(outcome, listen_timeout),
                Err(_) => {
                    tried_optimistic_send = true;
                    eprintln!("optimistic send failed; continuing with recover/poll");
                }
            }
        }

        if state == "error" {
            eprintln!("agent in error state; requesting recover");
            let _ = call_agent_api(http, reqwest::Method::POST, AGENT_API_RECOVER_PATH).await;
        } else if state == "creating"
            && !recovered_stalled_creating
            && recover_after_attempt > 0
            && attempt >= recover_after_attempt
        {
            eprintln!("agent still creating after {attempt} checks; requesting recover");
            let _ = call_agent_api(http, reqwest::Method::POST, AGENT_API_RECOVER_PATH).await;
            recovered_stalled_creating = true;
        }

        if attempt < poll_attempts {
            tokio::time::sleep(poll_delay).await;
        }
    }

    anyhow::ensure!(
        !last_agent_npub.is_empty(),
        "timed out waiting for personal agent"
    );
    eprintln!(
        "agent did not become ready (state={}); trying best-effort send",
        last_state
    );
    if let Ok(outcome) =
        send_to_agent_and_optionally_listen(&chat_cli, &last_agent_npub, message, listen_timeout)
            .await
    {
        return finish_chat_send(outcome, listen_timeout);
    }
    anyhow::bail!(
        "agent chat failed: state remained {} and send failed; try `pikachat agent recover --api-base-url {} --nsec <nsec>`",
        if last_state.is_empty() {
            "unknown"
        } else {
            &last_state
        },
        http.api_base_url
    );
}

fn normalized_agent_api_base_url(raw: &str) -> anyhow::Result<String> {
    let trimmed = raw.trim();
    anyhow::ensure!(!trimmed.is_empty(), "agent api base url cannot be empty");
    let parsed = reqwest::Url::parse(trimmed).context("parse agent api base url")?;
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

fn agent_api_nsec_value(raw: Option<&str>) -> anyhow::Result<String> {
    let candidate = raw
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("PIKA_TEST_NSEC")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .ok_or_else(|| {
            anyhow!("agent api nsec is required (--nsec, PIKA_AGENT_API_NSEC, or PIKA_TEST_NSEC)")
        })?;
    Keys::parse(&candidate).context("parse agent api nsec")?;
    Ok(candidate)
}

fn build_nip98_authorization_header(
    nsec: &str,
    method: &reqwest::Method,
    url: &str,
) -> anyhow::Result<String> {
    let keys = Keys::parse(nsec).context("parse Nostr signing key")?;
    let event = EventBuilder::new(Kind::Custom(27235), "")
        .tags([
            Tag::custom(TagKind::custom("u"), [url]),
            Tag::custom(
                TagKind::custom("method"),
                [method.as_str().to_ascii_uppercase()],
            ),
        ])
        .sign_with_keys(&keys)
        .context("sign NIP-98 event")?;
    let payload = serde_json::to_vec(&event).context("serialize NIP-98 event")?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(payload);
    Ok(format!("Nostr {encoded}"))
}

async fn call_agent_api(
    http: &AgentHttpArgs,
    method: reqwest::Method,
    path: &str,
) -> anyhow::Result<serde_json::Value> {
    let (status, body) = call_agent_api_raw(http, method.clone(), path).await?;
    if !status.is_success() {
        return Err(agent_api_http_error(&method, path, status, &body));
    }
    parse_agent_api_response_json(&body, path)
}

async fn call_agent_api_raw(
    http: &AgentHttpArgs,
    method: reqwest::Method,
    path: &str,
) -> anyhow::Result<(reqwest::StatusCode, String)> {
    let base_url = normalized_agent_api_base_url(&http.api_base_url)?;
    let nsec = agent_api_nsec_value(http.nsec.as_deref())?;
    let url = format!("{base_url}{path}");
    let auth = build_nip98_authorization_header(&nsec, &method, &url)?;

    let client = reqwest::Client::builder()
        .connect_timeout(AGENT_API_CONNECT_TIMEOUT)
        .timeout(AGENT_API_TIMEOUT)
        .build()
        .context("build agent api client")?;
    let mut request = client
        .request(method.clone(), &url)
        .header("Authorization", auth)
        .header("Accept", "application/json");
    if method == reqwest::Method::POST {
        request = request
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({}));
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("send {} {}", method, url))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok((status, body))
}

fn parse_agent_api_response_json(body: &str, path: &str) -> anyhow::Result<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(body)
        .with_context(|| format!("decode agent api response for {path}"))
}

fn agent_api_error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|raw| raw.as_str())
                .map(str::to_string)
        })
}

fn agent_api_http_error(
    method: &reqwest::Method,
    path: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> anyhow::Error {
    if let Some(code) = agent_api_error_code(body) {
        anyhow!(
            "agent api {} {} failed: HTTP {} ({code})",
            method,
            path,
            status
        )
    } else {
        anyhow!(
            "agent api {} {} failed: HTTP {} body={}",
            method,
            path,
            status,
            body
        )
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

        let base_url = nostr_sdk::Url::parse(default_primary_blossom_server())
            .context("parse blossom server URL")?;
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

async fn cmd_update_group_profile(
    cli: &Cli,
    group_hex: &str,
    name: Option<&str>,
    about: Option<&str>,
) -> anyhow::Result<()> {
    if name.is_none() && about.is_none() {
        anyhow::bail!("at least one of --name or --about is required");
    }

    let (keys, mdk) = open(cli)?;
    let group = find_group(&mdk, group_hex)?;
    let relays = relay_util::parse_relay_urls(&resolve_relays(cli))?;
    let client = client(cli, &keys).await?;

    // Build metadata JSON.
    let mut metadata = Metadata::new();
    if let Some(n) = name {
        let trimmed = n.trim();
        if !trimmed.is_empty() {
            metadata.name = Some(trimmed.to_string());
            metadata.display_name = Some(trimmed.to_string());
        }
    }
    if let Some(a) = about {
        let trimmed = a.trim();
        if !trimmed.is_empty() {
            metadata.about = Some(trimmed.to_string());
        }
    }

    let metadata_json = serde_json::to_string(&metadata)?;

    // Build kind-0 rumor and encrypt via MLS.
    let rumor = EventBuilder::new(Kind::Metadata, &metadata_json).build(keys.public_key());
    let msg_event = mdk
        .create_message(&group.mls_group_id, rumor)
        .context("create group profile message")?;
    relay_util::publish_and_confirm(&client, &relays, &msg_event, "update_group_profile").await?;
    client.shutdown().await;

    let ngid = hex::encode(group.nostr_group_id);
    print(json!({
        "nostr_group_id": ngid,
        "name": metadata.name,
        "about": metadata.about,
    }));
    Ok(())
}

/// Listen for new incoming messages and welcomes. Prints each as a JSON line to stdout.
async fn listen_for_incoming(
    cli: &Cli,
    timeout_sec: u64,
    lookback_sec: u64,
    expected_sender: Option<PublicKey>,
    expected_group_id_hex: Option<&str>,
    min_created_at: Option<u64>,
) -> anyhow::Result<ListenSummary> {
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
    let mut summary = ListenSummary::default();

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
            let Some(welcome) =
                mdk_util::ingest_welcome_from_giftwrap(&mdk, &keys, &event, |_| true)
                    .await
                    .unwrap_or_default()
            else {
                continue;
            };
            let line = json!({
                "type": "welcome",
                "wrapper_event_id": welcome.wrapper_event_id.to_hex(),
                "from_pubkey": welcome.sender.to_hex(),
                "nostr_group_id": welcome.nostr_group_id_hex,
                "group_name": welcome.group_name,
            });
            println!("{}", serde_json::to_string(&line).unwrap());
            continue;
        }

        // Group message.
        if event.kind == Kind::MlsGroupMessage
            && group_subs.contains_key(&subscription_id)
            && let Ok(Some(msg)) = mdk_util::ingest_application_message(&mdk, &event)
        {
            let Some((ngid, mls_group_id)) = group_subs.get(&subscription_id).cloned() else {
                continue;
            };
            let is_incoming = msg.pubkey != keys.public_key();
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
            let sender_matches = expected_sender.is_none_or(|sender| msg.pubkey == sender);
            let group_matches = expected_group_id_hex.is_none_or(|group_id| ngid == group_id);
            let is_fresh = min_created_at.is_none_or(|min| msg.created_at.as_secs() >= min);
            summary.saw_matching_message |=
                is_incoming && sender_matches && group_matches && is_fresh;
        }
    }

    client.unsubscribe_all().await;
    client.shutdown().await;
    Ok(summary)
}

/// This is the one subcommand that *does* stay running — it's an event tail.
async fn cmd_listen(cli: &Cli, timeout_sec: u64, lookback_sec: u64) -> anyhow::Result<()> {
    let _ = listen_for_incoming(cli, timeout_sec, lookback_sec, None, None, None).await?;
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
    use clap::CommandFactory;

    struct AgentHttpParse {
        api_base_url: String,
        nsec: Option<String>,
    }

    fn parse_agent_new(args: &[&str]) -> AgentHttpParse {
        let cli = Cli::try_parse_from(args).expect("parse args");
        match cli.cmd {
            Command::Agent {
                cmd: AgentCommand::New { http },
            } => AgentHttpParse {
                api_base_url: http.api_base_url,
                nsec: http.nsec,
            },
            _ => panic!("expected agent new command"),
        }
    }

    fn parse_agent_me(args: &[&str]) -> AgentHttpParse {
        let cli = Cli::try_parse_from(args).expect("parse args");
        match cli.cmd {
            Command::Agent {
                cmd: AgentCommand::Me { http },
            } => AgentHttpParse {
                api_base_url: http.api_base_url,
                nsec: http.nsec,
            },
            _ => panic!("expected agent me command"),
        }
    }

    fn event_id(hex: &str) -> EventId {
        EventId::from_hex(hex).expect("valid event id")
    }

    fn pending_welcome(
        wrapper_hex: &str,
        welcome_hex: &str,
    ) -> mdk_storage_traits::welcomes::types::Welcome {
        let welcomer = Keys::generate().public_key();
        let created_at = Timestamp::from(1_u64);
        mdk_storage_traits::welcomes::types::Welcome {
            id: event_id(welcome_hex),
            event: UnsignedEvent::new(
                welcomer,
                created_at,
                Kind::MlsWelcome,
                Tags::new(),
                "{}".to_string(),
            ),
            wrapper_event_id: event_id(wrapper_hex),
            welcomer,
            mls_group_id: GroupId::from_slice(&[1, 2, 3]),
            nostr_group_id: [1; 32],
            group_name: "cli test".to_string(),
            group_description: String::new(),
            group_image_hash: None,
            group_image_key: None,
            group_image_nonce: None,
            group_admin_pubkeys: std::collections::BTreeSet::new(),
            group_relays: std::collections::BTreeSet::new(),
            member_count: 2,
            state: mdk_storage_traits::welcomes::types::WelcomeState::Pending,
        }
    }

    fn make_key_package_event(mdk: &mdk_util::PikaMdk, keys: &Keys) -> Event {
        let relay = RelayUrl::parse("wss://test.relay").expect("relay url");
        let (content, tags, _hash_ref) = mdk
            .create_key_package_for_event(&keys.public_key(), vec![relay])
            .expect("create key package");
        EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign key package")
    }

    #[test]
    fn agent_chat_http_parse() {
        let cli = Cli::try_parse_from([
            "pikachat",
            "agent",
            "chat",
            "--api-base-url",
            "http://127.0.0.1:18080",
            "--nsec",
            "nsec1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqfu2v9v",
            "--listen-timeout",
            "9",
            "--poll-attempts",
            "7",
            "--poll-delay-sec",
            "3",
            "--recover-after-attempt",
            "4",
            "hello",
        ])
        .expect("parse args");
        match cli.cmd {
            Command::Agent {
                cmd:
                    AgentCommand::Chat {
                        http,
                        message,
                        listen_timeout,
                        poll_attempts,
                        poll_delay_sec,
                        recover_after_attempt,
                    },
            } => {
                assert_eq!(http.api_base_url, "http://127.0.0.1:18080");
                assert_eq!(
                    http.nsec.as_deref(),
                    Some("nsec1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqfu2v9v")
                );
                assert_eq!(message, "hello");
                assert_eq!(listen_timeout, 9);
                assert_eq!(poll_attempts, 7);
                assert_eq!(poll_delay_sec, 3);
                assert_eq!(recover_after_attempt, 4);
            }
            _ => panic!("expected agent chat command"),
        }
    }

    #[test]
    fn agent_new_http_parse() {
        let parsed = parse_agent_new(&[
            "pikachat",
            "agent",
            "new",
            "--api-base-url",
            "http://127.0.0.1:18080",
            "--nsec",
            "nsec1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqfu2v9v",
        ]);
        assert_eq!(parsed.api_base_url, "http://127.0.0.1:18080");
        assert!(parsed.nsec.is_some());
    }

    #[test]
    fn agent_new_defaults_base_url() {
        let parsed = parse_agent_new(&[
            "pikachat",
            "agent",
            "new",
            "--nsec",
            "nsec1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqfu2v9v",
        ]);
        assert_eq!(parsed.api_base_url, DEFAULT_AGENT_API_BASE_URL);
        assert!(parsed.nsec.is_some());
    }

    #[test]
    fn agent_me_http_parse() {
        let parsed = parse_agent_me(&[
            "pikachat",
            "agent",
            "me",
            "--api-base-url",
            "http://127.0.0.1:18080",
            "--nsec",
            "nsec1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqfu2v9v",
        ]);
        assert_eq!(parsed.api_base_url, "http://127.0.0.1:18080");
        assert!(parsed.nsec.is_some());
    }

    #[test]
    fn legacy_agent_runtime_subcommands_are_removed() {
        for legacy in ["list-runtimes", "get-runtime", "teardown"] {
            let err = Cli::try_parse_from(["pikachat", "agent", legacy])
                .expect_err("legacy runtime command should fail");
            assert!(err.to_string().contains("unrecognized subcommand"));
        }
    }

    #[test]
    fn agent_api_error_code_extracts_json_error() {
        let body = r#"{"error":"agent_exists"}"#;
        assert_eq!(agent_api_error_code(body).as_deref(), Some("agent_exists"));
    }

    #[test]
    fn agent_api_error_code_handles_non_json_payload() {
        assert!(agent_api_error_code("not-json").is_none());
    }

    #[test]
    fn parse_agent_fields_extracts_required_keys() {
        let payload = json!({
            "agent_id": "npub1test",
            "state": "creating"
        });
        let (agent_id, state) = parse_agent_fields(&payload).expect("extract fields");
        assert_eq!(agent_id, "npub1test");
        assert_eq!(state, "creating");
    }

    #[test]
    fn finish_chat_send_reports_no_reply_timeout() {
        let err = finish_chat_send(ChatSendOutcome::NoReplyWithinTimeout, 12)
            .expect_err("no reply must be an error");
        assert!(err.to_string().contains("no_reply_within_timeout"));
        assert!(err.to_string().contains("12s"));
    }

    #[test]
    fn agent_chat_help_mentions_no_reply_timeout() {
        let mut command = Cli::command();
        let agent = command
            .find_subcommand_mut("agent")
            .expect("agent subcommand");
        let chat = agent.find_subcommand_mut("chat").expect("chat subcommand");
        let mut help = Vec::new();
        chat.write_long_help(&mut help).expect("render help");
        let help = String::from_utf8(help).expect("utf8 help");
        assert!(help.contains("no_reply_within_timeout"));
        assert!(help.contains("--listen-timeout"));
    }

    #[test]
    fn accept_welcome_lookup_uses_shared_wrapper_or_welcome_match_rules() {
        let pending = vec![
            pending_welcome(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
            pending_welcome(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            ),
        ];

        let by_wrapper = find_pending_welcome_for_accept(&pending, &pending[0].wrapper_event_id)
            .expect("match wrapper id");
        assert_eq!(by_wrapper.wrapper_event_id, pending[0].wrapper_event_id);

        let by_welcome =
            find_pending_welcome_for_accept(&pending, &pending[1].id).expect("match welcome id");
        assert_eq!(by_welcome.id, pending[1].id);

        assert!(
            find_pending_welcome_for_accept(
                &pending,
                &event_id("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn invite_create_group_uses_shared_runtime_helper() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = mdk_util::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = mdk_util::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let relays = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let peer_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "CLI test".to_string(),
            String::new(),
            None,
            None,
            None,
            relays.clone(),
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let published = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Event>::new()));
        let published_capture = std::sync::Arc::clone(&published);

        let created = create_group_and_publish_welcomes_for_invite_with_publisher(
            &inviter_keys,
            &inviter_mdk,
            peer_kp,
            invitee_keys.public_key(),
            config,
            move |_receiver, giftwrap| {
                let published_capture = std::sync::Arc::clone(&published_capture);
                async move {
                    published_capture
                        .lock()
                        .expect("published lock")
                        .push(giftwrap);
                    Ok(())
                }
            },
        )
        .await
        .expect("create group and publish welcomes");

        assert_eq!(created.group.name, "CLI test");
        assert_eq!(created.published_welcomes.len(), 1);
        assert_eq!(
            created.published_welcomes[0].receiver,
            invitee_keys.public_key()
        );
        assert_eq!(
            created.published_welcomes[0].wrapper_event_id,
            published.lock().expect("published lock")[0].id
        );
    }
}
