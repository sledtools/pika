mod agent;
mod harness;
mod mdk_util;
mod relay_util;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow};
use clap::{Args, Parser, Subcommand, ValueEnum};
use hypernote_protocol as hn;
use mdk_core::encrypted_media::types::{MediaProcessingOptions, MediaReference};
use mdk_core::prelude::*;
use nostr_blossom::client::BlossomClient;
use nostr_sdk::prelude::*;
use pika_agent_control_plane::{
    AgentControlCmdEnvelope, AgentControlCommand, AgentControlErrorEnvelope,
    AgentControlResultEnvelope, AgentControlStatusEnvelope, AuthContext, CONTROL_CMD_KIND,
    CONTROL_ERROR_KIND, CONTROL_RESULT_KIND, CONTROL_STATUS_KIND, GetRuntimeCommand,
    ListRuntimesCommand, MicrovmProvisionParams, ProtocolKind, ProviderKind, ProvisionCommand,
    RuntimeLifecyclePhase, TeardownCommand,
};
use pika_agent_microvm::microvm_params_provided;
use pika_relay_profiles::{
    app_default_message_relays, default_key_package_relays, default_primary_blossom_server,
};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::agent::harness::AgentProtocol;

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

    /// Relay websocket URLs (default: relay.damus.io, relay.primal.net, nos.lol, us-east.nostr.pikachat.org, eu.nostr.pikachat.org)
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

        /// Blossom server URL (repeatable; defaults to https://us-east.nostr.pikachat.org)
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

    /// Manage AI agents (`fly` or `microvm`)
    Agent {
        #[command(subcommand)]
        cmd: AgentCommand,
    },
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// Create a new ACP agent runtime
    New {
        /// Agent name (default: agent-<random>)
        #[arg(long)]
        name: Option<String>,

        /// Runtime provider (`fly` keeps existing behavior)
        #[arg(long, value_enum, default_value_t = AgentProvider::Fly, env = "PIKA_AGENT_PROVIDER")]
        provider: AgentProvider,

        /// Target runtime class advertised by the server (optional routing hint)
        #[arg(long, env = "PIKA_AGENT_RUNTIME_CLASS")]
        runtime_class: Option<String>,

        /// Deprecated: provisioning is ACP-only; `--brain` no longer changes behavior.
        #[arg(long, hide = true)]
        brain: Option<String>,

        /// Print provision result as JSON and exit (no interactive chat)
        #[arg(long)]
        json: bool,

        /// Keep runtime alive after CLI exit (skip auto-teardown)
        #[arg(long)]
        keep: bool,

        #[command(flatten)]
        control: AgentControlArgs,

        #[command(flatten)]
        microvm: AgentNewMicrovmArgs,
    },

    /// List runtimes known to the control-plane server (filterable)
    ListRuntimes {
        /// Filter by provider
        #[arg(long, value_enum)]
        provider: Option<AgentProvider>,

        /// Filter by protocol compatibility
        #[arg(long, value_enum)]
        protocol: Option<AgentProtocol>,

        /// Filter by runtime lifecycle phase
        #[arg(long, value_enum)]
        phase: Option<AgentRuntimePhase>,

        /// Filter by runtime class
        #[arg(long)]
        runtime_class: Option<String>,

        /// Maximum number of runtimes to return
        #[arg(long)]
        limit: Option<usize>,

        #[command(flatten)]
        control: AgentControlArgs,
    },

    /// Fetch one runtime descriptor by id
    GetRuntime {
        /// Runtime id from provision/list results
        #[arg(long)]
        runtime_id: String,

        #[command(flatten)]
        control: AgentControlArgs,
    },

    /// Tear down a runtime by id
    Teardown {
        /// Runtime id to tear down
        #[arg(long)]
        runtime_id: String,

        #[command(flatten)]
        control: AgentControlArgs,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum AgentProvider {
    Fly,
    Microvm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum AgentRuntimePhase {
    Queued,
    Provisioning,
    Ready,
    Failed,
    Teardown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum AgentControlMode {
    Remote,
}

#[derive(Clone, Debug, Args)]
struct AgentControlArgs {
    /// Provider control-plane mode (`remote` only; local provisioning is removed)
    #[arg(long, value_enum, default_value_t = AgentControlMode::Remote, env = "PIKA_AGENT_CONTROL_MODE")]
    control_mode: AgentControlMode,

    /// Nostr pubkey (hex or npub) for the `pika-server` control-plane identity
    #[arg(long, env = "PIKA_AGENT_CONTROL_SERVER_PUBKEY")]
    control_server_pubkey: Option<String>,
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
                runtime_class,
                brain,
                json,
                keep,
                control,
                microvm,
            } => {
                let request = AgentNewRequest {
                    name: name.as_deref(),
                    provider: *provider,
                    runtime_class: runtime_class.as_deref(),
                    brain: brain.as_deref(),
                    json: *json,
                    keep: *keep,
                    microvm,
                };
                cmd_agent_new(&cli, control, request).await
            }
            AgentCommand::ListRuntimes {
                provider,
                protocol,
                phase,
                runtime_class,
                limit,
                control,
            } => {
                cmd_agent_list_runtimes(
                    &cli,
                    *provider,
                    *protocol,
                    *phase,
                    runtime_class.as_deref(),
                    *limit,
                    control,
                )
                .await
            }
            AgentCommand::GetRuntime {
                runtime_id,
                control,
            } => cmd_agent_get_runtime(&cli, runtime_id, control).await,
            AgentCommand::Teardown {
                runtime_id,
                control,
            } => cmd_agent_teardown(&cli, runtime_id, control).await,
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
        app_default_message_relays()
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

async fn cmd_agent_new(
    cli: &Cli,
    control: &AgentControlArgs,
    request: AgentNewRequest<'_>,
) -> anyhow::Result<()> {
    validate_agent_new_request(request.provider, request.brain, request.microvm)?;
    if control.control_mode != AgentControlMode::Remote {
        anyhow::bail!("--control-mode remote is required; local provisioning has been removed");
    }
    cmd_agent_new_remote(cli, control, &request).await
}

struct AgentNewRequest<'a> {
    name: Option<&'a str>,
    provider: AgentProvider,
    runtime_class: Option<&'a str>,
    brain: Option<&'a str>,
    json: bool,
    keep: bool,
    microvm: &'a AgentNewMicrovmArgs,
}

fn validate_agent_new_request(
    provider: AgentProvider,
    brain: Option<&str>,
    microvm: &AgentNewMicrovmArgs,
) -> anyhow::Result<()> {
    if let Some(value) = brain.map(str::trim).filter(|v| !v.is_empty()) {
        anyhow::bail!(
            "--brain is no longer supported: provisioning is ACP-only. Remove --brain (received: {value})"
        );
    }
    microvm.ensure_provider_compatible(provider)?;
    Ok(())
}

struct RemoteControlClient {
    client: Client,
    keys: Keys,
    relays: Vec<RelayUrl>,
    server_pubkey: PublicKey,
}

impl RemoteControlClient {
    async fn connect(
        keys: &Keys,
        relay_urls: &[String],
        server_pubkey: PublicKey,
    ) -> anyhow::Result<Self> {
        let relays = relay_util::parse_relay_urls(relay_urls)?;
        let client = relay_util::connect_client(keys, relay_urls).await?;
        Ok(Self {
            client,
            keys: keys.clone(),
            relays,
            server_pubkey,
        })
    }

    async fn send_command(
        &self,
        command: AgentControlCommand,
    ) -> anyhow::Result<AgentControlResultEnvelope> {
        let request_id = new_control_request_id("agent-ctl");
        let idempotency_key = new_control_request_id("idem");
        let envelope = AgentControlCmdEnvelope::v1(
            request_id.clone(),
            idempotency_key,
            command,
            AuthContext {
                acting_as_pubkey: Some(self.keys.public_key().to_hex()),
            },
        );

        let status_sub = self
            .client
            .subscribe(
                Filter::new()
                    .author(self.server_pubkey)
                    .kind(Kind::Custom(CONTROL_STATUS_KIND))
                    .custom_tag(
                        SingleLetterTag::lowercase(Alphabet::P),
                        self.keys.public_key().to_hex(),
                    )
                    .since(Timestamp::now()),
                None,
            )
            .await?;
        let result_sub = self
            .client
            .subscribe(
                Filter::new()
                    .author(self.server_pubkey)
                    .kind(Kind::Custom(CONTROL_RESULT_KIND))
                    .custom_tag(
                        SingleLetterTag::lowercase(Alphabet::P),
                        self.keys.public_key().to_hex(),
                    )
                    .since(Timestamp::now()),
                None,
            )
            .await?;
        let error_sub = self
            .client
            .subscribe(
                Filter::new()
                    .author(self.server_pubkey)
                    .kind(Kind::Custom(CONTROL_ERROR_KIND))
                    .custom_tag(
                        SingleLetterTag::lowercase(Alphabet::P),
                        self.keys.public_key().to_hex(),
                    )
                    .since(Timestamp::now()),
                None,
            )
            .await?;

        let content = serde_json::to_string(&envelope).context("encode control command")?;
        let encrypted = nostr_sdk::nostr::nips::nip44::encrypt(
            self.keys.secret_key(),
            &self.server_pubkey,
            content,
            nostr_sdk::nostr::nips::nip44::Version::V2,
        )
        .context("encrypt control command payload")?;
        let cmd_event = EventBuilder::new(Kind::Custom(CONTROL_CMD_KIND), encrypted)
            .tags([Tag::public_key(self.server_pubkey)])
            .sign_with_keys(&self.keys)
            .context("sign control command")?;
        let mut publish_error = None;
        for attempt in 1..=12 {
            match relay_util::publish_and_confirm(
                &self.client,
                &self.relays,
                &cmd_event,
                "agent control cmd",
            )
            .await
            {
                Ok(()) => {
                    publish_error = None;
                    break;
                }
                Err(err) => {
                    let message = err.to_string();
                    if attempt < 12
                        && (message.contains("relay not connected")
                            || message.contains("timeout")
                            || message.contains("Connection refused")
                            || message.contains("no one was listening"))
                    {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        continue;
                    }
                    publish_error = Some(err);
                    break;
                }
            }
        }
        if let Some(err) = publish_error {
            return Err(err);
        }

        let mut rx = self.client.notifications();
        let timeout = Duration::from_secs(180);
        let started = tokio::time::Instant::now();
        let mut seen = HashSet::<EventId>::new();
        loop {
            if started.elapsed() > timeout {
                self.client.unsubscribe_all().await;
                anyhow::bail!("timed out waiting for remote control reply ({request_id})");
            }

            let notification = match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                Ok(Ok(notification)) => notification,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(_)) => {
                    self.client.unsubscribe_all().await;
                    anyhow::bail!("remote control notification channel closed");
                }
                Err(_) => continue,
            };

            let RelayPoolNotification::Event {
                subscription_id,
                event,
                ..
            } = notification
            else {
                continue;
            };
            if subscription_id != status_sub.val
                && subscription_id != result_sub.val
                && subscription_id != error_sub.val
            {
                continue;
            }

            let event = *event;
            if !seen.insert(event.id) {
                continue;
            }
            let decrypted = match nostr_sdk::nostr::nips::nip44::decrypt(
                self.keys.secret_key(),
                &self.server_pubkey,
                event.content.as_str(),
            ) {
                Ok(content) => content,
                Err(_) => continue,
            };

            if event.kind == Kind::Custom(CONTROL_STATUS_KIND) {
                if let Ok(status) = serde_json::from_str::<AgentControlStatusEnvelope>(&decrypted)
                    && status.request_id == request_id
                {
                    render_control_status(&status);
                }
                continue;
            }

            if event.kind == Kind::Custom(CONTROL_RESULT_KIND) {
                if let Ok(result) = serde_json::from_str::<AgentControlResultEnvelope>(&decrypted)
                    && result.request_id == request_id
                {
                    self.client.unsubscribe_all().await;
                    return Ok(result);
                }
                continue;
            }

            if event.kind == Kind::Custom(CONTROL_ERROR_KIND)
                && let Ok(err) = serde_json::from_str::<AgentControlErrorEnvelope>(&decrypted)
                && err.request_id == request_id
            {
                self.client.unsubscribe_all().await;
                let hint = err.hint.unwrap_or_default();
                let detail = err.detail.unwrap_or_default();
                anyhow::bail!("remote control error {}: {} {}", err.code, hint, detail);
            }
        }
    }
}

fn render_control_status(status: &AgentControlStatusEnvelope) {
    let phase = match status.phase {
        RuntimeLifecyclePhase::Queued => "queued",
        RuntimeLifecyclePhase::Provisioning => "provisioning",
        RuntimeLifecyclePhase::Ready => "ready",
        RuntimeLifecyclePhase::Failed => "failed",
        RuntimeLifecyclePhase::Teardown => "teardown",
    };
    if let Some(msg) = &status.message {
        eprintln!("[control] {phase}: {msg}");
    } else {
        eprintln!("[control] {phase}");
    }
}

fn resolve_control_server_pubkey(control: &AgentControlArgs) -> anyhow::Result<PublicKey> {
    let raw = control
        .control_server_pubkey
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "missing control server pubkey (use --control-server-pubkey or PIKA_AGENT_CONTROL_SERVER_PUBKEY)"
            )
        })?;
    PublicKey::parse(raw).context("parse control server pubkey")
}

fn map_provider_kind(provider: AgentProvider) -> ProviderKind {
    match provider {
        AgentProvider::Fly => ProviderKind::Fly,
        AgentProvider::Microvm => ProviderKind::Microvm,
    }
}

fn map_protocol_kind(_protocol: AgentProtocol) -> ProtocolKind {
    ProtocolKind::Acp
}

fn map_microvm_control_params(microvm: &AgentNewMicrovmArgs) -> Option<MicrovmProvisionParams> {
    let spawn_variant = microvm.spawn_variant.map(|variant| match variant {
        MicrovmSpawnVariant::Prebuilt => "prebuilt".to_string(),
        MicrovmSpawnVariant::PrebuiltCow => "prebuilt-cow".to_string(),
    });
    let params = MicrovmProvisionParams {
        spawner_url: microvm.spawner_url.clone(),
        spawn_variant,
        flake_ref: microvm.flake_ref.clone(),
        dev_shell: microvm.dev_shell.clone(),
        cpu: microvm.cpu,
        memory_mb: microvm.memory_mb,
        ttl_seconds: microvm.ttl_seconds,
    };
    if microvm_params_provided(&params) {
        Some(params)
    } else {
        None
    }
}

fn new_control_request_id(prefix: &str) -> String {
    format!(
        "{prefix}-{:08x}{:08x}",
        rand::random::<u32>(),
        rand::random::<u32>()
    )
}

async fn cmd_agent_new_remote(
    cli: &Cli,
    control: &AgentControlArgs,
    request: &AgentNewRequest<'_>,
) -> anyhow::Result<()> {
    let name = request.name;
    let provider = request.provider;
    let runtime_class = request.runtime_class;
    let json_mode = request.json;
    let keep = request.keep;
    let microvm = request.microvm;

    let server_pubkey = resolve_control_server_pubkey(control)?;
    let relay_urls = resolve_relays(cli);
    let kp_relay_urls = resolve_kp_relays(cli);
    let (keys, mdk) = open(cli)?;
    eprintln!("Your pubkey: {}", keys.public_key().to_hex());

    let control_client = RemoteControlClient::connect(&keys, &relay_urls, server_pubkey).await?;
    let provision_result = control_client
        .send_command(AgentControlCommand::Provision(ProvisionCommand {
            provider: map_provider_kind(provider),
            protocol: map_protocol_kind(AgentProtocol::Acp),
            name: name.map(str::to_string),
            runtime_class: runtime_class
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string),
            relay_urls: relay_urls.clone(),
            keep,
            bot_secret_key_hex: None,
            microvm: if provider == AgentProvider::Microvm {
                map_microvm_control_params(microvm)
            } else {
                None
            },
        }))
        .await?;
    let runtime = provision_result.runtime;
    let runtime_id = runtime.runtime_id.clone();

    // --json mode: print provision result and exit (no interactive chat, no teardown)
    if json_mode {
        control_client.client.unsubscribe_all().await;
        control_client.client.shutdown().await;
        print(json!({
            "provider": match provider {
                AgentProvider::Fly => "fly",
                AgentProvider::Microvm => "microvm",
            },
            "protocol": "acp",
            "runtime_id": runtime.runtime_id,
            "runtime_class": runtime.runtime_class,
            "runtime": runtime,
            "payload": provision_result.payload,
        }));
        return Ok(());
    }

    // Interactive mode: setup chat, run loop, teardown on exit or setup failure.
    // All fallible steps after provisioning are wrapped so teardown always runs.
    let session_result = run_interactive_session(
        &keys,
        &mdk,
        &control_client,
        &relay_urls,
        &kp_relay_urls,
        &runtime,
    )
    .await;

    // Best-effort teardown unless --keep (runs on success, chat exit, AND setup failures)
    if keep {
        eprintln!("\n--keep: runtime {runtime_id} left alive.");
    } else {
        eprintln!("\nTearing down runtime {runtime_id}...");
        match control_client
            .send_command(AgentControlCommand::Teardown(TeardownCommand {
                runtime_id: runtime_id.clone(),
            }))
            .await
        {
            Ok(_) => eprintln!("Runtime {runtime_id} torn down."),
            Err(err) => {
                eprintln!("Teardown failed: {err:#}");
                eprintln!("Manual cleanup: pikachat agent teardown --runtime-id {runtime_id}");
            }
        }
    }

    control_client.client.unsubscribe_all().await;
    control_client.client.shutdown().await;

    session_result
}

async fn run_interactive_session(
    keys: &Keys,
    mdk: &mdk_util::PikaMdk,
    control_client: &RemoteControlClient,
    relay_urls: &[String],
    kp_relay_urls: &[String],
    runtime: &pika_agent_control_plane::RuntimeDescriptor,
) -> anyhow::Result<()> {
    let bot_pubkey_hex = runtime
        .bot_pubkey
        .as_deref()
        .ok_or_else(|| anyhow!("runtime did not return a bot_pubkey; cannot open chat"))?;
    let bot_pubkey =
        PublicKey::parse(bot_pubkey_hex).context("parse bot pubkey from provision result")?;

    eprintln!(
        "Runtime {} provisioned. Connecting to chat...",
        runtime.runtime_id
    );

    let relays = relay_util::parse_relay_urls(relay_urls)?;
    let kp_relays = relay_util::parse_relay_urls(kp_relay_urls)?;

    let kp_plan = agent::provider::KeyPackageWaitPlan {
        progress_message: "Waiting for bot key package...",
        timeout: Duration::from_secs(120),
        fetch_timeout: Duration::from_secs(5),
        retry_delay: Duration::from_secs(2),
    };
    let bot_kp = tokio::select! {
        result = agent::session::wait_for_latest_key_package(
            &control_client.client,
            bot_pubkey,
            &kp_relays,
            kp_plan,
        ) => result?,
        _ = tokio::signal::ctrl_c() => {
            anyhow::bail!("interrupted during key package wait");
        }
    };

    let group_plan = agent::provider::GroupCreatePlan {
        progress_message: "Creating MLS group...",
        create_group_context: "create MLS group for agent chat",
        build_welcome_context: "build welcome message",
        welcome_publish_label: "agent welcome",
    };
    let group = tokio::select! {
        result = agent::session::create_group_and_publish_welcomes(
            keys,
            mdk,
            &control_client.client,
            &relays,
            bot_kp,
            bot_pubkey,
            group_plan,
        ) => result?,
        _ = tokio::signal::ctrl_c() => {
            anyhow::bail!("interrupted during group creation");
        }
    };

    eprintln!("Chat ready. Type a message and press Enter. Ctrl-C to exit.");

    let chat_plan = agent::provider::ChatLoopPlan {
        outbound_publish_label: "agent chat msg",
        wait_for_pending_replies_on_eof: false,
        eof_reply_timeout: Duration::from_secs(30),
        projection_mode: pika_agent_protocol::projection::ProjectionMode::Chat,
    };
    agent::session::run_interactive_chat_loop(agent::session::ChatLoopContext {
        keys,
        mdk,
        send_client: &control_client.client,
        listen_client: &control_client.client,
        relays: &relays,
        bot_pubkey,
        mls_group_id: &group.mls_group_id,
        nostr_group_id_hex: &group.nostr_group_id_hex,
        plan: chat_plan,
        seen_mls_event_ids: None,
    })
    .await
}

fn map_agent_runtime_phase(phase: AgentRuntimePhase) -> RuntimeLifecyclePhase {
    match phase {
        AgentRuntimePhase::Queued => RuntimeLifecyclePhase::Queued,
        AgentRuntimePhase::Provisioning => RuntimeLifecyclePhase::Provisioning,
        AgentRuntimePhase::Ready => RuntimeLifecyclePhase::Ready,
        AgentRuntimePhase::Failed => RuntimeLifecyclePhase::Failed,
        AgentRuntimePhase::Teardown => RuntimeLifecyclePhase::Teardown,
    }
}

async fn cmd_agent_list_runtimes(
    cli: &Cli,
    provider: Option<AgentProvider>,
    protocol: Option<AgentProtocol>,
    phase: Option<AgentRuntimePhase>,
    runtime_class: Option<&str>,
    limit: Option<usize>,
    control: &AgentControlArgs,
) -> anyhow::Result<()> {
    let server_pubkey = resolve_control_server_pubkey(control)?;
    let relay_urls = resolve_relays(cli);
    let (keys, _mdk) = open(cli)?;
    let control_client = RemoteControlClient::connect(&keys, &relay_urls, server_pubkey).await?;
    let result = control_client
        .send_command(AgentControlCommand::ListRuntimes(ListRuntimesCommand {
            provider: provider.map(map_provider_kind),
            protocol: protocol.map(map_protocol_kind),
            lifecycle_phase: phase.map(map_agent_runtime_phase),
            runtime_class: runtime_class
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string),
            limit,
        }))
        .await?;
    control_client.client.unsubscribe_all().await;
    control_client.client.shutdown().await;
    print(json!({
        "operation": "list_runtimes",
        "count": result.payload.get("count").cloned().unwrap_or_else(|| json!(0)),
        "runtimes": result.payload.get("runtimes").cloned().unwrap_or_else(|| json!([])),
    }));
    Ok(())
}

async fn cmd_agent_get_runtime(
    cli: &Cli,
    runtime_id: &str,
    control: &AgentControlArgs,
) -> anyhow::Result<()> {
    let server_pubkey = resolve_control_server_pubkey(control)?;
    let relay_urls = resolve_relays(cli);
    let (keys, _mdk) = open(cli)?;
    let control_client = RemoteControlClient::connect(&keys, &relay_urls, server_pubkey).await?;
    let result = control_client
        .send_command(AgentControlCommand::GetRuntime(GetRuntimeCommand {
            runtime_id: runtime_id.to_string(),
        }))
        .await?;
    control_client.client.unsubscribe_all().await;
    control_client.client.shutdown().await;
    print(json!({
        "operation": "get_runtime",
        "runtime": result.runtime,
        "payload": result.payload,
    }));
    Ok(())
}

async fn cmd_agent_teardown(
    cli: &Cli,
    runtime_id: &str,
    control: &AgentControlArgs,
) -> anyhow::Result<()> {
    let server_pubkey = resolve_control_server_pubkey(control)?;
    let relay_urls = resolve_relays(cli);
    let (keys, _mdk) = open(cli)?;
    let control_client = RemoteControlClient::connect(&keys, &relay_urls, server_pubkey).await?;
    let result = control_client
        .send_command(AgentControlCommand::Teardown(TeardownCommand {
            runtime_id: runtime_id.to_string(),
        }))
        .await?;
    control_client.client.unsubscribe_all().await;
    control_client.client.shutdown().await;
    print(json!({
        "operation": "teardown",
        "runtime": result.runtime,
        "payload": result.payload,
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

    struct AgentListRuntimesParse {
        provider: Option<AgentProvider>,
        protocol: Option<AgentProtocol>,
        phase: Option<AgentRuntimePhase>,
        runtime_class: Option<String>,
        limit: Option<usize>,
    }

    struct AgentNewParse {
        provider: AgentProvider,
        runtime_class: Option<String>,
        json: bool,
        keep: bool,
        microvm: AgentNewMicrovmArgs,
    }

    fn parse_agent_new(args: &[&str]) -> AgentNewParse {
        let cli = Cli::try_parse_from(args).expect("parse args");
        match cli.cmd {
            Command::Agent {
                cmd:
                    AgentCommand::New {
                        provider,
                        runtime_class,
                        json,
                        keep,
                        microvm,
                        ..
                    },
            } => AgentNewParse {
                provider,
                runtime_class,
                json,
                keep,
                microvm,
            },
            _ => panic!("expected agent new command"),
        }
    }

    fn validate_agent_new_args(args: &[&str]) -> anyhow::Result<()> {
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
            } => validate_agent_new_request(provider, brain.as_deref(), &microvm),
            _ => panic!("expected agent new command"),
        }
    }

    fn parse_agent_new_control_mode(args: &[&str]) -> AgentControlMode {
        let cli = Cli::try_parse_from(args).expect("parse args");
        match cli.cmd {
            Command::Agent {
                cmd: AgentCommand::New { control, .. },
            } => control.control_mode,
            _ => panic!("expected agent new command"),
        }
    }

    fn parse_agent_list_runtimes(args: &[&str]) -> AgentListRuntimesParse {
        let cli = Cli::try_parse_from(args).expect("parse args");
        match cli.cmd {
            Command::Agent {
                cmd:
                    AgentCommand::ListRuntimes {
                        provider,
                        protocol,
                        phase,
                        runtime_class,
                        limit,
                        ..
                    },
            } => AgentListRuntimesParse {
                provider,
                protocol,
                phase,
                runtime_class,
                limit,
            },
            _ => panic!("expected agent list-runtimes command"),
        }
    }

    fn parse_agent_get_runtime(args: &[&str]) -> String {
        let cli = Cli::try_parse_from(args).expect("parse args");
        match cli.cmd {
            Command::Agent {
                cmd: AgentCommand::GetRuntime { runtime_id, .. },
            } => runtime_id,
            _ => panic!("expected agent get-runtime command"),
        }
    }

    #[test]
    fn agent_new_microvm_flags_parse() {
        let parsed = parse_agent_new(&[
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
        assert_eq!(parsed.provider, AgentProvider::Microvm);
        assert_eq!(parsed.runtime_class, None);
        assert!(parsed.keep);
        assert!(!parsed.json);
        assert_eq!(
            parsed.microvm.spawner_url.as_deref(),
            Some("http://127.0.0.1:8080")
        );
        assert_eq!(
            parsed.microvm.spawn_variant,
            Some(MicrovmSpawnVariant::PrebuiltCow)
        );
        assert_eq!(parsed.microvm.flake_ref.as_deref(), Some(".#nixpi"));
        assert_eq!(parsed.microvm.dev_shell.as_deref(), Some("default"));
        assert_eq!(parsed.microvm.cpu, Some(1));
        assert_eq!(parsed.microvm.memory_mb, Some(1024));
        assert_eq!(parsed.microvm.ttl_seconds, Some(600));
    }

    #[test]
    fn agent_new_existing_fly_parse_unchanged() {
        let parsed = parse_agent_new(&["pikachat", "agent", "new", "--provider", "fly"]);
        assert_eq!(parsed.provider, AgentProvider::Fly);
        assert_eq!(parsed.runtime_class, None);
        assert!(!parsed.keep);
        assert!(!parsed.json);
        assert!(parsed.microvm.provided_flag_names().is_empty());
    }

    #[test]
    fn agent_new_json_flag_parse() {
        let parsed = parse_agent_new(&["pikachat", "agent", "new", "--provider", "fly", "--json"]);
        assert!(parsed.json);
        assert!(!parsed.keep);
    }

    #[test]
    fn microvm_flags_rejected_for_non_microvm_provider() {
        let parsed = parse_agent_new(&[
            "pikachat",
            "agent",
            "new",
            "--provider",
            "fly",
            "--spawner-url",
            "http://127.0.0.1:8080",
        ]);
        let err = validate_agent_new_request(parsed.provider, None, &parsed.microvm)
            .expect_err("should fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("--spawner-url"));
        assert!(msg.contains("--provider microvm"));
    }

    #[test]
    fn agent_new_brain_flag_is_explicitly_rejected() {
        for value in ["pi", "acp"] {
            let err = validate_agent_new_args(&["pikachat", "agent", "new", "--brain", value])
                .expect_err("brain flag should be rejected");
            let msg = format!("{err:#}");
            assert!(msg.contains("--brain"));
            assert!(msg.contains("ACP-only"));
        }
    }

    #[test]
    fn agent_new_control_mode_is_strict_remote_only() {
        let mode = parse_agent_new_control_mode(&["pikachat", "agent", "new"]);
        assert_eq!(mode, AgentControlMode::Remote);

        for invalid in ["auto", "local"] {
            let err = Cli::try_parse_from(["pikachat", "agent", "new", "--control-mode", invalid])
                .expect_err("legacy control mode should fail");
            let msg = err.to_string();
            assert!(msg.contains("invalid value"));
        }
    }

    #[test]
    fn agent_new_runtime_class_parse() {
        let parsed = parse_agent_new(&[
            "pikachat",
            "agent",
            "new",
            "--provider",
            "fly",
            "--runtime-class",
            "fly-us-east",
        ]);
        assert_eq!(parsed.runtime_class.as_deref(), Some("fly-us-east"));
    }

    #[test]
    fn agent_list_runtimes_parse() {
        let parsed = parse_agent_list_runtimes(&[
            "pikachat",
            "agent",
            "list-runtimes",
            "--provider",
            "fly",
            "--protocol",
            "acp",
            "--phase",
            "ready",
            "--runtime-class",
            "fly-us-east",
            "--limit",
            "5",
        ]);
        assert_eq!(parsed.provider, Some(AgentProvider::Fly));
        assert_eq!(parsed.protocol, Some(AgentProtocol::Acp));
        assert_eq!(parsed.phase, Some(AgentRuntimePhase::Ready));
        assert_eq!(parsed.runtime_class.as_deref(), Some("fly-us-east"));
        assert_eq!(parsed.limit, Some(5));
    }

    #[test]
    fn agent_get_runtime_parse() {
        let runtime_id = parse_agent_get_runtime(&[
            "pikachat",
            "agent",
            "get-runtime",
            "--runtime-id",
            "runtime-123",
        ]);
        assert_eq!(runtime_id, "runtime-123");
    }

    #[test]
    fn agent_teardown_parse() {
        let cli = Cli::try_parse_from([
            "pikachat",
            "agent",
            "teardown",
            "--runtime-id",
            "fly-abc123",
        ])
        .expect("parse args");
        match cli.cmd {
            Command::Agent {
                cmd: AgentCommand::Teardown { runtime_id, .. },
            } => assert_eq!(runtime_id, "fly-abc123"),
            _ => panic!("expected agent teardown command"),
        }
    }
}
