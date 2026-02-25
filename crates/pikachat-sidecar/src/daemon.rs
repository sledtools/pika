use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, anyhow};
use mdk_core::encrypted_media::crypto::{DEFAULT_SCHEME_VERSION, derive_encryption_key};
use mdk_core::encrypted_media::types::{MediaProcessingOptions, MediaReference};
use mdk_core::prelude::*;
use mdk_sqlite_storage::MdkSqliteStorage;
use nostr_blossom::client::BlossomClient;
use nostr_sdk::hashes::{Hash as _, sha256};
use nostr_sdk::prelude::*;
use pika_media::codec_opus::{OpusCodec, OpusPacket};
use pika_media::crypto::{
    FrameInfo, FrameKeyMaterial, decrypt_frame, encrypt_frame, opaque_participant_label,
};
use pika_media::network::NetworkRelay;
use pika_media::session::{
    InMemoryRelay, MediaFrame, MediaSession, MediaSessionError, SessionConfig,
};
use pika_media::tracks::{TrackAddress, broadcast_path};
use pika_relay_profiles::default_primary_blossom_server;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::warn;

use crate::call_audio::OpusToAudioPipeline;
use crate::call_tts::synthesize_tts_pcm;

const PROTOCOL_VERSION: u32 = 1;
const RELAY_AUTH_CAP_PREFIX: &str = "capv1_";
const RELAY_AUTH_HEX_LEN: usize = 64;

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum InCmd {
    PublishKeypackage {
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default)]
        relays: Vec<String>,
    },
    SetRelays {
        #[serde(default)]
        request_id: Option<String>,
        relays: Vec<String>,
    },
    ListPendingWelcomes {
        #[serde(default)]
        request_id: Option<String>,
    },
    AcceptWelcome {
        #[serde(default)]
        request_id: Option<String>,
        wrapper_event_id: String,
    },
    ListGroups {
        #[serde(default)]
        request_id: Option<String>,
    },
    SendMessage {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        content: String,
    },
    SendMedia {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        file_path: String,
        #[serde(default)]
        mime_type: Option<String>,
        #[serde(default)]
        filename: Option<String>,
        #[serde(default)]
        caption: String,
        #[serde(default)]
        blossom_servers: Vec<String>,
    },
    SendTyping {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
    },
    InviteCall {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        peer_pubkey: String,
        #[serde(default)]
        call_id: Option<String>,
        moq_url: String,
        #[serde(default)]
        broadcast_base: Option<String>,
        #[serde(default)]
        track_name: Option<String>,
        #[serde(default)]
        track_codec: Option<String>,
        #[serde(default)]
        relay_auth: Option<String>,
    },
    AcceptCall {
        #[serde(default)]
        request_id: Option<String>,
        call_id: String,
    },
    RejectCall {
        #[serde(default)]
        request_id: Option<String>,
        call_id: String,
        #[serde(default = "default_reject_reason")]
        reason: String,
    },
    EndCall {
        #[serde(default)]
        request_id: Option<String>,
        call_id: String,
        #[serde(default = "default_end_reason")]
        reason: String,
    },
    SendAudioResponse {
        #[serde(default)]
        request_id: Option<String>,
        call_id: String,
        tts_text: String,
    },
    SendAudioFile {
        #[serde(default)]
        request_id: Option<String>,
        call_id: String,
        audio_path: String,
        sample_rate: u32,
        #[serde(default = "default_channels")]
        channels: u16,
    },
    SendCallData {
        #[serde(default)]
        request_id: Option<String>,
        call_id: String,
        payload_hex: String,
        #[serde(default)]
        track_name: Option<String>,
    },
    InitGroup {
        #[serde(default)]
        request_id: Option<String>,
        peer_pubkey: String,
        #[serde(default = "default_group_name")]
        group_name: String,
    },
    Shutdown {
        #[serde(default)]
        request_id: Option<String>,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutMsg {
    Ready {
        protocol_version: u32,
        pubkey: String,
        npub: String,
    },
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        code: String,
        message: String,
    },
    KeypackagePublished {
        event_id: String,
    },
    WelcomeReceived {
        wrapper_event_id: String,
        welcome_event_id: String,
        from_pubkey: String,
        nostr_group_id: String,
        group_name: String,
    },
    GroupJoined {
        nostr_group_id: String,
        mls_group_id: String,
        member_count: u32,
    },
    MessageReceived {
        nostr_group_id: String,
        from_pubkey: String,
        content: String,
        created_at: u64,
        message_id: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        media: Vec<MediaAttachmentOut>,
    },
    CallInviteReceived {
        call_id: String,
        from_pubkey: String,
        nostr_group_id: String,
    },
    CallSessionStarted {
        call_id: String,
        nostr_group_id: String,
        from_pubkey: String,
    },
    CallSessionEnded {
        call_id: String,
        reason: String,
    },
    CallDebug {
        call_id: String,
        tx_frames: u64,
        rx_frames: u64,
        rx_dropped: u64,
    },
    CallAudioChunk {
        call_id: String,
        audio_path: String,
        sample_rate: u32,
        channels: u8,
    },
    CallData {
        call_id: String,
        payload_hex: String,
        track_name: String,
    },
    GroupCreated {
        nostr_group_id: String,
        mls_group_id: String,
        peer_pubkey: String,
        member_count: u32,
    },
}

#[derive(Debug, Serialize)]
struct MediaAttachmentOut {
    url: String,
    mime_type: String,
    filename: String,
    original_hash_hex: String,
    nonce_hex: String,
    scheme_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    /// Local file path to the decrypted media (temp file). Only set on receive.
    #[serde(skip_serializing_if = "Option::is_none")]
    local_path: Option<String>,
}

fn default_channels() -> u16 {
    1
}

fn default_reject_reason() -> String {
    "declined".to_string()
}

fn default_end_reason() -> String {
    "user_hangup".to_string()
}

fn default_group_name() -> String {
    "DM".to_string()
}

const MAX_CHAT_MEDIA_BYTES: usize = 32 * 1024 * 1024;

fn is_imeta_tag(tag: &Tag) -> bool {
    matches!(tag.kind(), TagKind::Custom(kind) if kind.as_ref() == "imeta")
}

fn mime_from_extension(path: &std::path::Path) -> Option<&'static str> {
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
    vec![default_primary_blossom_server().to_string()]
}

fn media_ref_to_attachment(reference: MediaReference) -> MediaAttachmentOut {
    let (width, height) = reference
        .dimensions
        .map(|(w, h)| (Some(w), Some(h)))
        .unwrap_or((None, None));
    MediaAttachmentOut {
        url: reference.url,
        mime_type: reference.mime_type,
        filename: reference.filename,
        original_hash_hex: hex::encode(reference.original_hash),
        nonce_hex: hex::encode(reference.nonce),
        scheme_version: reference.scheme_version,
        width,
        height,
        local_path: None,
    }
}

/// Download encrypted media from Blossom, decrypt it, and write to a temp file.
/// Returns the local file path on success.
async fn download_and_decrypt_media(
    mdk: &MDK<MdkSqliteStorage>,
    mls_group_id: &GroupId,
    reference: &MediaReference,
    state_dir: &Path,
) -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    let response = client
        .get(&reference.url)
        .send()
        .await
        .with_context(|| format!("download encrypted media from {}", reference.url))?;
    if !response.status().is_success() {
        anyhow::bail!("download failed: HTTP {}", response.status());
    }
    let encrypted_data = response.bytes().await.context("read media body")?;
    let manager = mdk.media_manager(mls_group_id.clone());
    let decrypted = manager
        .decrypt_from_download(&encrypted_data, reference)
        .context("decrypt media")?;

    // Write to a temp directory under the state dir
    let media_dir = state_dir.join("media-tmp");
    std::fs::create_dir_all(&media_dir).context("create media-tmp dir")?;
    let filename = if reference.filename.is_empty() {
        "download.bin"
    } else {
        &reference.filename
    };
    let dest = media_dir.join(format!(
        "{}-{}",
        hex::encode(&reference.original_hash[..8]),
        filename,
    ));
    std::fs::write(&dest, &decrypted)
        .with_context(|| format!("write decrypted media to {}", dest.display()))?;
    Ok(dest.to_string_lossy().into_owned())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CallSessionParams {
    moq_url: String,
    broadcast_base: String,
    #[serde(default)]
    relay_auth: String,
    tracks: Vec<CallTrackSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CallTrackSpec {
    name: String,
    codec: String,
    sample_rate: u32,
    channels: u8,
    frame_ms: u16,
}

#[derive(Debug, Clone)]
struct PendingCallInvite {
    call_id: String,
    from_pubkey: String,
    nostr_group_id: String,
    session: CallSessionParams,
}

#[derive(Debug, Clone)]
struct PendingOutgoingCallInvite {
    call_id: String,
    peer_pubkey: String,
    nostr_group_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveCallMode {
    Audio,
    Data,
}

#[derive(Debug)]
struct ActiveCall {
    call_id: String,
    nostr_group_id: String,
    session: CallSessionParams,
    mode: ActiveCallMode,
    media_crypto: CallMediaCryptoContext,
    next_voice_seq: u64,
    next_data_seq: u64,
    worker: CallWorker,
}

#[derive(Debug, Clone)]
struct CallMediaCryptoContext {
    tx_keys: FrameKeyMaterial,
    rx_keys: FrameKeyMaterial,
    local_participant_label: String,
    peer_participant_label: String,
}

#[derive(Debug)]
enum CallWorkerEvent {
    AudioChunk {
        call_id: String,
        audio_path: String,
        sample_rate: u32,
        channels: u8,
    },
    AudioPublished {
        call_id: String,
        request_id: Option<String>,
        result: anyhow::Result<VoicePublishStats>,
    },
    DataFrame {
        call_id: String,
        payload: Vec<u8>,
        track_name: String,
    },
}

#[derive(Debug)]
struct CallWorker {
    stop: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl CallWorker {
    async fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.task.await;
    }
}

fn call_relay_pool() -> &'static Mutex<HashMap<String, InMemoryRelay>> {
    static RELAYS: OnceLock<Mutex<HashMap<String, InMemoryRelay>>> = OnceLock::new();
    RELAYS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn network_relay_pool() -> &'static Mutex<HashMap<String, NetworkRelay>> {
    static RELAYS: OnceLock<Mutex<HashMap<String, NetworkRelay>>> = OnceLock::new();
    RELAYS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn relay_key(params: &CallSessionParams) -> String {
    format!("{}|{}", params.moq_url, params.broadcast_base)
}

fn shared_call_relay(params: &CallSessionParams) -> InMemoryRelay {
    let mut relays = call_relay_pool().lock().expect("call relay pool poisoned");
    relays.entry(relay_key(params)).or_default().clone()
}

fn shared_network_relay(params: &CallSessionParams) -> anyhow::Result<NetworkRelay> {
    let mut relays = network_relay_pool()
        .lock()
        .expect("network relay pool poisoned");
    // Key by moq_url only; a single relay connection can handle multiple broadcast paths.
    let relay = match relays.get(&params.moq_url) {
        Some(r) => r.clone(),
        None => {
            let r = NetworkRelay::with_options(&params.moq_url)
                .map_err(|e| anyhow!("network relay init: {e}"))?;
            relays.insert(params.moq_url.clone(), r.clone());
            r
        }
    };
    relay
        .connect()
        .map_err(|e| anyhow!("network relay connect: {e}"))?;
    Ok(relay)
}

fn is_real_moq_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

#[derive(Clone)]
enum CallMediaTransport {
    InMemory { session: MediaSession },
    Network { relay: NetworkRelay },
}

impl CallMediaTransport {
    fn for_session(params: &CallSessionParams) -> anyhow::Result<Self> {
        if is_real_moq_url(&params.moq_url) {
            let relay = shared_network_relay(params)?;
            Ok(Self::Network { relay })
        } else {
            let im_relay = shared_call_relay(params);
            let mut session = MediaSession::with_relay(
                SessionConfig {
                    moq_url: params.moq_url.clone(),
                    relay_auth: params.relay_auth.clone(),
                },
                im_relay,
            );
            session
                .connect()
                .map_err(|e| anyhow!("in-memory connect: {e}"))?;
            Ok(Self::InMemory { session })
        }
    }

    fn publish(&self, track: &TrackAddress, frame: MediaFrame) -> Result<usize, MediaSessionError> {
        match self {
            Self::InMemory { session } => session.publish(track, frame),
            Self::Network { relay } => relay.publish(track, frame),
        }
    }

    fn subscribe(
        &self,
        track: &TrackAddress,
    ) -> Result<pika_media::subscription::MediaFrameSubscription, MediaSessionError> {
        match self {
            Self::InMemory { session } => session.subscribe(track),
            Self::Network { relay } => relay.subscribe(track),
        }
    }
}

fn default_audio_call_session(call_id: &str) -> CallSessionParams {
    CallSessionParams {
        moq_url: "https://us-east.moq.logos.surf/anon".to_string(),
        broadcast_base: format!("pika/calls/{call_id}"),
        relay_auth: "capv1_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
        tracks: vec![CallTrackSpec {
            name: "audio0".to_string(),
            codec: "opus".to_string(),
            sample_rate: 48_000,
            channels: 1,
            frame_ms: 20,
        }],
    }
}

#[derive(Debug, Clone)]
pub struct AudioEchoSmokeStats {
    pub sent_frames: u64,
    pub echoed_frames: u64,
}

fn out_error(request_id: Option<String>, code: &str, message: impl Into<String>) -> OutMsg {
    OutMsg::Error {
        request_id,
        code: code.to_string(),
        message: message.into(),
    }
}

fn out_ok(request_id: Option<String>, result: Option<serde_json::Value>) -> OutMsg {
    OutMsg::Ok { request_id, result }
}

/// Decode a hex nostr_group_id, validate it, and return the matching MLS group ID.
fn resolve_group(mdk: &MDK<MdkSqliteStorage>, nostr_group_id: &str) -> anyhow::Result<GroupId> {
    let group_id_bytes =
        hex::decode(nostr_group_id).map_err(|_| anyhow!("nostr_group_id must be hex"))?;
    if group_id_bytes.len() != 32 {
        anyhow::bail!("nostr_group_id must be 32 bytes hex");
    }
    let groups = mdk.get_groups().context("get_groups")?;
    let g = groups
        .iter()
        .find(|g| g.nostr_group_id.as_slice() == group_id_bytes.as_slice())
        .ok_or_else(|| anyhow!("group not found"))?;
    Ok(g.mls_group_id.clone())
}

/// Create an MLS message from a rumor, strip protected tags, sign, and publish.
async fn sign_and_publish(
    client: &Client,
    relay_urls: &[RelayUrl],
    mdk: &MDK<MdkSqliteStorage>,
    keys: &Keys,
    mls_group_id: &GroupId,
    rumor: UnsignedEvent,
    label: &str,
) -> anyhow::Result<Event> {
    let msg_event = mdk
        .create_message(mls_group_id, rumor)
        .context("create_message")?;
    let msg_tags: Tags = msg_event
        .tags
        .clone()
        .into_iter()
        .filter(|t| !matches!(t.kind(), TagKind::Protected))
        .collect();
    let signed = EventBuilder::new(msg_event.kind, msg_event.content)
        .tags(msg_tags)
        .sign_with_keys(keys)
        .context("sign event")?;
    if relay_urls.is_empty() {
        anyhow::bail!("no relays configured");
    }
    publish_and_confirm_multi(client, relay_urls, &signed, label).await?;
    Ok(signed)
}

#[derive(Debug)]
enum ParsedCallSignal {
    Invite {
        call_id: String,
        session: CallSessionParams,
    },
    Accept {
        call_id: String,
        session: CallSessionParams,
    },
    Reject {
        call_id: String,
        reason: String,
    },
    End {
        call_id: String,
        reason: String,
    },
}

#[derive(Debug, Deserialize)]
struct CallSignalEnvelope {
    v: u32,
    ns: String,
    #[serde(rename = "type")]
    msg_type: String,
    call_id: String,
    #[allow(dead_code)]
    #[serde(default)]
    ts_ms: i64,
    #[serde(default)]
    body: serde_json::Value,
}

enum OutgoingCallSignal<'a> {
    Invite(&'a CallSessionParams),
    Accept(&'a CallSessionParams),
    Reject { reason: &'a str },
    End { reason: &'a str },
}

fn parse_call_signal(content: &str) -> Option<ParsedCallSignal> {
    fn from_env(env: CallSignalEnvelope) -> Option<ParsedCallSignal> {
        if env.v != 1 || env.ns != "pika.call" {
            return None;
        }
        match env.msg_type.as_str() {
            "call.invite" => {
                let session: CallSessionParams = match serde_json::from_value(env.body) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(
                            "[pikachat] call.invite body parse failed call_id={} err={e:#}",
                            env.call_id
                        );
                        return None;
                    }
                };
                Some(ParsedCallSignal::Invite {
                    call_id: env.call_id,
                    session,
                })
            }
            "call.accept" => {
                let session: CallSessionParams = match serde_json::from_value(env.body) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(
                            "[pikachat] call.accept body parse failed call_id={} err={e:#}",
                            env.call_id
                        );
                        return None;
                    }
                };
                Some(ParsedCallSignal::Accept {
                    call_id: env.call_id,
                    session,
                })
            }
            "call.reject" => {
                let reason = env
                    .body
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("declined")
                    .to_string();
                Some(ParsedCallSignal::Reject {
                    call_id: env.call_id,
                    reason,
                })
            }
            "call.end" => {
                let reason = env
                    .body
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("remote_end")
                    .to_string();
                Some(ParsedCallSignal::End {
                    call_id: env.call_id,
                    reason,
                })
            }
            _ => None,
        }
    }

    // Fast path: expected envelope.
    match serde_json::from_str::<CallSignalEnvelope>(content) {
        Ok(env) => return from_env(env),
        Err(e) => {
            // If this looks like a call signal, surface the parse error.
            if content.contains("pika.call")
                || content.contains("call.invite")
                || content.contains("call.accept")
            {
                warn!(
                    "[pikachat] call signal envelope parse failed err={e:#} content={}",
                    content.chars().take(240).collect::<String>()
                );
            }
        }
    }

    // Compat: sometimes the application payload can be JSON-encoded as a string.
    // Example: "\"{...}\"" (double-encoded).
    if let Ok(inner) = serde_json::from_str::<String>(content) {
        let inner_trimmed = inner.trim();
        if inner_trimmed != content
            && let Some(sig) = parse_call_signal(inner_trimmed)
        {
            return Some(sig);
        }
    }

    // Compat: unwrap a JSON object with a nested `content` field.
    // This is useful if the sender serialized the whole rumor/event JSON rather than the plain
    // rumor content string.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(inner) = v.get("content").and_then(|x| x.as_str()) {
            let inner_trimmed = inner.trim();
            if inner_trimmed != content
                && let Some(sig) = parse_call_signal(inner_trimmed)
            {
                return Some(sig);
            }
        }
        // Compat: unwrap common nested shapes.
        if let Some(inner) = v
            .get("rumor")
            .and_then(|r| r.get("content"))
            .and_then(|x| x.as_str())
        {
            let inner_trimmed = inner.trim();
            if inner_trimmed != content
                && let Some(sig) = parse_call_signal(inner_trimmed)
            {
                return Some(sig);
            }
        }
    }

    // Debug hint: the content looked like a call signal but didn't parse.
    if content.contains("pika.call") && content.contains("call.") && content.contains("type") {
        warn!(
            "[pikachat] call signal parse failed (unexpected json shape): {}",
            content.chars().take(240).collect::<String>()
        );
    }

    None
}

fn build_call_signal_json(call_id: &str, signal: OutgoingCallSignal<'_>) -> anyhow::Result<String> {
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let value = match signal {
        OutgoingCallSignal::Invite(session) => json!({
            "v": 1,
            "ns": "pika.call",
            "type": "call.invite",
            "call_id": call_id,
            "ts_ms": ts_ms,
            "body": session,
        }),
        OutgoingCallSignal::Accept(session) => json!({
            "v": 1,
            "ns": "pika.call",
            "type": "call.accept",
            "call_id": call_id,
            "ts_ms": ts_ms,
            "body": session,
        }),
        OutgoingCallSignal::Reject { reason } => json!({
            "v": 1,
            "ns": "pika.call",
            "type": "call.reject",
            "call_id": call_id,
            "ts_ms": ts_ms,
            "body": { "reason": reason },
        }),
        OutgoingCallSignal::End { reason } => json!({
            "v": 1,
            "ns": "pika.call",
            "type": "call.end",
            "call_id": call_id,
            "ts_ms": ts_ms,
            "body": { "reason": reason },
        }),
    };
    serde_json::to_string(&value).context("serialize call signal")
}

fn context_hash(parts: &[&[u8]]) -> [u8; 32] {
    let mut buf = Vec::new();
    for part in parts {
        let len: u32 = part.len().try_into().unwrap_or(u32::MAX);
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(part);
    }
    sha256::Hash::hash(&buf).to_byte_array()
}

fn key_id_for_sender(sender_id: &[u8]) -> u64 {
    let digest = context_hash(&[b"pika.call.media.keyid.v1", sender_id]);
    u64::from_be_bytes(digest[0..8].try_into().expect("hash width"))
}

fn call_shared_seed(
    call_id: &str,
    session: &CallSessionParams,
    local_pubkey_hex: &str,
    peer_pubkey_hex: &str,
) -> String {
    let (left, right) = if local_pubkey_hex <= peer_pubkey_hex {
        (local_pubkey_hex, peer_pubkey_hex)
    } else {
        (peer_pubkey_hex, local_pubkey_hex)
    };
    format!(
        "pika-call-media-v1|{call_id}|{}|{}|{}|{}",
        session.moq_url, session.broadcast_base, left, right
    )
}

fn valid_relay_auth_token(token: &str) -> bool {
    let trimmed = token.trim();
    let Some(hex_part) = trimmed.strip_prefix(RELAY_AUTH_CAP_PREFIX) else {
        return false;
    };
    hex_part.len() == RELAY_AUTH_HEX_LEN && hex_part.chars().all(|c| c.is_ascii_hexdigit())
}

fn derive_relay_auth_token(
    mdk: &MDK<MdkSqliteStorage>,
    nostr_group_id: &str,
    call_id: &str,
    session: &CallSessionParams,
    local_pubkey_hex: &str,
    peer_pubkey_hex: &str,
) -> anyhow::Result<String> {
    let mls_group_id = resolve_group(mdk, nostr_group_id)?;

    let shared_seed = call_shared_seed(call_id, session, local_pubkey_hex, peer_pubkey_hex);
    let auth_hash = context_hash(&[
        b"pika.call.relay.auth.seed.v1",
        shared_seed.as_bytes(),
        call_id.as_bytes(),
    ]);
    let auth_key = *derive_encryption_key(
        mdk,
        &mls_group_id,
        DEFAULT_SCHEME_VERSION,
        &auth_hash,
        "application/pika-call-auth",
        &format!("call/{call_id}/relay-auth"),
    )
    .map_err(|e| anyhow!("derive relay auth token failed: {e}"))?;
    let token_hash = context_hash(&[
        b"pika.call.relay.auth.token.v1",
        &auth_key,
        call_id.as_bytes(),
        session.moq_url.as_bytes(),
        session.broadcast_base.as_bytes(),
    ]);
    Ok(format!("capv1_{}", hex::encode(token_hash)))
}

fn validate_relay_auth_token(
    mdk: &MDK<MdkSqliteStorage>,
    nostr_group_id: &str,
    call_id: &str,
    session: &CallSessionParams,
    local_pubkey_hex: &str,
    peer_pubkey_hex: &str,
) -> anyhow::Result<()> {
    if !valid_relay_auth_token(&session.relay_auth) {
        return Err(anyhow!("call relay auth token format invalid"));
    }
    let expected = derive_relay_auth_token(
        mdk,
        nostr_group_id,
        call_id,
        session,
        local_pubkey_hex,
        peer_pubkey_hex,
    )?;
    if expected != session.relay_auth {
        return Err(anyhow!("call relay auth token mismatch"));
    }
    Ok(())
}

fn derive_mls_media_crypto_context(
    mdk: &MDK<MdkSqliteStorage>,
    nostr_group_id: &str,
    call_id: &str,
    session: &CallSessionParams,
    local_pubkey_hex: &str,
    peer_pubkey_hex: &str,
) -> anyhow::Result<CallMediaCryptoContext> {
    let mls_group_id = resolve_group(mdk, nostr_group_id)?;
    let group = mdk
        .get_group(&mls_group_id)
        .map_err(|e| anyhow!("load mls group failed: {e}"))?
        .ok_or_else(|| anyhow!("mls group not found"))?;

    let shared_seed = call_shared_seed(call_id, session, local_pubkey_hex, peer_pubkey_hex);
    let track = session
        .tracks
        .first()
        .map(|t| t.name.as_str())
        .ok_or_else(|| anyhow!("call session must include at least one track"))?;
    let generation = 0u8;
    let tx_hash = context_hash(&[
        b"pika.call.media.base.v1",
        shared_seed.as_bytes(),
        local_pubkey_hex.as_bytes(),
        track.as_bytes(),
    ]);
    let rx_hash = context_hash(&[
        b"pika.call.media.base.v1",
        shared_seed.as_bytes(),
        peer_pubkey_hex.as_bytes(),
        track.as_bytes(),
    ]);
    let root_hash = context_hash(&[
        b"pika.call.media.root.v1",
        shared_seed.as_bytes(),
        track.as_bytes(),
    ]);

    let tx_filename = format!("call/{call_id}/{track}/{local_pubkey_hex}");
    let rx_filename = format!("call/{call_id}/{track}/{peer_pubkey_hex}");
    let root_filename = format!("call/{call_id}/{track}/group-root");

    let tx_base = *derive_encryption_key(
        mdk,
        &mls_group_id,
        DEFAULT_SCHEME_VERSION,
        &tx_hash,
        "application/pika-call",
        &tx_filename,
    )
    .map_err(|e| anyhow!("derive tx media key failed: {e}"))?;
    let rx_base = *derive_encryption_key(
        mdk,
        &mls_group_id,
        DEFAULT_SCHEME_VERSION,
        &rx_hash,
        "application/pika-call",
        &rx_filename,
    )
    .map_err(|e| anyhow!("derive rx media key failed: {e}"))?;
    let group_root = *derive_encryption_key(
        mdk,
        &mls_group_id,
        DEFAULT_SCHEME_VERSION,
        &root_hash,
        "application/pika-call",
        &root_filename,
    )
    .map_err(|e| anyhow!("derive media group root failed: {e}"))?;

    Ok(CallMediaCryptoContext {
        tx_keys: FrameKeyMaterial::from_base_key(
            tx_base,
            key_id_for_sender(local_pubkey_hex.as_bytes()),
            group.epoch,
            generation,
            track,
            group_root,
        ),
        rx_keys: FrameKeyMaterial::from_base_key(
            rx_base,
            key_id_for_sender(peer_pubkey_hex.as_bytes()),
            group.epoch,
            generation,
            track,
            group_root,
        ),
        local_participant_label: opaque_participant_label(&group_root, local_pubkey_hex.as_bytes()),
        peer_participant_label: opaque_participant_label(&group_root, peer_pubkey_hex.as_bytes()),
    })
}

fn active_call_mode(session: &CallSessionParams) -> ActiveCallMode {
    if call_audio_track_spec(session).is_some() {
        ActiveCallMode::Audio
    } else {
        ActiveCallMode::Data
    }
}

fn call_primary_track_name(session: &CallSessionParams) -> anyhow::Result<&str> {
    session
        .tracks
        .first()
        .map(|t| t.name.as_str())
        .ok_or_else(|| anyhow!("call session must include at least one track"))
}

#[allow(dead_code)]
async fn publish_group_message(
    client: &Client,
    relay_urls: &[RelayUrl],
    mdk: &MDK<MdkSqliteStorage>,
    keys: &Keys,
    nostr_group_id: &str,
    content: String,
    label: &str,
) -> anyhow::Result<()> {
    publish_group_event(
        client,
        relay_urls,
        mdk,
        keys,
        nostr_group_id,
        Kind::ChatMessage,
        content,
        label,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn publish_group_event(
    client: &Client,
    relay_urls: &[RelayUrl],
    mdk: &MDK<MdkSqliteStorage>,
    keys: &Keys,
    nostr_group_id: &str,
    kind: Kind,
    content: String,
    label: &str,
) -> anyhow::Result<()> {
    let mls_group_id = resolve_group(mdk, nostr_group_id)?;
    let rumor = EventBuilder::new(kind, content).build(keys.public_key());
    sign_and_publish(client, relay_urls, mdk, keys, &mls_group_id, rumor, label).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn send_call_signal(
    client: &Client,
    relay_urls: &[RelayUrl],
    mdk: &MDK<MdkSqliteStorage>,
    keys: &Keys,
    nostr_group_id: &str,
    call_id: &str,
    signal: OutgoingCallSignal<'_>,
    label: &str,
) -> anyhow::Result<()> {
    let payload = build_call_signal_json(call_id, signal)?;
    publish_group_event(
        client,
        relay_urls,
        mdk,
        keys,
        nostr_group_id,
        Kind::Custom(10),
        payload,
        label,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn send_call_invite_with_retry(
    client: &Client,
    relay_urls: &[RelayUrl],
    mdk: &MDK<MdkSqliteStorage>,
    keys: &Keys,
    nostr_group_id: &str,
    call_id: &str,
    session: &CallSessionParams,
    max_attempts: usize,
) -> anyhow::Result<()> {
    let attempts = max_attempts.max(1);
    for attempt in 1..=attempts {
        match send_call_signal(
            client,
            relay_urls,
            mdk,
            keys,
            nostr_group_id,
            call_id,
            OutgoingCallSignal::Invite(session),
            "call_invite",
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(err) => {
                if attempt == attempts {
                    return Err(err);
                }
                warn!(
                    "[marmotd] call invite publish attempt {attempt}/{attempts} failed call_id={call_id}: {err:#}; retrying"
                );
                tokio::time::sleep(Duration::from_millis(750)).await;
            }
        }
    }
    unreachable!("attempt loop must return");
}

fn call_audio_track_spec(session: &CallSessionParams) -> Option<&CallTrackSpec> {
    session
        .tracks
        .iter()
        .find(|t| t.codec.eq_ignore_ascii_case("opus") && t.channels > 0 && t.sample_rate > 0)
}

fn downmix_to_mono(pcm: &[i16], channels: u16) -> Vec<i16> {
    if channels <= 1 {
        return pcm.to_vec();
    }
    let channels = channels as usize;
    let mut out = Vec::with_capacity(pcm.len() / channels.max(1));
    for frame in pcm.chunks(channels.max(1)) {
        let sum: i32 = frame.iter().map(|s| *s as i32).sum();
        out.push((sum / frame.len().max(1) as i32) as i16);
    }
    out
}

fn resample_mono_linear(input: &[i16], in_rate: u32, out_rate: u32) -> Vec<i16> {
    if input.is_empty() || in_rate == out_rate {
        return input.to_vec();
    }
    let out_len =
        ((input.len() as u64).saturating_mul(out_rate as u64) / (in_rate as u64).max(1)) as usize;
    if out_len == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(out_len);
    for out_idx in 0..out_len {
        let pos_num = (out_idx as u64).saturating_mul(in_rate as u64);
        let idx = (pos_num / out_rate as u64) as usize;
        let frac = (pos_num % out_rate as u64) as f32 / out_rate as f32;
        let s0 = input[idx.min(input.len().saturating_sub(1))] as f32;
        let s1 = input[(idx + 1).min(input.len().saturating_sub(1))] as f32;
        out.push((s0 + (s1 - s0) * frac) as i16);
    }
    out
}

#[derive(Debug, Clone, Copy)]
struct VoicePublishStats {
    next_seq: u64,
    frames_published: u64,
}

fn publish_tts_audio_response_with_relay(
    session: &CallSessionParams,
    relay: InMemoryRelay,
    media_crypto: &CallMediaCryptoContext,
    start_seq: u64,
    tts_text: &str,
) -> anyhow::Result<VoicePublishStats> {
    let text = tts_text.to_string();
    let tts_pcm = std::thread::spawn(move || synthesize_tts_pcm(&text))
        .join()
        .map_err(|_| anyhow!("tts synthesis thread panicked"))?
        .context("synthesize call tts")?;
    publish_pcm_audio_response_with_relay(session, relay, media_crypto, start_seq, tts_pcm)
}

fn publish_pcm_audio_response_with_relay(
    session: &CallSessionParams,
    relay: InMemoryRelay,
    media_crypto: &CallMediaCryptoContext,
    start_seq: u64,
    tts_pcm: crate::call_tts::TtsPcm,
) -> anyhow::Result<VoicePublishStats> {
    let Some(track) = call_audio_track_spec(session) else {
        return Err(anyhow!("call session missing opus audio track"));
    };
    if track.channels != 1 {
        return Err(anyhow!(
            "tts publish only supports mono track for now (got channels={})",
            track.channels
        ));
    }

    let mut media = MediaSession::with_relay(
        SessionConfig {
            moq_url: session.moq_url.clone(),
            relay_auth: session.relay_auth.clone(),
        },
        relay,
    );
    media.connect().map_err(|e| anyhow::anyhow!("{e}"))?;
    let publish_track = TrackAddress {
        broadcast_path: broadcast_path(
            &session.broadcast_base,
            &media_crypto.local_participant_label,
        )
        .map_err(|e| anyhow!("invalid local broadcast path: {e}"))?,
        track_name: track.name.clone(),
    };
    tracing::info!(
        "[tts] publish init (relay) broadcast_base={} local_label={} peer_label={} publish_path={} track={} start_seq={}",
        session.broadcast_base,
        media_crypto.local_participant_label,
        media_crypto.peer_participant_label,
        publish_track.broadcast_path,
        publish_track.track_name,
        start_seq
    );

    let mono_pcm = downmix_to_mono(&tts_pcm.pcm_i16, tts_pcm.channels);
    let pcm = resample_mono_linear(&mono_pcm, tts_pcm.sample_rate_hz, track.sample_rate);
    if pcm.is_empty() {
        return Err(anyhow!("tts synthesis produced no pcm samples"));
    }

    let frame_samples = ((track.sample_rate as usize) * (track.frame_ms as usize) / 1000)
        .saturating_mul(track.channels as usize);
    if frame_samples == 0 {
        return Err(anyhow!("invalid frame size from track spec"));
    }

    let codec = OpusCodec;
    let mut seq = start_seq;
    let mut frames = 0u64;
    for chunk in pcm.chunks(frame_samples) {
        let frame_counter =
            u32::try_from(seq).map_err(|_| anyhow!("call media tx counter exhausted"))?;
        let mut frame_pcm = Vec::with_capacity(frame_samples);
        frame_pcm.extend_from_slice(chunk);
        if frame_pcm.len() < frame_samples {
            frame_pcm.resize(frame_samples, 0);
        }
        let packet = codec.encode_pcm_i16(&frame_pcm);
        let encrypted = encrypt_frame(
            &packet.0,
            &media_crypto.tx_keys,
            FrameInfo {
                counter: frame_counter,
                group_seq: seq,
                frame_idx: 0,
                keyframe: true,
            },
        )
        .map_err(|e| anyhow!("encrypt tts frame failed: {e}"))?;
        let frame = MediaFrame {
            seq,
            timestamp_us: seq.saturating_mul((track.frame_ms as u64) * 1_000),
            keyframe: true,
            payload: encrypted,
        };
        media
            .publish(&publish_track, frame)
            .context("publish tts frame")?;
        seq = seq.saturating_add(1);
        frames = frames.saturating_add(1);
    }

    Ok(VoicePublishStats {
        next_seq: seq,
        frames_published: frames,
    })
}

fn publish_pcm_audio_response_with_transport(
    session: &CallSessionParams,
    transport: CallMediaTransport,
    media_crypto: &CallMediaCryptoContext,
    start_seq: u64,
    tts_pcm: crate::call_tts::TtsPcm,
) -> anyhow::Result<VoicePublishStats> {
    let Some(track) = call_audio_track_spec(session) else {
        return Err(anyhow!("call session missing opus audio track"));
    };
    if track.channels != 1 {
        return Err(anyhow!(
            "tts publish only supports mono (got channels={})",
            track.channels
        ));
    }

    let publish_track = TrackAddress {
        broadcast_path: broadcast_path(
            &session.broadcast_base,
            &media_crypto.local_participant_label,
        )
        .map_err(|e| anyhow!("invalid local broadcast path: {e}"))?,
        track_name: track.name.clone(),
    };
    tracing::info!(
        "[tts] publish init (transport) broadcast_base={} local_label={} peer_label={} publish_path={} track={} start_seq={}",
        session.broadcast_base,
        media_crypto.local_participant_label,
        media_crypto.peer_participant_label,
        publish_track.broadcast_path,
        publish_track.track_name,
        start_seq,
    );

    let mono_pcm = downmix_to_mono(&tts_pcm.pcm_i16, tts_pcm.channels);
    let pcm = resample_mono_linear(&mono_pcm, tts_pcm.sample_rate_hz, track.sample_rate);
    if pcm.is_empty() {
        return Err(anyhow!("tts synthesis produced no pcm samples"));
    }

    let frame_samples = ((track.sample_rate as usize) * (track.frame_ms as usize) / 1000)
        .saturating_mul(track.channels as usize);
    if frame_samples == 0 {
        return Err(anyhow!("invalid frame size from track spec"));
    }

    let codec = OpusCodec;
    let mut seq = start_seq;
    let mut frames = 0u64;
    for chunk in pcm.chunks(frame_samples) {
        let frame_counter =
            u32::try_from(seq).map_err(|_| anyhow!("call media tx counter exhausted"))?;
        let mut frame_pcm = Vec::with_capacity(frame_samples);
        frame_pcm.extend_from_slice(chunk);
        if frame_pcm.len() < frame_samples {
            frame_pcm.resize(frame_samples, 0);
        }
        let packet = codec.encode_pcm_i16(&frame_pcm);
        let encrypted = encrypt_frame(
            &packet.0,
            &media_crypto.tx_keys,
            FrameInfo {
                counter: frame_counter,
                group_seq: seq,
                frame_idx: 0,
                keyframe: true,
            },
        )
        .map_err(|e| anyhow!("encrypt tts frame failed: {e}"))?;
        let frame = MediaFrame {
            seq,
            timestamp_us: seq.saturating_mul((track.frame_ms as u64) * 1_000),
            keyframe: true,
            payload: encrypted,
        };
        transport
            .publish(&publish_track, frame)
            .context("publish tts frame")?;
        seq = seq.saturating_add(1);
        frames = frames.saturating_add(1);
        // Pace frame delivery at ~real-time so the receiver doesn't get a
        // burst of frames it can't buffer properly.
        std::thread::sleep(Duration::from_millis(track.frame_ms as u64));
    }

    Ok(VoicePublishStats {
        next_seq: seq,
        frames_published: frames,
    })
}

fn publish_tts_audio_response_with_transport(
    session: &CallSessionParams,
    transport: CallMediaTransport,
    media_crypto: &CallMediaCryptoContext,
    start_seq: u64,
    tts_text: &str,
) -> anyhow::Result<VoicePublishStats> {
    // synthesize_tts_pcm uses reqwest::blocking::Client which panics if created
    // inside a tokio runtime. Run it on a dedicated thread.
    let text = tts_text.to_string();
    let tts_pcm = std::thread::spawn(move || synthesize_tts_pcm(&text))
        .join()
        .map_err(|_| anyhow!("tts synthesis thread panicked"))?
        .context("synthesize call tts")?;
    publish_pcm_audio_response_with_transport(session, transport, media_crypto, start_seq, tts_pcm)
}

fn publish_pcm_audio_response(
    session: &CallSessionParams,
    media_crypto: &CallMediaCryptoContext,
    start_seq: u64,
    tts_pcm: crate::call_tts::TtsPcm,
) -> anyhow::Result<VoicePublishStats> {
    if is_real_moq_url(&session.moq_url) {
        let transport = CallMediaTransport::for_session(session)?;
        publish_pcm_audio_response_with_transport(
            session,
            transport,
            media_crypto,
            start_seq,
            tts_pcm,
        )
    } else {
        let relay = shared_call_relay(session);
        publish_pcm_audio_response_with_relay(session, relay, media_crypto, start_seq, tts_pcm)
    }
}

fn publish_tts_audio_response(
    session: &CallSessionParams,
    media_crypto: &CallMediaCryptoContext,
    start_seq: u64,
    tts_text: &str,
) -> anyhow::Result<VoicePublishStats> {
    if is_real_moq_url(&session.moq_url) {
        let transport = CallMediaTransport::for_session(session)?;
        publish_tts_audio_response_with_transport(
            session,
            transport,
            media_crypto,
            start_seq,
            tts_text,
        )
    } else {
        let relay = shared_call_relay(session);
        publish_tts_audio_response_with_relay(session, relay, media_crypto, start_seq, tts_text)
    }
}

fn start_stt_worker(
    call_id: &str,
    session: &CallSessionParams,
    media_crypto: CallMediaCryptoContext,
    out_tx: mpsc::UnboundedSender<OutMsg>,
    call_evt_tx: mpsc::UnboundedSender<CallWorkerEvent>,
) -> anyhow::Result<CallWorker> {
    if is_real_moq_url(&session.moq_url) {
        let transport = CallMediaTransport::for_session(session)?;
        start_stt_worker_with_transport(
            call_id,
            session,
            transport,
            media_crypto,
            out_tx,
            call_evt_tx,
        )
    } else {
        let relay = shared_call_relay(session);
        start_stt_worker_with_relay(call_id, session, relay, media_crypto, out_tx, call_evt_tx)
    }
}

fn start_stt_worker_with_relay(
    call_id: &str,
    session: &CallSessionParams,
    relay: InMemoryRelay,
    media_crypto: CallMediaCryptoContext,
    out_tx: mpsc::UnboundedSender<OutMsg>,
    call_evt_tx: mpsc::UnboundedSender<CallWorkerEvent>,
) -> anyhow::Result<CallWorker> {
    let Some(track) = call_audio_track_spec(session) else {
        return Err(anyhow!("call session missing opus audio track"));
    };

    let mut media = MediaSession::with_relay(
        SessionConfig {
            moq_url: session.moq_url.clone(),
            relay_auth: session.relay_auth.clone(),
        },
        relay,
    );
    media.connect().map_err(|e| anyhow::anyhow!("{e}"))?;

    let subscribe_track = TrackAddress {
        broadcast_path: broadcast_path(
            &session.broadcast_base,
            &media_crypto.peer_participant_label,
        )
        .map_err(|e| anyhow!("invalid peer broadcast path: {e}"))?,
        track_name: track.name.clone(),
    };
    let rx = media
        .subscribe(&subscribe_track)
        .context("subscribe peer track for stt")?;

    let mut pipeline = OpusToAudioPipeline::new(track.sample_rate, track.channels)
        .context("initialize audio pipeline")?;

    let sample_rate = track.sample_rate;
    let channels = track.channels;
    let call_id = call_id.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_task = stop.clone();
    let task = tokio::task::spawn_blocking(move || {
        // Keep the media session alive for as long as the worker runs.
        // (Even if it is not used directly in this thread.)
        let _media = media;
        let tmp_dir = std::env::temp_dir().join(format!("pikachat-audio-{}", call_id));
        let _ = std::fs::create_dir_all(&tmp_dir);
        let mut chunk_seq = 0u64;
        let mut rx_frames = 0u64;
        let mut rx_decrypt_dropped = 0u64;
        let mut ticks = 0u64;

        let emit_chunk = |wav: Vec<u8>,
                          seq: &mut u64,
                          call_id: &str,
                          call_evt_tx: &mpsc::UnboundedSender<CallWorkerEvent>,
                          tmp_dir: &std::path::Path| {
            let wav_path = tmp_dir.join(format!("chunk_{seq}.wav"));
            if let Err(err) = std::fs::write(&wav_path, &wav) {
                warn!("[pikachat] write audio chunk failed call_id={call_id} err={err}");
                return;
            }
            *seq += 1;
            let _ = call_evt_tx.send(CallWorkerEvent::AudioChunk {
                call_id: call_id.to_string(),
                audio_path: wav_path.to_string_lossy().to_string(),
                sample_rate,
                channels,
            });
        };

        while !stop_for_task.load(Ordering::Relaxed) {
            while let Ok(inbound) = rx.try_recv() {
                let decrypted = match decrypt_frame(&inbound.payload, &media_crypto.rx_keys) {
                    Ok(v) => v,
                    Err(err) => {
                        rx_decrypt_dropped = rx_decrypt_dropped.saturating_add(1);
                        warn!(
                            "[pikachat] stt decrypt failed call_id={} err={err}",
                            call_id
                        );
                        continue;
                    }
                };
                rx_frames = rx_frames.saturating_add(1);
                if let Some(wav) = pipeline.ingest_packet(OpusPacket(decrypted.payload)) {
                    emit_chunk(wav, &mut chunk_seq, &call_id, &call_evt_tx, &tmp_dir);
                }
            }

            ticks = ticks.saturating_add(1);
            if ticks.is_multiple_of(5) {
                let _ = out_tx.send(OutMsg::CallDebug {
                    call_id: call_id.clone(),
                    tx_frames: 0,
                    rx_frames,
                    rx_dropped: rx_decrypt_dropped,
                });
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        if let Some(wav) = pipeline.flush() {
            emit_chunk(wav, &mut chunk_seq, &call_id, &call_evt_tx, &tmp_dir);
        }
    });

    Ok(CallWorker { stop, task })
}

fn start_stt_worker_with_transport(
    call_id: &str,
    session: &CallSessionParams,
    transport: CallMediaTransport,
    media_crypto: CallMediaCryptoContext,
    out_tx: mpsc::UnboundedSender<OutMsg>,
    call_evt_tx: mpsc::UnboundedSender<CallWorkerEvent>,
) -> anyhow::Result<CallWorker> {
    let Some(track) = call_audio_track_spec(session) else {
        return Err(anyhow!("call session missing opus audio track"));
    };

    let subscribe_track = TrackAddress {
        broadcast_path: broadcast_path(
            &session.broadcast_base,
            &media_crypto.peer_participant_label,
        )
        .map_err(|e| anyhow!("invalid peer broadcast path: {e}"))?,
        track_name: track.name.clone(),
    };
    let rx = transport
        .subscribe(&subscribe_track)
        .context("subscribe peer track for stt (network)")?;

    let sample_rate = track.sample_rate;
    let channels = track.channels;
    let call_id = call_id.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_task = stop.clone();
    let task = tokio::task::spawn_blocking(move || {
        // Critical: keep the transport (and thus NetworkRelay + its tokio runtime)
        // alive for as long as we're consuming frames.
        let _transport = transport;

        let mut pipeline = match OpusToAudioPipeline::new(sample_rate, channels) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("[stt] pipeline init failed: {e:#}");
                return;
            }
        };

        let tmp_dir = std::env::temp_dir().join(format!("pikachat-audio-{}", call_id));
        let _ = std::fs::create_dir_all(&tmp_dir);
        let mut chunk_seq = 0u64;

        let emit_chunk = |wav: Vec<u8>,
                          seq: &mut u64,
                          call_id: &str,
                          call_evt_tx: &mpsc::UnboundedSender<CallWorkerEvent>,
                          tmp_dir: &std::path::Path| {
            let wav_path = tmp_dir.join(format!("chunk_{seq}.wav"));
            if let Err(err) = std::fs::write(&wav_path, &wav) {
                warn!("[pikachat] write audio chunk failed call_id={call_id} err={err}");
                return;
            }
            *seq += 1;
            let _ = call_evt_tx.send(CallWorkerEvent::AudioChunk {
                call_id: call_id.to_string(),
                audio_path: wav_path.to_string_lossy().to_string(),
                sample_rate,
                channels,
            });
        };

        let mut rx_frames = 0u64;
        let mut rx_decrypt_dropped = 0u64;
        let mut ticks = 0u64;
        while !stop_for_task.load(Ordering::Relaxed) {
            while let Ok(inbound) = rx.try_recv() {
                let decrypted = match decrypt_frame(&inbound.payload, &media_crypto.rx_keys) {
                    Ok(v) => v,
                    Err(err) => {
                        rx_decrypt_dropped = rx_decrypt_dropped.saturating_add(1);
                        warn!(
                            "[pikachat] stt decrypt failed call_id={} err={err}",
                            call_id
                        );
                        continue;
                    }
                };
                rx_frames = rx_frames.saturating_add(1);
                if let Some(wav) = pipeline.ingest_packet(OpusPacket(decrypted.payload)) {
                    emit_chunk(wav, &mut chunk_seq, &call_id, &call_evt_tx, &tmp_dir);
                }
            }

            ticks = ticks.saturating_add(1);
            if ticks.is_multiple_of(5) {
                let _ = out_tx.send(OutMsg::CallDebug {
                    call_id: call_id.clone(),
                    tx_frames: 0,
                    rx_frames,
                    rx_dropped: rx_decrypt_dropped,
                });
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        if let Some(wav) = pipeline.flush() {
            emit_chunk(wav, &mut chunk_seq, &call_id, &call_evt_tx, &tmp_dir);
        }
    });

    Ok(CallWorker { stop, task })
}

fn start_echo_worker_with_relay(
    call_id: &str,
    session: &CallSessionParams,
    relay: InMemoryRelay,
    local_pubkey_hex: &str,
    peer_pubkey_hex: &str,
    out_tx: mpsc::UnboundedSender<OutMsg>,
) -> anyhow::Result<CallWorker> {
    let mut media = MediaSession::with_relay(
        SessionConfig {
            moq_url: session.moq_url.clone(),
            relay_auth: session.relay_auth.clone(),
        },
        relay,
    );
    media.connect().map_err(|e| anyhow::anyhow!("{e}"))?;

    let publish_track = TrackAddress {
        broadcast_path: broadcast_path(&session.broadcast_base, local_pubkey_hex)
            .map_err(|e| anyhow!("invalid local broadcast path: {e}"))?,
        track_name: "audio0".to_string(),
    };
    let subscribe_track = TrackAddress {
        broadcast_path: broadcast_path(&session.broadcast_base, peer_pubkey_hex)
            .map_err(|e| anyhow!("invalid peer broadcast path: {e}"))?,
        track_name: "audio0".to_string(),
    };
    let rx = media
        .subscribe(&subscribe_track)
        .context("subscribe peer track")?;

    let call_id = call_id.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_task = stop.clone();
    let task = tokio::spawn(async move {
        let codec = OpusCodec;
        let mut seq = 0u64;
        let mut tx_frames = 0u64;
        let mut rx_frames = 0u64;
        let mut ticks = 0u64;
        while !stop_for_task.load(Ordering::Relaxed) {
            while let Ok(inbound) = rx.try_recv() {
                rx_frames = rx_frames.saturating_add(1);
                let pcm = codec.decode_to_pcm_i16(&OpusPacket(inbound.payload));
                let packet = codec.encode_pcm_i16(&pcm);
                let frame = MediaFrame {
                    seq,
                    timestamp_us: seq.saturating_mul(20_000),
                    keyframe: true,
                    payload: packet.0,
                };
                if media.publish(&publish_track, frame).is_ok() {
                    tx_frames = tx_frames.saturating_add(1);
                    seq = seq.saturating_add(1);
                }
            }

            ticks = ticks.saturating_add(1);
            if ticks.is_multiple_of(5) {
                let _ = out_tx.send(OutMsg::CallDebug {
                    call_id: call_id.clone(),
                    tx_frames,
                    rx_frames,
                    rx_dropped: 0,
                });
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    Ok(CallWorker { stop, task })
}

fn start_echo_worker_with_transport(
    call_id: &str,
    session: &CallSessionParams,
    transport: CallMediaTransport,
    media_crypto: CallMediaCryptoContext,
    out_tx: mpsc::UnboundedSender<OutMsg>,
) -> anyhow::Result<CallWorker> {
    let Some(track) = call_audio_track_spec(session) else {
        return Err(anyhow!("call session missing opus audio track"));
    };

    let publish_track = TrackAddress {
        broadcast_path: broadcast_path(
            &session.broadcast_base,
            &media_crypto.local_participant_label,
        )
        .map_err(|e| anyhow!("invalid local broadcast path: {e}"))?,
        track_name: track.name.clone(),
    };
    let subscribe_track = TrackAddress {
        broadcast_path: broadcast_path(
            &session.broadcast_base,
            &media_crypto.peer_participant_label,
        )
        .map_err(|e| anyhow!("invalid peer broadcast path: {e}"))?,
        track_name: track.name.clone(),
    };
    tracing::info!(
        "[echo] publish_path={} subscribe_path={} track={}",
        publish_track.broadcast_path,
        subscribe_track.broadcast_path,
        publish_track.track_name,
    );
    let rx = transport
        .subscribe(&subscribe_track)
        .context("subscribe peer track for echo")?;

    let call_id = call_id.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_task = stop.clone();
    let task = tokio::task::spawn_blocking(move || {
        let codec = OpusCodec;
        let mut seq = 0u64;
        let mut tx_frames = 0u64;
        let mut rx_frames = 0u64;
        let mut rx_decrypt_dropped = 0u64;
        let mut ticks = 0u64;
        while !stop_for_task.load(Ordering::Relaxed) {
            while let Ok(inbound) = rx.try_recv() {
                let decrypted = match decrypt_frame(&inbound.payload, &media_crypto.rx_keys) {
                    Ok(v) => v,
                    Err(err) => {
                        rx_decrypt_dropped = rx_decrypt_dropped.saturating_add(1);
                        warn!(
                            "[pikachat] echo decrypt failed call_id={} err={err}",
                            call_id
                        );
                        continue;
                    }
                };
                rx_frames = rx_frames.saturating_add(1);

                let pcm = codec.decode_to_pcm_i16(&OpusPacket(decrypted.payload));
                let packet = codec.encode_pcm_i16(&pcm);
                let frame_counter = u32::try_from(seq).unwrap_or(u32::MAX);
                let encrypted = match encrypt_frame(
                    &packet.0,
                    &media_crypto.tx_keys,
                    FrameInfo {
                        counter: frame_counter,
                        group_seq: seq,
                        frame_idx: 0,
                        keyframe: true,
                    },
                ) {
                    Ok(v) => v,
                    Err(err) => {
                        warn!(
                            "[pikachat] echo encrypt failed call_id={} err={err}",
                            call_id
                        );
                        continue;
                    }
                };
                let frame = MediaFrame {
                    seq,
                    timestamp_us: seq.saturating_mul(20_000),
                    keyframe: true,
                    payload: encrypted,
                };
                if transport.publish(&publish_track, frame).is_ok() {
                    tx_frames = tx_frames.saturating_add(1);
                    seq = seq.saturating_add(1);
                }
            }

            ticks = ticks.saturating_add(1);
            if ticks.is_multiple_of(5) {
                let _ = out_tx.send(OutMsg::CallDebug {
                    call_id: call_id.clone(),
                    tx_frames,
                    rx_frames,
                    rx_dropped: rx_decrypt_dropped,
                });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });

    Ok(CallWorker { stop, task })
}

fn echo_mode_enabled() -> bool {
    std::env::var("PIKACHAT_ECHO_MODE")
        .map(|v| !v.trim().is_empty() && v.trim() != "0")
        .unwrap_or(false)
}

fn start_echo_worker(
    call_id: &str,
    session: &CallSessionParams,
    media_crypto: CallMediaCryptoContext,
    out_tx: mpsc::UnboundedSender<OutMsg>,
) -> anyhow::Result<CallWorker> {
    let transport = CallMediaTransport::for_session(session)?;
    start_echo_worker_with_transport(call_id, session, transport, media_crypto, out_tx)
}

fn start_data_worker(
    call_id: &str,
    session: &CallSessionParams,
    media_crypto: CallMediaCryptoContext,
    call_evt_tx: mpsc::UnboundedSender<CallWorkerEvent>,
) -> anyhow::Result<CallWorker> {
    let transport = CallMediaTransport::for_session(session)?;
    let mut subscriptions: Vec<(String, pika_media::subscription::MediaFrameSubscription)> =
        Vec::new();
    for track in &session.tracks {
        let subscribe_track = TrackAddress {
            broadcast_path: broadcast_path(
                &session.broadcast_base,
                &media_crypto.peer_participant_label,
            )
            .map_err(|e| anyhow!("invalid peer broadcast path: {e}"))?,
            track_name: track.name.clone(),
        };
        let sub = transport
            .subscribe(&subscribe_track)
            .context("subscribe peer track for data call")?;
        subscriptions.push((track.name.clone(), sub));
    }
    if subscriptions.is_empty() {
        return Err(anyhow!("call session must include at least one track"));
    }

    let call_id = call_id.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_task = stop.clone();
    let task = tokio::task::spawn_blocking(move || {
        while !stop_for_task.load(Ordering::Relaxed) {
            for (track_name, sub) in &subscriptions {
                while let Ok(inbound) = sub.try_recv() {
                    let decrypted = match decrypt_frame(&inbound.payload, &media_crypto.rx_keys) {
                        Ok(v) => v,
                        Err(err) => {
                            warn!(
                                "[marmotd] call data decrypt failed call_id={} track={} err={err}",
                                call_id, track_name
                            );
                            continue;
                        }
                    };
                    let _ = call_evt_tx.send(CallWorkerEvent::DataFrame {
                        call_id: call_id.clone(),
                        payload: decrypted.payload,
                        track_name: track_name.clone(),
                    });
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });

    Ok(CallWorker { stop, task })
}

fn publish_call_data(
    session: &CallSessionParams,
    media_crypto: &CallMediaCryptoContext,
    seq: u64,
    track_name: &str,
    payload: &[u8],
) -> anyhow::Result<u64> {
    let transport = CallMediaTransport::for_session(session)?;
    let publish_track = TrackAddress {
        broadcast_path: broadcast_path(
            &session.broadcast_base,
            &media_crypto.local_participant_label,
        )
        .map_err(|e| anyhow!("invalid local broadcast path: {e}"))?,
        track_name: track_name.to_string(),
    };
    let frame_counter =
        u32::try_from(seq).map_err(|_| anyhow!("call media tx counter exhausted"))?;
    let encrypted = encrypt_frame(
        payload,
        &media_crypto.tx_keys,
        FrameInfo {
            counter: frame_counter,
            group_seq: seq,
            frame_idx: 0,
            keyframe: true,
        },
    )
    .map_err(|e| anyhow!("encrypt call data failed: {e}"))?;
    let frame = MediaFrame {
        seq,
        timestamp_us: seq.saturating_mul(1_000),
        keyframe: true,
        payload: encrypted,
    };
    transport.publish(&publish_track, frame)?;
    Ok(seq.saturating_add(1))
}

pub async fn run_audio_echo_smoke(frame_count: u64) -> anyhow::Result<AudioEchoSmokeStats> {
    let call_id = "550e8400-e29b-41d4-a716-446655440000";
    let session = default_audio_call_session(call_id);
    let relay = InMemoryRelay::new();

    let mut peer = MediaSession::with_relay(
        SessionConfig {
            moq_url: session.moq_url.clone(),
            relay_auth: session.relay_auth.clone(),
        },
        relay.clone(),
    );
    let mut observer = MediaSession::with_relay(
        SessionConfig {
            moq_url: session.moq_url.clone(),
            relay_auth: session.relay_auth.clone(),
        },
        relay.clone(),
    );
    peer.connect().map_err(|e| anyhow::anyhow!("{e}"))?;
    observer.connect().map_err(|e| anyhow::anyhow!("{e}"))?;

    let peer_pubkey_hex = "11b9a894813efe60d39f8621ae9dc4c6d26de4732411c1cdf4bb15e88898a19c";
    let bot_pubkey_hex = "2284fc7b932b5dbbdaa2185c76a4e17a2ef928d4a82e29b812986b454b957f8f";
    let peer_track = TrackAddress {
        broadcast_path: broadcast_path(&session.broadcast_base, peer_pubkey_hex)
            .map_err(|e| anyhow!("peer broadcast path invalid: {e}"))?,
        track_name: "audio0".to_string(),
    };
    let bot_track = TrackAddress {
        broadcast_path: broadcast_path(&session.broadcast_base, bot_pubkey_hex)
            .map_err(|e| anyhow!("bot broadcast path invalid: {e}"))?,
        track_name: "audio0".to_string(),
    };
    let echoed_rx = observer
        .subscribe(&bot_track)
        .context("subscribe bot audio track")?;

    let (out_tx, _out_rx) = mpsc::unbounded_channel::<OutMsg>();
    let worker = start_echo_worker_with_relay(
        call_id,
        &session,
        relay,
        bot_pubkey_hex,
        peer_pubkey_hex,
        out_tx,
    )
    .context("start echo worker")?;

    let codec = OpusCodec;
    let mut sent_frames = 0u64;
    for i in 0..frame_count {
        let pcm = vec![i as i16, (i as i16).saturating_mul(-1)];
        let packet = codec.encode_pcm_i16(&pcm);
        let frame = MediaFrame {
            seq: i,
            timestamp_us: i * 20_000,
            keyframe: true,
            payload: packet.0,
        };
        let delivered = peer
            .publish(&peer_track, frame)
            .context("publish peer frame")?;
        if delivered > 0 {
            sent_frames = sent_frames.saturating_add(1);
        }
    }

    let mut echoed_frames = 0u64;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while echoed_frames < sent_frames && tokio::time::Instant::now() < deadline {
        while echoed_rx.try_recv().is_ok() {
            echoed_frames = echoed_frames.saturating_add(1);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    worker.stop().await;

    if echoed_frames != sent_frames {
        return Err(anyhow!(
            "audio echo frame mismatch: sent={sent_frames} echoed={echoed_frames}"
        ));
    }

    Ok(AudioEchoSmokeStats {
        sent_frames,
        echoed_frames,
    })
}

async fn publish_and_confirm_multi(
    client: &Client,
    relays: &[RelayUrl],
    event: &Event,
    label: &str,
) -> anyhow::Result<RelayUrl> {
    let out = client
        .send_event_to(relays.to_vec(), event)
        .await
        .with_context(|| format!("send_event_to failed ({label})"))?;
    if out.success.is_empty() {
        return Err(anyhow!(
            "event publish had no successful relays ({label}): {out:?}"
        ));
    }

    // Confirm we can fetch it back from at least one relay that reported success.
    for relay_url in out.success.iter().cloned() {
        let fetched = client
            .fetch_events_from(
                [relay_url.clone()],
                Filter::new().id(event.id),
                Duration::from_secs(5),
            )
            .await
            .with_context(|| format!("fetch_events_from failed ({label}) relay={relay_url}"))?;
        if fetched.iter().any(|e| e.id == event.id) {
            return Ok(relay_url);
        }
    }

    Err(anyhow!(
        "published event not found on any successful relay after send ({label}) id={}",
        event.id
    ))
}

async fn publish_without_confirm_multi(
    client: &Client,
    relays: &[RelayUrl],
    event: &Event,
    label: &str,
) -> anyhow::Result<()> {
    let out = client
        .send_event_to(relays.to_vec(), event)
        .await
        .with_context(|| format!("send_event_to failed ({label})"))?;
    if out.success.is_empty() {
        return Err(anyhow!(
            "event publish had no successful relays ({label}): {out:?}"
        ));
    }
    Ok(())
}

async fn stdout_writer(mut rx: mpsc::UnboundedReceiver<OutMsg>) -> anyhow::Result<()> {
    let mut stdout = tokio::io::stdout();
    while let Some(msg) = rx.recv().await {
        let line = serde_json::to_string(&msg).context("encode out msg")?;
        stdout.write_all(line.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }
    Ok(())
}

/// Forward OutMsg to a child process channel (used in --exec mode).
async fn forward_writer(
    mut rx: mpsc::UnboundedReceiver<OutMsg>,
    child_tx: mpsc::UnboundedSender<OutMsg>,
) -> anyhow::Result<()> {
    while let Some(msg) = rx.recv().await {
        // Log to stderr for debugging
        let line = serde_json::to_string(&msg).context("encode out msg")?;
        eprintln!("[pikachat] -> child: {line}");
        child_tx.send(msg).ok();
    }
    Ok(())
}

/// Write OutMsg JSONL to a child process's stdin.
async fn child_stdin_writer(
    mut rx: mpsc::UnboundedReceiver<OutMsg>,
    mut stdin: tokio::process::ChildStdin,
) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    while let Some(msg) = rx.recv().await {
        let line = serde_json::to_string(&msg).context("encode out msg")?;
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
    }
    Ok(())
}

fn parse_relay_list(relay: &str, relays_override: &[String]) -> anyhow::Result<Vec<RelayUrl>> {
    let mut out = Vec::new();
    if relays_override.is_empty() {
        out.push(RelayUrl::parse(relay).context("parse relay url")?);
        return Ok(out);
    }
    for r in relays_override {
        let trimmed = r.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push(RelayUrl::parse(trimmed).with_context(|| format!("parse relay url: {trimmed}"))?);
    }
    if out.is_empty() {
        return Err(anyhow!("relays list is empty"));
    }
    Ok(out)
}

const TYPING_INDICATOR_KIND: Kind = Kind::Custom(20_067);

fn is_typing_indicator(msg: &mdk_storage_traits::messages::types::Message) -> bool {
    msg.kind == TYPING_INDICATOR_KIND && msg.content == "typing"
}

fn event_h_tag_hex(ev: &Event) -> Option<String> {
    for t in ev.tags.iter() {
        if t.kind() == TagKind::h()
            && let Some(v) = t.content()
            && !v.is_empty()
        {
            return Some(v.to_string());
        }
    }
    None
}

pub async fn daemon_main(
    relays_arg: &[String],
    state_dir: &Path,
    giftwrap_lookback_sec: u64,
    allow_pubkeys: &[String],
    auto_accept_welcomes: bool,
    exec_cmd: Option<&str>,
) -> anyhow::Result<()> {
    crate::ensure_dir(state_dir).context("create state dir")?;

    // Use the first relay for initial connectivity check; all relays are added to the client below.
    let primary_relay = relays_arg
        .first()
        .map(|s| s.as_str())
        .unwrap_or("ws://127.0.0.1:18080");
    let skip_ready_check = std::env::var("MARMOTD_SKIP_RELAY_READY_CHECK")
        .ok()
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false);
    if !skip_ready_check {
        crate::check_relay_ready(primary_relay, Duration::from_secs(90))
            .await
            .with_context(|| format!("relay readiness check failed for {primary_relay}"))?;
    }

    let keys = crate::load_or_create_keys(&state_dir.join("identity.json"))?;
    let pubkey_hex = keys.public_key().to_hex().to_lowercase();
    let npub = keys
        .public_key()
        .to_bech32()
        .unwrap_or_else(|_| "<npub_err>".to_string());

    let (out_tx, out_rx) = mpsc::unbounded_channel::<OutMsg>();

    // When --exec is set, send OutMsg to the child process's stdin instead of real stdout.
    // (Normal mode continues to write JSONL to stdout for OpenClaw compatibility.)
    let (child_out_tx, child_out_rx) = mpsc::unbounded_channel::<OutMsg>();
    let has_exec = exec_cmd.is_some();

    {
        let out_rx_for_stdout = out_rx;
        let child_out_tx = child_out_tx.clone();
        tokio::spawn(async move {
            if has_exec {
                if let Err(err) = forward_writer(out_rx_for_stdout, child_out_tx).await {
                    eprintln!("[pikachat] forward writer failed: {err:#}");
                }
            } else if let Err(err) = stdout_writer(out_rx_for_stdout).await {
                eprintln!("[pikachat] stdout writer failed: {err:#}");
            }
        });
    }

    // Build pubkey allowlist. Empty = open (allow all).
    let allowlist: HashSet<String> = allow_pubkeys
        .iter()
        .map(|pk| pk.trim().to_lowercase())
        .filter(|pk| !pk.is_empty())
        .collect();
    let is_open = allowlist.is_empty();
    if is_open {
        eprintln!(
            "[pikachat] WARNING: no --allow-pubkey specified, accepting all senders (open mode)"
        );
    } else {
        eprintln!("[pikachat] allowlist: {} pubkeys", allowlist.len());
        for pk in &allowlist {
            eprintln!("[pikachat]   allow: {pk}");
        }
    }
    let sender_allowed = |pubkey_hex: &str| -> bool {
        is_open || allowlist.contains(&pubkey_hex.trim().to_lowercase())
    };

    out_tx
        .send(OutMsg::Ready {
            protocol_version: PROTOCOL_VERSION,
            pubkey: pubkey_hex.clone(),
            npub,
        })
        .ok();

    let mut relay_urls: Vec<RelayUrl> = Vec::new();
    for r in relays_arg {
        relay_urls
            .push(RelayUrl::parse(r.trim()).with_context(|| format!("parse relay url: {r}"))?);
    }
    if relay_urls.is_empty() {
        relay_urls
            .push(RelayUrl::parse("ws://127.0.0.1:18080").context("parse default relay url")?);
    }
    // Connect to the primary relay first, then add the rest.
    let client = crate::connect_client(&keys, primary_relay).await?;
    for r in relay_urls.iter().skip(1) {
        let _ = client.add_relay(r.clone()).await;
    }
    client.connect().await;
    let mdk = crate::new_mdk(state_dir, "daemon")?;

    let mut rx = client.notifications();

    // Subscribe to welcomes (GiftWrap kind 1059) addressed to us.
    // NOTE: `pubkey()` filter matches the event author, not the recipient.
    // GiftWraps can be authored by anyone, so we must filter by the recipient `p` tag.
    let since = Timestamp::now() - Duration::from_secs(giftwrap_lookback_sec);
    let gift_filter = Filter::new()
        .kind(Kind::GiftWrap)
        .custom_tag(SingleLetterTag::lowercase(Alphabet::P), pubkey_hex.clone())
        .since(since)
        .limit(200);
    let gift_sub = client.subscribe(gift_filter, None).await?;

    // Track which wrapper events and group message wrapper events we've already processed.
    let mut seen_welcomes: HashSet<EventId> = HashSet::new();
    let mut seen_group_events: HashSet<EventId> = HashSet::new();

    // Track group subscriptions.
    let mut group_subs: HashMap<SubscriptionId, String> = HashMap::new();
    let mut pending_call_invites: HashMap<String, PendingCallInvite> = HashMap::new();
    let mut pending_outgoing_call_invites: HashMap<String, PendingOutgoingCallInvite> =
        HashMap::new();
    let mut active_call: Option<ActiveCall> = None;
    let (call_evt_tx, mut call_evt_rx) = mpsc::unbounded_channel::<CallWorkerEvent>();

    // On startup, subscribe to any groups already present in state, so the daemon is restart-safe.
    if let Ok(groups) = mdk.get_groups() {
        for g in groups.iter() {
            let nostr_group_id_hex = hex::encode(g.nostr_group_id);
            match crate::subscribe_group_msgs(&client, &nostr_group_id_hex).await {
                Ok(sid) => {
                    group_subs.insert(sid.clone(), nostr_group_id_hex.clone());
                }
                Err(err) => {
                    warn!(
                        "[pikachat] subscribe existing group failed nostr_group_id={nostr_group_id_hex} err={err:#}"
                    );
                }
            }
        }
    }

    // command reader (stdin or child process stdout)
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<InCmd>();
    let cmd_tx_for_auto = cmd_tx.clone();

    if let Some(exec_cmd) = exec_cmd {
        // --exec mode: spawn child, pipe OutMsg to its stdin, read InCmd from its stdout
        eprintln!("[pikachat] exec mode: spawning child: {exec_cmd}");
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(exec_cmd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .context("spawn --exec child")?;

        let child_stdin = child.stdin.take().context("child stdin")?;
        let child_stdout = child.stdout.take().context("child stdout")?;

        // Write OutMsg JSONL to child's stdin
        tokio::spawn(async move {
            if let Err(err) = child_stdin_writer(child_out_rx, child_stdin).await {
                eprintln!("[pikachat] child stdin writer failed: {err:#}");
            }
        });

        // Read InCmd JSONL from child's stdout
        let cmd_tx_clone = cmd_tx.clone();
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(child_stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<InCmd>(trimmed) {
                    Ok(cmd) => {
                        cmd_tx_clone.send(cmd).ok();
                    }
                    Err(err) => {
                        eprintln!("[pikachat] invalid cmd from child: {err} line={trimmed}");
                    }
                }
            }
            eprintln!("[pikachat] child stdout closed");
        });

        // Wait for child to exit in background
        tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => eprintln!("[pikachat] child exited: {status}"),
                Err(err) => eprintln!("[pikachat] child wait failed: {err:#}"),
            }
        });
    } else {
        // Normal mode: read from real stdin
        drop(child_out_rx); // not used
        tokio::spawn(async move {
            let stdin = tokio::io::stdin();
            let mut lines = tokio::io::BufReader::new(stdin).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<InCmd>(trimmed) {
                    Ok(cmd) => {
                        cmd_tx.send(cmd).ok();
                    }
                    Err(err) => {
                        eprintln!("[pikachat] invalid cmd json: {err} line={trimmed}");
                    }
                }
            }
        });
    }

    let mut shutdown = false;
    while !shutdown {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break; };
                match cmd {
                    InCmd::PublishKeypackage { request_id, relays } => {
                        let selected = match parse_relay_list(primary_relay, &relays) {
                            Ok(v) => v,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "bad_relays", e.to_string())).ok();
                                continue;
                            }
                        };
                        relay_urls = selected.clone();
                        // Ensure client knows about relays.
                        for r in selected.iter() {
                            let _ = client.add_relay(r.clone()).await;
                        }
                        client.connect().await;

                        let (kp_content, kp_tags, _hash_ref) = match mdk
                            .create_key_package_for_event(&keys.public_key(), selected.clone())
                        {
                            Ok(v) => v,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "mdk_error", format!("{e:#}"))).ok();
                                continue;
                            }
                        };
                        // Many public relays reject NIP-70 "protected" events. Keypackages and MLS
                        // wrapper events are safe to publish without protection, so strip it to keep
                        // public-relay deployments working.
                        let kp_tags: Tags = kp_tags
                            .into_iter()
                            .filter(|t: &Tag| !matches!(t.kind(), TagKind::Protected))
                            .collect();
                        let ev = match EventBuilder::new(Kind::MlsKeyPackage, kp_content)
                            .tags(kp_tags)
                            .sign_with_keys(&keys)
                        {
                            Ok(v) => v,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "sign_failed", format!("{e:#}"))).ok();
                                continue;
                            }
                        };

                        match publish_without_confirm_multi(&client, &selected, &ev, "keypackage")
                            .await
                        {
                            Ok(_relay_confirmed) => {
                                out_tx.send(out_ok(request_id, Some(json!({"event_id": ev.id.to_hex()})))).ok();
                                out_tx.send(OutMsg::KeypackagePublished { event_id: ev.id.to_hex() }).ok();
                            }
                            Err(e) => {
                                out_tx.send(out_error(request_id, "publish_failed", format!("{e:#}"))).ok();
                            }
                        };
                    }
                    InCmd::SetRelays { request_id, relays } => {
                        match parse_relay_list(primary_relay, &relays) {
                            Ok(v) => {
                                relay_urls = v.clone();
                                for r in v.iter() {
                                    let _ = client.add_relay(r.clone()).await;
                                }
                                client.connect().await;
                                out_tx.send(out_ok(request_id, Some(json!({"relays": v.iter().map(|r| r.to_string()).collect::<Vec<_>>()})))).ok();
                            }
                            Err(e) => {
                                out_tx.send(out_error(request_id, "bad_relays", e.to_string())).ok();
                            }
                        }
                    }
                    InCmd::ListPendingWelcomes { request_id } => {
                        match mdk.get_pending_welcomes(None) {
                            Ok(list) => {
                                let out = list
                                    .iter()
                                    .map(|w| {
                                    json!({
                                        "wrapper_event_id": w.wrapper_event_id.to_hex(),
                                        "welcome_event_id": w.id.to_hex(),
                                        "from_pubkey": w.welcomer.to_hex().to_lowercase(),
                                        "nostr_group_id": hex::encode(w.nostr_group_id),
                                        "group_name": w.group_name,
                                    })
                                    })
                                    .collect::<Vec<_>>();
                                let _ = out_tx.send(out_ok(request_id, Some(json!({ "welcomes": out }))));
                            }
                            Err(e) => {
                                let _ = out_tx.send(out_error(request_id, "mdk_error", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::AcceptWelcome { request_id, wrapper_event_id } => {
                        let wrapper = match EventId::from_hex(&wrapper_event_id) {
                            Ok(id) => id,
                            Err(_) => {
                                out_tx.send(out_error(request_id, "bad_event_id", "wrapper_event_id must be hex")).ok();
                                continue;
                            }
                        };
                        match mdk.get_pending_welcomes(None) {
                            Ok(list) => {
                                let found = list.into_iter().find(|w| w.wrapper_event_id == wrapper);
                                let Some(w) = found else {
                                    out_tx.send(out_error(request_id, "not_found", "pending welcome not found")).ok();
                                    continue;
                                };
                                let nostr_group_id_hex = hex::encode(w.nostr_group_id);
                                let mls_group_id_hex = hex::encode(w.mls_group_id.as_slice());
                                match mdk.accept_welcome(&w) {
                                    Ok(_) => {
                                        // Subscribe to group messages for this group.
                                        match crate::subscribe_group_msgs(&client, &nostr_group_id_hex).await {
                                            Ok(sid) => {
                                                group_subs.insert(sid.clone(), nostr_group_id_hex.clone());
                                            }
                                            Err(err) => {
                                                warn!("[pikachat] subscribe group msgs failed: {err:#}");
                                            }
                                        }

                                        // Backfill recent group messages, but dedupe by wrapper id.
                                        if let Some(relay0) = relay_urls.first().cloned() {
                                            let filter = Filter::new()
                                                .kind(Kind::MlsGroupMessage)
                                                .custom_tag(SingleLetterTag::lowercase(Alphabet::H), &nostr_group_id_hex)
                                                .since(Timestamp::now() - Duration::from_secs(60 * 60))
                                                .limit(200);
                                            if let Ok(events) = client.fetch_events_from([relay0], filter, Duration::from_secs(10)).await {
                                                for ev in events.iter() {
                                                    if !seen_group_events.insert(ev.id) {
                                                        continue;
                                                    }
                                                    if let Ok(MessageProcessingResult::ApplicationMessage(msg)) = mdk.process_message(ev) {
                                                        if !sender_allowed(&msg.pubkey.to_hex()) {
                                                            continue;
                                                        }
                                                        if is_typing_indicator(&msg) {
                                                            continue;
                                                        }
                                                        // Backfill: parse imeta tags but skip download (too slow for bulk history)
                                                        let media: Vec<MediaAttachmentOut> = {
                                                            let mgr = mdk.media_manager(msg.mls_group_id.clone());
                                                            msg.tags.iter()
                                                                .filter(|t| is_imeta_tag(t))
                                                                .filter_map(|t| mgr.parse_imeta_tag(t).ok())
                                                                .map(media_ref_to_attachment)
                                                                .collect()
                                                        };
                                                        out_tx.send(OutMsg::MessageReceived{
                                                            nostr_group_id: event_h_tag_hex(ev).unwrap_or_else(|| nostr_group_id_hex.clone()),
                                                            from_pubkey: msg.pubkey.to_hex().to_lowercase(),
                                                            content: msg.content,
                                                            created_at: msg.created_at.as_secs(),
                                                            message_id: msg.id.to_hex(),
                                                            media,
                                                        }).ok();
                                                    }
                                                }
                                            }
                                        }

                                        out_tx.send(out_ok(request_id, Some(json!({
                                            "nostr_group_id": nostr_group_id_hex,
                                            "mls_group_id": mls_group_id_hex,
                                        })))).ok();
                                        let member_count = mdk.get_members(&w.mls_group_id).map(|m| m.len() as u32).unwrap_or(0);
                                        out_tx.send(OutMsg::GroupJoined { nostr_group_id: nostr_group_id_hex, mls_group_id: mls_group_id_hex, member_count }).ok();
                                    }
                                    Err(e) => {
                                        out_tx.send(out_error(request_id, "mdk_error", format!("{e:#}"))).ok();
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = out_tx.send(out_error(request_id, "mdk_error", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::ListGroups { request_id } => {
                        match mdk.get_groups() {
                            Ok(gs) => {
                                let out = gs.iter().map(|g| {
                                    let member_count = mdk.get_members(&g.mls_group_id).map(|m| m.len() as u32).unwrap_or(0);
                                    json!({
                                        "nostr_group_id": hex::encode(g.nostr_group_id),
                                        "mls_group_id": hex::encode(g.mls_group_id.as_slice()),
                                        "name": g.name,
                                        "description": g.description,
                                        "member_count": member_count,
                                    })
                                }).collect::<Vec<_>>();
                                let _ = out_tx.send(out_ok(request_id, Some(json!({"groups": out}))));
                            }
                            Err(e) => {
                                let _ = out_tx.send(out_error(request_id, "mdk_error", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::SendMessage { request_id, nostr_group_id, content } => {
                        let mls_group_id = match resolve_group(&mdk, &nostr_group_id) {
                            Ok(id) => id,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "bad_group_id", format!("{e:#}"))).ok();
                                continue;
                            }
                        };
                        let rumor = EventBuilder::new(Kind::ChatMessage, content).build(keys.public_key());
                        match sign_and_publish(&client, &relay_urls, &mdk, &keys, &mls_group_id, rumor, "daemon_send").await {
                            Ok(ev) => {
                                let _ = out_tx.send(out_ok(request_id, Some(json!({"event_id": ev.id.to_hex()}))));
                            }
                            Err(e) => {
                                let _ = out_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::SendMedia {
                        request_id,
                        nostr_group_id,
                        file_path,
                        mime_type,
                        filename,
                        caption,
                        blossom_servers,
                    } => {
                        let mls_group_id = match resolve_group(&mdk, &nostr_group_id) {
                            Ok(id) => id,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "bad_group_id", format!("{e:#}"))).ok();
                                continue;
                            }
                        };

                        // Read and validate file
                        let path = std::path::Path::new(&file_path);
                        let bytes = match std::fs::read(path) {
                            Ok(b) => b,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "file_error", format!("read {file_path}: {e}"))).ok();
                                continue;
                            }
                        };
                        if bytes.is_empty() {
                            out_tx.send(out_error(request_id, "file_error", "file is empty")).ok();
                            continue;
                        }
                        if bytes.len() > MAX_CHAT_MEDIA_BYTES {
                            out_tx.send(out_error(request_id, "file_error", "file too large (max 32 MB)")).ok();
                            continue;
                        }

                        // Resolve mime type and filename
                        let resolved_filename = filename
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(ToOwned::to_owned)
                            .or_else(|| {
                                path.file_name()
                                    .and_then(|f| f.to_str())
                                    .map(ToOwned::to_owned)
                            })
                            .unwrap_or_else(|| "file.bin".to_string());
                        let resolved_mime = mime_type
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .or_else(|| mime_from_extension(path))
                            .unwrap_or("application/octet-stream")
                            .to_string();

                        // Encrypt
                        let manager = mdk.media_manager(mls_group_id.clone());
                        let mut upload = match manager.encrypt_for_upload_with_options(
                            &bytes,
                            &resolved_mime,
                            &resolved_filename,
                            &MediaProcessingOptions::default(),
                        ) {
                            Ok(u) => u,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "encrypt_error", format!("{e:#}"))).ok();
                                continue;
                            }
                        };
                        let encrypted_data = std::mem::take(&mut upload.encrypted_data);
                        let expected_hash_hex = hex::encode(upload.encrypted_hash);

                        // Upload to Blossom
                        let upload_servers = blossom_servers_or_default(&blossom_servers);
                        let mut uploaded_url: Option<String> = None;
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
                                    Some(&keys),
                                )
                                .await
                            {
                                Ok(d) => d,
                                Err(e) => {
                                    last_error = Some(format!("{server}: {e}"));
                                    continue;
                                }
                            };
                            let descriptor_hash_hex = descriptor.sha256.to_string();
                            if !descriptor_hash_hex.eq_ignore_ascii_case(&expected_hash_hex) {
                                last_error = Some(format!(
                                    "{server}: hash mismatch (expected {expected_hash_hex}, got {descriptor_hash_hex})"
                                ));
                                continue;
                            }
                            uploaded_url = Some(descriptor.url.to_string());
                            break;
                        }
                        let Some(uploaded_url) = uploaded_url else {
                            out_tx.send(out_error(
                                request_id,
                                "upload_failed",
                                format!("blossom upload failed: {}", last_error.unwrap_or_else(|| "unknown".into())),
                            )).ok();
                            continue;
                        };

                        // Build imeta tag and message
                        let imeta_tag = manager.create_imeta_tag(&upload, &uploaded_url);
                        let rumor = EventBuilder::new(Kind::ChatMessage, &caption)
                            .tag(imeta_tag)
                            .build(keys.public_key());
                        match sign_and_publish(&client, &relay_urls, &mdk, &keys, &mls_group_id, rumor, "daemon_send_media").await {
                            Ok(ev) => {
                                let _ = out_tx.send(out_ok(request_id, Some(json!({
                                    "event_id": ev.id.to_hex(),
                                    "uploaded_url": uploaded_url,
                                    "original_hash_hex": hex::encode(upload.original_hash),
                                }))));
                            }
                            Err(e) => {
                                let _ = out_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::SendTyping { request_id, nostr_group_id } => {
                        let mls_group_id = match resolve_group(&mdk, &nostr_group_id) {
                            Ok(id) => id,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "bad_group_id", format!("{e:#}"))).ok();
                                continue;
                            }
                        };

                        let expires_at = Timestamp::now().as_secs() + 10;
                        let rumor = UnsignedEvent::new(
                            keys.public_key(),
                            Timestamp::now(),
                            TYPING_INDICATOR_KIND,
                            [
                                Tag::custom(TagKind::d(), ["pika"]),
                                Tag::expiration(Timestamp::from_secs(expires_at)),
                            ],
                            "typing",
                        );

                        let wrapper = match mdk.create_message(&mls_group_id, rumor) {
                            Ok(ev) => ev,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "mdk_error", format!("{e:#}"))).ok();
                                continue;
                            }
                        };

                        if relay_urls.is_empty() {
                            out_tx.send(out_error(request_id, "bad_relays", "no relays configured")).ok();
                            continue;
                        }
                        // Fire-and-forget: typing indicators are best-effort
                        let client_clone = client.clone();
                        let relay_urls_clone = relay_urls.clone();
                        let out_tx_clone = out_tx.clone();
                        tokio::spawn(async move {
                            match publish_and_confirm_multi(&client_clone, &relay_urls_clone, &wrapper, "daemon_typing").await {
                                Ok(_) => {
                                    let _ = out_tx_clone.send(out_ok(request_id, None));
                                }
                                Err(e) => {
                                    let _ = out_tx_clone.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                                }
                            }
                        });
                    }
                    InCmd::InviteCall {
                        request_id,
                        nostr_group_id,
                        peer_pubkey,
                        call_id,
                        moq_url,
                        broadcast_base,
                        track_name,
                        track_codec,
                        relay_auth,
                    } => {
                        if active_call.is_some() {
                            let _ = out_tx.send(out_error(request_id, "busy", "call already active"));
                            continue;
                        }
                        let peer_pubkey = match PublicKey::parse(peer_pubkey.trim()) {
                            Ok(pk) => pk,
                            Err(e) => {
                                let _ = out_tx.send(out_error(
                                    request_id,
                                    "bad_pubkey",
                                    format!("invalid peer_pubkey: {e}"),
                                ));
                                continue;
                            }
                        };
                        let peer_pubkey_hex = peer_pubkey.to_hex().to_lowercase();
                        let call_id = call_id
                            .filter(|id| !id.trim().is_empty())
                            .unwrap_or_else(|| {
                                let a = rand::random::<u32>();
                                let b = rand::random::<u16>();
                                let c = rand::random::<u16>();
                                let d = rand::random::<u16>();
                                let e = rand::random::<u64>() & 0x0000_FFFF_FFFF_FFFF;
                                format!("{a:08x}-{b:04x}-{c:04x}-{d:04x}-{e:012x}")
                            });
                        let track_name = track_name
                            .filter(|v| !v.trim().is_empty())
                            .unwrap_or_else(|| "pty0".to_string());
                        let track_codec = track_codec
                            .filter(|v| !v.trim().is_empty())
                            .unwrap_or_else(|| "bytes".to_string());
                        let mut session = CallSessionParams {
                            moq_url,
                            broadcast_base: broadcast_base
                                .filter(|v| !v.trim().is_empty())
                                .unwrap_or_else(|| format!("pika/pty/{call_id}")),
                            relay_auth: relay_auth.unwrap_or_default(),
                            tracks: vec![CallTrackSpec {
                                name: track_name,
                                codec: track_codec,
                                sample_rate: 1,
                                channels: 1,
                                frame_ms: 1,
                            }],
                        };
                        if session.relay_auth.trim().is_empty() {
                            match derive_relay_auth_token(
                                &mdk,
                                &nostr_group_id,
                                &call_id,
                                &session,
                                &pubkey_hex,
                                &peer_pubkey_hex,
                            ) {
                                Ok(token) => {
                                    session.relay_auth = token;
                                }
                                Err(e) => {
                                    let _ = out_tx.send(out_error(
                                        request_id,
                                        "runtime_error",
                                        format!("derive relay auth token failed: {e:#}"),
                                    ));
                                    continue;
                                }
                            }
                        }
                        match send_call_invite_with_retry(
                            &client,
                            &relay_urls,
                            &mdk,
                            &keys,
                            &nostr_group_id,
                            &call_id,
                            &session,
                            3,
                        )
                        .await {
                            Ok(()) => {
                                pending_outgoing_call_invites.insert(
                                    call_id.clone(),
                                    PendingOutgoingCallInvite {
                                        call_id: call_id.clone(),
                                        peer_pubkey: peer_pubkey_hex,
                                        nostr_group_id: nostr_group_id.clone(),
                                    },
                                );
                                let _ = out_tx.send(out_ok(
                                    request_id,
                                    Some(json!({
                                        "call_id": call_id,
                                        "nostr_group_id": nostr_group_id,
                                        "session": session,
                                    })),
                                ));
                            }
                            Err(e) => {
                                let _ =
                                    out_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::AcceptCall { request_id, call_id } => {
                        if active_call.is_some() {
                            let _ = out_tx.send(out_error(request_id, "busy", "call already active"));
                            continue;
                        }
                        let Some(invite) = pending_call_invites.remove(&call_id) else {
                            let _ = out_tx.send(out_error(request_id, "not_found", "pending call invite not found"));
                            continue;
                        };
                        if let Err(err) = validate_relay_auth_token(
                            &mdk,
                            &invite.nostr_group_id,
                            &invite.call_id,
                            &invite.session,
                            &pubkey_hex,
                            &invite.from_pubkey,
                        ) {
                            let _ = send_call_signal(
                                &client,
                                &relay_urls,
                                &mdk,
                                &keys,
                                &invite.nostr_group_id,
                                &invite.call_id,
                                OutgoingCallSignal::Reject { reason: "auth_failed" },
                                "call_reject_auth_failed",
                            )
                            .await;
                            let _ = out_tx.send(out_error(request_id, "auth_failed", format!("{err:#}")));
                            continue;
                        }
                        let media_crypto = match derive_mls_media_crypto_context(
                            &mdk,
                            &invite.nostr_group_id,
                            &invite.call_id,
                            &invite.session,
                            &pubkey_hex,
                            &invite.from_pubkey,
                        ) {
                            Ok(v) => v,
                            Err(e) => {
                                let _ = out_tx.send(out_error(request_id, "runtime_error", format!("{e:#}")));
                                continue;
                            }
                        };

                        match send_call_signal(
                            &client,
                            &relay_urls,
                            &mdk,
                            &keys,
                            &invite.nostr_group_id,
                            &invite.call_id,
                            OutgoingCallSignal::Accept(&invite.session),
                            "call_accept",
                        ).await {
                            Ok(()) => {}
                            Err(e) => {
                                let _ = out_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                                continue;
                            }
                        }

                        let mode = active_call_mode(&invite.session);
                        let worker = match mode {
                            ActiveCallMode::Audio => {
                                if echo_mode_enabled() {
                                    match start_echo_worker(
                                        &invite.call_id,
                                        &invite.session,
                                        media_crypto.clone(),
                                        out_tx.clone(),
                                    ) {
                                        Ok(v) => v,
                                        Err(e) => {
                                            let _ = out_tx.send(out_error(
                                                request_id,
                                                "runtime_error",
                                                format!("{e:#}"),
                                            ));
                                            continue;
                                        }
                                    }
                                } else {
                                    match start_stt_worker(
                                        &invite.call_id,
                                        &invite.session,
                                        media_crypto.clone(),
                                        out_tx.clone(),
                                        call_evt_tx.clone(),
                                    ) {
                                        Ok(v) => v,
                                        Err(e) => {
                                            let _ = out_tx.send(out_error(
                                                request_id,
                                                "runtime_error",
                                                format!("{e:#}"),
                                            ));
                                            continue;
                                        }
                                    }
                                }
                            }
                            ActiveCallMode::Data => match start_data_worker(
                                &invite.call_id,
                                &invite.session,
                                media_crypto.clone(),
                                call_evt_tx.clone(),
                            ) {
                                Ok(v) => v,
                                Err(e) => {
                                    let _ = out_tx.send(out_error(
                                        request_id,
                                        "runtime_error",
                                        format!("{e:#}"),
                                    ));
                                    continue;
                                }
                            },
                        };

                        active_call = Some(ActiveCall {
                            call_id: invite.call_id.clone(),
                            nostr_group_id: invite.nostr_group_id.clone(),
                            session: invite.session.clone(),
                            mode,
                            media_crypto,
                            next_voice_seq: 0,
                            next_data_seq: 0,
                            worker,
                        });
                        if let Some(call) = active_call.as_ref() {
                            tracing::info!(
                                "[pikachat] call active call_id={} group={} moq_url={} broadcast_base={} local_label={} peer_label={}",
                                call.call_id,
                                call.nostr_group_id,
                                call.session.moq_url,
                                call.session.broadcast_base,
                                call.media_crypto.local_participant_label,
                                call.media_crypto.peer_participant_label
                            );
                        }
                        let _ = out_tx.send(out_ok(request_id, Some(json!({
                            "call_id": invite.call_id,
                            "nostr_group_id": invite.nostr_group_id,
                        }))));
                        let _ = out_tx.send(OutMsg::CallSessionStarted {
                            call_id: invite.call_id,
                            nostr_group_id: invite.nostr_group_id,
                            from_pubkey: invite.from_pubkey,
                        });
                    }
                    InCmd::RejectCall {
                        request_id,
                        call_id,
                        reason,
                    } => {
                        let Some(invite) = pending_call_invites.remove(&call_id) else {
                            let _ = out_tx.send(out_error(request_id, "not_found", "pending call invite not found"));
                            continue;
                        };
                        match send_call_signal(
                            &client,
                            &relay_urls,
                            &mdk,
                            &keys,
                            &invite.nostr_group_id,
                            &invite.call_id,
                            OutgoingCallSignal::Reject { reason: &reason },
                            "call_reject",
                        ).await {
                            Ok(()) => {
                                let _ = out_tx.send(out_ok(request_id, Some(json!({ "call_id": invite.call_id }))));
                            }
                            Err(e) => {
                                let _ = out_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::EndCall {
                        request_id,
                        call_id,
                        reason,
                    } => {
                        let Some(current) = active_call.take() else {
                            let _ = out_tx.send(out_error(request_id, "not_found", "active call not found"));
                            continue;
                        };
                        if current.call_id != call_id {
                            active_call = Some(current);
                            let _ = out_tx.send(out_error(request_id, "not_found", "active call id mismatch"));
                            continue;
                        }

                        let _ = send_call_signal(
                            &client,
                            &relay_urls,
                            &mdk,
                            &keys,
                            &current.nostr_group_id,
                            &call_id,
                            OutgoingCallSignal::End { reason: &reason },
                            "call_end",
                        )
                        .await;
                        current.worker.stop().await;
                        let _ = out_tx.send(out_ok(request_id, Some(json!({ "call_id": call_id }))));
                        let _ = out_tx.send(OutMsg::CallSessionEnded {
                            call_id,
                            reason,
                        });
                    }
                    InCmd::SendAudioResponse {
                        request_id,
                        call_id,
                        tts_text,
                    } => {
                        let Some(current) = active_call.as_mut() else {
                            let _ = out_tx.send(out_error(request_id, "not_found", "active call not found"));
                            continue;
                        };
                        if current.call_id != call_id {
                            let _ = out_tx.send(out_error(request_id, "not_found", "active call id mismatch"));
                            continue;
                        }
                        if current.mode != ActiveCallMode::Audio {
                            let _ = out_tx.send(out_error(
                                request_id,
                                "bad_request",
                                "active call is not an audio call",
                            ));
                            continue;
                        }
                        if tts_text.trim().is_empty() {
                            let _ = out_tx.send(out_error(request_id, "bad_request", "tts_text must not be empty"));
                            continue;
                        }
                        tracing::info!(
                            "[pikachat] send_audio_response start call_id={} text_len={}",
                            call_id,
                            tts_text.len()
                        );
                        match publish_tts_audio_response(
                            &current.session,
                            &current.media_crypto,
                            current.next_voice_seq,
                            &tts_text,
                        ) {
                            Ok(stats) => {
                                current.next_voice_seq = stats.next_seq;
                                tracing::info!(
                                    "[pikachat] send_audio_response ok call_id={} frames={} next_seq={}",
                                    call_id,
                                    stats.frames_published,
                                    stats.next_seq
                                );
                                let publish_path = broadcast_path(
                                    &current.session.broadcast_base,
                                    &current.media_crypto.local_participant_label,
                                )
                                .ok();
                                let subscribe_path = broadcast_path(
                                    &current.session.broadcast_base,
                                    &current.media_crypto.peer_participant_label,
                                )
                                .ok();
                                let track_name = call_audio_track_spec(&current.session)
                                    .map(|t| t.name.clone())
                                    .unwrap_or_default();
                                let _ = out_tx.send(out_ok(
                                    request_id,
                                    Some(json!({
                                        "call_id": call_id,
                                        "frames_published": stats.frames_published,
                                        "publish_path": publish_path,
                                        "subscribe_path": subscribe_path,
                                        "track": track_name,
                                        "local_label": current.media_crypto.local_participant_label,
                                        "peer_label": current.media_crypto.peer_participant_label,
                                    })),
                                ));
                            }
                            Err(err) => {
                                warn!(
                                    "[pikachat] send_audio_response failed call_id={} err={err:#}",
                                    call_id
                                );
                                let _ = out_tx.send(out_error(
                                    request_id,
                                    "runtime_error",
                                    format!("tts publish failed: {err:#}"),
                                ));
                            }
                        }
                    }
                    InCmd::SendAudioFile {
                        request_id,
                        call_id,
                        audio_path,
                        sample_rate,
                        channels,
                    } => {
                        let Some(current) = active_call.as_mut() else {
                            let _ = out_tx.send(out_error(request_id, "not_found", "active call not found"));
                            continue;
                        };
                        if current.call_id != call_id {
                            let _ = out_tx.send(out_error(request_id, "not_found", "active call id mismatch"));
                            continue;
                        }
                        if current.mode != ActiveCallMode::Audio {
                            let _ = out_tx.send(out_error(
                                request_id,
                                "bad_request",
                                "active call is not an audio call",
                            ));
                            continue;
                        }
                        tracing::info!(
                            "[pikachat] send_audio_file start call_id={} path={} sample_rate={} channels={}",
                            call_id, audio_path, sample_rate, channels
                        );
                        let raw_bytes = match std::fs::read(&audio_path) {
                            Ok(b) => b,
                            Err(err) => {
                                let _ = out_tx.send(out_error(
                                    request_id,
                                    "io_error",
                                    format!("failed to read audio file {audio_path}: {err}"),
                                ));
                                continue;
                            }
                        };
                        let pcm_i16: Vec<i16> = raw_bytes
                            .chunks_exact(2)
                            .map(|c| i16::from_le_bytes([c[0], c[1]]))
                            .collect();
                        let tts_pcm = crate::call_tts::TtsPcm {
                            sample_rate_hz: sample_rate,
                            channels,
                            pcm_i16,
                        };
                        // Reserve the sequence range upfront so the main loop
                        // can continue processing commands while audio publishes.
                        let session = current.session.clone();
                        let media_crypto = current.media_crypto.clone();
                        let start_seq = current.next_voice_seq;
                        // Estimate frames so we can reserve the seq range.
                        let track_sample_rate = call_audio_track_spec(&current.session)
                            .map(|t| t.sample_rate)
                            .unwrap_or(48_000);
                        let track_frame_ms = call_audio_track_spec(&current.session)
                            .map(|t| t.frame_ms)
                            .unwrap_or(20);
                        let resampled_len = ((tts_pcm.pcm_i16.len() as u64)
                            .saturating_mul(track_sample_rate as u64)
                            / (tts_pcm.sample_rate_hz as u64).max(1)) as usize;
                        let frame_samples = ((track_sample_rate as usize) * (track_frame_ms as usize) / 1000).max(1);
                        let estimated_frames = resampled_len.div_ceil(frame_samples);
                        current.next_voice_seq = start_seq.saturating_add(estimated_frames as u64);

                        let evt_tx = call_evt_tx.clone();
                        std::thread::spawn(move || {
                            let result = publish_pcm_audio_response(
                                &session,
                                &media_crypto,
                                start_seq,
                                tts_pcm,
                            );
                            let _ = evt_tx.send(CallWorkerEvent::AudioPublished {
                                call_id,
                                request_id,
                                result,
                            });
                        });
                    }
                    InCmd::SendCallData {
                        request_id,
                        call_id,
                        payload_hex,
                        track_name,
                    } => {
                        let Some(current) = active_call.as_mut() else {
                            let _ = out_tx.send(out_error(request_id, "not_found", "active call not found"));
                            continue;
                        };
                        if current.call_id != call_id {
                            let _ = out_tx.send(out_error(request_id, "not_found", "active call id mismatch"));
                            continue;
                        }
                        if current.mode != ActiveCallMode::Data {
                            let _ = out_tx.send(out_error(
                                request_id,
                                "bad_request",
                                "active call is not a data call",
                            ));
                            continue;
                        }
                        let payload = match hex::decode(payload_hex.trim()) {
                            Ok(v) => v,
                            Err(_) => {
                                let _ = out_tx.send(out_error(
                                    request_id,
                                    "bad_request",
                                    "payload_hex must be valid hex",
                                ));
                                continue;
                            }
                        };
                        let track_name = match track_name {
                            Some(name) if !name.trim().is_empty() => name,
                            _ => match call_primary_track_name(&current.session) {
                                Ok(name) => name.to_string(),
                                Err(err) => {
                                    let _ = out_tx.send(out_error(
                                        request_id,
                                        "runtime_error",
                                        format!("{err:#}"),
                                    ));
                                    continue;
                                }
                            },
                        };
                        match publish_call_data(
                            &current.session,
                            &current.media_crypto,
                            current.next_data_seq,
                            &track_name,
                            &payload,
                        ) {
                            Ok(next_seq) => {
                                current.next_data_seq = next_seq;
                                let _ = out_tx.send(out_ok(request_id, None));
                            }
                            Err(err) => {
                                let _ = out_tx.send(out_error(
                                    request_id,
                                    "runtime_error",
                                    format!("publish call data failed: {err:#}"),
                                ));
                            }
                        }
                    }
                    InCmd::InitGroup { request_id, peer_pubkey: peer_str, group_name } => {
                        let peer_pubkey = match PublicKey::parse(&peer_str) {
                            Ok(pk) => pk,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "bad_pubkey", format!("invalid peer_pubkey: {e}"))).ok();
                                continue;
                            }
                        };

                        if relay_urls.is_empty() {
                            out_tx.send(out_error(request_id, "bad_relays", "no relays configured")).ok();
                            continue;
                        }

                        // Fetch latest peer key package from configured relays.
                        let kp_filter = Filter::new()
                            .author(peer_pubkey)
                            .kind(Kind::MlsKeyPackage)
                            .limit(1);
                        let kp_events = match client
                            .fetch_events_from(relay_urls.clone(), kp_filter, Duration::from_secs(10))
                            .await
                        {
                            Ok(evs) => evs,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "fetch_failed", format!("fetch key package: {e:#}"))).ok();
                                continue;
                            }
                        };

                        let peer_kp = match kp_events.into_iter().next() {
                            Some(ev) => ev,
                            None => {
                                out_tx.send(out_error(request_id, "no_key_packages", "no key package found for peer")).ok();
                                continue;
                            }
                        };

                        // Create group.
                        let config = NostrGroupConfigData::new(
                            group_name,
                            String::new(),
                            None,
                            None,
                            None,
                            relay_urls.clone(),
                            vec![keys.public_key(), peer_pubkey],
                        );

                        let group_result = match mdk.create_group(&keys.public_key(), vec![peer_kp], config) {
                            Ok(r) => r,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "mdk_error", format!("create_group: {e:#}"))).ok();
                                continue;
                            }
                        };

                        let nostr_group_id_hex = hex::encode(group_result.group.nostr_group_id);
                        let mls_group_id_hex = hex::encode(group_result.group.mls_group_id.as_slice());

                        // Send welcome giftwraps to the peer.
                        let expires = Timestamp::from_secs(Timestamp::now().as_secs() + 30 * 24 * 60 * 60);
                        let mut publish_failed = false;
                        for rumor in group_result.welcome_rumors {
                            let giftwrap = match EventBuilder::gift_wrap(
                                &keys,
                                &peer_pubkey,
                                rumor,
                                [Tag::expiration(expires)],
                            )
                            .await
                            {
                                Ok(gw) => gw,
                                Err(e) => {
                                    out_tx.send(out_error(request_id.clone(), "gift_wrap_failed", format!("{e:#}"))).ok();
                                    publish_failed = true;
                                    break;
                                }
                            };
                            if let Err(e) = publish_and_confirm_multi(&client, &relay_urls, &giftwrap, "init_group_welcome").await {
                                out_tx.send(out_error(request_id.clone(), "publish_failed", format!("{e:#}"))).ok();
                                publish_failed = true;
                                break;
                            }
                        }
                        if publish_failed {
                            continue;
                        }

                        // Subscribe to new group messages.
                        match crate::subscribe_group_msgs(&client, &nostr_group_id_hex).await {
                            Ok(sid) => {
                                group_subs.insert(sid, nostr_group_id_hex.clone());
                            }
                            Err(err) => {
                                warn!("[pikachat] subscribe group msgs failed after init_group: {err:#}");
                            }
                        }

                        out_tx.send(out_ok(request_id, Some(json!({
                            "nostr_group_id": nostr_group_id_hex,
                            "mls_group_id": mls_group_id_hex,
                            "peer_pubkey": peer_pubkey.to_hex(),
                        })))).ok();
                        let member_count = mdk.get_members(&group_result.group.mls_group_id).map(|m| m.len() as u32).unwrap_or(0);
                        out_tx.send(OutMsg::GroupCreated {
                            nostr_group_id: nostr_group_id_hex,
                            mls_group_id: mls_group_id_hex,
                            peer_pubkey: peer_pubkey.to_hex(),
                            member_count,
                        }).ok();
                    }
                    InCmd::Shutdown { request_id } => {
                        out_tx.send(out_ok(request_id, None)).ok();
                        shutdown = true;
                    }
                }
            }
            call_evt = call_evt_rx.recv() => {
                let Some(call_evt) = call_evt else { continue; };
                match call_evt {
                    CallWorkerEvent::AudioChunk { call_id, audio_path, sample_rate, channels } => {
                        let _ = out_tx.send(OutMsg::CallAudioChunk {
                            call_id,
                            audio_path,
                            sample_rate,
                            channels,
                        });
                    }
                    CallWorkerEvent::AudioPublished { call_id, request_id, result } => {
                        match result {
                            Ok(stats) => {
                                // Update next_voice_seq to the actual value (may differ
                                // slightly from the estimate used when spawning).
                                if let Some(call) = active_call.as_mut().filter(|c| c.call_id == call_id) {
                                    call.next_voice_seq = stats.next_seq;
                                }
                                tracing::info!(
                                    "[pikachat] send_audio_file ok call_id={} frames={} next_seq={}",
                                    call_id, stats.frames_published, stats.next_seq
                                );
                                let (publish_path, subscribe_path, track_name) = active_call
                                    .as_ref()
                                    .filter(|c| c.call_id == call_id)
                                    .map(|c| {
                                        let pp = broadcast_path(
                                            &c.session.broadcast_base,
                                            &c.media_crypto.local_participant_label,
                                        ).ok();
                                        let sp = broadcast_path(
                                            &c.session.broadcast_base,
                                            &c.media_crypto.peer_participant_label,
                                        ).ok();
                                        let tn = call_audio_track_spec(&c.session)
                                            .map(|t| t.name.clone())
                                            .unwrap_or_default();
                                        (pp, sp, tn)
                                    })
                                    .unwrap_or((None, None, String::new()));
                                let _ = out_tx.send(out_ok(
                                    request_id,
                                    Some(json!({
                                        "call_id": call_id,
                                        "frames_published": stats.frames_published,
                                        "publish_path": publish_path,
                                        "subscribe_path": subscribe_path,
                                        "track": track_name,
                                    })),
                                ));
                            }
                            Err(err) => {
                                warn!(
                                    "[pikachat] send_audio_file failed call_id={} err={err:#}",
                                    call_id
                                );
                                let _ = out_tx.send(out_error(
                                    request_id,
                                    "runtime_error",
                                    format!("audio file publish failed: {err:#}"),
                                ));
                            }
                        }
                    }
                    CallWorkerEvent::DataFrame {
                        call_id,
                        payload,
                        track_name,
                    } => {
                        let _ = out_tx.send(OutMsg::CallData {
                            call_id,
                            payload_hex: hex::encode(payload),
                            track_name,
                        });
                    }
                }
            }
            notification = rx.recv() => {
                let notification = match notification {
                    Ok(n) => n,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };

                let RelayPoolNotification::Event { subscription_id, event, .. } = notification else {
                    continue;
                };
                let event = *event;

                if subscription_id == gift_sub.val {
                    if event.kind != Kind::GiftWrap {
                        continue;
                    }
                    if !seen_welcomes.insert(event.id) {
                        continue;
                    }

                    let welcome = match crate::ingest_welcome_from_giftwrap(
                        &mdk,
                        &keys,
                        &event,
                        |sender_hex| sender_allowed(sender_hex),
                    )
                    .await
                    {
                        Ok(Some(w)) => w,
                        Ok(None) => continue,
                        Err(e) => {
                            warn!("[pikachat] welcome ingest failed wrapper_id={} err={e:#}", event.id.to_hex());
                            continue;
                        }
                    };

                    let wid_hex = welcome.wrapper_event_id.to_hex();
                    out_tx.send(OutMsg::WelcomeReceived {
                        wrapper_event_id: wid_hex.clone(),
                        welcome_event_id: welcome.welcome_event_id.to_hex(),
                        from_pubkey: welcome.sender_hex,
                        nostr_group_id: welcome.nostr_group_id_hex,
                        group_name: welcome.group_name,
                    }).ok();

                    if auto_accept_welcomes {
                        eprintln!("[pikachat] auto-accepting welcome wrapper_id={wid_hex}");
                        cmd_tx_for_auto
                            .send(InCmd::AcceptWelcome {
                                request_id: Some("auto-accept".into()),
                                wrapper_event_id: wid_hex,
                            })
                            .ok();
                    }

                    continue;
                }

                if event.kind == Kind::MlsGroupMessage {
                    // Only process messages for subscriptions we created.
                    if !group_subs.contains_key(&subscription_id) {
                        continue;
                    }
                    if !seen_group_events.insert(event.id) {
                        continue;
                    }

                    let nostr_group_id = event_h_tag_hex(&event).unwrap_or_else(|| group_subs.get(&subscription_id).cloned().unwrap_or_default());
                    match crate::ingest_application_message(&mdk, &event) {
                        Ok(Some(msg)) => {
                            let sender_hex = msg.pubkey.to_hex().to_lowercase();
                            if !sender_allowed(&sender_hex) {
                                warn!("[pikachat] drop message (sender not allowed) from={sender_hex}");
                                continue;
                            }
                            if let Some(signal) = parse_call_signal(&msg.content) {
                                match signal {
                                    ParsedCallSignal::Invite { call_id, session } => {
                                        // Reject video calls — pikachat only supports audio.
                                        if session.tracks.iter().any(|t| t.name == "video0") {
                                            tracing::info!(call_id = %call_id, "rejecting video call (unsupported)");
                                            let _ = send_call_signal(
                                                &client,
                                                &relay_urls,
                                                &mdk,
                                                &keys,
                                                &nostr_group_id,
                                                &call_id,
                                                OutgoingCallSignal::Reject { reason: "unsupported_video" },
                                                "call_video_reject",
                                            )
                                            .await;
                                            continue;
                                        }
                                        if active_call.is_some() {
                                            let _ = send_call_signal(
                                                &client,
                                                &relay_urls,
                                                &mdk,
                                                &keys,
                                                &nostr_group_id,
                                                &call_id,
                                                OutgoingCallSignal::Reject { reason: "busy" },
                                                "call_busy_reject",
                                            )
                                            .await;
                                            continue;
                                        }
                                        pending_call_invites.insert(
                                            call_id.clone(),
                                            PendingCallInvite {
                                                call_id: call_id.clone(),
                                                from_pubkey: sender_hex.clone(),
                                                nostr_group_id: nostr_group_id.clone(),
                                                session,
                                            },
                                        );
                                        out_tx
                                            .send(OutMsg::CallInviteReceived {
                                                call_id,
                                                from_pubkey: sender_hex,
                                                nostr_group_id,
                                            })
                                            .ok();
                                    }
                                    ParsedCallSignal::Accept { call_id, session } => {
                                        let Some(pending) = pending_outgoing_call_invites.get(&call_id).cloned() else {
                                            continue;
                                        };
                                        if active_call.is_some() {
                                            continue;
                                        }
                                        if sender_hex != pending.peer_pubkey {
                                            warn!(
                                                "[marmotd] call.accept sender mismatch call_id={} expected={} got={}",
                                                call_id, pending.peer_pubkey, sender_hex
                                            );
                                            continue;
                                        }
                                        if let Err(err) = validate_relay_auth_token(
                                            &mdk,
                                            &pending.nostr_group_id,
                                            &pending.call_id,
                                            &session,
                                            &pubkey_hex,
                                            &sender_hex,
                                        ) {
                                            warn!("[marmotd] call.accept auth failed call_id={} err={err:#}", call_id);
                                            continue;
                                        }
                                        let media_crypto = match derive_mls_media_crypto_context(
                                            &mdk,
                                            &pending.nostr_group_id,
                                            &pending.call_id,
                                            &session,
                                            &pubkey_hex,
                                            &sender_hex,
                                        ) {
                                            Ok(v) => v,
                                            Err(err) => {
                                                warn!(
                                                    "[marmotd] call.accept derive media context failed call_id={} err={err:#}",
                                                    call_id
                                                );
                                                continue;
                                            }
                                        };
                                        let mode = active_call_mode(&session);
                                        let worker = match mode {
                                            ActiveCallMode::Audio => {
                                                if echo_mode_enabled() {
                                                    match start_echo_worker(
                                                        &pending.call_id,
                                                        &session,
                                                        media_crypto.clone(),
                                                        out_tx.clone(),
                                                    ) {
                                                        Ok(v) => v,
                                                        Err(err) => {
                                                            warn!(
                                                                "[marmotd] start echo worker failed call_id={} err={err:#}",
                                                                call_id
                                                            );
                                                            continue;
                                                        }
                                                    }
                                                } else {
                                                    match start_stt_worker(
                                                        &pending.call_id,
                                                        &session,
                                                        media_crypto.clone(),
                                                        out_tx.clone(),
                                                        call_evt_tx.clone(),
                                                    ) {
                                                        Ok(v) => v,
                                                        Err(err) => {
                                                            warn!(
                                                                "[marmotd] start stt worker failed call_id={} err={err:#}",
                                                                call_id
                                                            );
                                                            continue;
                                                        }
                                                    }
                                                }
                                            }
                                            ActiveCallMode::Data => match start_data_worker(
                                                &pending.call_id,
                                                &session,
                                                media_crypto.clone(),
                                                call_evt_tx.clone(),
                                            ) {
                                                Ok(v) => v,
                                                Err(err) => {
                                                    warn!(
                                                        "[marmotd] start data worker failed call_id={} err={err:#}",
                                                        call_id
                                                    );
                                                    continue;
                                                }
                                            },
                                        };
                                        active_call = Some(ActiveCall {
                                            call_id: pending.call_id.clone(),
                                            nostr_group_id: pending.nostr_group_id.clone(),
                                            session: session.clone(),
                                            mode,
                                            media_crypto,
                                            next_voice_seq: 0,
                                            next_data_seq: 0,
                                            worker,
                                        });
                                        pending_outgoing_call_invites.remove(&call_id);
                                        out_tx
                                            .send(OutMsg::CallSessionStarted {
                                                call_id: pending.call_id,
                                                from_pubkey: sender_hex,
                                                nostr_group_id: pending.nostr_group_id,
                                            })
                                            .ok();
                                    }
                                    ParsedCallSignal::Reject { call_id, reason } => {
                                        pending_call_invites.remove(&call_id);
                                        pending_outgoing_call_invites.remove(&call_id);
                                        if active_call
                                            .as_ref()
                                            .map(|c| c.call_id == call_id)
                                            .unwrap_or(false)
                                        {
                                            if let Some(current) = active_call.take() {
                                                current.worker.stop().await;
                                            }
                                            out_tx
                                                .send(OutMsg::CallSessionEnded { call_id, reason })
                                                .ok();
                                        }
                                    }
                                    ParsedCallSignal::End { call_id, reason } => {
                                        pending_call_invites.remove(&call_id);
                                        pending_outgoing_call_invites.remove(&call_id);
                                        if active_call
                                            .as_ref()
                                            .map(|c| c.call_id == call_id)
                                            .unwrap_or(false)
                                        {
                                            if let Some(current) = active_call.take() {
                                                current.worker.stop().await;
                                            }
                                            out_tx
                                                .send(OutMsg::CallSessionEnded { call_id, reason })
                                                .ok();
                                        }
                                    }
                                }
                                continue;
                            }
                            if is_typing_indicator(&msg) {
                                continue;
                            }
                            let mut media: Vec<MediaAttachmentOut> = Vec::new();
                            {
                                let mgr = mdk.media_manager(msg.mls_group_id.clone());
                                let refs: Vec<MediaReference> = msg.tags.iter()
                                    .filter(|t| is_imeta_tag(t))
                                    .filter_map(|t| mgr.parse_imeta_tag(t).ok())
                                    .collect();
                                for r in refs {
                                    let mut att = media_ref_to_attachment(r.clone());
                                    match download_and_decrypt_media(&mdk, &msg.mls_group_id, &r, state_dir).await {
                                        Ok(path) => att.local_path = Some(path),
                                        Err(e) => warn!("[pikachat] media download failed url={}: {e:#}", r.url),
                                    }
                                    media.push(att);
                                }
                            }
                            out_tx.send(OutMsg::MessageReceived {
                                nostr_group_id,
                                from_pubkey: sender_hex,
                                content: msg.content,
                                created_at: msg.created_at.as_secs(),
                                message_id: msg.id.to_hex(),
                                media,
                            }).ok();
                        }
                        Ok(None) => {}
                        Err(e) => {
                            warn!("[pikachat] process_message failed id={} err={e:#}", event.id.to_hex());
                        }
                    }
                }
            }
        }
    }

    // Best-effort cleanup
    if let Some(current) = active_call.take() {
        current.worker.stop().await;
    }
    let _ = client.unsubscribe(&gift_sub.val).await;
    client.unsubscribe_all().await;
    client.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_call_invite_signal() {
        let content = serde_json::json!({
            "v": 1,
            "ns": "pika.call",
            "type": "call.invite",
            "call_id": "550e8400-e29b-41d4-a716-446655440000",
            "ts_ms": 1730000000000i64,
            "body": {
                "moq_url": "https://moq.local/anon",
                "broadcast_base": "pika/calls/550e8400-e29b-41d4-a716-446655440000",
                "tracks": [{
                    "name": "audio0",
                    "codec": "opus",
                    "sample_rate": 48000,
                    "channels": 1,
                    "frame_ms": 20
                }]
            }
        })
        .to_string();
        let parsed = parse_call_signal(&content).expect("parse call signal");
        match parsed {
            ParsedCallSignal::Invite { call_id, session } => {
                assert_eq!(call_id, "550e8400-e29b-41d4-a716-446655440000");
                assert_eq!(session.moq_url, "https://moq.local/anon");
                assert_eq!(
                    session.broadcast_base,
                    "pika/calls/550e8400-e29b-41d4-a716-446655440000"
                );
            }
            other => panic!("expected invite signal, got {other:?}"),
        }
    }

    #[test]
    fn parses_call_invite_signal_when_double_encoded() {
        let raw = serde_json::json!({
            "v": 1,
            "ns": "pika.call",
            "type": "call.invite",
            "call_id": "550e8400-e29b-41d4-a716-446655440000",
            "ts_ms": 1730000000000i64,
            "body": {
                "moq_url": "https://moq.local/anon",
                "broadcast_base": "pika/calls/550e8400-e29b-41d4-a716-446655440000",
                "tracks": [{
                    "name": "audio0",
                    "codec": "opus",
                    "sample_rate": 48000,
                    "channels": 1,
                    "frame_ms": 20
                }]
            }
        })
        .to_string();

        // JSON string containing JSON.
        let content = serde_json::to_string(&raw).expect("double encode");
        let parsed = parse_call_signal(&content).expect("parse call signal");
        match parsed {
            ParsedCallSignal::Invite { call_id, .. } => {
                assert_eq!(call_id, "550e8400-e29b-41d4-a716-446655440000");
            }
            other => panic!("expected invite signal, got {other:?}"),
        }
    }

    #[test]
    fn parses_call_invite_signal_when_wrapped_in_object_with_content_field() {
        let inner = serde_json::json!({
            "v": 1,
            "ns": "pika.call",
            "type": "call.invite",
            "call_id": "550e8400-e29b-41d4-a716-446655440000",
            "ts_ms": 1730000000000i64,
            "body": {
                "moq_url": "https://moq.local/anon",
                "broadcast_base": "pika/calls/550e8400-e29b-41d4-a716-446655440000",
                "tracks": [{
                    "name": "audio0",
                    "codec": "opus",
                    "sample_rate": 48000,
                    "channels": 1,
                    "frame_ms": 20
                }]
            }
        })
        .to_string();

        let outer = serde_json::json!({
            "kind": 9,
            "content": inner,
            "id": "deadbeef"
        })
        .to_string();

        let parsed = parse_call_signal(&outer).expect("parse call signal");
        match parsed {
            ParsedCallSignal::Invite { call_id, .. } => {
                assert_eq!(call_id, "550e8400-e29b-41d4-a716-446655440000");
            }
            other => panic!("expected invite signal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn echo_worker_republishes_frames() {
        let stats = run_audio_echo_smoke(10).await.expect("audio echo smoke");
        assert_eq!(stats.sent_frames, 10);
        assert_eq!(stats.echoed_frames, 10);
    }

    #[test]
    fn tts_pcm_publish_reaches_subscriber() {
        let call_id = "550e8400-e29b-41d4-a716-446655440123";
        let session = default_audio_call_session(call_id);
        let relay = InMemoryRelay::new();
        let bot_pubkey_hex = "2284fc7b932b5dbbdaa2185c76a4e17a2ef928d4a82e29b812986b454b957f8f";
        let peer_pubkey_hex = "11b9a894813efe60d39f8621ae9dc4c6d26de4732411c1cdf4bb15e88898a19c";
        let group_root = [7u8; 32];
        let media_crypto = CallMediaCryptoContext {
            tx_keys: FrameKeyMaterial::from_base_key(
                [9u8; 32],
                key_id_for_sender(bot_pubkey_hex.as_bytes()),
                1,
                0,
                "audio0",
                group_root,
            ),
            rx_keys: FrameKeyMaterial::from_base_key(
                [5u8; 32],
                key_id_for_sender(peer_pubkey_hex.as_bytes()),
                1,
                0,
                "audio0",
                group_root,
            ),
            local_participant_label: opaque_participant_label(
                &group_root,
                bot_pubkey_hex.as_bytes(),
            ),
            peer_participant_label: opaque_participant_label(
                &group_root,
                peer_pubkey_hex.as_bytes(),
            ),
        };

        let mut observer = MediaSession::with_relay(
            SessionConfig {
                moq_url: session.moq_url.clone(),
                relay_auth: session.relay_auth.clone(),
            },
            relay.clone(),
        );
        observer.connect().expect("observer connect");
        let bot_track = TrackAddress {
            broadcast_path: broadcast_path(
                &session.broadcast_base,
                &media_crypto.local_participant_label,
            )
            .expect("bot broadcast path"),
            track_name: "audio0".to_string(),
        };
        let echoed_rx = observer.subscribe(&bot_track).expect("subscribe bot track");

        let frame_samples = 960usize; // 20ms @ 48kHz
        let total_frames = 5usize;
        let mut pcm = Vec::with_capacity(frame_samples * total_frames);
        for i in 0..(frame_samples * total_frames) {
            pcm.push((i as i16 % 200) - 100);
        }

        let stats = publish_pcm_audio_response_with_relay(
            &session,
            relay,
            &media_crypto,
            0,
            crate::call_tts::TtsPcm {
                sample_rate_hz: 48_000,
                channels: 1,
                pcm_i16: pcm,
            },
        )
        .expect("publish tts pcm");
        assert_eq!(stats.frames_published, total_frames as u64);

        let mut echoed_frames = 0u64;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while echoed_frames < stats.frames_published && std::time::Instant::now() < deadline {
            while let Ok(frame) = echoed_rx.try_recv() {
                let opened =
                    decrypt_frame(&frame.payload, &media_crypto.tx_keys).expect("decrypt frame");
                let _ = OpusCodec.decode_to_pcm_i16(&OpusPacket(opened.payload));
                echoed_frames = echoed_frames.saturating_add(1);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(echoed_frames, stats.frames_published);
    }

    // ── Media helper tests ─────────────────────────────────────────────

    #[test]
    fn is_imeta_tag_matches() {
        let tag = Tag::parse([
            "imeta".to_string(),
            "url https://example.com/file.jpg".to_string(),
        ])
        .unwrap();
        assert!(is_imeta_tag(&tag));
    }

    #[test]
    fn is_imeta_tag_rejects_other_tags() {
        let tag = Tag::parse(["e".to_string(), "deadbeef".to_string()]).unwrap();
        assert!(!is_imeta_tag(&tag));
        let tag = Tag::parse(["p".to_string(), "deadbeef".to_string()]).unwrap();
        assert!(!is_imeta_tag(&tag));
    }

    #[test]
    fn mime_from_extension_common_types() {
        use std::path::Path;
        assert_eq!(
            mime_from_extension(Path::new("photo.jpg")),
            Some("image/jpeg")
        );
        assert_eq!(
            mime_from_extension(Path::new("photo.JPEG")),
            Some("image/jpeg")
        );
        assert_eq!(
            mime_from_extension(Path::new("image.png")),
            Some("image/png")
        );
        assert_eq!(
            mime_from_extension(Path::new("clip.mp4")),
            Some("video/mp4")
        );
        assert_eq!(
            mime_from_extension(Path::new("song.mp3")),
            Some("audio/mpeg")
        );
        assert_eq!(
            mime_from_extension(Path::new("doc.pdf")),
            Some("application/pdf")
        );
        assert_eq!(
            mime_from_extension(Path::new("notes.txt")),
            Some("text/plain")
        );
        assert_eq!(
            mime_from_extension(Path::new("notes.md")),
            Some("text/plain")
        );
    }

    #[test]
    fn mime_from_extension_unknown() {
        use std::path::Path;
        assert_eq!(mime_from_extension(Path::new("archive.xyz")), None);
        assert_eq!(mime_from_extension(Path::new("noext")), None);
    }

    #[test]
    fn blossom_servers_or_default_uses_provided() {
        let servers = vec!["https://blossom.example.com".to_string()];
        let result = blossom_servers_or_default(&servers);
        assert_eq!(result, vec!["https://blossom.example.com"]);
    }

    #[test]
    fn blossom_servers_or_default_falls_back() {
        let result = blossom_servers_or_default(&[]);
        assert_eq!(result, vec![default_primary_blossom_server()]);
    }

    #[test]
    fn blossom_servers_or_default_skips_empty_and_invalid() {
        let servers = vec!["".to_string(), "  ".to_string(), "not a url".to_string()];
        let result = blossom_servers_or_default(&servers);
        // All invalid → falls back to default
        assert_eq!(result, vec![default_primary_blossom_server()]);
    }

    #[test]
    fn blossom_servers_or_default_filters_invalid_keeps_valid() {
        let servers = vec![
            "https://good.example.com".to_string(),
            "not a url".to_string(),
        ];
        let result = blossom_servers_or_default(&servers);
        assert_eq!(result, vec!["https://good.example.com"]);
    }

    // ── InCmd serde round-trip tests ───────────────────────────────────

    #[test]
    fn deserialize_send_media_full() {
        let json = r#"{
            "cmd": "send_media",
            "request_id": "r1",
            "nostr_group_id": "aa",
            "file_path": "/tmp/photo.jpg",
            "mime_type": "image/jpeg",
            "filename": "photo.jpg",
            "caption": "Check this out",
            "blossom_servers": ["https://blossom.example.com"]
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::SendMedia {
                request_id,
                nostr_group_id,
                file_path,
                mime_type,
                filename,
                caption,
                blossom_servers,
            } => {
                assert_eq!(request_id.as_deref(), Some("r1"));
                assert_eq!(nostr_group_id, "aa");
                assert_eq!(file_path, "/tmp/photo.jpg");
                assert_eq!(mime_type.as_deref(), Some("image/jpeg"));
                assert_eq!(filename.as_deref(), Some("photo.jpg"));
                assert_eq!(caption, "Check this out");
                assert_eq!(blossom_servers, vec!["https://blossom.example.com"]);
            }
            other => panic!("expected SendMedia, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_send_media_minimal() {
        let json = r#"{
            "cmd": "send_media",
            "nostr_group_id": "bb",
            "file_path": "/tmp/file.bin"
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::SendMedia {
                request_id,
                mime_type,
                filename,
                caption,
                blossom_servers,
                ..
            } => {
                assert!(request_id.is_none());
                assert!(mime_type.is_none());
                assert!(filename.is_none());
                assert_eq!(caption, "");
                assert!(blossom_servers.is_empty());
            }
            other => panic!("expected SendMedia, got {other:?}"),
        }
    }

    // ── OutMsg serialization tests ─────────────────────────────────────

    #[test]
    fn serialize_message_received_without_media() {
        let msg = OutMsg::MessageReceived {
            nostr_group_id: "aabb".into(),
            from_pubkey: "cc".into(),
            content: "hello".into(),
            created_at: 123,
            message_id: "dd".into(),
            media: vec![],
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "message_received");
        assert_eq!(json["content"], "hello");
        // Empty media vec should be omitted
        assert!(json.get("media").is_none());
    }

    #[test]
    fn serialize_message_received_with_media() {
        let msg = OutMsg::MessageReceived {
            nostr_group_id: "aabb".into(),
            from_pubkey: "cc".into(),
            content: "look at this".into(),
            created_at: 456,
            message_id: "dd".into(),
            media: vec![MediaAttachmentOut {
                url: "https://blossom.example.com/abc123".into(),
                mime_type: "image/png".into(),
                filename: "screenshot.png".into(),
                original_hash_hex: "deadbeef".into(),
                nonce_hex: "cafebabe".into(),
                scheme_version: "v1".into(),
                width: Some(800),
                height: Some(600),
                local_path: Some("/tmp/decrypted.png".into()),
            }],
        };
        let json = serde_json::to_value(&msg).unwrap();
        let media = json["media"].as_array().expect("media should be array");
        assert_eq!(media.len(), 1);
        assert_eq!(media[0]["url"], "https://blossom.example.com/abc123");
        assert_eq!(media[0]["mime_type"], "image/png");
        assert_eq!(media[0]["filename"], "screenshot.png");
        assert_eq!(media[0]["width"], 800);
        assert_eq!(media[0]["height"], 600);
    }

    #[test]
    fn serialize_message_received_media_omits_null_dimensions() {
        let msg = OutMsg::MessageReceived {
            nostr_group_id: "aabb".into(),
            from_pubkey: "cc".into(),
            content: "".into(),
            created_at: 0,
            message_id: "dd".into(),
            media: vec![MediaAttachmentOut {
                url: "https://example.com/file".into(),
                mime_type: "application/pdf".into(),
                filename: "doc.pdf".into(),
                original_hash_hex: "aa".into(),
                nonce_hex: "bb".into(),
                scheme_version: "v1".into(),
                width: None,
                height: None,
                local_path: None,
            }],
        };
        let json = serde_json::to_value(&msg).unwrap();
        let media = &json["media"][0];
        assert!(media.get("width").is_none());
        assert!(media.get("height").is_none());
    }
}
