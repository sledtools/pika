use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, anyhow};
use hypernote_protocol as hn;
use mdk_core::prelude::*;
use mdk_sqlite_storage::MdkSqliteStorage;
use nostr_sdk::prelude::*;
use pika_marmot_runtime::call::{
    CallCryptoDeriveContext, CallMediaCryptoContext, CallSessionParams, CallTrackSpec,
    ParsedCallSignal, derive_relay_auth_token as derive_shared_relay_auth_token,
    parse_call_signal as parse_shared_call_signal,
};
use pika_marmot_runtime::call_runtime::{
    GroupCallContext, InboundCallPolicy, InboundCallSignalOutcome, PendingIncomingCall,
    PendingOutgoingCall,
};
use pika_marmot_runtime::conversation::ConversationEvent;
use pika_marmot_runtime::group::{CreatedGroup, create_group_and_publish_welcomes};
use pika_marmot_runtime::message::{
    CALL_SIGNAL_KIND, MessageClassification, classify_message as classify_shared_message,
};
use pika_marmot_runtime::outbound::{OutboundConversationAction, PreparedConversationAction};
use pika_marmot_runtime::runtime::{
    BootstrappedRuntimeSession, MarmotRuntime, bootstrap_runtime_session,
    subscribe_group_messages_individual, subscribe_welcome_inbox,
};
use pika_marmot_runtime::welcome::{
    AcceptedWelcome, accept_welcome_and_catch_up, take_pending_welcome,
};
use pika_media::codec_opus::{OpusCodec, OpusPacket};
use pika_media::crypto::{FrameInfo, decrypt_frame, encrypt_frame};
use pika_media::network::NetworkRelay;
use pika_media::session::{
    InMemoryRelay, MediaFrame, MediaSession, MediaSessionError, SessionConfig,
};
use pika_media::tracks::{TrackAddress, broadcast_path};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::warn;

use crate::call_audio::OpusToAudioPipeline;
use crate::call_tts::synthesize_tts_pcm;

#[cfg(test)]
use pika_marmot_runtime::call::key_id_for_sender;
#[cfg(test)]
use pika_marmot_runtime::welcome::find_pending_welcome_index;
#[cfg(test)]
use pika_media::crypto::{FrameKeyMaterial, opaque_participant_label};

const PROTOCOL_VERSION: u32 = 1;
const ACCEPT_WELCOME_BACKLOG_LIMIT: usize = 200;
const INIT_GROUP_WELCOME_EXPIRATION_SECS: u64 = 30 * 24 * 60 * 60;

fn bootstrap_runtime_for_daemon(
    state_dir: &Path,
    keys: &Keys,
) -> anyhow::Result<BootstrappedRuntimeSession> {
    let signer: Arc<dyn NostrSigner> = Arc::new(keys.clone());
    bootstrap_runtime_session(keys.public_key(), signer, || {
        crate::new_mdk(state_dir, "daemon")
    })
}

async fn accept_welcome_with_backfill<F, Fut>(
    mdk: &MDK<MdkSqliteStorage>,
    client: &Client,
    relay_urls: &[RelayUrl],
    welcome: &mdk_storage_traits::welcomes::types::Welcome,
    seen_group_events: &mut HashSet<EventId>,
    after_accept: F,
) -> anyhow::Result<AcceptedWelcome>
where
    F: FnOnce(&AcceptedWelcome) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let backlog_relays: Vec<RelayUrl> = relay_urls.first().cloned().into_iter().collect();
    accept_welcome_and_catch_up(
        mdk,
        client,
        &backlog_relays,
        welcome,
        seen_group_events,
        ACCEPT_WELCOME_BACKLOG_LIMIT,
        after_accept,
    )
    .await
}

async fn create_group_and_publish_welcomes_for_init_group<F, Fut>(
    keys: &Keys,
    mdk: &MDK<MdkSqliteStorage>,
    peer_kp: Event,
    peer_pubkey: PublicKey,
    config: NostrGroupConfigData,
    publish_giftwrap: F,
) -> anyhow::Result<CreatedGroup>
where
    F: FnMut(PublicKey, Event) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    const INIT_GROUP_BUILD_WELCOME_MARKER: &str = "init_group_build_welcome";
    let expires =
        Timestamp::from_secs(Timestamp::now().as_secs() + INIT_GROUP_WELCOME_EXPIRATION_SECS);
    let result = create_group_and_publish_welcomes(
        keys,
        mdk,
        vec![peer_kp],
        config,
        &[peer_pubkey],
        vec![Tag::expiration(expires)],
        publish_giftwrap,
    )
    .await;
    match result {
        Ok(created) => Ok(created),
        Err(err) if chain_has_message(&err, "build welcome giftwrap") => {
            Err(err.context(INIT_GROUP_BUILD_WELCOME_MARKER))
        }
        Err(err) => Err(err.context("init_group")),
    }
}

async fn create_group_and_publish_welcomes_for_init_group_with_confirm(
    keys: &Keys,
    mdk: &MDK<MdkSqliteStorage>,
    client: &Client,
    relay_urls: &[RelayUrl],
    peer_kp: Event,
    peer_pubkey: PublicKey,
    config: NostrGroupConfigData,
) -> anyhow::Result<CreatedGroup> {
    const INIT_GROUP_PUBLISH_WELCOME_MARKER: &str = "init_group_publish_welcome";
    create_group_and_publish_welcomes_for_init_group(
        keys,
        mdk,
        peer_kp,
        peer_pubkey,
        config,
        |_receiver, giftwrap| async move {
            publish_and_confirm_multi(client, relay_urls, &giftwrap, "init_group_welcome")
                .await
                .map(|_| ())
                .context(INIT_GROUP_PUBLISH_WELCOME_MARKER)
        },
    )
    .await
}

struct DaemonHostContext<'a> {
    client: &'a Client,
    relay_urls: &'a [RelayUrl],
    mdk: &'a MDK<MdkSqliteStorage>,
    keys: &'a Keys,
    pubkey_hex: String,
}

impl<'a> DaemonHostContext<'a> {
    fn new(
        client: &'a Client,
        relay_urls: &'a [RelayUrl],
        mdk: &'a MDK<MdkSqliteStorage>,
        keys: &'a Keys,
        pubkey_hex: impl Into<String>,
    ) -> Self {
        Self {
            client,
            relay_urls,
            mdk,
            keys,
            pubkey_hex: pubkey_hex.into(),
        }
    }

    fn runtime(&self) -> MarmotRuntime<'a> {
        MarmotRuntime::with_client(self.mdk, self.client)
    }

    fn resolve_group(&self, nostr_group_id: &str) -> anyhow::Result<GroupId> {
        self.runtime()
            .mls_group_id_for_nostr_group_id(nostr_group_id)
    }

    fn list_groups(
        &self,
    ) -> anyhow::Result<Vec<pika_marmot_runtime::conversation::RuntimeGroupSummary>> {
        self.runtime().list_groups()
    }

    fn get_messages(
        &self,
        nostr_group_id: &str,
        pagination: Option<mdk_storage_traits::groups::Pagination>,
    ) -> anyhow::Result<Vec<mdk_storage_traits::messages::types::Message>> {
        self.runtime().get_messages(nostr_group_id, pagination)
    }

    fn parse_message_media_attachments(
        &self,
        message: &mdk_storage_traits::messages::types::Message,
    ) -> Vec<ParsedMediaAttachment> {
        self.runtime().parse_message_attachments(message)
    }

    async fn download_and_decrypt_media(
        &self,
        mls_group_id: &GroupId,
        attachment: &ParsedMediaAttachment,
        state_dir: &Path,
    ) -> anyhow::Result<String> {
        let downloaded = self
            .runtime()
            .download_media(mls_group_id, &attachment.reference, None)
            .await?;

        let media_dir = state_dir.join("media-tmp");
        std::fs::create_dir_all(&media_dir).context("create media-tmp dir")?;
        let filename = if attachment.attachment.filename.is_empty() {
            "download.bin"
        } else {
            &attachment.attachment.filename
        };
        let dest = media_dir.join(format!(
            "{}-{}",
            &attachment.attachment.original_hash_hex[..16],
            filename,
        ));
        std::fs::write(&dest, &downloaded.decrypted_data)
            .with_context(|| format!("write decrypted media to {}", dest.display()))?;
        Ok(dest.to_string_lossy().into_owned())
    }

    async fn publish_prepared(
        &self,
        prepared: &PreparedConversationAction,
        label: &str,
    ) -> anyhow::Result<Event> {
        let signed = resign_wrapper_without_protected_tags(self.keys, &prepared.wrapper)?;
        if self.relay_urls.is_empty() {
            anyhow::bail!("no relays configured");
        }
        publish_and_confirm_multi(self.client, self.relay_urls, &signed, label).await?;
        Ok(signed)
    }

    async fn sign_and_publish_rumor(
        &self,
        mls_group_id: &GroupId,
        rumor: UnsignedEvent,
        label: &str,
    ) -> anyhow::Result<Event> {
        let msg_event = self
            .mdk
            .create_message(mls_group_id, rumor)
            .context("create_message")?;
        let signed = resign_wrapper_without_protected_tags(self.keys, &msg_event)?;
        if self.relay_urls.is_empty() {
            anyhow::bail!("no relays configured");
        }
        publish_and_confirm_multi(self.client, self.relay_urls, &signed, label).await?;
        Ok(signed)
    }

    async fn publish_group_event(
        &self,
        nostr_group_id: &str,
        kind: Kind,
        content: String,
        label: &str,
    ) -> anyhow::Result<()> {
        let mls_group_id = self.resolve_group(nostr_group_id)?;
        let rumor = EventBuilder::new(kind, content).build(self.keys.public_key());
        self.sign_and_publish_rumor(&mls_group_id, rumor, label)
            .await?;
        Ok(())
    }

    async fn publish_call_payload(
        &self,
        nostr_group_id: &str,
        payload_json: String,
        label: &str,
    ) -> anyhow::Result<()> {
        self.publish_group_event(nostr_group_id, CALL_SIGNAL_KIND, payload_json, label)
            .await
    }

    fn prepare_outbound_action(
        &self,
        nostr_group_id: &str,
        action: OutboundConversationAction,
    ) -> Result<PreparedConversationAction, DaemonPrepareError> {
        let target = self
            .runtime()
            .resolve_outbound_target(nostr_group_id)
            .map_err(DaemonPrepareError::BadGroup)?;
        self.runtime()
            .prepare_outbound_action_for_target(self.keys.public_key(), target, action)
            .map_err(DaemonPrepareError::Prepare)
    }

    fn derive_relay_auth_token(
        &self,
        nostr_group_id: &str,
        call_id: &str,
        session: &CallSessionParams,
        peer_pubkey_hex: &str,
    ) -> anyhow::Result<String> {
        let mls_group_id = self.resolve_group(nostr_group_id)?;
        let derive_ctx = CallCryptoDeriveContext {
            mdk: self.mdk,
            mls_group_id: &mls_group_id,
            group_epoch: 0,
            call_id,
            session,
            local_pubkey_hex: &self.pubkey_hex,
            peer_pubkey_hex,
        };
        derive_shared_relay_auth_token(&derive_ctx).map_err(anyhow::Error::msg)
    }

    fn prepare_call_invite(
        &self,
        nostr_group_id: &str,
        peer_pubkey_hex: &str,
        call_id: &str,
        session: &CallSessionParams,
    ) -> anyhow::Result<(
        PendingOutgoingCall,
        pika_marmot_runtime::call_runtime::PreparedCallSignal,
    )> {
        self.runtime()
            .prepare_outgoing_call_invite(nostr_group_id, peer_pubkey_hex, call_id, session)
            .map_err(anyhow::Error::msg)
    }

    fn prepare_accept_call(
        &self,
        invite: &PendingIncomingCall,
    ) -> Result<pika_marmot_runtime::call_runtime::PreparedAcceptedCall, String> {
        let mls_group_id = self
            .resolve_group(&invite.target_id)
            .map_err(|e| format!("resolve call group failed: {e:#}"))?;
        self.runtime().prepare_accept_incoming_call(
            invite,
            GroupCallContext {
                mls_group_id: &mls_group_id,
                local_pubkey_hex: &self.pubkey_hex,
            },
        )
    }

    fn prepare_reject_call_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<pika_marmot_runtime::call_runtime::PreparedCallSignal, String> {
        self.runtime().prepare_reject_call_signal(call_id, reason)
    }

    fn prepare_end_call_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<pika_marmot_runtime::call_runtime::PreparedCallSignal, String> {
        self.runtime().prepare_end_call_signal(call_id, reason)
    }

    fn process_event(&self, event: &Event) -> anyhow::Result<Option<ConversationEvent>> {
        self.runtime().process_event(event)
    }

    fn handle_inbound_call_signal(
        &self,
        ctx: pika_marmot_runtime::call_runtime::InboundSignalContext<'_>,
        signal: ParsedCallSignal,
    ) -> InboundCallSignalOutcome {
        self.runtime().handle_inbound_call_signal(ctx, signal)
    }

    fn prepare_upload(
        &self,
        mls_group_id: &GroupId,
        bytes: &[u8],
        mime_type: Option<&str>,
        filename: Option<&str>,
    ) -> anyhow::Result<pika_marmot_runtime::media::PreparedMediaUpload> {
        self.runtime()
            .prepare_upload(mls_group_id, bytes, mime_type, filename)
    }

    fn finish_upload(
        &self,
        mls_group_id: &GroupId,
        upload: &mdk_core::encrypted_media::types::EncryptedMediaUpload,
        uploaded_blob: pika_marmot_runtime::media::UploadedBlob,
    ) -> pika_marmot_runtime::media::RuntimeMediaUploadResult {
        self.runtime()
            .finish_upload(mls_group_id, upload, uploaded_blob)
    }
}

fn map_init_group_error(err: &anyhow::Error) -> (&'static str, String) {
    if chain_has_message(err, "init_group_build_welcome")
        || chain_has_message(err, "build welcome giftwrap")
    {
        ("gift_wrap_failed", format!("{err:#}"))
    } else if chain_has_message(err, "init_group_publish_welcome")
        || chain_has_message(err, "publish welcome to")
        || chain_has_message(err, "init_group_welcome")
    {
        ("publish_failed", format!("{err:#}"))
    } else {
        ("mdk_error", format!("create_group: {err:#}"))
    }
}

fn chain_has_message(err: &anyhow::Error, needle: &str) -> bool {
    err.chain().any(|cause| cause.to_string().contains(needle))
}

fn accept_welcome_event_id_hint() -> &'static str {
    "use wrapper_event_id or welcome_event_id from list_pending_welcomes"
}

fn accept_welcome_bad_event_id_message() -> String {
    format!(
        "wrapper_event_id must be hex; {}",
        accept_welcome_event_id_hint()
    )
}

fn accept_welcome_not_found_message() -> String {
    format!(
        "pending welcome not found; {}",
        accept_welcome_event_id_hint()
    )
}

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum InCmd {
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
    HypernoteCatalog {
        #[serde(default)]
        request_id: Option<String>,
    },
    SendMessage {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        content: String,
    },
    SendHypernote {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        content: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        state: Option<String>,
    },
    React {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        event_id: String,
        emoji: String,
    },
    SubmitHypernoteAction {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        event_id: String,
        action: String,
        #[serde(default)]
        form: HashMap<String, String>,
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
    SendMediaBatch {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        file_paths: Vec<String>,
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
    GetMessages {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        #[serde(default = "default_get_messages_limit")]
        limit: usize,
    },
    Shutdown {
        #[serde(default)]
        request_id: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutMsg {
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
        kind: u16,
        created_at: u64,
        event_id: String,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct MediaAttachmentOut {
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

/// Wrapper that optionally carries a per-command response sender for socket connections.
pub struct DaemonCmd {
    pub cmd: InCmd,
    /// If Some, route Ok/Error responses to this sender instead of the main out_tx.
    pub response_tx: Option<mpsc::UnboundedSender<OutMsg>>,
}

fn default_get_messages_limit() -> usize {
    50
}

fn default_group_name() -> String {
    "DM".to_string()
}

use pika_marmot_runtime::key_package::normalize_peer_key_package_event_for_mdk;
use pika_marmot_runtime::media::{
    MAX_CHAT_MEDIA_BYTES, ParsedMediaAttachment, RuntimeMediaAttachment, resolve_upload_metadata,
    upload_encrypted_blob,
};
use pika_marmot_runtime::relay::{fetch_latest_key_package_for_mdk, publish_and_confirm};

fn blossom_servers_or_default(values: &[String]) -> Vec<String> {
    pika_relay_profiles::blossom_servers_or_default(values)
}

fn media_attachment_to_out(attachment: RuntimeMediaAttachment) -> MediaAttachmentOut {
    MediaAttachmentOut {
        url: attachment.url,
        mime_type: attachment.mime_type,
        filename: attachment.filename,
        original_hash_hex: attachment.original_hash_hex,
        nonce_hex: attachment.nonce_hex,
        scheme_version: attachment.scheme_version,
        width: attachment.width,
        height: attachment.height,
        local_path: None,
    }
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
        tracks: vec![CallTrackSpec::audio0_opus_default()],
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

fn resign_wrapper_without_protected_tags(keys: &Keys, wrapper: &Event) -> anyhow::Result<Event> {
    let msg_tags: Tags = wrapper
        .tags
        .clone()
        .into_iter()
        .filter(|t| !matches!(t.kind(), TagKind::Protected))
        .collect();
    EventBuilder::new(wrapper.kind, wrapper.content.clone())
        .tags(msg_tags)
        .sign_with_keys(keys)
        .context("sign event")
}

#[derive(Debug)]
enum DaemonPrepareError {
    BadGroup(anyhow::Error),
    Prepare(anyhow::Error),
}

#[derive(Debug, Deserialize)]
struct CallSignalEnvelopeCompat {
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

#[derive(Debug, Deserialize)]
struct CompatCallSessionParams {
    moq_url: String,
    broadcast_base: String,
    #[serde(default)]
    relay_auth: String,
    tracks: Vec<CallTrackSpec>,
}

fn parse_call_signal(content: &str) -> Option<ParsedCallSignal> {
    fn parse_session(
        body: serde_json::Value,
        call_id: &str,
        msg_type: &str,
    ) -> Option<CallSessionParams> {
        match serde_json::from_value::<CompatCallSessionParams>(body) {
            Ok(session) => Some(CallSessionParams {
                moq_url: session.moq_url,
                broadcast_base: session.broadcast_base,
                relay_auth: session.relay_auth,
                tracks: session.tracks,
            }),
            Err(e) => {
                warn!("[pikachat] {msg_type} body parse failed call_id={call_id} err={e:#}",);
                None
            }
        }
    }

    fn from_env(env: CallSignalEnvelopeCompat) -> Option<ParsedCallSignal> {
        if env.v != 1 || env.ns != "pika.call" {
            return None;
        }
        match env.msg_type.as_str() {
            "call.invite" => {
                let session = parse_session(env.body, &env.call_id, "call.invite")?;
                Some(ParsedCallSignal::Invite {
                    call_id: env.call_id,
                    session,
                })
            }
            "call.accept" => {
                let session = parse_session(env.body, &env.call_id, "call.accept")?;
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
    if let Some(signal) = parse_shared_call_signal(content) {
        return Some(signal);
    }

    match serde_json::from_str::<CallSignalEnvelopeCompat>(content) {
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

async fn send_call_invite_with_retry(
    host: &DaemonHostContext<'_>,
    nostr_group_id: &str,
    payload_json: &str,
    call_id: &str,
    max_attempts: usize,
) -> anyhow::Result<()> {
    let attempts = max_attempts.max(1);
    for attempt in 1..=attempts {
        match host
            .publish_call_payload(nostr_group_id, payload_json.to_string(), "call_invite")
            .await
        {
            Ok(()) => return Ok(()),
            Err(err) => {
                if attempt == attempts {
                    return Err(err);
                }
                warn!(
                    "[pikachat] call invite publish attempt {attempt}/{attempts} failed call_id={call_id}: {err:#}; retrying"
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
                                "[pikachat] call data decrypt failed call_id={} track={} err={err}",
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
    publish_and_confirm(client, relays, event, label).await
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

fn classify_daemon_message(
    msg: &mdk_storage_traits::messages::types::Message,
) -> Option<MessageClassification> {
    classify_shared_message(msg.kind, &msg.content, msg.tags.iter())
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
    let skip_ready_check = std::env::var("PIKACHAT_SKIP_RELAY_READY_CHECK")
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
    let bootstrapped = bootstrap_runtime_for_daemon(state_dir, &keys)?;
    let client = bootstrapped.session.client.clone();
    let mdk = bootstrapped.session.mdk;

    // Daemon keeps its primary-relay-first connect policy local.
    client
        .add_relay(primary_relay)
        .await
        .with_context(|| format!("add primary relay {primary_relay}"))?;
    for r in relay_urls.iter().skip(1) {
        let _ = client.add_relay(r.clone()).await;
    }
    client.connect().await;

    let mut rx = client.notifications();

    let gift_sub = subscribe_welcome_inbox(
        &client,
        keys.public_key(),
        Some(Duration::from_secs(giftwrap_lookback_sec)),
        Some(200),
    )
    .await?;

    // Track which wrapper events and group message wrapper events we've already processed.
    let mut seen_welcomes: HashSet<EventId> = bootstrapped.startup.seen_welcomes;
    let mut seen_group_events: HashSet<EventId> = bootstrapped.startup.seen_group_events;

    // Track group subscriptions.
    let mut group_subs: HashMap<SubscriptionId, String> =
        subscribe_group_messages_individual(&client, &bootstrapped.startup.existing_group_ids)
            .await?;
    let mut pending_call_invites: HashMap<String, PendingIncomingCall> = HashMap::new();
    let mut pending_outgoing_call_invites: HashMap<String, PendingOutgoingCall> = HashMap::new();
    let mut active_call: Option<ActiveCall> = None;
    let (call_evt_tx, mut call_evt_rx) = mpsc::unbounded_channel::<CallWorkerEvent>();

    // command reader (stdin or child process stdout)
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<DaemonCmd>();
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
                        cmd_tx_clone
                            .send(DaemonCmd {
                                cmd,
                                response_tx: None,
                            })
                            .ok();
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
                        cmd_tx
                            .send(DaemonCmd {
                                cmd,
                                response_tx: None,
                            })
                            .ok();
                    }
                    Err(err) => {
                        eprintln!("[pikachat] invalid cmd json: {err} line={trimmed}");
                    }
                }
            }
        });
    }

    // Unix domain socket for --remote CLI connections
    let sock_path = state_dir.join("daemon.sock");
    // Clean up stale socket
    let _ = std::fs::remove_file(&sock_path);
    let unix_listener = tokio::net::UnixListener::bind(&sock_path)
        .with_context(|| format!("bind unix socket {}", sock_path.display()))?;
    eprintln!("[pikachat] listening on {}", sock_path.display());

    // Spawn socket acceptor
    let cmd_tx_for_sock = cmd_tx_for_auto.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = match unix_listener.accept().await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[pikachat] unix accept error: {e:#}");
                    continue;
                }
            };
            let cmd_tx = cmd_tx_for_sock.clone();
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut lines = tokio::io::BufReader::new(reader).lines();
                let (resp_tx, mut resp_rx) = mpsc::unbounded_channel::<OutMsg>();

                // Writer task: send responses back to the client
                let write_handle = tokio::spawn(async move {
                    while let Some(msg) = resp_rx.recv().await {
                        let mut line = serde_json::to_string(&msg).unwrap_or_default();
                        line.push('\n');
                        if writer.write_all(line.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                });

                // Read commands from the client
                while let Ok(Some(line)) = lines.next_line().await {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<InCmd>(trimmed) {
                        Ok(cmd) => {
                            cmd_tx
                                .send(DaemonCmd {
                                    cmd,
                                    response_tx: Some(resp_tx.clone()),
                                })
                                .ok();
                        }
                        Err(err) => {
                            let err_msg = OutMsg::Error {
                                request_id: None,
                                code: "parse_error".to_string(),
                                message: format!("{err}"),
                            };
                            let mut line = serde_json::to_string(&err_msg).unwrap_or_default();
                            line.push('\n');
                            // Can't write directly, send through resp_tx
                            resp_tx.send(err_msg).ok();
                        }
                    }
                }
                drop(resp_tx);
                let _ = write_handle.await;
            });
        }
    });

    let mut shutdown = false;
    while !shutdown {
        tokio::select! {
            daemon_cmd = cmd_rx.recv() => {
                let Some(daemon_cmd) = daemon_cmd else { break; };
                let DaemonCmd { cmd, response_tx: per_cmd_tx } = daemon_cmd;
                // For Ok/Error responses, use per-connection sender if provided, else main out_tx
                let reply_tx = per_cmd_tx.as_ref().unwrap_or(&out_tx);
                match cmd {
                    InCmd::PublishKeypackage { request_id, relays } => {
                        let selected = match parse_relay_list(primary_relay, &relays) {
                            Ok(v) => v,
                            Err(e) => {
                                reply_tx.send(out_error(request_id, "bad_relays", e.to_string())).ok();
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
                                reply_tx.send(out_error(request_id, "mdk_error", format!("{e:#}"))).ok();
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
                                reply_tx.send(out_error(request_id, "sign_failed", format!("{e:#}"))).ok();
                                continue;
                            }
                        };

                        match publish_without_confirm_multi(&client, &selected, &ev, "keypackage")
                            .await
                        {
                            Ok(_relay_confirmed) => {
                                reply_tx.send(out_ok(request_id, Some(json!({"event_id": ev.id.to_hex()})))).ok();
                                out_tx.send(OutMsg::KeypackagePublished { event_id: ev.id.to_hex() }).ok();
                            }
                            Err(e) => {
                                reply_tx.send(out_error(request_id, "publish_failed", format!("{e:#}"))).ok();
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
                                reply_tx.send(out_ok(request_id, Some(json!({"relays": v.iter().map(|r| r.to_string()).collect::<Vec<_>>()})))).ok();
                            }
                            Err(e) => {
                                reply_tx.send(out_error(request_id, "bad_relays", e.to_string())).ok();
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
                                let _ = reply_tx.send(out_ok(request_id, Some(json!({ "welcomes": out }))));
                            }
                            Err(e) => {
                                let _ = reply_tx.send(out_error(request_id, "mdk_error", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::AcceptWelcome { request_id, wrapper_event_id } => {
                        let wrapper = match EventId::from_hex(&wrapper_event_id) {
                            Ok(id) => id,
                            Err(_) => {
                                reply_tx
                                    .send(out_error(
                                        request_id,
                                        "bad_event_id",
                                        accept_welcome_bad_event_id_message(),
                                    ))
                                    .ok();
                                continue;
                            }
                        };
                        match mdk.get_pending_welcomes(None) {
                            Ok(mut list) => {
                                let found = take_pending_welcome(&mut list, &wrapper);
                                let Some(w) = found else {
                                    reply_tx
                                        .send(out_error(
                                            request_id,
                                            "not_found",
                                            accept_welcome_not_found_message(),
                                        ))
                                        .ok();
                                    continue;
                                };
                                let subscribed_group =
                                    Arc::new(Mutex::new(None::<(SubscriptionId, String)>));
                                let accept_client = client.clone();
                                match accept_welcome_with_backfill(
                                    &mdk,
                                    &client,
                                    &relay_urls,
                                    &w,
                                    &mut seen_group_events,
                                    |accepted| {
                                        let client = accept_client.clone();
                                        let nostr_group_id_hex =
                                            accepted.nostr_group_id_hex.clone();
                                        let subscribed_group = Arc::clone(&subscribed_group);
                                        async move {
                                            // Daemon accept is intentionally stronger than the
                                            // app/CLI manual accept paths today: it subscribes
                                            // immediately before backlog catch-up.
                                            match crate::subscribe_group_msgs(
                                                &client,
                                                &nostr_group_id_hex,
                                            )
                                            .await
                                            {
                                                Ok(sid) => {
                                                    *subscribed_group.lock().expect("subscription lock") =
                                                        Some((sid, nostr_group_id_hex));
                                                }
                                                Err(err) => {
                                                    warn!("[pikachat] subscribe group msgs failed: {err:#}");
                                                }
                                            }
                                            Ok(())
                                        }
                                    },
                                )
                                .await
                                {
                                    Ok(accepted) => {
                                        let host = DaemonHostContext::new(
                                            &client,
                                            &relay_urls,
                                            &mdk,
                                            &keys,
                                            &pubkey_hex,
                                        );
                                        if let Some((sid, nostr_group_id_hex)) = subscribed_group
                                            .lock()
                                            .expect("subscription lock")
                                            .take()
                                        {
                                            group_subs.insert(sid.clone(), nostr_group_id_hex);
                                        }
                                        for msg in accepted.ingested_messages {
                                            if !sender_allowed(&msg.pubkey.to_hex()) {
                                                continue;
                                            }
                                            if classify_daemon_message(&msg)
                                                == Some(MessageClassification::TypingIndicator)
                                            {
                                                continue;
                                            }
                                            let media: Vec<MediaAttachmentOut> = {
                                                host.parse_message_media_attachments(&msg)
                                                    .into_iter()
                                                    .map(|attachment| media_attachment_to_out(attachment.attachment))
                                                    .collect()
                                            };
                                            out_tx.send(OutMsg::MessageReceived{
                                                nostr_group_id: accepted.nostr_group_id_hex.clone(),
                                                from_pubkey: msg.pubkey.to_hex().to_lowercase(),
                                                content: msg.content,
                                                kind: msg.kind.as_u16(),
                                                created_at: msg.created_at.as_secs(),
                                                event_id: msg.id.to_hex(),
                                                message_id: msg.id.to_hex(),
                                                media,
                                            }).ok();
                                        }

                                        let mls_group_id_hex =
                                            hex::encode(accepted.mls_group_id.as_slice());
                                        reply_tx.send(out_ok(request_id, Some(json!({
                                            "nostr_group_id": accepted.nostr_group_id_hex.clone(),
                                            "mls_group_id": mls_group_id_hex,
                                        })))).ok();
                                        let member_count = mdk.get_members(&accepted.mls_group_id).map(|m| m.len() as u32).unwrap_or(0);
                                        out_tx.send(OutMsg::GroupJoined {
                                            nostr_group_id: accepted.nostr_group_id_hex,
                                            mls_group_id: mls_group_id_hex,
                                            member_count,
                                        }).ok();
                                    }
                                    Err(e) => {
                                        reply_tx.send(out_error(request_id, "mdk_error", format!("{e:#}"))).ok();
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = reply_tx.send(out_error(request_id, "mdk_error", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::ListGroups { request_id } => {
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        match host.list_groups() {
                            Ok(groups) => {
                                let out = groups
                                    .iter()
                                    .map(|group| {
                                        json!({
                                            "nostr_group_id": group.nostr_group_id_hex,
                                            "mls_group_id": group.mls_group_id_hex,
                                            "name": group.name,
                                            "description": group.description,
                                            "member_count": group.member_count,
                                        })
                                    })
                                    .collect::<Vec<_>>();
                                let _ =
                                    reply_tx.send(out_ok(request_id, Some(json!({"groups": out}))));
                            }
                            Err(e) => {
                                let _ = reply_tx
                                    .send(out_error(request_id, "mdk_error", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::GetMessages { request_id, nostr_group_id, limit } => {
                        let pagination = mdk_storage_traits::groups::Pagination::new(Some(limit), None);
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        match host.get_messages(&nostr_group_id, Some(pagination)) {
                            Ok(msgs) => {
                                let out: Vec<serde_json::Value> = msgs.iter().map(|m| {
                                    json!({
                                        "message_id": m.id.to_hex(),
                                        "from_pubkey": m.pubkey.to_hex(),
                                        "content": m.content,
                                        "created_at": m.created_at.as_secs(),
                                    })
                                }).collect();
                                let _ = reply_tx.send(out_ok(request_id, Some(json!({"messages": out}))));
                            }
                            Err(e) => {
                                let _ = reply_tx.send(out_error(request_id, "mdk_error", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::HypernoteCatalog { request_id } => {
                        let _ = reply_tx.send(out_ok(request_id, Some(json!({
                            "catalog": hn::hypernote_catalog_value(),
                        }))));
                    }
                    InCmd::SendMessage { request_id, nostr_group_id, content } => {
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let prepared = match host.prepare_outbound_action(
                            &nostr_group_id,
                            OutboundConversationAction::Message {
                                kind: Kind::ChatMessage,
                                content,
                                tags: vec![],
                                created_at: Timestamp::now(),
                            },
                        ) {
                            Ok(prepared) => prepared,
                            Err(DaemonPrepareError::BadGroup(e)) => {
                                reply_tx.send(out_error(request_id, "bad_group_id", format!("{e:#}"))).ok();
                                continue;
                            }
                            Err(DaemonPrepareError::Prepare(e)) => {
                                reply_tx.send(out_error(request_id, "publish_failed", format!("{e:#}"))).ok();
                                continue;
                            }
                        };
                        let inner_id = prepared.rumor_id.to_hex();
                        match host.publish_prepared(&prepared, "daemon_send").await {
                            Ok(_) => {
                                let _ = reply_tx.send(out_ok(request_id, Some(json!({"event_id": inner_id}))));
                            }
                            Err(e) => {
                                let _ = reply_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::SendHypernote {
                        request_id,
                        nostr_group_id,
                        content,
                        title,
                        state,
                    } => {
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let prepared = match host.prepare_outbound_action(
                            &nostr_group_id,
                            OutboundConversationAction::Hypernote {
                                content,
                                title,
                                state,
                                created_at: Timestamp::now(),
                            },
                        ) {
                            Ok(prepared) => prepared,
                            Err(DaemonPrepareError::BadGroup(e)) => {
                                reply_tx.send(out_error(request_id, "bad_group_id", format!("{e:#}"))).ok();
                                continue;
                            }
                            Err(DaemonPrepareError::Prepare(e)) => {
                                reply_tx.send(out_error(request_id, "publish_failed", format!("{e:#}"))).ok();
                                continue;
                            }
                        };
                        // Save the inner rumor ID before MLS wrapping — this is the ID
                        // that receivers see in message_received.event_id and that
                        // submit_hypernote_action must reference.
                        let inner_id = prepared.rumor_id.to_hex();
                        match host.publish_prepared(&prepared, "daemon_send_hypernote").await {
                            Ok(_) => {
                                let _ = reply_tx.send(out_ok(request_id, Some(json!({"event_id": inner_id}))));
                            }
                            Err(e) => {
                                let _ = reply_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::React {
                        request_id,
                        nostr_group_id,
                        event_id,
                        emoji,
                    } => {
                        let target = match EventId::from_hex(event_id.trim()) {
                            Ok(id) => id,
                            Err(_) => {
                                out_tx
                                    .send(out_error(
                                        request_id,
                                        "bad_event_id",
                                        "event_id must be hex",
                                    ))
                                    .ok();
                                continue;
                            }
                        };
                        let emoji = emoji.trim();
                        if emoji.is_empty() {
                            out_tx
                                .send(out_error(request_id, "bad_emoji", "emoji is required"))
                                .ok();
                            continue;
                        }
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let prepared = match host.prepare_outbound_action(
                            &nostr_group_id,
                            OutboundConversationAction::Reaction {
                                target_event_id: target,
                                emoji: emoji.to_string(),
                                created_at: Timestamp::now(),
                            },
                        ) {
                            Ok(prepared) => prepared,
                            Err(DaemonPrepareError::BadGroup(e)) => {
                                out_tx
                                    .send(out_error(request_id, "bad_group_id", format!("{e:#}")))
                                    .ok();
                                continue;
                            }
                            Err(DaemonPrepareError::Prepare(e)) => {
                                out_tx
                                    .send(out_error(request_id, "publish_failed", format!("{e:#}")))
                                    .ok();
                                continue;
                            }
                        };
                        match host.publish_prepared(&prepared, "daemon_react").await {
                            Ok(ev) => {
                                let _ = reply_tx.send(out_ok(
                                    request_id,
                                    Some(json!({"event_id": ev.id.to_hex()})),
                                ));
                            }
                            Err(e) => {
                                let _ =
                                    reply_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::SubmitHypernoteAction {
                        request_id,
                        nostr_group_id,
                        event_id,
                        action,
                        form,
                    } => {
                        let target = match EventId::from_hex(event_id.trim()) {
                            Ok(id) => id,
                            Err(_) => {
                                out_tx
                                    .send(out_error(
                                        request_id,
                                        "bad_event_id",
                                        "event_id must be hex",
                                    ))
                                    .ok();
                                continue;
                            }
                        };
                        let action = action.trim();
                        if action.is_empty() {
                            out_tx
                                .send(out_error(
                                    request_id,
                                    "bad_action",
                                    "action is required",
                                ))
                                .ok();
                            continue;
                        }
                        let payload = hn::build_action_response_payload(action, &form).to_string();
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let prepared = match host.prepare_outbound_action(
                            &nostr_group_id,
                            OutboundConversationAction::Message {
                                kind: Kind::Custom(hn::HYPERNOTE_ACTION_RESPONSE_KIND),
                                content: payload,
                                tags: vec![Tag::event(target)],
                                created_at: Timestamp::now(),
                            },
                        ) {
                            Ok(prepared) => prepared,
                            Err(DaemonPrepareError::BadGroup(e)) => {
                                out_tx
                                    .send(out_error(request_id, "bad_group_id", format!("{e:#}")))
                                    .ok();
                                continue;
                            }
                            Err(DaemonPrepareError::Prepare(e)) => {
                                out_tx
                                    .send(out_error(request_id, "publish_failed", format!("{e:#}")))
                                    .ok();
                                continue;
                            }
                        };
                        match host
                            .publish_prepared(&prepared, "daemon_submit_hypernote_action")
                            .await
                        {
                            Ok(ev) => {
                                let _ = reply_tx.send(out_ok(
                                    request_id,
                                    Some(json!({"event_id": ev.id.to_hex()})),
                                ));
                            }
                            Err(e) => {
                                let _ =
                                    reply_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
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
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let mls_group_id = match host.resolve_group(&nostr_group_id) {
                            Ok(id) => id,
                            Err(e) => {
                                reply_tx.send(out_error(request_id, "bad_group_id", format!("{e:#}"))).ok();
                                continue;
                            }
                        };

                        // Read and validate file
                        let path = std::path::Path::new(&file_path);
                        let bytes = match std::fs::read(path) {
                            Ok(b) => b,
                            Err(e) => {
                                reply_tx.send(out_error(request_id, "file_error", format!("read {file_path}: {e}"))).ok();
                                continue;
                            }
                        };
                        if bytes.is_empty() {
                            reply_tx.send(out_error(request_id, "file_error", "file is empty")).ok();
                            continue;
                        }
                        if bytes.len() > MAX_CHAT_MEDIA_BYTES {
                            reply_tx.send(out_error(request_id, "file_error", "file too large (max 32 MB)")).ok();
                            continue;
                        }

                        let resolved = resolve_upload_metadata(path, mime_type.as_deref(), filename.as_deref());
                        let prepared = match host.prepare_upload(
                            &mls_group_id,
                            &bytes,
                            Some(&resolved.mime_type),
                            Some(&resolved.filename),
                        ) {
                            Ok(prepared) => prepared,
                            Err(e) => {
                                reply_tx.send(out_error(request_id, "encrypt_error", format!("{e:#}"))).ok();
                                continue;
                            }
                        };
                        let upload_servers = blossom_servers_or_default(&blossom_servers);
                        let uploaded = match upload_encrypted_blob(
                            &keys,
                            prepared.encrypted_data,
                            &prepared.upload.mime_type,
                            &hex::encode(prepared.upload.encrypted_hash),
                            &upload_servers,
                        )
                        .await
                        {
                            Ok(uploaded) => uploaded,
                            Err(e) => {
                                reply_tx.send(out_error(
                                    request_id,
                                    "upload_failed",
                                    format!("{e:#}"),
                                )).ok();
                                continue;
                            }
                        };

                        let result =
                            host.finish_upload(&mls_group_id, &prepared.upload, uploaded.clone());

                        // Build imeta tag and message
                        let rumor = EventBuilder::new(Kind::ChatMessage, &caption)
                            .tag(result.imeta_tag.clone())
                            .build(keys.public_key());
                        match host.sign_and_publish_rumor(&mls_group_id, rumor, "daemon_send_media").await {
                            Ok(ev) => {
                                let _ = reply_tx.send(out_ok(request_id, Some(json!({
                                    "event_id": ev.id.to_hex(),
                                    "uploaded_url": uploaded.uploaded_url,
                                    "original_hash_hex": result.attachment.original_hash_hex,
                                }))));
                            }
                            Err(e) => {
                                let _ = reply_tx.send(out_error(
                                    request_id,
                                    "publish_failed",
                                    format!("{e:#}"),
                                ));
                            }
                        }
                    }
                    InCmd::SendMediaBatch {
                        request_id,
                        nostr_group_id,
                        file_paths,
                        caption,
                        blossom_servers,
                    } => {
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let mls_group_id = match host.resolve_group(&nostr_group_id) {
                            Ok(id) => id,
                            Err(e) => {
                                reply_tx.send(out_error(request_id, "bad_group_id", format!("{e:#}"))).ok();
                                continue;
                            }
                        };

                        if file_paths.is_empty() {
                            reply_tx.send(out_error(request_id, "file_error", "no file paths provided")).ok();
                            continue;
                        }
                        if file_paths.len() > 32 {
                            reply_tx.send(out_error(request_id, "file_error", "too many files (max 32)")).ok();
                            continue;
                        }

                        let upload_servers = blossom_servers_or_default(&blossom_servers);
                        // Process all files: read, encrypt, upload sequentially.
                        #[allow(clippy::type_complexity)]
                        let batch_result: Result<(Vec<Tag>, Vec<String>, Vec<String>), ()> = async {
                            let mut imeta_tags = Vec::new();
                            let mut original_hashes = Vec::new();
                            let mut uploaded_urls = Vec::new();

                            for file_path in &file_paths {
                                let path = std::path::Path::new(file_path);
                                let bytes = match std::fs::read(path) {
                                    Ok(b) => b,
                                    Err(e) => {
                                        reply_tx.send(out_error(request_id.clone(), "file_error", format!("read {file_path}: {e}"))).ok();
                                        return Err(());
                                    }
                                };
                                if bytes.is_empty() {
                                    reply_tx.send(out_error(request_id.clone(), "file_error", format!("file is empty: {file_path}"))).ok();
                                    return Err(());
                                }
                                if bytes.len() > MAX_CHAT_MEDIA_BYTES {
                                    reply_tx.send(out_error(request_id.clone(), "file_error", format!("file too large (max 32 MB): {file_path}"))).ok();
                                    return Err(());
                                }

                                let resolved = resolve_upload_metadata(path, None, None);
                                let prepared = match host.prepare_upload(
                                    &mls_group_id,
                                    &bytes,
                                    Some(&resolved.mime_type),
                                    Some(&resolved.filename),
                                ) {
                                    Ok(prepared) => prepared,
                                    Err(e) => {
                                        reply_tx.send(out_error(request_id.clone(), "encrypt_error", format!("{e:#}"))).ok();
                                        return Err(());
                                    }
                                };
                                let uploaded = match upload_encrypted_blob(
                                    &keys,
                                    prepared.encrypted_data,
                                    &prepared.upload.mime_type,
                                    &hex::encode(prepared.upload.encrypted_hash),
                                    &upload_servers,
                                )
                                .await
                                {
                                    Ok(uploaded) => uploaded,
                                    Err(e) => {
                                        reply_tx.send(out_error(
                                            request_id.clone(),
                                            "upload_failed",
                                            format!("upload {file_path}: {e:#}"),
                                        )).ok();
                                        return Err(());
                                    }
                                };
                                let result = host.finish_upload(
                                    &mls_group_id,
                                    &prepared.upload,
                                    uploaded.clone(),
                                );

                                if result.uploaded_blob.uploaded_url.is_empty() {
                                    reply_tx.send(out_error(
                                        request_id.clone(),
                                        "upload_failed",
                                        format!("upload {file_path}: missing upload URL"),
                                    )).ok();
                                    return Err(());
                                }

                                original_hashes.push(result.attachment.original_hash_hex);
                                uploaded_urls.push(result.uploaded_blob.uploaded_url);
                                imeta_tags.push(result.imeta_tag);
                            }

                            Ok((imeta_tags, original_hashes, uploaded_urls))
                        }.await;

                        let (imeta_tags, original_hashes, uploaded_urls) = match batch_result {
                            Ok(v) => v,
                            Err(()) => continue,
                        };

                        let mut builder = EventBuilder::new(Kind::ChatMessage, &caption);
                        for tag in &imeta_tags {
                            builder = builder.tag(tag.clone());
                        }
                        let rumor = builder.build(keys.public_key());
                        match host.sign_and_publish_rumor(&mls_group_id, rumor, "daemon_send_media_batch").await {
                            Ok(ev) => {
                                let _ = reply_tx.send(out_ok(request_id, Some(json!({
                                    "event_id": ev.id.to_hex(),
                                    "uploaded_urls": uploaded_urls,
                                    "original_hashes": original_hashes,
                                }))));
                            }
                            Err(e) => {
                                let _ = reply_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::SendTyping { request_id, nostr_group_id } => {
                        let expires_at = Timestamp::from_secs(Timestamp::now().as_secs() + 10);
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let prepared = match host.prepare_outbound_action(
                            &nostr_group_id,
                            OutboundConversationAction::Typing {
                                created_at: Timestamp::now(),
                                expires_at,
                            },
                        ) {
                            Ok(prepared) => prepared,
                            Err(DaemonPrepareError::BadGroup(e)) => {
                                reply_tx.send(out_error(request_id, "bad_group_id", format!("{e:#}"))).ok();
                                continue;
                            }
                            Err(DaemonPrepareError::Prepare(e)) => {
                                reply_tx.send(out_error(request_id, "mdk_error", format!("{e:#}"))).ok();
                                continue;
                            }
                        };

                        if relay_urls.is_empty() {
                            reply_tx.send(out_error(request_id, "bad_relays", "no relays configured")).ok();
                            continue;
                        }
                        // Fire-and-forget: typing indicators are best-effort
                        let client_clone = client.clone();
                        let relay_urls_clone = relay_urls.clone();
                        let out_tx_clone = out_tx.clone();
                        tokio::spawn(async move {
                            match publish_and_confirm_multi(&client_clone, &relay_urls_clone, &prepared.wrapper, "daemon_typing").await {
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
                            let _ = reply_tx.send(out_error(request_id, "busy", "call already active"));
                            continue;
                        }
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let peer_pubkey = match PublicKey::parse(peer_pubkey.trim()) {
                            Ok(pk) => pk,
                            Err(e) => {
                                let _ = reply_tx.send(out_error(
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
                            match host
                                .derive_relay_auth_token(
                                    &nostr_group_id,
                                    &call_id,
                                    &session,
                                    &peer_pubkey_hex,
                                )
                            {
                                Ok(token) => {
                                    session.relay_auth = token;
                                }
                                Err(e) => {
                                    let _ = reply_tx.send(out_error(
                                        request_id,
                                        "runtime_error",
                                        format!("derive relay auth token failed: {e:#}"),
                                    ));
                                    continue;
                                }
                            }
                        }
                        let (pending, prepared_invite) = match host.prepare_call_invite(
                            &nostr_group_id,
                            &peer_pubkey_hex,
                            &call_id,
                            &session,
                        ) {
                            Ok(v) => v,
                            Err(e) => {
                                let _ = reply_tx.send(out_error(
                                    request_id,
                                    "runtime_error",
                                    format!("prepare call invite failed: {e}"),
                                ));
                                continue;
                            }
                        };
                        match send_call_invite_with_retry(
                            &host,
                            &nostr_group_id,
                            &prepared_invite.payload_json,
                            &call_id,
                            3,
                        )
                        .await {
                            Ok(()) => {
                                pending_outgoing_call_invites.insert(call_id.clone(), pending);
                                let _ = reply_tx.send(out_ok(
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
                                    reply_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::AcceptCall { request_id, call_id } => {
                        if active_call.is_some() {
                            let _ = reply_tx.send(out_error(request_id, "busy", "call already active"));
                            continue;
                        }
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let Some(invite) = pending_call_invites.remove(&call_id) else {
                            let _ = reply_tx.send(out_error(request_id, "not_found", "pending call invite not found"));
                            continue;
                        };
                        let prepared = match host.prepare_accept_call(&invite) {
                            Ok(v) => v,
                            Err(err) => {
                                if let Ok(signal) =
                                    host.prepare_reject_call_signal(&invite.call_id, "auth_failed")
                                {
                                    let _ = host
                                        .publish_call_payload(
                                            &invite.target_id,
                                            signal.payload_json,
                                            "call_reject_auth_failed",
                                        )
                                        .await;
                                }
                                let _ = reply_tx.send(out_error(request_id, "auth_failed", err));
                                continue;
                            }
                        };

                        match host
                            .publish_call_payload(
                                &invite.target_id,
                                prepared.signal.payload_json,
                                "call_accept",
                            )
                            .await
                        {
                            Ok(()) => {}
                            Err(e) => {
                                let _ = reply_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                                continue;
                            }
                        }

                        let mode = active_call_mode(&prepared.incoming.session);
                        let worker = match mode {
                            ActiveCallMode::Audio => {
                                if echo_mode_enabled() {
                                    match start_echo_worker(
                                        &prepared.incoming.call_id,
                                        &prepared.incoming.session,
                                        prepared.media_crypto.clone(),
                                        out_tx.clone(),
                                    ) {
                                        Ok(v) => v,
                                        Err(e) => {
                                            let _ = reply_tx.send(out_error(
                                                request_id,
                                                "runtime_error",
                                                format!("{e:#}"),
                                            ));
                                            continue;
                                        }
                                    }
                                } else {
                                    match start_stt_worker(
                                        &prepared.incoming.call_id,
                                        &prepared.incoming.session,
                                        prepared.media_crypto.clone(),
                                        out_tx.clone(),
                                        call_evt_tx.clone(),
                                    ) {
                                        Ok(v) => v,
                                        Err(e) => {
                                            let _ = reply_tx.send(out_error(
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
                                &prepared.incoming.call_id,
                                &prepared.incoming.session,
                                prepared.media_crypto.clone(),
                                call_evt_tx.clone(),
                            ) {
                                Ok(v) => v,
                                Err(e) => {
                                    let _ = reply_tx.send(out_error(
                                        request_id,
                                        "runtime_error",
                                        format!("{e:#}"),
                                    ));
                                    continue;
                                }
                            },
                        };

                        active_call = Some(ActiveCall {
                            call_id: prepared.incoming.call_id.clone(),
                            nostr_group_id: invite.target_id.clone(),
                            session: prepared.incoming.session.clone(),
                            mode,
                            media_crypto: prepared.media_crypto,
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
                        let _ = reply_tx.send(out_ok(request_id, Some(json!({
                            "call_id": prepared.incoming.call_id,
                            "nostr_group_id": invite.target_id,
                        }))));
                        let _ = out_tx.send(OutMsg::CallSessionStarted {
                            call_id: prepared.incoming.call_id,
                            nostr_group_id: invite.target_id,
                            from_pubkey: invite.from_pubkey_hex,
                        });
                    }
                    InCmd::RejectCall {
                        request_id,
                        call_id,
                        reason,
                    } => {
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let Some(invite) = pending_call_invites.remove(&call_id) else {
                            let _ = reply_tx.send(out_error(request_id, "not_found", "pending call invite not found"));
                            continue;
                        };
                        let signal = match host.prepare_reject_call_signal(&invite.call_id, &reason) {
                            Ok(signal) => signal,
                            Err(e) => {
                                let _ = reply_tx.send(out_error(
                                    request_id,
                                    "runtime_error",
                                    format!("prepare call reject failed: {e}"),
                                ));
                                continue;
                            }
                        };
                        match host
                            .publish_call_payload(&invite.target_id, signal.payload_json, "call_reject")
                            .await
                        {
                            Ok(()) => {
                                let _ = reply_tx.send(out_ok(request_id, Some(json!({ "call_id": invite.call_id }))));
                            }
                            Err(e) => {
                                let _ = reply_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::EndCall {
                        request_id,
                        call_id,
                        reason,
                    } => {
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let Some(current) = active_call.take() else {
                            let _ = reply_tx.send(out_error(request_id, "not_found", "active call not found"));
                            continue;
                        };
                        if current.call_id != call_id {
                            active_call = Some(current);
                            let _ = reply_tx.send(out_error(request_id, "not_found", "active call id mismatch"));
                            continue;
                        }

                        if let Ok(signal) = host.prepare_end_call_signal(&call_id, &reason)
                        {
                            let _ = host
                                .publish_call_payload(
                                    &current.nostr_group_id,
                                    signal.payload_json,
                                    "call_end",
                                )
                                .await;
                        }
                        current.worker.stop().await;
                        let _ = reply_tx.send(out_ok(request_id, Some(json!({ "call_id": call_id }))));
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
                            let _ = reply_tx.send(out_error(request_id, "not_found", "active call not found"));
                            continue;
                        };
                        if current.call_id != call_id {
                            let _ = reply_tx.send(out_error(request_id, "not_found", "active call id mismatch"));
                            continue;
                        }
                        if current.mode != ActiveCallMode::Audio {
                            let _ = reply_tx.send(out_error(
                                request_id,
                                "bad_request",
                                "active call is not an audio call",
                            ));
                            continue;
                        }
                        if tts_text.trim().is_empty() {
                            let _ = reply_tx.send(out_error(request_id, "bad_request", "tts_text must not be empty"));
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
                                let _ = reply_tx.send(out_ok(
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
                                let _ = reply_tx.send(out_error(
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
                            let _ = reply_tx.send(out_error(request_id, "not_found", "active call not found"));
                            continue;
                        };
                        if current.call_id != call_id {
                            let _ = reply_tx.send(out_error(request_id, "not_found", "active call id mismatch"));
                            continue;
                        }
                        if current.mode != ActiveCallMode::Audio {
                            let _ = reply_tx.send(out_error(
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
                                let _ = reply_tx.send(out_error(
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
                            let _ = reply_tx.send(out_error(request_id, "not_found", "active call not found"));
                            continue;
                        };
                        if current.call_id != call_id {
                            let _ = reply_tx.send(out_error(request_id, "not_found", "active call id mismatch"));
                            continue;
                        }
                        if current.mode != ActiveCallMode::Data {
                            let _ = reply_tx.send(out_error(
                                request_id,
                                "bad_request",
                                "active call is not a data call",
                            ));
                            continue;
                        }
                        let payload = match hex::decode(payload_hex.trim()) {
                            Ok(v) => v,
                            Err(_) => {
                                let _ = reply_tx.send(out_error(
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
                                    let _ = reply_tx.send(out_error(
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
                                let _ = reply_tx.send(out_ok(request_id, None));
                            }
                            Err(err) => {
                                let _ = reply_tx.send(out_error(
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
                                reply_tx.send(out_error(request_id, "bad_pubkey", format!("invalid peer_pubkey: {e}"))).ok();
                                continue;
                            }
                        };

                        if relay_urls.is_empty() {
                            reply_tx.send(out_error(request_id, "bad_relays", "no relays configured")).ok();
                            continue;
                        }

                        let peer_kp = match fetch_latest_key_package_for_mdk(
                            &client,
                            &peer_pubkey,
                            &relay_urls,
                            Duration::from_secs(10),
                        )
                        .await
                        {
                            Ok(ev) => ev,
                            Err(e) => {
                                let (code, message) = if e
                                    .chain()
                                    .any(|cause| cause.to_string().contains("no keypackage found for"))
                                {
                                    (
                                        "no_key_packages",
                                        "no key package found for peer".to_string(),
                                    )
                                } else {
                                    ("fetch_failed", format!("fetch key package: {e:#}"))
                                };
                                reply_tx.send(out_error(request_id, code, message)).ok();
                                continue;
                            }
                        };
                        let peer_kp = normalize_peer_key_package_event_for_mdk(&peer_kp);

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

                        let created = match create_group_and_publish_welcomes_for_init_group_with_confirm(
                            &keys,
                            &mdk,
                            &client,
                            &relay_urls,
                            peer_kp,
                            peer_pubkey,
                            config,
                        )
                        .await
                        {
                            Ok(created) => created,
                            Err(e) => {
                                let (code, message) = map_init_group_error(&e);
                                reply_tx.send(out_error(request_id, code, message)).ok();
                                continue;
                            }
                        };

                        let nostr_group_id_hex = hex::encode(created.group.nostr_group_id);
                        let mls_group_id_hex = hex::encode(created.group.mls_group_id.as_slice());

                        // Daemon init_group is stricter than app create: it
                        // waits for welcome delivery and subscribes before
                        // reporting success to the host protocol.

                        // Subscribe to new group messages.
                        match crate::subscribe_group_msgs(&client, &nostr_group_id_hex).await {
                            Ok(sid) => {
                                group_subs.insert(sid, nostr_group_id_hex.clone());
                            }
                            Err(err) => {
                                warn!("[pikachat] subscribe group msgs failed after init_group: {err:#}");
                            }
                        }

                        reply_tx.send(out_ok(request_id, Some(json!({
                            "nostr_group_id": nostr_group_id_hex,
                            "mls_group_id": mls_group_id_hex,
                            "peer_pubkey": peer_pubkey.to_hex(),
                        })))).ok();
                        let member_count = mdk.get_members(&created.group.mls_group_id).map(|m| m.len() as u32).unwrap_or(0);
                        out_tx.send(OutMsg::GroupCreated {
                            nostr_group_id: nostr_group_id_hex,
                            mls_group_id: mls_group_id_hex,
                            peer_pubkey: peer_pubkey.to_hex(),
                            member_count,
                        }).ok();
                    }
                    InCmd::Shutdown { request_id } => {
                        reply_tx.send(out_ok(request_id, None)).ok();
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

                if subscription_id == gift_sub {
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
                            .send(DaemonCmd {
                                cmd: InCmd::AcceptWelcome {
                                    request_id: Some("auto-accept".into()),
                                    wrapper_event_id: wid_hex,
                                },
                                response_tx: None,
                            })
                            .ok();
                    }

                    continue;
                }

                if event.kind == Kind::MlsGroupMessage {
                    let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                    // Only process messages for subscriptions we created.
                    if !group_subs.contains_key(&subscription_id) {
                        continue;
                    }
                    if !seen_group_events.insert(event.id) {
                        continue;
                    }

                    match host.process_event(&event) {
                        Ok(Some(ConversationEvent::Application(runtime_msg))) => {
                            let classification = runtime_msg.classification;
                            let nostr_group_id = runtime_msg.nostr_group_id_hex;
                            let msg = runtime_msg.message;
                            let sender_hex = msg.pubkey.to_hex().to_lowercase();
                            if !sender_allowed(&sender_hex) {
                                warn!("[pikachat] drop message (sender not allowed) from={sender_hex}");
                                continue;
                            }
                            if let Some(signal) = parse_call_signal(&msg.content) {
                                let mls_group_id = match host.resolve_group(&nostr_group_id) {
                                    Ok(group_id) => group_id,
                                    Err(err) => {
                                        warn!(
                                            "[pikachat] resolve call group failed group={} err={err:#}",
                                            nostr_group_id
                                        );
                                        continue;
                                    }
                                };
                                let pending_outgoing = match &signal {
                                    ParsedCallSignal::Accept { call_id, .. } => {
                                        pending_outgoing_call_invites.get(call_id)
                                    }
                                    _ => None,
                                };
                                match host.handle_inbound_call_signal(
                                    pika_marmot_runtime::call_runtime::InboundSignalContext {
                                        target_id: &nostr_group_id,
                                        sender_pubkey_hex: &sender_hex,
                                        group: GroupCallContext {
                                            mls_group_id: &mls_group_id,
                                            local_pubkey_hex: &pubkey_hex,
                                        },
                                        policy: InboundCallPolicy {
                                            allow_group_calls: true,
                                            allow_video_calls: false,
                                        },
                                        has_live_call: active_call.is_some(),
                                        pending_outgoing,
                                    },
                                    signal,
                                ) {
                                    InboundCallSignalOutcome::Ignore => {}
                                    InboundCallSignalOutcome::RejectIncoming(rejected) => {
                                        let label = match rejected.reason_code.as_str() {
                                            "unsupported_video" => "call_video_reject",
                                            "busy" => "call_busy_reject",
                                            _ => "call_reject",
                                        };
                                        if let Some(err) = rejected.error {
                                            warn!(
                                                "[pikachat] reject incoming call call_id={} reason={} err={}",
                                                rejected.call_id, rejected.reason_code, err
                                            );
                                        }
                                        let _ = host
                                            .publish_call_payload(
                                                &nostr_group_id,
                                                rejected.signal.payload_json,
                                                label,
                                            )
                                            .await;
                                    }
                                    InboundCallSignalOutcome::IncomingInvite(invite) => {
                                        pending_call_invites.insert(
                                            invite.call_id.clone(),
                                            (*invite).clone(),
                                        );
                                        out_tx
                                            .send(OutMsg::CallInviteReceived {
                                                call_id: invite.call_id.clone(),
                                                from_pubkey: invite.from_pubkey_hex.clone(),
                                                nostr_group_id: invite.target_id.clone(),
                                            })
                                            .ok();
                                    }
                                    InboundCallSignalOutcome::OutgoingAccepted(accepted) => {
                                        if active_call.is_some() {
                                            continue;
                                        }
                                        let mode = active_call_mode(&accepted.session);
                                        let worker = match mode {
                                            ActiveCallMode::Audio => {
                                                if echo_mode_enabled() {
                                                    match start_echo_worker(
                                                        &accepted.pending.call_id,
                                                        &accepted.session,
                                                        accepted.media_crypto.clone(),
                                                        out_tx.clone(),
                                                    ) {
                                                        Ok(v) => v,
                                                        Err(err) => {
                                                            warn!(
                                                                "[pikachat] start echo worker failed call_id={} err={err:#}",
                                                                accepted.pending.call_id
                                                            );
                                                            continue;
                                                        }
                                                    }
                                                } else {
                                                    match start_stt_worker(
                                                        &accepted.pending.call_id,
                                                        &accepted.session,
                                                        accepted.media_crypto.clone(),
                                                        out_tx.clone(),
                                                        call_evt_tx.clone(),
                                                    ) {
                                                        Ok(v) => v,
                                                        Err(err) => {
                                                            warn!(
                                                                "[pikachat] start stt worker failed call_id={} err={err:#}",
                                                                accepted.pending.call_id
                                                            );
                                                            continue;
                                                        }
                                                    }
                                                }
                                            }
                                            ActiveCallMode::Data => match start_data_worker(
                                                &accepted.pending.call_id,
                                                &accepted.session,
                                                accepted.media_crypto.clone(),
                                                call_evt_tx.clone(),
                                            ) {
                                                Ok(v) => v,
                                                Err(err) => {
                                                    warn!(
                                                        "[pikachat] start data worker failed call_id={} err={err:#}",
                                                        accepted.pending.call_id
                                                    );
                                                    continue;
                                                }
                                            },
                                        };
                                        active_call = Some(ActiveCall {
                                            call_id: accepted.pending.call_id.clone(),
                                            nostr_group_id: accepted.pending.target_id.clone(),
                                            session: accepted.session.clone(),
                                            mode,
                                            media_crypto: accepted.media_crypto,
                                            next_voice_seq: 0,
                                            next_data_seq: 0,
                                            worker,
                                        });
                                        pending_outgoing_call_invites.remove(&accepted.pending.call_id);
                                        out_tx
                                            .send(OutMsg::CallSessionStarted {
                                                call_id: accepted.pending.call_id,
                                                from_pubkey: sender_hex,
                                                nostr_group_id: accepted.pending.target_id,
                                            })
                                            .ok();
                                    }
                                    InboundCallSignalOutcome::IncomingAcceptFailed(failure) => {
                                        warn!(
                                            "[pikachat] call.accept failed call_id={} kind={:?} err={}",
                                            failure.call_id, failure.kind, failure.error
                                        );
                                    }
                                    InboundCallSignalOutcome::RemoteTermination(ended) => {
                                        pending_call_invites.remove(&ended.call_id);
                                        pending_outgoing_call_invites.remove(&ended.call_id);
                                        if active_call
                                            .as_ref()
                                            .map(|c| c.call_id == ended.call_id)
                                            .unwrap_or(false)
                                        {
                                            if let Some(current) = active_call.take() {
                                                current.worker.stop().await;
                                            }
                                            out_tx
                                                .send(OutMsg::CallSessionEnded {
                                                    call_id: ended.call_id,
                                                    reason: ended.reason,
                                                })
                                                .ok();
                                        }
                                    }
                                }
                                continue;
                            }
                            if classification == MessageClassification::TypingIndicator {
                                continue;
                            }
                            let mut media: Vec<MediaAttachmentOut> = Vec::new();
                            {
                                let attachments = host.parse_message_media_attachments(&msg);
                                for attachment in attachments {
                                    let mut att = media_attachment_to_out(attachment.attachment.clone());
                                    match host
                                        .download_and_decrypt_media(
                                            &msg.mls_group_id,
                                            &attachment,
                                            state_dir,
                                        )
                                        .await
                                    {
                                        Ok(path) => att.local_path = Some(path),
                                        Err(e) => warn!("[pikachat] media download failed url={}: {e:#}", attachment.attachment.url),
                                    }
                                    media.push(att);
                                }
                            }
                            out_tx.send(OutMsg::MessageReceived {
                                nostr_group_id,
                                from_pubkey: sender_hex,
                                content: msg.content,
                                kind: msg.kind.as_u16(),
                                created_at: msg.created_at.as_secs(),
                                event_id: msg.id.to_hex(),
                                message_id: msg.id.to_hex(),
                                media,
                            }).ok();
                        }
                        Ok(Some(_)) => {}
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
    let _ = client.unsubscribe(&gift_sub).await;
    client.unsubscribe_all().await;
    client.shutdown().await;
    // Clean up Unix socket
    let _ = std::fs::remove_file(&sock_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdk_core::prelude::NostrGroupConfigData;
    use pika_marmot_runtime::media::{is_imeta_tag, mime_from_extension};
    use pika_marmot_runtime::message::TYPING_INDICATOR_KIND;

    fn event_id(hex: &str) -> EventId {
        EventId::from_hex(hex).expect("valid event id")
    }

    fn make_key_package_event(mdk: &crate::PikaMdk, keys: &Keys) -> Event {
        let relay = RelayUrl::parse("wss://test.relay").expect("relay url");
        let (content, tags, _hash_ref) = mdk
            .create_key_package_for_event(&keys.public_key(), vec![relay])
            .expect("create key package");
        EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign key package")
    }

    fn test_host<'a>(
        mdk: &'a crate::PikaMdk,
        keys: &'a Keys,
        client: &'a Client,
        relay_urls: &'a [RelayUrl],
    ) -> DaemonHostContext<'a> {
        DaemonHostContext::new(client, relay_urls, mdk, keys, keys.public_key().to_hex())
    }

    fn make_test_message(
        kind: Kind,
        content: &str,
        tags: Tags,
    ) -> mdk_storage_traits::messages::types::Message {
        let pubkey = Keys::generate().public_key();
        let created_at = Timestamp::from(123_u64);
        let mls_group_id = GroupId::from_slice(&[1, 2, 3]);
        mdk_storage_traits::messages::types::Message {
            id: EventId::all_zeros(),
            mls_group_id: mls_group_id.clone(),
            pubkey,
            kind,
            created_at,
            processed_at: created_at,
            content: content.to_string(),
            tags: tags.clone(),
            event: UnsignedEvent::new(pubkey, created_at, kind, tags, content.to_string()),
            wrapper_event_id: EventId::all_zeros(),
            epoch: None,
            state: message_types::MessageState::Processed,
        }
    }

    fn make_pending_welcome(
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
            mls_group_id: GroupId::from_slice(&[1, 2, 3]),
            nostr_group_id: [1; 32],
            group_name: "test".to_string(),
            group_description: String::new(),
            group_image_hash: None,
            group_image_key: None,
            group_image_nonce: None,
            group_admin_pubkeys: std::collections::BTreeSet::new(),
            group_relays: std::collections::BTreeSet::new(),
            welcomer,
            member_count: 2,
            state: mdk_storage_traits::welcomes::types::WelcomeState::Pending,
            wrapper_event_id: event_id(wrapper_hex),
        }
    }

    #[test]
    fn pending_welcome_lookup_uses_shared_runtime_match_rules() {
        let items = vec![
            make_pending_welcome(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
            make_pending_welcome(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            ),
        ];

        assert_eq!(
            find_pending_welcome_index(&items, &items[0].wrapper_event_id),
            Some(0)
        );
        assert_eq!(find_pending_welcome_index(&items, &items[1].id), Some(1));
        assert_eq!(
            find_pending_welcome_index(
                &items,
                &event_id("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
            ),
            None
        );
    }

    #[test]
    fn daemon_group_lookup_uses_shared_runtime_facade() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon conversation lookup".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let resolved = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls)
            .resolve_group(&hex::encode(created.group.nostr_group_id))
            .expect("resolve group");
        assert_eq!(resolved, created.group.mls_group_id);
    }

    #[test]
    fn daemon_outbound_prepare_uses_shared_runtime_facade() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon outbound".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let prepared = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls)
            .prepare_outbound_action(
                &hex::encode(created.group.nostr_group_id),
                OutboundConversationAction::Reaction {
                    target_event_id: EventId::all_zeros(),
                    emoji: "👍".to_string(),
                    created_at: Timestamp::from(123_u64),
                },
            )
            .expect("prepare outbound action");

        assert_eq!(
            prepared.target.nostr_group_id_hex,
            hex::encode(created.group.nostr_group_id)
        );
        assert_eq!(prepared.kind, Kind::Reaction);
        assert_eq!(prepared.wrapper.kind, Kind::MlsGroupMessage);
    }

    #[tokio::test]
    async fn accept_welcome_with_backfill_uses_shared_runtime_helper() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let invitee_client = Client::builder().signer(invitee_keys.clone()).build();

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon accept test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let group_result = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let welcome_rumor = group_result
            .welcome_rumors
            .into_iter()
            .next()
            .expect("welcome rumor");
        let wrapper =
            EventBuilder::gift_wrap(&inviter_keys, &invitee_keys.public_key(), welcome_rumor, [])
                .await
                .expect("build giftwrap");

        crate::ingest_welcome_from_giftwrap(&invitee_mdk, &invitee_keys, &wrapper, |_| true)
            .await
            .expect("ingest welcome")
            .expect("welcome should ingest");

        let pending = invitee_mdk
            .get_pending_welcomes(None)
            .expect("get pending welcomes");
        let welcome = pending.first().expect("pending welcome");
        let mut seen_group_events = HashSet::new();
        let accepted = accept_welcome_with_backfill(
            &invitee_mdk,
            &invitee_client,
            &[],
            welcome,
            &mut seen_group_events,
            |_| async { Ok(()) },
        )
        .await
        .expect("accept welcome with backfill");

        assert_eq!(
            accepted.nostr_group_id_hex,
            hex::encode(group_result.group.nostr_group_id)
        );
        assert_eq!(accepted.group_name, "Daemon accept test");
        assert!(
            accepted.ingested_messages.is_empty(),
            "empty relay list should keep daemon wrapper catch-up narrow in tests"
        );
        assert!(
            invitee_mdk
                .get_pending_welcomes(None)
                .expect("get pending welcomes")
                .is_empty(),
            "shared daemon helper should clear the pending welcome"
        );
    }

    #[test]
    fn daemon_runtime_bootstrap_uses_shared_session_service() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon runtime bootstrap".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");

        let bootstrapped =
            bootstrap_runtime_for_daemon(inviter_dir.path(), &inviter_keys).expect("bootstrap");

        assert_eq!(bootstrapped.session.pubkey, inviter_keys.public_key());
        assert_eq!(
            bootstrapped.startup.existing_group_ids,
            vec![hex::encode(created.group.nostr_group_id)]
        );
    }

    #[tokio::test]
    async fn init_group_uses_shared_runtime_helper_and_keeps_expiration_tag() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon init_group test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let published =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::<(PublicKey, Event)>::new()));
        let published_capture = std::sync::Arc::clone(&published);

        let created = create_group_and_publish_welcomes_for_init_group(
            &inviter_keys,
            &inviter_mdk,
            invitee_kp,
            invitee_keys.public_key(),
            config,
            move |receiver, giftwrap| {
                let published_capture = std::sync::Arc::clone(&published_capture);
                async move {
                    published_capture
                        .lock()
                        .expect("published lock")
                        .push((receiver, giftwrap));
                    Ok(())
                }
            },
        )
        .await
        .expect("init group create/publish");

        assert_eq!(created.group.name, "Daemon init_group test");
        assert_eq!(created.published_welcomes.len(), 1);
        assert_eq!(
            created.published_welcomes[0].receiver,
            invitee_keys.public_key()
        );

        let published = published.lock().expect("published lock");
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].0, invitee_keys.public_key());
        assert_eq!(published[0].1.kind, Kind::GiftWrap);
        assert!(
            published[0].1.tags.expiration().is_some(),
            "daemon init_group should keep its expiration-tag policy local"
        );
    }

    #[test]
    fn init_group_error_mapping_uses_daemon_publish_marker() {
        let err = anyhow::anyhow!("relay confirm failed").context("init_group_publish_welcome");
        let (code, message) = map_init_group_error(&err);
        assert_eq!(code, "publish_failed");
        assert!(message.contains("init_group_publish_welcome"));
    }

    #[test]
    fn accept_welcome_error_messages_mention_both_event_ids() {
        assert!(
            accept_welcome_bad_event_id_message().contains("wrapper_event_id or welcome_event_id")
        );
        assert!(
            accept_welcome_not_found_message().contains("wrapper_event_id or welcome_event_id")
        );
        assert!(accept_welcome_not_found_message().contains("list_pending_welcomes"));
    }

    #[test]
    fn daemon_typing_detection_uses_shared_classifier() {
        let typing = make_test_message(
            TYPING_INDICATOR_KIND,
            "typing",
            vec![Tag::parse(["d", "pika"]).expect("pika tag")]
                .into_iter()
                .collect(),
        );
        let unmarked = make_test_message(TYPING_INDICATOR_KIND, "typing", Tags::new());

        assert_eq!(
            classify_daemon_message(&typing),
            Some(MessageClassification::TypingIndicator)
        );
        assert_eq!(classify_daemon_message(&unmarked), None);
    }

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

    #[test]
    fn parses_call_accept_signal() {
        let content = serde_json::json!({
            "v": 1,
            "ns": "pika.call",
            "type": "call.accept",
            "call_id": "550e8400-e29b-41d4-a716-446655440001",
            "ts_ms": 1730000000000i64,
            "body": {
                "moq_url": "https://moq.local/anon",
                "broadcast_base": "pika/calls/550e8400-e29b-41d4-a716-446655440001",
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
        let parsed = parse_call_signal(&content).expect("parse call.accept");
        match parsed {
            ParsedCallSignal::Accept { call_id, session } => {
                assert_eq!(call_id, "550e8400-e29b-41d4-a716-446655440001");
                assert_eq!(session.moq_url, "https://moq.local/anon");
            }
            other => panic!("expected accept signal, got {other:?}"),
        }
    }

    #[test]
    fn daemon_prepare_call_invite_uses_shared_runtime_facade() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mdk = crate::open_mdk(dir.path()).expect("open mdk");
        let keys = Keys::generate();
        let signer: Arc<dyn NostrSigner> = Arc::new(keys.clone());
        let client = Client::new(signer);
        let relay_urls: Vec<RelayUrl> = Vec::new();
        let peer = Keys::generate();
        let session = default_audio_call_session("550e8400-e29b-41d4-a716-446655440010");

        let (pending, prepared) = test_host(&mdk, &keys, &client, &relay_urls)
            .prepare_call_invite(
                "deadbeef",
                &peer.public_key().to_hex(),
                "550e8400-e29b-41d4-a716-446655440010",
                &session,
            )
            .expect("prepare daemon call invite");

        assert_eq!(pending.target_id, "deadbeef");
        assert_eq!(pending.peer_pubkey_hex, peer.public_key().to_hex());
        assert!(prepared.payload_json.contains("call.invite"));
    }

    #[test]
    fn parses_call_reject_and_end_signal_variants() {
        let reject = serde_json::json!({
            "v": 1,
            "ns": "pika.call",
            "type": "call.reject",
            "call_id": "550e8400-e29b-41d4-a716-446655440002",
            "ts_ms": 1730000000000i64,
            "body": { "reason": "busy" }
        })
        .to_string();
        match parse_call_signal(&reject).expect("parse call.reject") {
            ParsedCallSignal::Reject { call_id, reason } => {
                assert_eq!(call_id, "550e8400-e29b-41d4-a716-446655440002");
                assert_eq!(reason, "busy");
            }
            other => panic!("expected reject signal, got {other:?}"),
        }

        let end_inner = serde_json::json!({
            "v": 1,
            "ns": "pika.call",
            "type": "call.end",
            "call_id": "550e8400-e29b-41d4-a716-446655440003",
            "ts_ms": 1730000000000i64,
            "body": {}
        })
        .to_string();
        let end_wrapped = serde_json::json!({
            "rumor": { "content": end_inner }
        })
        .to_string();
        match parse_call_signal(&end_wrapped).expect("parse wrapped call.end") {
            ParsedCallSignal::End { call_id, reason } => {
                assert_eq!(call_id, "550e8400-e29b-41d4-a716-446655440003");
                assert_eq!(reason, "remote_end");
            }
            other => panic!("expected end signal, got {other:?}"),
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
            video_tx_keys: None,
            video_rx_keys: None,
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
    fn daemon_media_parsing_uses_shared_runtime_service() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "daemon media".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let runtime = MarmotRuntime::new(&inviter_mdk);
        let prepared = runtime
            .prepare_upload(
                &created.group.mls_group_id,
                b"daemon attachment",
                Some("text/plain"),
                Some("daemon.txt"),
            )
            .expect("prepare upload");
        let completed = runtime.finish_upload(
            &created.group.mls_group_id,
            &prepared.upload,
            pika_marmot_runtime::media::UploadedBlob {
                blossom_server: "https://example.com".to_string(),
                uploaded_url: "https://example.com/blob".to_string(),
                descriptor_sha256_hex: hex::encode(prepared.upload.encrypted_hash),
            },
        );
        let message = make_test_message(
            Kind::ChatMessage,
            "hi",
            Tags::from_list(vec![completed.imeta_tag]),
        );

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls: Vec<RelayUrl> = Vec::new();
        let attachments = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls)
            .parse_message_media_attachments(&message);

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].attachment.filename, "daemon.txt");
        assert_eq!(attachments[0].attachment.mime_type, "text/plain");
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
        assert!(!result.is_empty());
        assert!(result[0].starts_with("https://"));
    }

    #[test]
    fn blossom_servers_or_default_skips_empty_and_invalid() {
        let servers = vec!["".to_string(), "  ".to_string(), "not a url".to_string()];
        let result = blossom_servers_or_default(&servers);
        assert!(!result.is_empty());
        assert!(result[0].starts_with("https://"));
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

    #[test]
    fn deserialize_hypernote_catalog_cmd() {
        let json = r#"{"cmd":"hypernote_catalog","request_id":"r2"}"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::HypernoteCatalog { request_id } => {
                assert_eq!(request_id.as_deref(), Some("r2"));
            }
            other => panic!("expected HypernoteCatalog, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_react_cmd() {
        let json = r#"{
            "cmd": "react",
            "request_id": "r3",
            "nostr_group_id": "aa",
            "event_id": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "emoji": "🧇"
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::React {
                request_id,
                nostr_group_id,
                event_id,
                emoji,
            } => {
                assert_eq!(request_id.as_deref(), Some("r3"));
                assert_eq!(nostr_group_id, "aa");
                assert_eq!(
                    event_id,
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                );
                assert_eq!(emoji, "🧇");
            }
            other => panic!("expected React, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_submit_hypernote_action_cmd() {
        let json = r#"{
            "cmd": "submit_hypernote_action",
            "request_id": "r4",
            "nostr_group_id": "aa",
            "event_id": "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
            "action": "vote_yes",
            "form": {"note":"ship it"}
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::SubmitHypernoteAction {
                request_id,
                nostr_group_id,
                event_id,
                action,
                form,
            } => {
                assert_eq!(request_id.as_deref(), Some("r4"));
                assert_eq!(nostr_group_id, "aa");
                assert_eq!(
                    event_id,
                    "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
                );
                assert_eq!(action, "vote_yes");
                assert_eq!(form.get("note").map(String::as_str), Some("ship it"));
            }
            other => panic!("expected SubmitHypernoteAction, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_send_audio_response_cmd() {
        let json = r#"{
            "cmd": "send_audio_response",
            "request_id": "r5",
            "call_id": "550e8400-e29b-41d4-a716-446655440010",
            "tts_text": "hello from sidecar"
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize send_audio_response");
        match cmd {
            InCmd::SendAudioResponse {
                request_id,
                call_id,
                tts_text,
            } => {
                assert_eq!(request_id.as_deref(), Some("r5"));
                assert_eq!(call_id, "550e8400-e29b-41d4-a716-446655440010");
                assert_eq!(tts_text, "hello from sidecar");
            }
            other => panic!("expected SendAudioResponse, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_send_audio_file_cmd_defaults_channels() {
        let json = r#"{
            "cmd": "send_audio_file",
            "request_id": "r6",
            "call_id": "550e8400-e29b-41d4-a716-446655440011",
            "audio_path": "/tmp/reply.pcm",
            "sample_rate": 24000
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize send_audio_file");
        match cmd {
            InCmd::SendAudioFile {
                request_id,
                call_id,
                audio_path,
                sample_rate,
                channels,
            } => {
                assert_eq!(request_id.as_deref(), Some("r6"));
                assert_eq!(call_id, "550e8400-e29b-41d4-a716-446655440011");
                assert_eq!(audio_path, "/tmp/reply.pcm");
                assert_eq!(sample_rate, 24_000);
                assert_eq!(channels, 1);
            }
            other => panic!("expected SendAudioFile, got {other:?}"),
        }
    }

    // ── OutMsg serialization tests ─────────────────────────────────────

    #[test]
    fn serialize_message_received_without_media() {
        let msg = OutMsg::MessageReceived {
            nostr_group_id: "aabb".into(),
            from_pubkey: "cc".into(),
            content: "hello".into(),
            kind: Kind::ChatMessage.as_u16(),
            created_at: 123,
            event_id: "ee".into(),
            message_id: "dd".into(),
            media: vec![],
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "message_received");
        assert_eq!(json["content"], "hello");
        assert_eq!(json["kind"], Kind::ChatMessage.as_u16());
        assert_eq!(json["event_id"], "ee");
        // Empty media vec should be omitted
        assert!(json.get("media").is_none());
    }

    #[test]
    fn serialize_message_received_with_media() {
        let msg = OutMsg::MessageReceived {
            nostr_group_id: "aabb".into(),
            from_pubkey: "cc".into(),
            content: "look at this".into(),
            kind: Kind::ChatMessage.as_u16(),
            created_at: 456,
            event_id: "ee".into(),
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
        assert_eq!(json["kind"], Kind::ChatMessage.as_u16());
        assert_eq!(json["event_id"], "ee");
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
            kind: Kind::ChatMessage.as_u16(),
            created_at: 0,
            event_id: "ee".into(),
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

    #[test]
    fn deserialize_send_media_batch_full() {
        let json = r#"{
            "cmd": "send_media_batch",
            "request_id": "r1",
            "nostr_group_id": "bb",
            "file_paths": ["/tmp/a.jpg", "/tmp/b.png"],
            "caption": "Two photos",
            "blossom_servers": ["https://blossom.example.com"]
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::SendMediaBatch {
                request_id,
                nostr_group_id,
                file_paths,
                caption,
                blossom_servers,
            } => {
                assert_eq!(request_id.as_deref(), Some("r1"));
                assert_eq!(nostr_group_id, "bb");
                assert_eq!(file_paths, vec!["/tmp/a.jpg", "/tmp/b.png"]);
                assert_eq!(caption, "Two photos");
                assert_eq!(blossom_servers, vec!["https://blossom.example.com"]);
            }
            other => panic!("expected SendMediaBatch, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_send_media_batch_defaults() {
        let json = r#"{
            "cmd": "send_media_batch",
            "nostr_group_id": "cc",
            "file_paths": ["/tmp/x.jpg"]
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::SendMediaBatch {
                request_id,
                caption,
                blossom_servers,
                file_paths,
                ..
            } => {
                assert!(request_id.is_none());
                assert_eq!(caption, "");
                assert!(blossom_servers.is_empty());
                assert_eq!(file_paths.len(), 1);
            }
            other => panic!("expected SendMediaBatch, got {other:?}"),
        }
    }
}
