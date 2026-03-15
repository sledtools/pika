mod host_context;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, anyhow};
use base64::Engine;
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
    BootstrappedRuntimeSession, CallSignalPublishKind, CallSignalPublishStatus,
    CompletedMediaUpload, InboundRelayEvent, MarmotRuntime, MediaUploadStatus, PublishedCallSignal,
    RuntimeApplicationMessageInterpretation, RuntimeBaseSessionSyncExecution,
    RuntimeConversationEventInterpretation, RuntimeSessionOpenRequest, RuntimeSessionSyncPlan,
    RuntimeWelcomeInboxSubscriptionIntent, bootstrap_runtime_session, classify_inbound_relay_event,
    subscribe_group_messages_individual,
};
use pika_marmot_runtime::welcome::{
    AcceptedWelcome, accept_welcome_and_catch_up, ingest_unwrapped_welcome, publish_welcome_rumors,
};
use pika_media::codec_opus::{OpusCodec, OpusPacket};
use pika_media::crypto::{FrameInfo, decrypt_frame, encrypt_frame};
use pika_media::network::NetworkRelay;
use pika_media::session::{
    InMemoryRelay, MediaFrame, MediaSession, MediaSessionError, SessionConfig,
};
use pika_media::tracks::{TrackAddress, broadcast_path};

use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::warn;

use crate::acp::{AcpBackendConfig, AcpBackendManager, AcpTurnCompletion};
use crate::call_audio::OpusToAudioPipeline;
use crate::call_tts::synthesize_tts_pcm;
use crate::protocol::{
    AddMembersResultOut, DaemonCmd, GroupMemberOut, GroupProfileOut, GroupUpdateKindOut,
    GroupUpdatedOut, InCmd, LeaveGroupResultOut, ListMembersResultOut, MediaAttachmentOut, OutMsg,
    RemoveMembersResultOut, out_error, out_ok,
};
use host_context::{DaemonHostContext, DaemonPrepareError};

#[cfg(test)]
use pika_marmot_runtime::call::key_id_for_sender;
#[cfg(test)]
use pika_marmot_runtime::welcome::find_pending_welcome_index;
#[cfg(test)]
use pika_media::crypto::{FrameKeyMaterial, opaque_participant_label};

const PROTOCOL_VERSION: u32 = 1;
const ACCEPT_WELCOME_BACKLOG_LIMIT: usize = 200;
const INIT_GROUP_WELCOME_EXPIRATION_SECS: u64 = 30 * 24 * 60 * 60;
const DAEMON_WELCOME_SUBSCRIPTION_LIMIT: usize = 200;
const MAX_GROUP_PROFILE_IMAGE_BYTES: usize = 8 * 1024 * 1024;

type ProtocolEventSinks = Arc<Mutex<Vec<mpsc::UnboundedSender<OutMsg>>>>;

struct GroupUpdatedEmission<'a> {
    host: &'a DaemonHostContext<'a>,
    local_pubkey: &'a PublicKey,
    kind: GroupUpdateKindOut,
    nostr_group_id: &'a str,
    context: &'static str,
}

fn daemon_open_request(
    subscribed_group_ids: Vec<String>,
    relay_urls: Vec<RelayUrl>,
    giftwrap_lookback_sec: u64,
) -> RuntimeSessionOpenRequest {
    RuntimeSessionOpenRequest {
        subscribed_group_ids,
        long_lived_session_relays: relay_urls,
        temporary_key_package_relays: Vec::new(),
        welcome_inbox: daemon_welcome_inbox_intent(giftwrap_lookback_sec),
    }
}

fn bootstrap_runtime_for_daemon(
    state_dir: &Path,
    keys: &Keys,
    relay_urls: Vec<RelayUrl>,
    giftwrap_lookback_sec: u64,
) -> anyhow::Result<BootstrappedRuntimeSession> {
    let signer: Arc<dyn NostrSigner> = Arc::new(keys.clone());
    bootstrap_runtime_session(
        keys.public_key(),
        signer,
        || crate::new_mdk(state_dir, "daemon"),
        daemon_open_request(Vec::new(), relay_urls, giftwrap_lookback_sec),
    )
}

#[cfg(test)]
fn plan_daemon_group_subscriptions(
    host: &DaemonHostContext<'_>,
    subscribed_group_ids: Vec<String>,
) -> anyhow::Result<pika_marmot_runtime::runtime::RuntimeGroupSubscriptionPlan> {
    Ok(host
        .refresh_session_state(subscribed_group_ids, 90)?
        .sync_plan
        .group_subscriptions)
}

fn daemon_welcome_inbox_intent(
    giftwrap_lookback_sec: u64,
) -> RuntimeWelcomeInboxSubscriptionIntent {
    RuntimeWelcomeInboxSubscriptionIntent {
        lookback: Some(Duration::from_secs(giftwrap_lookback_sec)),
        limit: Some(DAEMON_WELCOME_SUBSCRIPTION_LIMIT),
    }
}

fn broadcast_protocol_event(
    out_tx: &mpsc::UnboundedSender<OutMsg>,
    event_sinks: &ProtocolEventSinks,
    event: OutMsg,
) {
    let _ = out_tx.send(event.clone());
    let mut sinks = event_sinks.lock().expect("protocol event sinks lock");
    sinks.retain(|sink| sink.send(event.clone()).is_ok());
}

fn emit_group_updated_snapshot(
    out_tx: &mpsc::UnboundedSender<OutMsg>,
    event_sinks: &ProtocolEventSinks,
    emission: GroupUpdatedEmission<'_>,
) -> bool {
    match build_group_updated_snapshot(
        emission.host,
        emission.local_pubkey,
        emission.kind,
        emission.nostr_group_id,
    ) {
        Ok(update) => {
            broadcast_protocol_event(out_tx, event_sinks, OutMsg::GroupUpdated { update });
            true
        }
        Err(err) => {
            warn!(
                "[pikachat] build group_updated event after {} failed: {err:#}",
                emission.context
            );
            false
        }
    }
}

fn emit_group_updated_if_ok(
    reply: &OutMsg,
    out_tx: &mpsc::UnboundedSender<OutMsg>,
    event_sinks: &ProtocolEventSinks,
    emission: GroupUpdatedEmission<'_>,
) -> bool {
    matches!(reply, OutMsg::Ok { .. }) && emit_group_updated_snapshot(out_tx, event_sinks, emission)
}

fn emit_left_group_updated(
    out_tx: &mpsc::UnboundedSender<OutMsg>,
    event_sinks: &ProtocolEventSinks,
    nostr_group_id: &str,
) {
    broadcast_protocol_event(
        out_tx,
        event_sinks,
        OutMsg::GroupUpdated {
            update: build_left_group_updated(nostr_group_id),
        },
    );
}

fn daemon_base_session_sync_plan(
    sync_plan: &RuntimeSessionSyncPlan,
    primary_relay_url: &RelayUrl,
) -> RuntimeSessionSyncPlan {
    let mut ordered = sync_plan.clone();
    let mut session_connect_relays = vec![primary_relay_url.clone()];
    session_connect_relays.extend(
        sync_plan
            .relay_roles
            .session_connect_relays
            .iter()
            .filter(|relay| *relay != primary_relay_url)
            .cloned(),
    );
    ordered.relay_roles.session_connect_relays = session_connect_relays;
    ordered
}

async fn execute_daemon_base_session_sync(
    session: &pika_marmot_runtime::runtime::RuntimeSession,
    sync_plan: &RuntimeSessionSyncPlan,
    primary_relay_url: &RelayUrl,
) -> anyhow::Result<RuntimeBaseSessionSyncExecution> {
    session
        .execute_base_session_sync(
            &daemon_base_session_sync_plan(sync_plan, primary_relay_url),
            false,
            None,
        )
        .await
        .context("execute daemon base session sync")
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
    create_group_and_publish_welcomes(
        keys,
        mdk,
        vec![peer_kp],
        config,
        &[peer_pubkey],
        vec![Tag::expiration(expires)],
        publish_giftwrap,
    )
    .await
    .map_err(|err| {
        if chain_has_message(&err, "build welcome giftwrap") {
            err.context(INIT_GROUP_BUILD_WELCOME_MARKER)
        } else {
            err.context("init_group")
        }
    })
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

use pika_marmot_runtime::key_package::normalize_peer_key_package_event_for_mdk;
use pika_marmot_runtime::media::{
    MAX_CHAT_MEDIA_BYTES, ParsedMediaAttachment, PreparedMediaUpload, RuntimeMediaAttachment,
    UploadedBlob, resolve_upload_metadata, upload_encrypted_blob,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DaemonMediaWorkflowError {
    code: &'static str,
    message: String,
}

impl DaemonMediaWorkflowError {
    fn file(message: impl Into<String>) -> Self {
        Self {
            code: "file_error",
            message: message.into(),
        }
    }

    fn encrypt(err: anyhow::Error) -> Self {
        Self {
            code: "encrypt_error",
            message: format!("{err:#}"),
        }
    }

    fn upload(message: impl Into<String>) -> Self {
        Self {
            code: "upload_failed",
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DaemonMembershipWorkflowError {
    code: &'static str,
    message: String,
}

impl DaemonMembershipWorkflowError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            code: "bad_request",
            message: message.into(),
        }
    }

    fn bad_pubkey(message: impl Into<String>) -> Self {
        Self {
            code: "bad_pubkey",
            message: message.into(),
        }
    }

    fn bad_group(message: impl Into<String>) -> Self {
        Self {
            code: "bad_group_id",
            message: message.into(),
        }
    }

    fn bad_relays(message: impl Into<String>) -> Self {
        Self {
            code: "bad_relays",
            message: message.into(),
        }
    }

    fn no_key_packages(message: impl Into<String>) -> Self {
        Self {
            code: "no_key_packages",
            message: message.into(),
        }
    }

    fn fetch_failed(message: impl Into<String>) -> Self {
        Self {
            code: "fetch_failed",
            message: message.into(),
        }
    }

    fn mdk(err: anyhow::Error) -> Self {
        Self {
            code: "mdk_error",
            message: format!("{err:#}"),
        }
    }

    fn publish_failed(message: impl Into<String>) -> Self {
        Self {
            code: "publish_failed",
            message: message.into(),
        }
    }

    fn merge_failed(message: impl Into<String>) -> Self {
        Self {
            code: "merge_failed",
            message: message.into(),
        }
    }

    fn runtime(message: impl Into<String>) -> Self {
        Self {
            code: "runtime_error",
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DaemonGroupProfileWorkflowError {
    code: &'static str,
    message: String,
}

impl DaemonGroupProfileWorkflowError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            code: "bad_request",
            message: message.into(),
        }
    }

    fn bad_group(message: impl Into<String>) -> Self {
        Self {
            code: "bad_group_id",
            message: message.into(),
        }
    }

    fn bad_mime_type(message: impl Into<String>) -> Self {
        Self {
            code: "bad_mime_type",
            message: message.into(),
        }
    }

    fn prepare(err: anyhow::Error) -> Self {
        Self {
            code: "mdk_error",
            message: format!("{err:#}"),
        }
    }

    fn upload(message: impl Into<String>) -> Self {
        Self {
            code: "upload_failed",
            message: message.into(),
        }
    }

    fn publish(message: impl Into<String>) -> Self {
        Self {
            code: "publish_failed",
            message: message.into(),
        }
    }
}

fn normalize_requested_member_pubkeys(
    peer_pubkeys: &[String],
) -> Result<Vec<PublicKey>, DaemonMembershipWorkflowError> {
    if peer_pubkeys.is_empty() {
        return Err(DaemonMembershipWorkflowError::bad_request(
            "peer_pubkeys must not be empty",
        ));
    }

    let mut out = Vec::with_capacity(peer_pubkeys.len());
    let mut seen = HashSet::new();
    for raw in peer_pubkeys {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DaemonMembershipWorkflowError::bad_pubkey(
                "peer_pubkey must not be empty",
            ));
        }
        let pubkey = PublicKey::parse(trimmed).map_err(|err| {
            DaemonMembershipWorkflowError::bad_pubkey(format!("invalid peer_pubkey: {err}"))
        })?;
        if seen.insert(pubkey) {
            out.push(pubkey);
        }
    }
    Ok(out)
}

fn map_member_key_package_fetch_error(
    peer_pubkey: &PublicKey,
    err: &anyhow::Error,
) -> DaemonMembershipWorkflowError {
    if err
        .chain()
        .any(|cause| cause.to_string().contains("no keypackage found for"))
    {
        DaemonMembershipWorkflowError::no_key_packages(format!(
            "no key package found for peer {}",
            peer_pubkey.to_hex()
        ))
    } else {
        DaemonMembershipWorkflowError::fetch_failed(format!(
            "fetch key package for {}: {err:#}",
            peer_pubkey.to_hex()
        ))
    }
}

async fn handle_add_members_request_with<Fetch, FetchFut, Publish, PublishFut>(
    request_id: Option<String>,
    host: &DaemonHostContext<'_>,
    keys: &Keys,
    nostr_group_id: &str,
    peer_pubkeys: &[String],
    mut fetch_key_package: Fetch,
    mut publish_event: Publish,
) -> OutMsg
where
    Fetch: FnMut(PublicKey) -> FetchFut,
    FetchFut: Future<Output = anyhow::Result<Event>>,
    Publish: FnMut(Event, &'static str) -> PublishFut,
    PublishFut: Future<Output = anyhow::Result<()>>,
{
    let requested_pubkeys = match normalize_requested_member_pubkeys(peer_pubkeys) {
        Ok(pubkeys) => pubkeys,
        Err(err) => return out_error(request_id, err.code, err.message),
    };

    let mut key_package_events = Vec::with_capacity(requested_pubkeys.len());
    for peer_pubkey in &requested_pubkeys {
        let key_package = match fetch_key_package(*peer_pubkey).await {
            Ok(event) => event,
            Err(err) => {
                let mapped = map_member_key_package_fetch_error(peer_pubkey, &err);
                return out_error(request_id, mapped.code, mapped.message);
            }
        };
        key_package_events.push(normalize_peer_key_package_event_for_mdk(&key_package));
    }

    let prepared = match host.prepare_add_members(nostr_group_id, &key_package_events) {
        Ok(prepared) => prepared,
        Err(DaemonPrepareError::BadGroup(err)) => {
            let mapped = DaemonMembershipWorkflowError::bad_group(format!("{err:#}"));
            return out_error(request_id, mapped.code, mapped.message);
        }
        Err(DaemonPrepareError::Prepare(err)) => {
            let mapped = DaemonMembershipWorkflowError::mdk(err);
            return out_error(request_id, mapped.code, mapped.message);
        }
    };

    let publish_status = match publish_event(prepared.evolution_event.clone(), "add_members").await
    {
        Ok(()) => pika_marmot_runtime::membership::EvolutionPublishStatus::Published,
        Err(err) => pika_marmot_runtime::membership::EvolutionPublishStatus::PublishFailed(
            format!("{err:#}"),
        ),
    };

    let result = match host
        .complete_membership_evolution_operation(prepared, publish_status)
        .into_membership_evolution_result()
    {
        Ok(result) => result,
        Err(err) => {
            let mapped = DaemonMembershipWorkflowError::publish_failed(err);
            return out_error(request_id, mapped.code, mapped.message);
        }
    };

    let pika_marmot_runtime::membership::MembershipUpdateResult {
        mls_group_id: _,
        nostr_group_id_hex,
        added_pubkeys,
        merge_error,
        welcome_delivery,
    } = result;

    if let Some(merge_error) = merge_error {
        let mapped = DaemonMembershipWorkflowError::merge_failed(merge_error);
        return out_error(request_id, mapped.code, mapped.message);
    }

    let welcome_delivery_count = if let Some(plan) = welcome_delivery {
        match publish_welcome_rumors(
            keys,
            &plan.welcome_rumors,
            &plan.recipients,
            Vec::new(),
            |_receiver, giftwrap| publish_event(giftwrap, "add_members_welcome"),
        )
        .await
        {
            Ok(published) => published.len() as u32,
            Err(err) => {
                let mapped = DaemonMembershipWorkflowError::publish_failed(format!("{err:#}"));
                return out_error(request_id, mapped.code, mapped.message);
            }
        }
    } else {
        0
    };

    let member_count = match host.lookup_joined_group_snapshot(&nostr_group_id_hex) {
        Ok(snapshot) => snapshot.member_count(),
        Err(err) => {
            let mapped = DaemonMembershipWorkflowError::runtime(format!(
                "lookup joined group after add_members: {err:#}"
            ));
            return out_error(request_id, mapped.code, mapped.message);
        }
    };

    let result = AddMembersResultOut {
        nostr_group_id: nostr_group_id_hex,
        added_pubkeys: added_pubkeys
            .into_iter()
            .map(|pubkey| pubkey.to_hex())
            .collect(),
        member_count,
        welcome_delivery_count,
    };
    out_ok(
        request_id,
        Some(serde_json::to_value(result).expect("serialize add_members result")),
    )
}

async fn handle_add_members_request(
    request_id: Option<String>,
    host: &DaemonHostContext<'_>,
    keys: &Keys,
    client: &Client,
    relay_urls: &[RelayUrl],
    nostr_group_id: &str,
    peer_pubkeys: &[String],
) -> OutMsg {
    if relay_urls.is_empty() {
        let err = DaemonMembershipWorkflowError::bad_relays("no relays configured");
        return out_error(request_id, err.code, err.message);
    }

    handle_add_members_request_with(
        request_id,
        host,
        keys,
        nostr_group_id,
        peer_pubkeys,
        |peer_pubkey| {
            let client = client.clone();
            let relay_urls = relay_urls.to_vec();
            async move {
                fetch_latest_key_package_for_mdk(
                    &client,
                    &peer_pubkey,
                    &relay_urls,
                    Duration::from_secs(10),
                )
                .await
            }
        },
        |event, label| {
            let client = client.clone();
            let relay_urls = relay_urls.to_vec();
            async move {
                publish_and_confirm_multi(&client, &relay_urls, &event, label)
                    .await
                    .map(|_| ())
            }
        },
    )
    .await
}

fn handle_list_members_request(
    request_id: Option<String>,
    host: &DaemonHostContext<'_>,
    nostr_group_id: &str,
) -> OutMsg {
    let snapshot = match host.lookup_joined_group_snapshot(nostr_group_id) {
        Ok(snapshot) => snapshot,
        Err(err) => return out_error(request_id, "bad_group_id", format!("{err:#}")),
    };

    let members = group_member_outputs(snapshot.member_snapshots);

    let result = ListMembersResultOut {
        nostr_group_id: snapshot.nostr_group_id_hex,
        member_count: members.len() as u32,
        members,
    };
    out_ok(
        request_id,
        Some(serde_json::to_value(result).expect("serialize list_members result")),
    )
}

fn group_member_outputs(
    member_snapshots: Vec<pika_marmot_runtime::conversation::RuntimeJoinedGroupMemberSnapshot>,
) -> Vec<GroupMemberOut> {
    let mut members: Vec<GroupMemberOut> = member_snapshots
        .into_iter()
        .map(|member| GroupMemberOut {
            pubkey: member.pubkey.to_hex(),
            is_admin: member.is_admin,
        })
        .collect();
    members.sort_by(|left, right| left.pubkey.cmp(&right.pubkey));
    members
}

async fn handle_remove_members_request_with<Publish, PublishFut>(
    request_id: Option<String>,
    host: &DaemonHostContext<'_>,
    nostr_group_id: &str,
    peer_pubkeys: &[String],
    mut publish_event: Publish,
) -> OutMsg
where
    Publish: FnMut(Event, &'static str) -> PublishFut,
    PublishFut: Future<Output = anyhow::Result<()>>,
{
    let requested_pubkeys = match normalize_requested_member_pubkeys(peer_pubkeys) {
        Ok(pubkeys) => pubkeys,
        Err(err) => return out_error(request_id, err.code, err.message),
    };
    let removed_pubkeys: Vec<String> = requested_pubkeys
        .iter()
        .map(|pubkey| pubkey.to_hex())
        .collect();

    let prepared = match host.prepare_remove_members(nostr_group_id, &requested_pubkeys) {
        Ok(prepared) => prepared,
        Err(DaemonPrepareError::BadGroup(err)) => {
            let mapped = DaemonMembershipWorkflowError::bad_group(format!("{err:#}"));
            return out_error(request_id, mapped.code, mapped.message);
        }
        Err(DaemonPrepareError::Prepare(err)) => {
            let mapped = DaemonMembershipWorkflowError::mdk(err);
            return out_error(request_id, mapped.code, mapped.message);
        }
    };

    let publish_status =
        match publish_event(prepared.evolution_event.clone(), "remove_members").await {
            Ok(()) => pika_marmot_runtime::membership::EvolutionPublishStatus::Published,
            Err(err) => pika_marmot_runtime::membership::EvolutionPublishStatus::PublishFailed(
                format!("{err:#}"),
            ),
        };

    let result = match host
        .complete_membership_evolution_operation(prepared, publish_status)
        .into_membership_evolution_result()
    {
        Ok(result) => result,
        Err(err) => {
            let mapped = DaemonMembershipWorkflowError::publish_failed(err);
            return out_error(request_id, mapped.code, mapped.message);
        }
    };

    let pika_marmot_runtime::membership::MembershipUpdateResult {
        mls_group_id: _,
        nostr_group_id_hex,
        added_pubkeys: _,
        merge_error,
        welcome_delivery: _,
    } = result;

    if let Some(merge_error) = merge_error {
        let mapped = DaemonMembershipWorkflowError::merge_failed(merge_error);
        return out_error(request_id, mapped.code, mapped.message);
    }

    let member_count = match host.lookup_joined_group_snapshot(&nostr_group_id_hex) {
        Ok(snapshot) => snapshot.member_count(),
        Err(err) => {
            let mapped = DaemonMembershipWorkflowError::runtime(format!(
                "lookup joined group after remove_members: {err:#}"
            ));
            return out_error(request_id, mapped.code, mapped.message);
        }
    };

    // MVP contract: this echoes the requested removals after a successful MLS
    // mutation, rather than diffing before/after membership state.
    let result = RemoveMembersResultOut {
        nostr_group_id: nostr_group_id_hex,
        removed_pubkeys,
        member_count,
    };
    out_ok(
        request_id,
        Some(serde_json::to_value(result).expect("serialize remove_members result")),
    )
}

async fn handle_remove_members_request(
    request_id: Option<String>,
    host: &DaemonHostContext<'_>,
    client: &Client,
    relay_urls: &[RelayUrl],
    nostr_group_id: &str,
    peer_pubkeys: &[String],
) -> OutMsg {
    if relay_urls.is_empty() {
        let err = DaemonMembershipWorkflowError::bad_relays("no relays configured");
        return out_error(request_id, err.code, err.message);
    }

    handle_remove_members_request_with(
        request_id,
        host,
        nostr_group_id,
        peer_pubkeys,
        |event, label| {
            let client = client.clone();
            let relay_urls = relay_urls.to_vec();
            async move {
                publish_and_confirm_multi(&client, &relay_urls, &event, label)
                    .await
                    .map(|_| ())
            }
        },
    )
    .await
}

async fn leave_group_result_with<Publish, PublishFut>(
    host: &DaemonHostContext<'_>,
    nostr_group_id: &str,
    mut publish_event: Publish,
) -> Result<LeaveGroupResultOut, DaemonMembershipWorkflowError>
where
    Publish: FnMut(Event, &'static str) -> PublishFut,
    PublishFut: Future<Output = anyhow::Result<()>>,
{
    let prepared = match host.prepare_leave_group(nostr_group_id) {
        Ok(prepared) => prepared,
        Err(DaemonPrepareError::BadGroup(err)) => {
            return Err(DaemonMembershipWorkflowError::bad_group(format!("{err:#}")));
        }
        Err(DaemonPrepareError::Prepare(err)) => {
            return Err(DaemonMembershipWorkflowError::mdk(err));
        }
    };

    let publish_status = match publish_event(prepared.evolution_event.clone(), "leave_group").await
    {
        Ok(()) => pika_marmot_runtime::membership::EvolutionPublishStatus::Published,
        Err(err) => pika_marmot_runtime::membership::EvolutionPublishStatus::PublishFailed(
            format!("{err:#}"),
        ),
    };

    let result = host
        .complete_membership_evolution_operation(prepared, publish_status)
        .into_membership_evolution_result()
        .map_err(DaemonMembershipWorkflowError::publish_failed)?;

    if let Some(merge_error) = result.merge_error {
        return Err(DaemonMembershipWorkflowError::merge_failed(merge_error));
    }

    Ok(LeaveGroupResultOut {
        nostr_group_id: result.nostr_group_id_hex,
    })
}

async fn handle_leave_group_request(
    request_id: Option<String>,
    host: &DaemonHostContext<'_>,
    client: &Client,
    relay_urls: &[RelayUrl],
    nostr_group_id: &str,
) -> OutMsg {
    if relay_urls.is_empty() {
        let err = DaemonMembershipWorkflowError::bad_relays("no relays configured");
        return out_error(request_id, err.code, err.message);
    }

    match leave_group_result_with(host, nostr_group_id, |event, label| {
        let client = client.clone();
        let relay_urls = relay_urls.to_vec();
        async move {
            publish_and_confirm_multi(&client, &relay_urls, &event, label)
                .await
                .map(|_| ())
        }
    })
    .await
    {
        Ok(result) => out_ok(
            request_id,
            Some(serde_json::to_value(result).expect("serialize leave_group result")),
        ),
        Err(err) => out_error(request_id, err.code, err.message),
    }
}

async fn unsubscribe_group_subscriptions(
    client: &Client,
    group_subs: &mut HashMap<SubscriptionId, String>,
    nostr_group_id: &str,
) {
    let stale_sub_ids: Vec<SubscriptionId> = group_subs
        .iter()
        .filter(|(_, group_id)| group_id.as_str() == nostr_group_id)
        .map(|(sub_id, _)| sub_id.clone())
        .collect();

    for sub_id in stale_sub_ids {
        client.unsubscribe(&sub_id).await;
        group_subs.remove(&sub_id);
    }
}

fn build_group_profile_metadata(
    current_picture: Option<String>,
    name: &str,
    about: &str,
) -> Result<(String, String, String), DaemonGroupProfileWorkflowError> {
    let normalized_name = name.trim().to_string();
    let normalized_about = about.trim().to_string();
    if normalized_name.is_empty() && normalized_about.is_empty() {
        return Err(DaemonGroupProfileWorkflowError::bad_request(
            "name or about must not be empty",
        ));
    }

    let mut metadata = Metadata::new();
    if !normalized_name.is_empty() {
        metadata.name = Some(normalized_name.clone());
        metadata.display_name = Some(normalized_name.clone());
    }
    if !normalized_about.is_empty() {
        metadata.about = Some(normalized_about.clone());
    }
    metadata.picture = current_picture;

    let metadata_json = serde_json::to_string(&metadata)
        .map_err(|err| DaemonGroupProfileWorkflowError::prepare(anyhow!(err)))?;

    Ok((normalized_name, normalized_about, metadata_json))
}

fn trim_group_profile_field(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn build_group_profile_metadata_json(
    name: Option<&str>,
    about: Option<&str>,
    picture: Option<String>,
) -> Result<String, DaemonGroupProfileWorkflowError> {
    let mut metadata = Metadata::new();
    if let Some(name) = trim_group_profile_field(name) {
        metadata.name = Some(name.clone());
        metadata.display_name = Some(name);
    }
    if let Some(about) = trim_group_profile_field(about) {
        metadata.about = Some(about);
    }
    metadata.picture = picture;

    serde_json::to_string(&metadata)
        .map_err(|err| DaemonGroupProfileWorkflowError::prepare(anyhow!(err)))
}

fn get_group_profile_result(
    host: &DaemonHostContext<'_>,
    local_pubkey: &PublicKey,
    nostr_group_id: &str,
) -> Result<GroupProfileOut, DaemonGroupProfileWorkflowError> {
    let group = host
        .lookup_joined_group_snapshot(nostr_group_id)
        .map_err(|err| DaemonGroupProfileWorkflowError::bad_group(format!("{err:#}")))?;
    let owner_candidates: Vec<PublicKey> = group
        .member_snapshots
        .iter()
        .filter(|member| member.is_admin)
        .map(|member| member.pubkey)
        .collect();
    let snapshot = host
        .lookup_group_profile_snapshot_for_owners(nostr_group_id, &owner_candidates)
        .map_err(|err| DaemonGroupProfileWorkflowError::bad_group(format!("{err:#}")))?;

    let name = snapshot
        .as_ref()
        .and_then(|snapshot| {
            trim_group_profile_field(snapshot.metadata.display_name.as_deref())
                .or_else(|| trim_group_profile_field(snapshot.metadata.name.as_deref()))
        })
        .unwrap_or_else(|| group.name.clone());
    let about = snapshot
        .as_ref()
        .and_then(|snapshot| trim_group_profile_field(snapshot.metadata.about.as_deref()))
        .unwrap_or_else(|| group.description.clone());
    let picture_url = snapshot
        .as_ref()
        .and_then(|snapshot| trim_group_profile_field(snapshot.metadata.picture.as_deref()));

    Ok(GroupProfileOut {
        nostr_group_id: group.nostr_group_id_hex,
        owner_pubkey: snapshot
            .as_ref()
            .map(|snapshot| snapshot.owner_pubkey.to_hex())
            .unwrap_or_else(|| local_pubkey.to_hex()),
        name,
        about,
        picture_url,
    })
}

fn build_group_updated_snapshot(
    host: &DaemonHostContext<'_>,
    local_pubkey: &PublicKey,
    kind: GroupUpdateKindOut,
    nostr_group_id: &str,
) -> Result<GroupUpdatedOut, anyhow::Error> {
    let snapshot = host.lookup_joined_group_snapshot(nostr_group_id)?;
    let member_count = snapshot.member_count();
    let members = group_member_outputs(snapshot.member_snapshots);
    let profile = get_group_profile_result(host, local_pubkey, nostr_group_id)
        .map_err(|err| anyhow!("build group profile for group_updated: {}", err.message))?;

    Ok(GroupUpdatedOut {
        kind,
        nostr_group_id: snapshot.nostr_group_id_hex,
        member_count: Some(member_count),
        members,
        profile: Some(profile),
    })
}

fn build_left_group_updated(nostr_group_id: &str) -> GroupUpdatedOut {
    GroupUpdatedOut {
        kind: GroupUpdateKindOut::Left,
        nostr_group_id: nostr_group_id.to_string(),
        member_count: None,
        members: Vec::new(),
        profile: None,
    }
}

fn infer_remote_membership_update_kind(
    local_pubkey: &PublicKey,
    before: Option<&pika_marmot_runtime::conversation::RuntimeJoinedGroupSnapshot>,
    after: Option<&pika_marmot_runtime::conversation::RuntimeJoinedGroupSnapshot>,
) -> Option<GroupUpdateKindOut> {
    let before_contains_local = before.is_some_and(|snapshot| {
        snapshot
            .member_snapshots
            .iter()
            .any(|member| member.pubkey == *local_pubkey)
    });
    let after_contains_local = after.is_some_and(|snapshot| {
        snapshot
            .member_snapshots
            .iter()
            .any(|member| member.pubkey == *local_pubkey)
    });

    if before_contains_local && !after_contains_local {
        return Some(GroupUpdateKindOut::Left);
    }

    let (Some(before), Some(after)) = (before, after) else {
        return None;
    };

    let before_members: HashSet<PublicKey> = before
        .member_snapshots
        .iter()
        .map(|member| member.pubkey)
        .collect();
    let after_members: HashSet<PublicKey> = after
        .member_snapshots
        .iter()
        .map(|member| member.pubkey)
        .collect();
    let added = after_members.difference(&before_members).count();
    let removed = before_members.difference(&after_members).count();

    match (added > 0, removed > 0) {
        (true, false) => Some(GroupUpdateKindOut::MembersAdded),
        (false, true) => Some(GroupUpdateKindOut::MembersRemoved),
        (true, true) => Some(if after.member_count() >= before.member_count() {
            GroupUpdateKindOut::MembersAdded
        } else {
            GroupUpdateKindOut::MembersRemoved
        }),
        (false, false) => None,
    }
}

fn emit_remote_group_commit_updated(
    out_tx: &mpsc::UnboundedSender<OutMsg>,
    event_sinks: &ProtocolEventSinks,
    host: &DaemonHostContext<'_>,
    local_pubkey: &PublicKey,
    before: Option<&pika_marmot_runtime::conversation::RuntimeJoinedGroupSnapshot>,
    nostr_group_id: &str,
) -> bool {
    let after = host.lookup_joined_group_snapshot(nostr_group_id).ok();
    let Some(kind) = infer_remote_membership_update_kind(local_pubkey, before, after.as_ref())
    else {
        return false;
    };

    if kind == GroupUpdateKindOut::Left {
        emit_left_group_updated(out_tx, event_sinks, nostr_group_id);
        return true;
    }

    emit_group_updated_snapshot(
        out_tx,
        event_sinks,
        GroupUpdatedEmission {
            host,
            local_pubkey,
            kind,
            nostr_group_id,
            context: "remote_group_commit",
        },
    )
}

fn emit_remote_group_profile_updated(
    out_tx: &mpsc::UnboundedSender<OutMsg>,
    event_sinks: &ProtocolEventSinks,
    host: &DaemonHostContext<'_>,
    local_pubkey: &PublicKey,
    nostr_group_id: &str,
) -> bool {
    emit_group_updated_snapshot(
        out_tx,
        event_sinks,
        GroupUpdatedEmission {
            host,
            local_pubkey,
            kind: GroupUpdateKindOut::ProfileUpdated,
            nostr_group_id,
            context: "remote_group_profile",
        },
    )
}

fn decode_group_profile_image(
    image_base64: &str,
) -> Result<Vec<u8>, DaemonGroupProfileWorkflowError> {
    let trimmed = image_base64.trim();
    if trimmed.is_empty() {
        return Err(DaemonGroupProfileWorkflowError::bad_request(
            "image_base64 must not be empty",
        ));
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(trimmed)
        .map_err(|err| {
            DaemonGroupProfileWorkflowError::bad_request(format!("invalid image data: {err}"))
        })?;
    if bytes.is_empty() {
        return Err(DaemonGroupProfileWorkflowError::bad_request(
            "image data must not be empty",
        ));
    }
    if bytes.len() > MAX_GROUP_PROFILE_IMAGE_BYTES {
        return Err(DaemonGroupProfileWorkflowError::bad_request(
            "image too large (max 8 MB)",
        ));
    }

    Ok(bytes)
}

fn normalize_group_profile_image_mime_type(
    mime_type: &str,
) -> Result<String, DaemonGroupProfileWorkflowError> {
    let normalized = mime_type.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(DaemonGroupProfileWorkflowError::bad_mime_type(
            "mime_type must not be empty",
        ));
    }
    if !normalized.starts_with("image/") {
        return Err(DaemonGroupProfileWorkflowError::bad_mime_type(
            "mime_type must start with image/",
        ));
    }
    Ok(normalized)
}

fn group_profile_image_filename(mime_type: &str) -> String {
    match mime_type {
        "image/jpeg" | "image/jpg" => "group-profile.jpg".to_string(),
        "image/png" => "group-profile.png".to_string(),
        "image/webp" => "group-profile.webp".to_string(),
        "image/gif" => "group-profile.gif".to_string(),
        _ => {
            let extension = mime_type
                .strip_prefix("image/")
                .filter(|extension| !extension.is_empty())
                .unwrap_or("bin");
            format!("group-profile.{extension}")
        }
    }
}

async fn publish_group_profile_metadata_with<Publish, PublishFut>(
    host: &DaemonHostContext<'_>,
    nostr_group_id: &str,
    metadata_json: String,
    tags: Vec<Tag>,
    mut publish_prepared: Publish,
) -> Result<String, DaemonGroupProfileWorkflowError>
where
    Publish: FnMut(PreparedConversationAction) -> PublishFut,
    PublishFut: Future<Output = anyhow::Result<EventId>>,
{
    let prepared = match host.prepare_outbound_action(
        nostr_group_id,
        OutboundConversationAction::Message {
            kind: Kind::Metadata,
            content: metadata_json,
            tags,
            created_at: Timestamp::now(),
        },
    ) {
        Ok(prepared) => prepared,
        Err(DaemonPrepareError::BadGroup(err)) => {
            return Err(DaemonGroupProfileWorkflowError::bad_group(format!(
                "{err:#}"
            )));
        }
        Err(DaemonPrepareError::Prepare(err)) => {
            return Err(DaemonGroupProfileWorkflowError::prepare(err));
        }
    };

    let publish_status = match publish_prepared(prepared.clone()).await {
        Ok(wrapper_event_id) => {
            pika_marmot_runtime::outbound::OutboundConversationPublishStatus::Published {
                wrapper_event_id,
            }
        }
        Err(err) => {
            pika_marmot_runtime::outbound::OutboundConversationPublishStatus::PublishFailed(
                format!("{err:#}"),
            )
        }
    };

    host.complete_outbound_publish_operation(prepared, publish_status)
        .into_outbound_conversation_publish_result()
        .map(|result| result.target.nostr_group_id_hex)
        .map_err(DaemonGroupProfileWorkflowError::publish)
}

async fn handle_update_group_profile_request_with<Publish, PublishFut>(
    request_id: Option<String>,
    host: &DaemonHostContext<'_>,
    local_pubkey: &PublicKey,
    nostr_group_id: &str,
    name: &str,
    about: &str,
    publish_prepared: Publish,
) -> OutMsg
where
    Publish: FnMut(PreparedConversationAction) -> PublishFut,
    PublishFut: Future<Output = anyhow::Result<EventId>>,
{
    let current_picture = match get_group_profile_result(host, local_pubkey, nostr_group_id) {
        Ok(profile) => profile.picture_url,
        Err(err) => return out_error(request_id, err.code, err.message),
    };

    let (normalized_name, normalized_about, metadata_json) =
        match build_group_profile_metadata(current_picture.clone(), name, about) {
            Ok(built) => built,
            Err(err) => return out_error(request_id, err.code, err.message),
        };

    let nostr_group_id = match publish_group_profile_metadata_with(
        host,
        nostr_group_id,
        metadata_json,
        vec![],
        publish_prepared,
    )
    .await
    {
        Ok(nostr_group_id) => nostr_group_id,
        Err(err) => {
            return out_error(request_id, err.code, err.message);
        }
    };

    let result = GroupProfileOut {
        nostr_group_id,
        owner_pubkey: local_pubkey.to_hex(),
        name: normalized_name,
        about: normalized_about,
        picture_url: current_picture,
    };
    out_ok(
        request_id,
        Some(serde_json::to_value(result).expect("serialize update_group_profile result")),
    )
}

async fn handle_update_group_profile_request(
    request_id: Option<String>,
    host: &DaemonHostContext<'_>,
    local_pubkey: &PublicKey,
    nostr_group_id: &str,
    name: &str,
    about: &str,
) -> OutMsg {
    handle_update_group_profile_request_with(
        request_id,
        host,
        local_pubkey,
        nostr_group_id,
        name,
        about,
        |prepared| {
            let host_ctx = host;
            async move {
                host_ctx
                    .publish_prepared(&prepared, "update_group_profile")
                    .await
                    .map(|wrapper| wrapper.id)
            }
        },
    )
    .await
}

fn handle_get_group_profile_request(
    request_id: Option<String>,
    host: &DaemonHostContext<'_>,
    local_pubkey: &PublicKey,
    nostr_group_id: &str,
) -> OutMsg {
    match get_group_profile_result(host, local_pubkey, nostr_group_id) {
        Ok(result) => out_ok(
            request_id,
            Some(serde_json::to_value(result).expect("serialize get_group_profile result")),
        ),
        Err(err) => out_error(request_id, err.code, err.message),
    }
}

async fn handle_upload_group_profile_image_request_with<Upload, UploadFut, Publish, PublishFut>(
    request_id: Option<String>,
    host: &DaemonHostContext<'_>,
    local_pubkey: &PublicKey,
    input: GroupProfileImageUploadInput<'_>,
    mut upload_blob: Upload,
    publish_prepared: Publish,
) -> OutMsg
where
    Upload: FnMut(Vec<u8>, String, String) -> UploadFut,
    UploadFut: Future<Output = anyhow::Result<UploadedBlob>>,
    Publish: FnMut(PreparedConversationAction) -> PublishFut,
    PublishFut: Future<Output = anyhow::Result<EventId>>,
{
    let current_profile = match get_group_profile_result(host, local_pubkey, input.nostr_group_id) {
        Ok(result) => result,
        Err(err) => return out_error(request_id, err.code, err.message),
    };
    let image_bytes = match decode_group_profile_image(input.image_base64) {
        Ok(bytes) => bytes,
        Err(err) => return out_error(request_id, err.code, err.message),
    };
    let normalized_mime_type = match normalize_group_profile_image_mime_type(input.mime_type) {
        Ok(mime_type) => mime_type,
        Err(err) => return out_error(request_id, err.code, err.message),
    };
    let mls_group_id = match host.resolve_group(input.nostr_group_id) {
        Ok(group_id) => group_id,
        Err(err) => {
            let mapped = DaemonGroupProfileWorkflowError::bad_group(format!("{err:#}"));
            return out_error(request_id, mapped.code, mapped.message);
        }
    };
    let filename = group_profile_image_filename(&normalized_mime_type);

    let PreparedMediaUpload {
        upload,
        encrypted_data,
    } = match host.prepare_upload(
        &mls_group_id,
        &image_bytes,
        Some(&normalized_mime_type),
        Some(&filename),
    ) {
        Ok(prepared) => prepared,
        Err(err) => {
            let mapped = DaemonGroupProfileWorkflowError::prepare(err);
            return out_error(request_id, mapped.code, mapped.message);
        }
    };
    let expected_hash_hex = hex::encode(upload.encrypted_hash);
    let media_upload_status =
        match upload_blob(encrypted_data, upload.mime_type.clone(), expected_hash_hex).await {
            Ok(uploaded) => MediaUploadStatus::Uploaded(uploaded),
            Err(err) => MediaUploadStatus::UploadFailed(format!("{err:#}")),
        };
    let completed = match host
        .complete_media_upload_operation(
            &mls_group_id,
            input.nostr_group_id.to_string(),
            &upload,
            media_upload_status,
        )
        .into_media_upload_result()
    {
        Ok(completed) => completed,
        Err(err) => {
            let mapped = DaemonGroupProfileWorkflowError::upload(err);
            return out_error(request_id, mapped.code, mapped.message);
        }
    };
    if completed.result.uploaded_blob.uploaded_url.is_empty() {
        let mapped =
            DaemonGroupProfileWorkflowError::upload("uploaded profile image URL was empty");
        return out_error(request_id, mapped.code, mapped.message);
    }

    let metadata_json = match build_group_profile_metadata_json(
        Some(&current_profile.name),
        Some(&current_profile.about),
        Some(completed.result.uploaded_blob.uploaded_url.clone()),
    ) {
        Ok(metadata_json) => metadata_json,
        Err(err) => return out_error(request_id, err.code, err.message),
    };
    let picture_url = completed.result.uploaded_blob.uploaded_url.clone();
    let nostr_group_id = match publish_group_profile_metadata_with(
        host,
        input.nostr_group_id,
        metadata_json,
        vec![completed.result.imeta_tag.clone()],
        publish_prepared,
    )
    .await
    {
        Ok(nostr_group_id) => nostr_group_id,
        Err(err) => return out_error(request_id, err.code, err.message),
    };

    let result = GroupProfileOut {
        nostr_group_id,
        owner_pubkey: current_profile.owner_pubkey,
        name: current_profile.name,
        about: current_profile.about,
        picture_url: Some(picture_url),
    };
    out_ok(
        request_id,
        Some(serde_json::to_value(result).expect("serialize upload_group_profile_image result")),
    )
}

async fn handle_upload_group_profile_image_request(
    request_id: Option<String>,
    host: &DaemonHostContext<'_>,
    keys: &Keys,
    local_pubkey: &PublicKey,
    nostr_group_id: &str,
    image_base64: &str,
    mime_type: &str,
) -> OutMsg {
    let blossom_servers = blossom_servers_or_default(&[]);
    handle_upload_group_profile_image_request_with(
        request_id,
        host,
        local_pubkey,
        GroupProfileImageUploadInput {
            nostr_group_id,
            image_base64,
            mime_type,
        },
        |encrypted_data, upload_mime_type, expected_hash_hex| {
            let keys = keys.clone();
            let blossom_servers = blossom_servers.clone();
            async move {
                upload_encrypted_blob(
                    &keys,
                    encrypted_data,
                    &upload_mime_type,
                    &expected_hash_hex,
                    &blossom_servers,
                )
                .await
            }
        },
        |prepared| {
            let host_ctx = host;
            async move {
                host_ctx
                    .publish_prepared(&prepared, "upload_group_profile_image")
                    .await
                    .map(|wrapper| wrapper.id)
            }
        },
    )
    .await
}

struct GroupProfileImageUploadInput<'a> {
    nostr_group_id: &'a str,
    image_base64: &'a str,
    mime_type: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletedMediaBatchFields {
    imeta_tags: Vec<Tag>,
    original_hashes: Vec<String>,
    uploaded_urls: Vec<String>,
}

struct DaemonMediaUploadInput<'a> {
    nostr_group_id: &'a str,
    file_path: &'a str,
    mime_type: Option<&'a str>,
    filename: Option<&'a str>,
    include_path_in_validation_errors: bool,
    require_uploaded_url: bool,
}

fn batch_media_fields_from_completed_uploads(
    completed_uploads: &[CompletedMediaUpload],
) -> CompletedMediaBatchFields {
    CompletedMediaBatchFields {
        imeta_tags: completed_uploads
            .iter()
            .map(|completed| completed.result.imeta_tag.clone())
            .collect(),
        original_hashes: completed_uploads
            .iter()
            .map(|completed| completed.result.attachment.original_hash_hex.clone())
            .collect(),
        uploaded_urls: completed_uploads
            .iter()
            .map(|completed| completed.result.uploaded_blob.uploaded_url.clone())
            .collect(),
    }
}

fn read_daemon_media_file(
    file_path: &str,
    include_path_in_validation_errors: bool,
) -> Result<Vec<u8>, DaemonMediaWorkflowError> {
    let bytes = std::fs::read(file_path)
        .map_err(|err| DaemonMediaWorkflowError::file(format!("read {file_path}: {err}")))?;
    if bytes.is_empty() {
        let message = if include_path_in_validation_errors {
            format!("file is empty: {file_path}")
        } else {
            "file is empty".to_string()
        };
        return Err(DaemonMediaWorkflowError::file(message));
    }
    if bytes.len() > MAX_CHAT_MEDIA_BYTES {
        let message = if include_path_in_validation_errors {
            format!("file too large (max 32 MB): {file_path}")
        } else {
            "file too large (max 32 MB)".to_string()
        };
        return Err(DaemonMediaWorkflowError::file(message));
    }
    Ok(bytes)
}

fn daemon_media_upload_error_message(
    err: &anyhow::Error,
    file_path: &str,
    include_path: bool,
) -> String {
    if include_path {
        format!("upload {file_path}: {err:#}")
    } else {
        format!("{err:#}")
    }
}

fn daemon_media_missing_upload_url(file_path: &str, include_path: bool) -> String {
    if include_path {
        format!("upload {file_path}: missing upload URL")
    } else {
        "missing upload URL".to_string()
    }
}

async fn upload_daemon_media_file(
    host: &DaemonHostContext<'_>,
    keys: &Keys,
    mls_group_id: &GroupId,
    blossom_servers: &[String],
    input: DaemonMediaUploadInput<'_>,
) -> Result<CompletedMediaUpload, DaemonMediaWorkflowError> {
    let bytes = read_daemon_media_file(input.file_path, input.include_path_in_validation_errors)?;
    let path = Path::new(input.file_path);
    let resolved = resolve_upload_metadata(path, input.mime_type, input.filename);
    let pika_marmot_runtime::media::PreparedMediaUpload {
        upload,
        encrypted_data,
    } = host
        .prepare_upload(
            mls_group_id,
            &bytes,
            Some(&resolved.mime_type),
            Some(&resolved.filename),
        )
        .map_err(DaemonMediaWorkflowError::encrypt)?;
    let expected_hash_hex = hex::encode(upload.encrypted_hash);
    let status = match upload_encrypted_blob(
        keys,
        encrypted_data,
        &upload.mime_type,
        &expected_hash_hex,
        blossom_servers,
    )
    .await
    {
        Ok(uploaded) => MediaUploadStatus::Uploaded(uploaded),
        Err(err) => MediaUploadStatus::UploadFailed(daemon_media_upload_error_message(
            &err,
            input.file_path,
            input.include_path_in_validation_errors,
        )),
    };
    let completed = host
        .complete_media_upload_operation(
            mls_group_id,
            input.nostr_group_id.to_string(),
            &upload,
            status,
        )
        .into_media_upload_result()
        .map_err(DaemonMediaWorkflowError::upload)?;
    if input.require_uploaded_url && completed.result.uploaded_blob.uploaded_url.is_empty() {
        return Err(DaemonMediaWorkflowError::upload(
            daemon_media_missing_upload_url(
                input.file_path,
                input.include_path_in_validation_errors,
            ),
        ));
    }
    Ok(completed)
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
    signed: &Event,
    call_id: &str,
    max_attempts: usize,
) -> anyhow::Result<()> {
    let attempts = max_attempts.max(1);
    for attempt in 1..=attempts {
        match host
            .publish_signed_call_payload(signed, "call_invite")
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

fn call_signal_publish_status(
    wrapper_event_id: EventId,
    publish_result: anyhow::Result<()>,
) -> CallSignalPublishStatus {
    match publish_result {
        Ok(()) => CallSignalPublishStatus::Published { wrapper_event_id },
        Err(err) => CallSignalPublishStatus::PublishFailed {
            wrapper_event_id,
            error: format!("{err:#}"),
        },
    }
}

fn complete_daemon_call_signal_publish_result(
    host: &DaemonHostContext<'_>,
    kind: CallSignalPublishKind,
    nostr_group_id_hex: String,
    prepared: pika_marmot_runtime::call_runtime::PreparedCallSignal,
    publish_status: CallSignalPublishStatus,
) -> Result<PublishedCallSignal, String> {
    host.complete_call_signal_publish_operation(kind, nostr_group_id_hex, prepared, publish_status)
        .into_call_signal_publish_result()
}

async fn publish_signed_call_signal_result(
    host: &DaemonHostContext<'_>,
    kind: CallSignalPublishKind,
    nostr_group_id_hex: String,
    prepared: pika_marmot_runtime::call_runtime::PreparedCallSignal,
    signed: &Event,
    label: &str,
) -> Result<PublishedCallSignal, String> {
    complete_daemon_call_signal_publish_result(
        host,
        kind,
        nostr_group_id_hex,
        prepared,
        call_signal_publish_status(
            signed.id,
            host.publish_signed_call_payload(signed, label).await,
        ),
    )
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

fn should_prompt_acp_reply(
    classification: MessageClassification,
    sender_hex: &str,
    local_pubkey_hex: &str,
    content: &str,
) -> bool {
    classification == MessageClassification::Chat
        && sender_hex != local_pubkey_hex
        && !content.trim().is_empty()
}

fn build_acp_prompt(nostr_group_id: &str, sender_hex: &str, content: &str) -> String {
    format!(
        "conversation_id: {nostr_group_id}\nsender_pubkey: {sender_hex}\nmessage:\n{}",
        content.trim()
    )
}

pub async fn daemon_main(
    relays_arg: &[String],
    state_dir: &Path,
    giftwrap_lookback_sec: u64,
    allow_pubkeys: &[String],
    auto_accept_welcomes: bool,
    exec_cmd: Option<&str>,
    acp_backend: Option<AcpBackendConfig>,
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

    let mut relay_urls: Vec<RelayUrl> = Vec::new();
    for r in relays_arg {
        relay_urls
            .push(RelayUrl::parse(r.trim()).with_context(|| format!("parse relay url: {r}"))?);
    }
    if relay_urls.is_empty() {
        relay_urls
            .push(RelayUrl::parse("ws://127.0.0.1:18080").context("parse default relay url")?);
    }
    let primary_relay_url = relay_urls
        .first()
        .cloned()
        .context("missing primary relay after relay setup")?;
    let (acp_backend, mut acp_completion_rx) = match acp_backend {
        Some(config) => {
            let (manager, completion_rx) = AcpBackendManager::spawn(config)
                .await
                .context("start ACP backend manager")?;
            (Some(manager), Some(completion_rx))
        }
        None => (None, None),
    };
    let bootstrapped =
        bootstrap_runtime_for_daemon(state_dir, &keys, relay_urls.clone(), giftwrap_lookback_sec)?;
    let BootstrappedRuntimeSession {
        session: runtime_session,
        open: startup_open,
    } = bootstrapped;
    let client = runtime_session.client.clone();
    let mut rx = client.notifications();
    let gift_sub = execute_daemon_base_session_sync(
        &runtime_session,
        &startup_open.sync_plan,
        &primary_relay_url,
    )
    .await?
    .welcome_inbox_sub;
    let mdk = runtime_session.mdk;

    // Track inbound relay events we've already processed. Seed from bootstrapped
    // startup state so reconnects do not immediately replay known wrappers.
    let mut seen_inbound = startup_open.unbounded_inbound_relay_seen_cache();
    let mut seen_group_events = startup_open.seed_seen_group_events();

    // Track group subscriptions.
    let mut group_subs: HashMap<SubscriptionId, String> = subscribe_group_messages_individual(
        &client,
        &startup_open.current_group_subscriptions().target_group_ids,
    )
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

    let protocol_event_sinks: ProtocolEventSinks = Arc::new(Mutex::new(Vec::new()));

    // Unix domain socket for --remote CLI connections
    let sock_path = crate::resolve_daemon_socket_path(state_dir);
    // Clean up stale socket
    let _ = std::fs::remove_file(&sock_path);
    let unix_listener = tokio::net::UnixListener::bind(&sock_path)
        .with_context(|| format!("bind unix socket {}", sock_path.display()))?;
    eprintln!("[pikachat] listening on {}", sock_path.display());

    // Spawn socket acceptor
    let cmd_tx_for_sock = cmd_tx_for_auto.clone();
    let protocol_event_sinks_for_sock = Arc::clone(&protocol_event_sinks);
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
            let protocol_event_sinks = Arc::clone(&protocol_event_sinks_for_sock);
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut lines = tokio::io::BufReader::new(reader).lines();
                let (resp_tx, mut resp_rx) = mpsc::unbounded_channel::<OutMsg>();
                protocol_event_sinks
                    .lock()
                    .expect("protocol event sinks lock")
                    .push(resp_tx.clone());

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

    out_tx
        .send(OutMsg::Ready {
            protocol_version: PROTOCOL_VERSION,
            pubkey: pubkey_hex.clone(),
            npub,
        })
        .ok();

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
                        let host =
                            DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        match host.list_pending_welcome_snapshots() {
                            Ok(list) => {
                                let out = list
                                    .iter()
                                    .map(|w| {
                                    json!({
                                        "wrapper_event_id": w.wrapper_event_id.to_hex(),
                                        "welcome_event_id": w.welcome_event_id.to_hex(),
                                        "from_pubkey": w.welcomer.to_hex().to_lowercase(),
                                        "nostr_group_id": w.nostr_group_id_hex.clone(),
                                        "group_name": w.group_name.clone(),
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
                        let host =
                            DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        match host.lookup_pending_welcome(&wrapper) {
                            Ok(Some(w)) => {
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
                            Ok(None) => {
                                reply_tx
                                    .send(out_error(
                                        request_id,
                                        "not_found",
                                        accept_welcome_not_found_message(),
                                    ))
                                    .ok();
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
                    InCmd::AddMembers {
                        request_id,
                        nostr_group_id,
                        peer_pubkeys,
                    } => {
                        let host =
                            DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let reply = handle_add_members_request(
                            request_id,
                            &host,
                            &keys,
                            &client,
                            &relay_urls,
                            &nostr_group_id,
                            &peer_pubkeys,
                        )
                        .await;
                        let _ = emit_group_updated_if_ok(
                            &reply,
                            &out_tx,
                            &protocol_event_sinks,
                            GroupUpdatedEmission {
                                host: &host,
                                local_pubkey: &keys.public_key(),
                                kind: GroupUpdateKindOut::MembersAdded,
                                nostr_group_id: &nostr_group_id,
                                context: "add_members",
                            },
                        );
                        let _ = reply_tx.send(reply);
                    }
                    InCmd::ListMembers {
                        request_id,
                        nostr_group_id,
                    } => {
                        let host =
                            DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let _ = reply_tx
                            .send(handle_list_members_request(request_id, &host, &nostr_group_id));
                    }
                    InCmd::RemoveMembers {
                        request_id,
                        nostr_group_id,
                        peer_pubkeys,
                    } => {
                        let host =
                            DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let reply = handle_remove_members_request(
                            request_id,
                            &host,
                            &client,
                            &relay_urls,
                            &nostr_group_id,
                            &peer_pubkeys,
                        )
                        .await;
                        let _ = emit_group_updated_if_ok(
                            &reply,
                            &out_tx,
                            &protocol_event_sinks,
                            GroupUpdatedEmission {
                                host: &host,
                                local_pubkey: &keys.public_key(),
                                kind: GroupUpdateKindOut::MembersRemoved,
                                nostr_group_id: &nostr_group_id,
                                context: "remove_members",
                            },
                        );
                        let _ = reply_tx.send(reply);
                    }
                    InCmd::LeaveGroup {
                        request_id,
                        nostr_group_id,
                    } => {
                        let host =
                            DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let reply = handle_leave_group_request(
                            request_id,
                            &host,
                            &client,
                            &relay_urls,
                            &nostr_group_id,
                        )
                        .await;
                        let left_group = matches!(reply, OutMsg::Ok { .. });
                        if left_group {
                            unsubscribe_group_subscriptions(&client, &mut group_subs, &nostr_group_id)
                                .await;
                        }
                        if left_group {
                            emit_left_group_updated(&out_tx, &protocol_event_sinks, &nostr_group_id);
                        }
                        let _ = reply_tx.send(reply);
                    }
                    InCmd::UpdateGroupProfile {
                        request_id,
                        nostr_group_id,
                        name,
                        about,
                    } => {
                        let host =
                            DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let reply = handle_update_group_profile_request(
                            request_id,
                            &host,
                            &keys.public_key(),
                            &nostr_group_id,
                            &name,
                            &about,
                        )
                        .await;
                        let _ = emit_group_updated_if_ok(
                            &reply,
                            &out_tx,
                            &protocol_event_sinks,
                            GroupUpdatedEmission {
                                host: &host,
                                local_pubkey: &keys.public_key(),
                                kind: GroupUpdateKindOut::ProfileUpdated,
                                nostr_group_id: &nostr_group_id,
                                context: "update_group_profile",
                            },
                        );
                        let _ = reply_tx.send(reply);
                    }
                    InCmd::GetGroupProfile {
                        request_id,
                        nostr_group_id,
                    } => {
                        let host =
                            DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let reply = handle_get_group_profile_request(
                            request_id,
                            &host,
                            &keys.public_key(),
                            &nostr_group_id,
                        );
                        let _ = reply_tx.send(reply);
                    }
                    InCmd::UploadGroupProfileImage {
                        request_id,
                        nostr_group_id,
                        image_base64,
                        mime_type,
                    } => {
                        let host =
                            DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let reply = handle_upload_group_profile_image_request(
                            request_id,
                            &host,
                            &keys,
                            &keys.public_key(),
                            &nostr_group_id,
                            &image_base64,
                            &mime_type,
                        )
                        .await;
                        let _ = emit_group_updated_if_ok(
                            &reply,
                            &out_tx,
                            &protocol_event_sinks,
                            GroupUpdatedEmission {
                                host: &host,
                                local_pubkey: &keys.public_key(),
                                kind: GroupUpdateKindOut::ProfileUpdated,
                                nostr_group_id: &nostr_group_id,
                                context: "upload_group_profile_image",
                            },
                        );
                        let _ = reply_tx.send(reply);
                    }
                    InCmd::GetMessages { request_id, nostr_group_id, limit } => {
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let query =
                            pika_marmot_runtime::conversation::RuntimeMessagePageQuery::new(limit, 0);
                        match host.load_message_page(&nostr_group_id, query) {
                            Ok(page) => {
                                let out: Vec<serde_json::Value> = page.messages.iter().map(|m| {
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
                        let rumor_id = prepared.rumor_id;
                        let (published, publish_status) =
                            match host.publish_prepared(&prepared, "daemon_send").await {
                            Ok(wrapper) => (
                                true,
                                pika_marmot_runtime::outbound::OutboundConversationPublishStatus::Published {
                                    wrapper_event_id: wrapper.id,
                                },
                            ),
                            Err(e) => (
                                false,
                                pika_marmot_runtime::outbound::OutboundConversationPublishStatus::PublishFailed(
                                    format!("{e:#}"),
                                ),
                            ),
                        };
                        let operation =
                            host.complete_outbound_publish_operation(prepared, publish_status);
                        match operation.into_outbound_conversation_publish_result() {
                            Ok(result) if published => {
                                let _ = reply_tx.send(out_ok(
                                    request_id,
                                    Some(json!({"event_id": result.rumor_id.to_hex()})),
                                ));
                            }
                            Ok(_) => {
                                warn!(
                                    "[pikachat] unexpected completed outbound publish result for daemon_send: rumor_id={rumor_id}"
                                );
                                let _ = reply_tx.send(out_error(
                                    request_id,
                                    "publish_failed",
                                    "unexpected outbound publish result",
                                ));
                            }
                            Err(error) if published => {
                                warn!(
                                    "[pikachat] unexpected outbound publish result for daemon_send: {error}"
                                );
                                let _ = reply_tx.send(out_error(
                                    request_id,
                                    "publish_failed",
                                    "unexpected outbound publish result".to_string(),
                                ));
                            }
                            Err(error) => {
                                let _ = reply_tx.send(out_error(
                                    request_id,
                                    "publish_failed",
                                    error,
                                ));
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
                        let upload_servers = blossom_servers_or_default(&blossom_servers);

                        let completed = match upload_daemon_media_file(
                            &host,
                            &keys,
                            &mls_group_id,
                            &upload_servers,
                            DaemonMediaUploadInput {
                                nostr_group_id: &nostr_group_id,
                                file_path: &file_path,
                                mime_type: mime_type.as_deref(),
                                filename: filename.as_deref(),
                                include_path_in_validation_errors: false,
                                require_uploaded_url: false,
                            },
                        )
                        .await
                        {
                            Ok(completed) => completed,
                            Err(err) => {
                                reply_tx
                                    .send(out_error(request_id, err.code, err.message))
                                    .ok();
                                continue;
                            }
                        };

                        // Build imeta tag and message
                        let rumor = EventBuilder::new(Kind::ChatMessage, &caption)
                            .tag(completed.result.imeta_tag.clone())
                            .build(keys.public_key());
                        match host.sign_and_publish_rumor(&mls_group_id, rumor, "daemon_send_media").await {
                            Ok(ev) => {
                                let _ = reply_tx.send(out_ok(request_id, Some(json!({
                                    "event_id": ev.id.to_hex(),
                                    "uploaded_url": completed.result.uploaded_blob.uploaded_url,
                                    "original_hash_hex": completed.result.attachment.original_hash_hex,
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
                        let mut completed_uploads = Vec::with_capacity(file_paths.len());
                        for file_path in &file_paths {
                            let completed = match upload_daemon_media_file(
                                &host,
                                &keys,
                                &mls_group_id,
                                &upload_servers,
                                DaemonMediaUploadInput {
                                    nostr_group_id: &nostr_group_id,
                                    file_path,
                                    mime_type: None,
                                    filename: None,
                                    include_path_in_validation_errors: true,
                                    require_uploaded_url: true,
                                },
                            )
                            .await
                            {
                                Ok(completed) => completed,
                                Err(err) => {
                                    reply_tx
                                        .send(out_error(request_id.clone(), err.code, err.message))
                                        .ok();
                                    completed_uploads.clear();
                                    break;
                                }
                            };
                            completed_uploads.push(completed);
                        }
                        if completed_uploads.len() != file_paths.len() {
                            continue;
                        }
                        let batch_fields =
                            batch_media_fields_from_completed_uploads(&completed_uploads);

                        let mut builder = EventBuilder::new(Kind::ChatMessage, &caption);
                        for tag in &batch_fields.imeta_tags {
                            builder = builder.tag(tag.clone());
                        }
                        let rumor = builder.build(keys.public_key());
                        match host.sign_and_publish_rumor(&mls_group_id, rumor, "daemon_send_media_batch").await {
                            Ok(ev) => {
                                let _ = reply_tx.send(out_ok(request_id, Some(json!({
                                    "event_id": ev.id.to_hex(),
                                    "uploaded_urls": batch_fields.uploaded_urls,
                                    "original_hashes": batch_fields.original_hashes,
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
                        let signed_invite =
                            match host.sign_call_payload(&nostr_group_id, prepared_invite.payload_json.clone())
                            {
                                Ok(signed) => signed,
                                Err(e) => {
                                    let _ = reply_tx.send(out_error(
                                        request_id,
                                        "runtime_error",
                                        format!("sign call invite failed: {e:#}"),
                                    ));
                                    continue;
                                }
                            };

                        match complete_daemon_call_signal_publish_result(
                            &host,
                            CallSignalPublishKind::Invite,
                            nostr_group_id.clone(),
                            prepared_invite,
                            call_signal_publish_status(
                                signed_invite.id,
                                send_call_invite_with_retry(&host, &signed_invite, &call_id, 3)
                                    .await,
                            ),
                        ) {
                            Ok(result) => {
                                pending_outgoing_call_invites.insert(call_id.clone(), pending);
                                let _ = reply_tx.send(out_ok(
                                    request_id,
                                    Some(json!({
                                        "call_id": result.call_id,
                                        "nostr_group_id": result.nostr_group_id_hex,
                                        "session": session,
                                    })),
                                ));
                            }
                            Err(error) => {
                                let _ = reply_tx.send(out_error(
                                    request_id,
                                    "publish_failed",
                                    error,
                                ));
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
                                    && let Ok(signed) = host
                                        .sign_call_payload(&invite.target_id, signal.payload_json.clone())
                                {
                                    let _ = publish_signed_call_signal_result(
                                        &host,
                                        CallSignalPublishKind::Reject,
                                        invite.target_id.clone(),
                                        signal,
                                        &signed,
                                        "call_reject_auth_failed",
                                    )
                                    .await;
                                }
                                let _ = reply_tx.send(out_error(request_id, "auth_failed", err));
                                continue;
                            }
                        };
                        let signed_accept =
                            match host.sign_call_payload(&invite.target_id, prepared.signal.payload_json.clone()) {
                                Ok(signed) => signed,
                                Err(e) => {
                                    let _ = reply_tx.send(out_error(
                                        request_id,
                                        "runtime_error",
                                        format!("sign call accept failed: {e:#}"),
                                    ));
                                    continue;
                                }
                            };
                        let pika_marmot_runtime::call_runtime::PreparedAcceptedCall {
                            incoming,
                            signal,
                            media_crypto,
                        } = prepared;
                        let published = match publish_signed_call_signal_result(
                            &host,
                            CallSignalPublishKind::Accept,
                            invite.target_id.clone(),
                            signal,
                            &signed_accept,
                            "call_accept",
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(error) => {
                                let _ = reply_tx.send(out_error(request_id, "publish_failed", error));
                                continue;
                            }
                        };

                        let mode = active_call_mode(&incoming.session);
                        let worker = match mode {
                            ActiveCallMode::Audio => {
                                if echo_mode_enabled() {
                                    match start_echo_worker(
                                        &incoming.call_id,
                                        &incoming.session,
                                        media_crypto.clone(),
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
                                        &incoming.call_id,
                                        &incoming.session,
                                        media_crypto.clone(),
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
                                &incoming.call_id,
                                &incoming.session,
                                media_crypto.clone(),
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
                            call_id: published.call_id.clone(),
                            nostr_group_id: published.nostr_group_id_hex.clone(),
                            session: incoming.session.clone(),
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
                        let _ = reply_tx.send(out_ok(request_id, Some(json!({
                            "call_id": published.call_id,
                            "nostr_group_id": published.nostr_group_id_hex,
                        }))));
                        let _ = out_tx.send(OutMsg::CallSessionStarted {
                            call_id: published.call_id,
                            nostr_group_id: published.nostr_group_id_hex,
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
                        let signed_reject =
                            match host.sign_call_payload(&invite.target_id, signal.payload_json.clone()) {
                                Ok(signed) => signed,
                                Err(e) => {
                                    let _ = reply_tx.send(out_error(
                                        request_id,
                                        "runtime_error",
                                        format!("sign call reject failed: {e:#}"),
                                    ));
                                    continue;
                                }
                            };
                        match publish_signed_call_signal_result(
                            &host,
                            CallSignalPublishKind::Reject,
                            invite.target_id.clone(),
                            signal,
                            &signed_reject,
                            "call_reject",
                        )
                        .await
                        {
                            Ok(result) => {
                                let _ = reply_tx.send(out_ok(
                                    request_id,
                                    Some(json!({ "call_id": result.call_id })),
                                ));
                            }
                            Err(error) => {
                                let _ = reply_tx.send(out_error(request_id, "publish_failed", error));
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
                            && let Ok(signed) = host
                                .sign_call_payload(&current.nostr_group_id, signal.payload_json.clone())
                        {
                            let _ = publish_signed_call_signal_result(
                                &host,
                                CallSignalPublishKind::End,
                                current.nostr_group_id.clone(),
                                signal,
                                &signed,
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
                                let mapped = map_member_key_package_fetch_error(&peer_pubkey, &e);
                                reply_tx
                                    .send(out_error(request_id, mapped.code, mapped.message))
                                    .ok();
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

                        let host =
                            DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let refreshed = match host.refresh_session_state(
                            group_subs.values().cloned().collect(),
                            giftwrap_lookback_sec,
                        ) {
                            Ok(refreshed) => refreshed,
                            Err(err) => {
                                reply_tx
                                    .send(out_error(
                                        request_id,
                                        "runtime_error",
                                        format!("refresh session state: {err:#}"),
                                    ))
                                    .ok();
                                continue;
                            }
                        };

                        // Subscribe to newly planned group message targets.
                        for planned_group_id in refreshed.sync_plan.group_subscriptions.added_group_ids
                        {
                            match crate::subscribe_group_msgs(&client, &planned_group_id).await {
                                Ok(sid) => {
                                    group_subs.insert(sid, planned_group_id);
                                }
                                Err(err) => {
                                    warn!(
                                        "[pikachat] subscribe group msgs failed after init_group: {err:#}"
                                    );
                                }
                            }
                        }

                        let _ = emit_group_updated_snapshot(
                            &out_tx,
                            &protocol_event_sinks,
                            GroupUpdatedEmission {
                                host: &host,
                                local_pubkey: &keys.public_key(),
                                kind: GroupUpdateKindOut::Created,
                                nostr_group_id: &nostr_group_id_hex,
                                context: "init_group",
                            },
                        );
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
            acp_completion = async {
                match acp_completion_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                let Some(AcpTurnCompletion { conversation_id, result }) = acp_completion else {
                    acp_completion_rx = None;
                    continue;
                };
                match result {
                    Ok(reply) => {
                        let final_text = reply.final_text.trim();
                        if final_text.is_empty() {
                            continue;
                        }
                        let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                        let prepared = match host.prepare_outbound_action(
                            &conversation_id,
                            OutboundConversationAction::Message {
                                kind: Kind::ChatMessage,
                                content: final_text.to_string(),
                                tags: vec![],
                                created_at: Timestamp::now(),
                            },
                        ) {
                            Ok(prepared) => prepared,
                            Err(DaemonPrepareError::BadGroup(err)) => {
                                warn!(
                                    "[pikachat] ACP reply group resolution failed group={} session={} err={err:#}",
                                    conversation_id,
                                    reply.session_id,
                                );
                                continue;
                            }
                            Err(DaemonPrepareError::Prepare(err)) => {
                                warn!(
                                    "[pikachat] ACP reply prepare failed group={} session={} err={err:#}",
                                    conversation_id,
                                    reply.session_id,
                                );
                                continue;
                            }
                        };
                        if let Err(err) = host
                            .publish_prepared(&prepared, "daemon_acp_reply")
                            .await
                        {
                            warn!(
                                "[pikachat] ACP reply publish failed group={} session={} err={err:#}",
                                conversation_id,
                                reply.session_id,
                            );
                        }
                    }
                    Err(err) => {
                        warn!(
                            "[pikachat] ACP prompt failed group={} err={}",
                            conversation_id,
                            err
                        );
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
                    let inbound =
                        match classify_inbound_relay_event(&client, &mut seen_inbound, event).await
                        {
                        Ok(Some(inbound)) => inbound,
                        Ok(None) => continue,
                        Err(e) => {
                            warn!("[pikachat] notification ingress failed err={e:#}");
                            continue;
                        }
                    };
                    let InboundRelayEvent::Welcome {
                        wrapper,
                        sender,
                        rumor,
                    } = inbound
                    else {
                        continue;
                    };

                    let welcome = match ingest_unwrapped_welcome(
                        &mdk,
                        &wrapper.id,
                        sender,
                        &rumor,
                        |sender_hex| sender_allowed(sender_hex),
                    ) {
                        Ok(Some(w)) => w,
                        Ok(None) => continue,
                        Err(e) => {
                            warn!(
                                "[pikachat] welcome ingest failed wrapper_id={} err={e:#}",
                                wrapper.id.to_hex()
                            );
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

                let host = DaemonHostContext::new(&client, &relay_urls, &mdk, &keys, &pubkey_hex);
                let Some(subscribed_group_id) = group_subs.get(&subscription_id).cloned() else {
                    continue;
                };
                let before_group_snapshot =
                    host.lookup_joined_group_snapshot(&subscribed_group_id).ok();
                let inbound =
                    match classify_inbound_relay_event(&client, &mut seen_inbound, event).await {
                    Ok(Some(inbound)) => inbound,
                    Ok(None) => continue,
                    Err(e) => {
                        warn!("[pikachat] notification ingress failed err={e:#}");
                        continue;
                    }
                };
                let processed = match host.process_classified_inbound_group_message(inbound) {
                    Ok(Some(processed)) => processed,
                    Ok(None) => continue,
                    Err(e) => {
                        warn!("[pikachat] process_message failed err={e:#}");
                        continue;
                    }
                };
                let event_id = processed.event_id();
                seen_group_events.insert(event_id);

                let Some(conversation_event) = processed.into_conversation_event() else {
                    continue;
                };
                match host.interpret_conversation_event(conversation_event) {
                    RuntimeConversationEventInterpretation::Application { message } => {
                        let interpreted = host.interpret_runtime_application_message(*message);
                        let sender_hex =
                            interpreted.message().message.pubkey.to_hex().to_lowercase();
                        if !sender_allowed(&sender_hex) {
                            warn!("[pikachat] drop message (sender not allowed) from={sender_hex}");
                            continue;
                        }
                        let (classification, nostr_group_id, msg, emit_profile_update) = match interpreted {
                            RuntimeApplicationMessageInterpretation::TypingIndicator { .. } => {
                                continue;
                            }
                            RuntimeApplicationMessageInterpretation::CallSignal {
                                message,
                                parsed_signal,
                            } => {
                                let classification = message.classification;
                                let nostr_group_id = message.nostr_group_id_hex;
                                let msg = message.message;
                                let parsed_signal =
                                    parsed_signal.or_else(|| parse_call_signal(&msg.content));
                                if let Some(signal) = parsed_signal {
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
                                            if let Ok(signed) = host
                                                .sign_call_payload(
                                                    &nostr_group_id,
                                                    rejected.signal.payload_json.clone(),
                                                )
                                            {
                                                let _ = publish_signed_call_signal_result(
                                                    &host,
                                                    CallSignalPublishKind::Reject,
                                                    nostr_group_id.clone(),
                                                    rejected.signal,
                                                    &signed,
                                                    label,
                                                )
                                                .await;
                                            }
                                        }
                                        InboundCallSignalOutcome::IncomingInvite(invite) => {
                                            pending_call_invites
                                                .insert(invite.call_id.clone(), (*invite).clone());
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
                                                ActiveCallMode::Data => {
                                                    match start_data_worker(
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
                                                    }
                                                }
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
                                            pending_outgoing_call_invites
                                                .remove(&accepted.pending.call_id);
                                            out_tx
                                                .send(OutMsg::CallSessionStarted {
                                                    call_id: accepted.pending.call_id,
                                                    from_pubkey: sender_hex.clone(),
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
                                (classification, nostr_group_id, msg, false)
                            }
                            RuntimeApplicationMessageInterpretation::Content { message } => (
                                message.classification,
                                message.nostr_group_id_hex,
                                message.message,
                                false,
                            ),
                            RuntimeApplicationMessageInterpretation::GroupProfile { message } => (
                                message.classification,
                                message.nostr_group_id_hex,
                                message.message,
                                true,
                            ),
                        };
                        let mut media: Vec<MediaAttachmentOut> = Vec::new();
                        {
                            let attachments = host.parse_message_media_attachments(&msg);
                            for attachment in attachments {
                                let mut att =
                                    media_attachment_to_out(attachment.attachment.clone());
                                match host
                                    .download_and_decrypt_media(
                                        &msg.mls_group_id,
                                        &attachment,
                                        state_dir,
                                    )
                                    .await
                                {
                                    Ok(path) => att.local_path = Some(path),
                                    Err(e) => warn!(
                                        "[pikachat] media download failed url={}: {e:#}",
                                        attachment.attachment.url
                                    ),
                                }
                                media.push(att);
                            }
                        }
                        let acp_nostr_group_id = nostr_group_id.clone();
                        let acp_sender_hex = sender_hex.clone();
                        let acp_content = msg.content.clone();
                        out_tx
                            .send(OutMsg::MessageReceived {
                                nostr_group_id,
                                from_pubkey: sender_hex,
                                content: msg.content,
                                kind: msg.kind.as_u16(),
                                created_at: msg.created_at.as_secs(),
                                event_id: msg.id.to_hex(),
                                message_id: msg.id.to_hex(),
                                media,
                            })
                            .ok();
                        if let Some(acp) = acp_backend.as_ref()
                            && should_prompt_acp_reply(
                                classification,
                                &acp_sender_hex,
                                &pubkey_hex,
                                &acp_content,
                            )
                        {
                            let prompt = build_acp_prompt(
                                &acp_nostr_group_id,
                                &acp_sender_hex,
                                &acp_content,
                            );
                            if let Err(err) =
                                acp.enqueue_prompt(&acp_nostr_group_id, &prompt).await
                            {
                                warn!(
                                    "[pikachat] ACP enqueue failed group={} sender={} err={err:#}",
                                    acp_nostr_group_id, acp_sender_hex
                                );
                            }
                        }
                        if emit_profile_update {
                            let _ = emit_remote_group_profile_updated(
                                &out_tx,
                                &protocol_event_sinks,
                                &host,
                                &keys.public_key(),
                                &acp_nostr_group_id,
                            );
                        }
                    }
                    RuntimeConversationEventInterpretation::GroupUpdate { update, is_commit } => {
                        if is_commit {
                            let _ = emit_remote_group_commit_updated(
                                &out_tx,
                                &protocol_event_sinks,
                                &host,
                                &keys.public_key(),
                                before_group_snapshot.as_ref(),
                                &update.nostr_group_id_hex,
                            );
                        }
                    }
                    RuntimeConversationEventInterpretation::NeedsFullRefresh { .. } => {}
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
    use pika_marmot_runtime::conversation::{RuntimeGroupUpdate, RuntimeGroupUpdateKind};
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

    fn expect_group_updated(event: OutMsg) -> GroupUpdatedOut {
        let OutMsg::GroupUpdated { update } = event else {
            panic!("expected group_updated event");
        };
        update
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

    fn make_group_message_event(
        mdk: &crate::PikaMdk,
        keys: &Keys,
        mls_group_id: &GroupId,
        kind: Kind,
        content: &str,
        tags: Tags,
    ) -> Event {
        let rumor = EventBuilder::new(kind, content)
            .tags(tags)
            .build(keys.public_key());
        mdk.create_message(mls_group_id, rumor)
            .expect("create group message event")
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
    fn acp_prompt_mapping_keeps_group_and_sender_context() {
        let prompt = build_acp_prompt("001122", "abcdef", "hello from nostr");
        assert!(prompt.contains("conversation_id: 001122"));
        assert!(prompt.contains("sender_pubkey: abcdef"));
        assert!(prompt.contains("message:\nhello from nostr"));
    }

    #[test]
    fn acp_prompt_trigger_skips_self_and_empty_messages() {
        assert!(should_prompt_acp_reply(
            MessageClassification::Chat,
            "peer",
            "self",
            "hello",
        ));
        assert!(!should_prompt_acp_reply(
            MessageClassification::TypingIndicator,
            "peer",
            "self",
            "typing",
        ));
        assert!(!should_prompt_acp_reply(
            MessageClassification::Chat,
            "self",
            "self",
            "hello",
        ));
        assert!(!should_prompt_acp_reply(
            MessageClassification::Chat,
            "peer",
            "self",
            "   ",
        ));
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
    fn daemon_pending_welcome_queries_use_shared_query_boundary() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon pending welcome query".to_string(),
            "Shared pending welcome snapshot".to_string(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let group_result = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let mut welcome_rumor = group_result
            .welcome_rumors
            .into_iter()
            .next()
            .expect("welcome rumor");
        let welcome_event_id = welcome_rumor.id();
        let wrapper = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                EventBuilder::gift_wrap(
                    &inviter_keys,
                    &invitee_keys.public_key(),
                    welcome_rumor.clone(),
                    [],
                )
                .await
                .expect("build giftwrap")
            });
        invitee_mdk
            .process_welcome(&wrapper.id, &welcome_rumor)
            .expect("process welcome");

        let signer: Arc<dyn NostrSigner> = Arc::new(invitee_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&invitee_mdk, &invitee_keys, &client, &relay_urls);

        let snapshots = host
            .list_pending_welcome_snapshots()
            .expect("list pending welcome snapshots");
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].wrapper_event_id, wrapper.id);
        assert_eq!(snapshots[0].welcome_event_id, welcome_event_id);
        assert_eq!(
            snapshots[0].nostr_group_id_hex,
            hex::encode(group_result.group.nostr_group_id)
        );
        assert_eq!(snapshots[0].group_name, "Daemon pending welcome query");

        let looked_up = host
            .lookup_pending_welcome(&wrapper.id)
            .expect("lookup pending welcome")
            .expect("pending welcome should exist");
        assert_eq!(looked_up.id, welcome_event_id);
        assert_eq!(looked_up.wrapper_event_id, wrapper.id);
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
        let snapshot = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls)
            .lookup_joined_group_snapshot(&hex::encode(created.group.nostr_group_id))
            .expect("lookup joined group snapshot");
        assert_eq!(snapshot.mls_group_id, created.group.mls_group_id);
        assert_eq!(snapshot.relay_urls, relay_urls);
        assert_eq!(snapshot.member_snapshots.len(), 2);
    }

    #[test]
    fn daemon_list_groups_uses_shared_joined_group_snapshots() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon list groups".to_string(),
            "Shared snapshot projection".to_string(),
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
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);

        let snapshots = host
            .list_joined_group_snapshots()
            .expect("list joined group snapshots");
        let groups = host.list_groups().expect("list groups");

        assert_eq!(snapshots.len(), 1);
        assert_eq!(groups.len(), 1);
        assert_eq!(snapshots[0].member_snapshots.len(), 2);
        assert_eq!(
            snapshots[0].is_admin(&inviter_keys.public_key()),
            created
                .group
                .admin_pubkeys
                .contains(&inviter_keys.public_key())
        );
        assert_eq!(
            snapshots[0].is_admin(&invitee_keys.public_key()),
            created
                .group
                .admin_pubkeys
                .contains(&invitee_keys.public_key())
        );
        assert_eq!(
            groups[0].nostr_group_id_hex,
            snapshots[0].nostr_group_id_hex
        );
        assert_eq!(groups[0].mls_group_id_hex, snapshots[0].mls_group_id_hex);
        assert_eq!(groups[0].name, snapshots[0].name);
        assert_eq!(groups[0].description, snapshots[0].description);
        assert_eq!(groups[0].member_count, snapshots[0].member_count());
    }

    #[test]
    fn daemon_message_history_uses_shared_runtime_page_query() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon message history".to_string(),
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
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge pending commit");
        for content in ["one", "two", "three"] {
            let event = make_group_message_event(
                &inviter_mdk,
                &inviter_keys,
                &created.group.mls_group_id,
                Kind::ChatMessage,
                content,
                Tags::new(),
            );
            inviter_mdk
                .process_message(&event)
                .expect("process group message");
        }

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let nostr_group_id_hex = hex::encode(created.group.nostr_group_id);

        let first_page = host
            .load_message_page(
                &nostr_group_id_hex,
                pika_marmot_runtime::conversation::RuntimeMessagePageQuery::new(2, 0),
            )
            .expect("load first page");
        let second_page = host
            .load_message_page(
                &nostr_group_id_hex,
                pika_marmot_runtime::conversation::RuntimeMessagePageQuery::new(2, 2),
            )
            .expect("load second page");

        assert_eq!(first_page.nostr_group_id_hex, nostr_group_id_hex);
        assert_eq!(first_page.fetched_count, 2);
        assert_eq!(first_page.next_offset, 2);
        assert!(!first_page.storage_exhausted);
        assert_eq!(first_page.messages.len(), 2);
        assert_eq!(second_page.fetched_count, 1);
        assert_eq!(second_page.next_offset, 3);
        assert!(second_page.storage_exhausted);
        assert_eq!(second_page.messages.len(), 1);
    }

    #[test]
    fn daemon_outbound_prepare_uses_shared_command_boundary() {
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

    #[test]
    fn daemon_outbound_publish_operation_result_uses_shared_runtime_event_boundary() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon outbound publish".to_string(),
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
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let prepared = host
            .prepare_outbound_action(
                &hex::encode(created.group.nostr_group_id),
                OutboundConversationAction::Message {
                    kind: Kind::ChatMessage,
                    content: "hello".to_string(),
                    tags: vec![],
                    created_at: Timestamp::from(123_u64),
                },
            )
            .expect("prepare outbound action");
        let operation_id = prepared.rumor_id;

        let operation = host.complete_outbound_publish_operation(
            prepared,
            pika_marmot_runtime::outbound::OutboundConversationPublishStatus::Published {
                wrapper_event_id: EventId::all_zeros(),
            },
        );
        let completed = operation
            .into_outbound_conversation_publish_result()
            .expect("completed outbound publish");

        assert_eq!(completed.rumor_id, operation_id);
        assert_eq!(
            completed.target.nostr_group_id_hex,
            hex::encode(created.group.nostr_group_id)
        );
        assert_eq!(completed.kind, Kind::ChatMessage);
        assert_eq!(completed.wrapper_event_id, EventId::all_zeros());
    }

    #[test]
    fn daemon_call_signal_publish_operation_result_uses_shared_runtime_event_boundary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mdk = crate::open_mdk(dir.path()).expect("open mdk");
        let keys = Keys::generate();
        let signer: Arc<dyn NostrSigner> = Arc::new(keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&mdk, &keys, &client, &relay_urls);
        let wrapper_event_id =
            EventId::from_hex("3333333333333333333333333333333333333333333333333333333333333333")
                .expect("event id");
        for (kind, payload_json) in [
            (CallSignalPublishKind::Invite, "{\"type\":\"call.invite\"}"),
            (CallSignalPublishKind::Accept, "{\"type\":\"call.accept\"}"),
            (CallSignalPublishKind::Reject, "{\"type\":\"call.reject\"}"),
            (CallSignalPublishKind::End, "{\"type\":\"call.end\"}"),
        ] {
            let result = complete_daemon_call_signal_publish_result(
                &host,
                kind,
                "deadbeef".to_string(),
                pika_marmot_runtime::call_runtime::PreparedCallSignal {
                    call_id: "550e8400-e29b-41d4-a716-446655440017".to_string(),
                    payload_json: payload_json.to_string(),
                },
                CallSignalPublishStatus::Published { wrapper_event_id },
            )
            .expect("completed call signal publish");

            assert_eq!(result.kind, kind);
            assert_eq!(result.nostr_group_id_hex, "deadbeef");
            assert_eq!(result.call_id, "550e8400-e29b-41d4-a716-446655440017");
            assert_eq!(result.wrapper_event_id, wrapper_event_id);
        }
    }

    #[test]
    fn daemon_call_signal_publish_failure_uses_shared_runtime_event_boundary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mdk = crate::open_mdk(dir.path()).expect("open mdk");
        let keys = Keys::generate();
        let signer: Arc<dyn NostrSigner> = Arc::new(keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&mdk, &keys, &client, &relay_urls);
        let wrapper_event_id =
            EventId::from_hex("4444444444444444444444444444444444444444444444444444444444444444")
                .expect("event id");

        let error = complete_daemon_call_signal_publish_result(
            &host,
            CallSignalPublishKind::Accept,
            "deadbeef".to_string(),
            pika_marmot_runtime::call_runtime::PreparedCallSignal {
                call_id: "550e8400-e29b-41d4-a716-446655440018".to_string(),
                payload_json: "{\"type\":\"call.accept\"}".to_string(),
            },
            CallSignalPublishStatus::PublishFailed {
                wrapper_event_id,
                error: "offline".to_string(),
            },
        )
        .expect_err("failed call signal publish");

        assert_eq!(error, "offline");
    }

    #[test]
    fn daemon_add_members_preparation_and_finalize_use_shared_command_boundary() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let peer_dir = tempfile::tempdir().expect("peer tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let peer_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let peer_mdk = crate::open_mdk(peer_dir.path()).expect("open peer mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let peer_kp = make_key_package_event(&peer_mdk, &peer_keys);
        let config = NostrGroupConfigData::new(
            "Daemon membership boundary".to_string(),
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
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let before_merge = inviter_mdk
            .get_members(&created.group.mls_group_id)
            .expect("members before merge")
            .len();

        let prepared = host
            .prepare_add_members(&hex::encode(created.group.nostr_group_id), &[peer_kp])
            .expect("prepare add members");
        let finalized = host.finalize_published_evolution(prepared);

        let after_merge = inviter_mdk
            .get_members(&created.group.mls_group_id)
            .expect("members after merge")
            .len();
        assert_eq!(before_merge + 1, after_merge);
        assert!(finalized.merge_error.is_none());
        assert_eq!(
            finalized
                .welcome_delivery
                .as_ref()
                .expect("welcome delivery")
                .recipients,
            vec![peer_keys.public_key()]
        );
    }

    #[test]
    fn daemon_membership_operation_result_uses_shared_runtime_event_boundary() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let peer_dir = tempfile::tempdir().expect("peer tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let peer_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let peer_mdk = crate::open_mdk(peer_dir.path()).expect("open peer mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let peer_kp = make_key_package_event(&peer_mdk, &peer_keys);
        let config = NostrGroupConfigData::new(
            "Daemon membership operation".to_string(),
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
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let prepared = host
            .prepare_add_members(&hex::encode(created.group.nostr_group_id), &[peer_kp])
            .expect("prepare add members");
        let operation_id = prepared.evolution_event.id;

        let operation = host.complete_membership_evolution_operation(
            prepared,
            pika_marmot_runtime::membership::EvolutionPublishStatus::Published,
        );
        let completed_id = operation.operation_id();
        let result = operation
            .into_membership_evolution_result()
            .expect("completed membership evolution");

        assert_eq!(completed_id, operation_id);
        assert_eq!(
            result.nostr_group_id_hex,
            hex::encode(created.group.nostr_group_id)
        );
        assert_eq!(result.added_pubkeys, vec![peer_keys.public_key()]);
        assert!(result.merge_error.is_none());
        assert_eq!(
            result
                .welcome_delivery
                .as_ref()
                .expect("welcome delivery")
                .recipients,
            vec![peer_keys.public_key()]
        );
    }

    #[tokio::test]
    async fn daemon_add_members_request_succeeds_on_production_handler() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let peer_dir = tempfile::tempdir().expect("peer tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let peer_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let peer_mdk = crate::open_mdk(peer_dir.path()).expect("open peer mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let peer_kp = make_key_package_event(&peer_mdk, &peer_keys);
        let config = NostrGroupConfigData::new(
            "Daemon add members request".to_string(),
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
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);

        let reply = handle_add_members_request_with(
            Some("req-1".to_string()),
            &host,
            &inviter_keys,
            &hex::encode(created.group.nostr_group_id),
            &[peer_keys.public_key().to_hex()],
            move |_| {
                let peer_kp = peer_kp.clone();
                async move { Ok(peer_kp) }
            },
            |_event, _label| async move { Ok(()) },
        )
        .await;

        let OutMsg::Ok {
            request_id,
            result: Some(result),
        } = reply
        else {
            panic!("expected successful add_members reply");
        };
        assert_eq!(request_id.as_deref(), Some("req-1"));
        let result: AddMembersResultOut =
            serde_json::from_value(result).expect("deserialize add_members result");
        assert_eq!(
            result.nostr_group_id,
            hex::encode(created.group.nostr_group_id)
        );
        assert_eq!(result.added_pubkeys, vec![peer_keys.public_key().to_hex()]);
        assert_eq!(result.member_count, 3);
        assert_eq!(result.welcome_delivery_count, 1);
    }

    #[tokio::test]
    async fn daemon_add_members_request_reports_missing_key_package() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let missing_peer_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon add members missing kp".to_string(),
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
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let missing_pubkey = missing_peer_keys.public_key();

        let reply =
            handle_add_members_request_with(
                Some("req-2".to_string()),
                &host,
                &inviter_keys,
                &hex::encode(created.group.nostr_group_id),
                &[missing_pubkey.to_hex()],
                move |_| async move {
                    anyhow::bail!("no keypackage found for {}", missing_pubkey.to_hex())
                },
                |_event, _label| async move { Ok(()) },
            )
            .await;

        let OutMsg::Error {
            request_id,
            code,
            message,
        } = reply
        else {
            panic!("expected add_members error reply");
        };
        assert_eq!(request_id.as_deref(), Some("req-2"));
        assert_eq!(code, "no_key_packages");
        assert!(message.contains(&missing_pubkey.to_hex()));
    }

    #[tokio::test]
    async fn daemon_add_members_group_updated_event_uses_shared_snapshots() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let peer_dir = tempfile::tempdir().expect("peer tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let peer_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let peer_mdk = crate::open_mdk(peer_dir.path()).expect("open peer mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let peer_kp = make_key_package_event(&peer_mdk, &peer_keys);
        let config = NostrGroupConfigData::new(
            "Daemon add-members event".to_string(),
            "Shared event snapshot".to_string(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let nostr_group_id_hex = hex::encode(created.group.nostr_group_id);

        let reply = handle_add_members_request_with(
            Some("req-add-event".to_string()),
            &host,
            &inviter_keys,
            &nostr_group_id_hex,
            &[peer_keys.public_key().to_hex()],
            move |_| {
                let peer_kp = peer_kp.clone();
                async move { Ok(peer_kp) }
            },
            |event, label| {
                let mdk = &inviter_mdk;
                async move {
                    if label == "add_members" {
                        mdk.process_message(&event)
                            .expect("process add_members event");
                    }
                    Ok(())
                }
            },
        )
        .await;
        assert!(matches!(reply, OutMsg::Ok { .. }));

        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let event_sinks: ProtocolEventSinks = Arc::new(Mutex::new(Vec::new()));
        assert!(emit_group_updated_if_ok(
            &reply,
            &out_tx,
            &event_sinks,
            GroupUpdatedEmission {
                host: &host,
                local_pubkey: &inviter_keys.public_key(),
                kind: GroupUpdateKindOut::MembersAdded,
                nostr_group_id: &nostr_group_id_hex,
                context: "add_members",
            },
        ));
        let update = expect_group_updated(out_rx.try_recv().expect("group_updated event"));
        assert_eq!(update.kind, GroupUpdateKindOut::MembersAdded);
        assert_eq!(update.nostr_group_id, nostr_group_id_hex);
        assert_eq!(update.member_count, Some(3));
        assert_eq!(update.members.len(), 3);
        assert_eq!(
            update.profile.expect("profile").name,
            "Daemon add-members event"
        );
    }

    #[test]
    fn daemon_list_members_request_returns_shared_snapshot_members() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon list members request".to_string(),
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
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);

        let reply = handle_list_members_request(
            Some("req-3".to_string()),
            &host,
            &hex::encode(created.group.nostr_group_id),
        );

        let OutMsg::Ok {
            request_id,
            result: Some(result),
        } = reply
        else {
            panic!("expected successful list_members reply");
        };
        assert_eq!(request_id.as_deref(), Some("req-3"));
        let result: ListMembersResultOut =
            serde_json::from_value(result).expect("deserialize list_members result");
        assert_eq!(
            result.nostr_group_id,
            hex::encode(created.group.nostr_group_id)
        );
        assert_eq!(result.member_count, 2);
        let mut expected_members: Vec<GroupMemberOut> = host
            .lookup_joined_group_snapshot(&hex::encode(created.group.nostr_group_id))
            .expect("lookup joined group snapshot")
            .member_snapshots
            .into_iter()
            .map(|member| GroupMemberOut {
                pubkey: member.pubkey.to_hex(),
                is_admin: member.is_admin,
            })
            .collect();
        expected_members.sort_by(|left, right| left.pubkey.cmp(&right.pubkey));
        assert_eq!(result.members, expected_members);
    }

    #[test]
    fn daemon_list_members_request_rejects_unknown_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        let keys = Keys::generate();
        let signer: Arc<dyn NostrSigner> = Arc::new(keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let mdk = crate::open_mdk(dir.path()).expect("open mdk");
        let host = test_host(&mdk, &keys, &client, &relay_urls);

        let reply = handle_list_members_request(Some("req-4".to_string()), &host, "deadbeef");

        let OutMsg::Error {
            request_id,
            code,
            message,
        } = reply
        else {
            panic!("expected list_members error reply");
        };
        assert_eq!(request_id.as_deref(), Some("req-4"));
        assert_eq!(code, "bad_group_id");
        assert!(message.contains("deadbeef") || message.contains("group"));
    }

    #[tokio::test]
    async fn daemon_remove_members_request_succeeds_on_production_handler() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon remove members request".to_string(),
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
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);

        let reply = handle_remove_members_request_with(
            Some("req-remove".to_string()),
            &host,
            &hex::encode(created.group.nostr_group_id),
            &[invitee_keys.public_key().to_hex()],
            |_event, _label| async move { Ok(()) },
        )
        .await;

        let OutMsg::Ok {
            request_id,
            result: Some(result),
        } = reply
        else {
            panic!("expected successful remove_members reply");
        };
        assert_eq!(request_id.as_deref(), Some("req-remove"));
        let result: RemoveMembersResultOut =
            serde_json::from_value(result).expect("deserialize remove_members result");
        assert_eq!(
            result.nostr_group_id,
            hex::encode(created.group.nostr_group_id)
        );
        assert_eq!(
            result.removed_pubkeys,
            vec![invitee_keys.public_key().to_hex()]
        );
        assert_eq!(result.member_count, 1);
    }

    #[tokio::test]
    async fn daemon_remove_members_request_rejects_invalid_pubkey() {
        let dir = tempfile::tempdir().expect("tempdir");
        let keys = Keys::generate();
        let signer: Arc<dyn NostrSigner> = Arc::new(keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let mdk = crate::open_mdk(dir.path()).expect("open mdk");
        let host = test_host(&mdk, &keys, &client, &relay_urls);

        let reply = handle_remove_members_request_with(
            Some("req-remove-bad".to_string()),
            &host,
            "deadbeef",
            &["not-a-pubkey".to_string()],
            |_event, _label| async move { Ok(()) },
        )
        .await;

        let OutMsg::Error {
            request_id,
            code,
            message,
        } = reply
        else {
            panic!("expected remove_members error reply");
        };
        assert_eq!(request_id.as_deref(), Some("req-remove-bad"));
        assert_eq!(code, "bad_pubkey");
        assert!(message.contains("invalid peer_pubkey"));
    }

    #[tokio::test]
    async fn daemon_remove_members_group_updated_event_uses_shared_snapshots() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon remove-members event".to_string(),
            "Shared event snapshot".to_string(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let nostr_group_id_hex = hex::encode(created.group.nostr_group_id);

        let reply = handle_remove_members_request_with(
            Some("req-remove-event".to_string()),
            &host,
            &nostr_group_id_hex,
            &[invitee_keys.public_key().to_hex()],
            |event, _label| {
                let mdk = &inviter_mdk;
                async move {
                    mdk.process_message(&event)
                        .expect("process remove_members event");
                    Ok(())
                }
            },
        )
        .await;
        assert!(matches!(reply, OutMsg::Ok { .. }));

        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let event_sinks: ProtocolEventSinks = Arc::new(Mutex::new(Vec::new()));
        assert!(emit_group_updated_if_ok(
            &reply,
            &out_tx,
            &event_sinks,
            GroupUpdatedEmission {
                host: &host,
                local_pubkey: &inviter_keys.public_key(),
                kind: GroupUpdateKindOut::MembersRemoved,
                nostr_group_id: &nostr_group_id_hex,
                context: "remove_members",
            },
        ));
        let update = expect_group_updated(out_rx.try_recv().expect("group_updated event"));
        assert_eq!(update.kind, GroupUpdateKindOut::MembersRemoved);
        assert_eq!(update.member_count, Some(1));
        assert_eq!(update.members.len(), 1);
        assert_eq!(
            update.profile.expect("profile").name,
            "Daemon remove-members event"
        );
    }

    #[tokio::test]
    async fn daemon_leave_group_request_succeeds_on_production_handler() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon leave group request".to_string(),
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
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);

        let result = leave_group_result_with(
            &host,
            &hex::encode(created.group.nostr_group_id),
            |_event, _label| async move { Ok(()) },
        )
        .await
        .expect("leave group result");

        assert_eq!(
            result.nostr_group_id,
            hex::encode(created.group.nostr_group_id)
        );
    }

    #[tokio::test]
    async fn daemon_leave_group_request_rejects_unknown_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        let keys = Keys::generate();
        let signer: Arc<dyn NostrSigner> = Arc::new(keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let mdk = crate::open_mdk(dir.path()).expect("open mdk");
        let host = test_host(&mdk, &keys, &client, &relay_urls);

        let err =
            leave_group_result_with(&host, "deadbeef", |_event, _label| async move { Ok(()) })
                .await
                .expect_err("leave group should fail for unknown group");

        assert_eq!(err.code, "bad_group_id");
        assert!(err.message.contains("deadbeef") || err.message.contains("group"));
    }

    #[tokio::test]
    async fn daemon_update_group_profile_request_succeeds_on_production_handler() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon update group profile request".to_string(),
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
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let existing_profile = UnsignedEvent::new(
            inviter_keys.public_key(),
            Timestamp::from(10_u64),
            Kind::Metadata,
            Tags::new(),
            r#"{"display_name":"Old Name","picture":"https://example.com/group.jpg"}"#,
        );
        let existing_wrapper = inviter_mdk
            .create_message(&created.group.mls_group_id, existing_profile)
            .expect("create existing group profile");
        inviter_mdk
            .process_message(&existing_wrapper)
            .expect("process existing group profile");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);

        let reply = handle_update_group_profile_request_with(
            Some("req-profile".to_string()),
            &host,
            &inviter_keys.public_key(),
            &hex::encode(created.group.nostr_group_id),
            "New Name",
            "New About",
            |prepared| {
                let mdk = &inviter_mdk;
                async move {
                    let processed = mdk
                        .process_message(&prepared.wrapper)
                        .expect("process prepared profile");
                    match processed {
                        MessageProcessingResult::ApplicationMessage(message) => {
                            let metadata: Metadata =
                                serde_json::from_str(&message.content).expect("parse metadata");
                            assert_eq!(message.kind, Kind::Metadata);
                            assert_eq!(metadata.display_name.as_deref(), Some("New Name"));
                            assert_eq!(metadata.about.as_deref(), Some("New About"));
                            assert_eq!(
                                metadata.picture.as_deref(),
                                Some("https://example.com/group.jpg")
                            );
                        }
                        other => panic!("expected application message, got {other:?}"),
                    }
                    Ok(EventId::all_zeros())
                }
            },
        )
        .await;

        let OutMsg::Ok {
            request_id,
            result: Some(result),
        } = reply
        else {
            panic!("expected successful update_group_profile reply");
        };
        assert_eq!(request_id.as_deref(), Some("req-profile"));
        let result: GroupProfileOut =
            serde_json::from_value(result).expect("deserialize update_group_profile result");
        assert_eq!(
            result.nostr_group_id,
            hex::encode(created.group.nostr_group_id)
        );
        assert_eq!(result.owner_pubkey, inviter_keys.public_key().to_hex());
        assert_eq!(result.name, "New Name");
        assert_eq!(result.about, "New About");
        assert_eq!(
            result.picture_url.as_deref(),
            Some("https://example.com/group.jpg")
        );
    }

    #[tokio::test]
    async fn daemon_update_group_profile_request_rejects_empty_metadata() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon empty group profile".to_string(),
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
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);

        let reply = handle_update_group_profile_request_with(
            Some("req-profile-empty".to_string()),
            &host,
            &inviter_keys.public_key(),
            &hex::encode(created.group.nostr_group_id),
            "   ",
            "",
            |_prepared| async move { Ok(EventId::all_zeros()) },
        )
        .await;

        let OutMsg::Error {
            request_id,
            code,
            message,
        } = reply
        else {
            panic!("expected update_group_profile error reply");
        };
        assert_eq!(request_id.as_deref(), Some("req-profile-empty"));
        assert_eq!(code, "bad_request");
        assert!(message.contains("name or about"));
    }

    #[tokio::test]
    async fn daemon_update_group_profile_group_updated_event_uses_shared_profile_snapshot() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon profile event".to_string(),
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
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let existing_profile = UnsignedEvent::new(
            inviter_keys.public_key(),
            Timestamp::from(10_u64),
            Kind::Metadata,
            Tags::new(),
            r#"{"display_name":"Old Name","picture":"https://example.com/group.jpg"}"#,
        );
        let existing_wrapper = inviter_mdk
            .create_message(&created.group.mls_group_id, existing_profile)
            .expect("create existing group profile");
        inviter_mdk
            .process_message(&existing_wrapper)
            .expect("process existing group profile");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let nostr_group_id_hex = hex::encode(created.group.nostr_group_id);

        let reply = handle_update_group_profile_request_with(
            Some("req-profile-event".to_string()),
            &host,
            &inviter_keys.public_key(),
            &nostr_group_id_hex,
            "Updated Name",
            "Updated About",
            |prepared| {
                let mdk = &inviter_mdk;
                async move {
                    mdk.process_message(&prepared.wrapper)
                        .expect("process updated profile");
                    Ok(EventId::all_zeros())
                }
            },
        )
        .await;
        assert!(matches!(reply, OutMsg::Ok { .. }));

        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let event_sinks: ProtocolEventSinks = Arc::new(Mutex::new(Vec::new()));
        assert!(emit_group_updated_if_ok(
            &reply,
            &out_tx,
            &event_sinks,
            GroupUpdatedEmission {
                host: &host,
                local_pubkey: &inviter_keys.public_key(),
                kind: GroupUpdateKindOut::ProfileUpdated,
                nostr_group_id: &nostr_group_id_hex,
                context: "update_group_profile",
            },
        ));
        let update = expect_group_updated(out_rx.try_recv().expect("group_updated event"));
        assert_eq!(update.kind, GroupUpdateKindOut::ProfileUpdated);
        assert_eq!(update.member_count, Some(2));
        assert_eq!(update.members.len(), 2);
        let profile = update.profile.expect("profile");
        assert_eq!(profile.name, "Updated Name");
        assert_eq!(profile.about, "Updated About");
        assert_eq!(
            profile.picture_url.as_deref(),
            Some("https://example.com/group.jpg")
        );
    }

    #[tokio::test]
    async fn daemon_remote_membership_commit_emits_group_updated_snapshot() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let peer_dir = tempfile::tempdir().expect("peer tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let peer_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let peer_mdk = crate::open_mdk(peer_dir.path()).expect("open peer mdk");
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let created = inviter_mdk
            .create_group(
                &inviter_keys.public_key(),
                vec![invitee_kp],
                NostrGroupConfigData::new(
                    "Daemon remote membership event".to_string(),
                    "Remote commit".to_string(),
                    None,
                    None,
                    None,
                    relay_urls.clone(),
                    vec![inviter_keys.public_key(), invitee_keys.public_key()],
                ),
            )
            .expect("create group");
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let welcome_rumor = created
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

        let invitee_client = Client::builder().signer(invitee_keys.clone()).build();
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
        .expect("accept welcome");

        let invitee_host = test_host(&invitee_mdk, &invitee_keys, &invitee_client, &relay_urls);
        let inviter_client = Client::builder().signer(inviter_keys.clone()).build();
        let inviter_host = test_host(&inviter_mdk, &inviter_keys, &inviter_client, &relay_urls);
        let peer_kp = make_key_package_event(&peer_mdk, &peer_keys);
        let prepared = inviter_host
            .prepare_add_members(&accepted.nostr_group_id_hex, &[peer_kp])
            .expect("prepare add members");
        let before_snapshot = invitee_host
            .lookup_joined_group_snapshot(&accepted.nostr_group_id_hex)
            .ok();
        let processed = invitee_host
            .process_classified_inbound_group_message(InboundRelayEvent::GroupMessage {
                event: prepared.evolution_event.clone(),
            })
            .expect("process classified inbound group message")
            .expect("processed inbound group message");
        let interpreted = invitee_host.interpret_conversation_event(
            processed
                .into_conversation_event()
                .expect("conversation event for remote commit"),
        );

        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let event_sinks: ProtocolEventSinks = Arc::new(Mutex::new(Vec::new()));
        match interpreted {
            RuntimeConversationEventInterpretation::GroupUpdate { update, is_commit } => {
                assert!(is_commit);
                assert!(emit_remote_group_commit_updated(
                    &out_tx,
                    &event_sinks,
                    &invitee_host,
                    &invitee_keys.public_key(),
                    before_snapshot.as_ref(),
                    &update.nostr_group_id_hex,
                ));
            }
            other => panic!("expected inbound commit group update, got {other:?}"),
        }

        let update = expect_group_updated(out_rx.try_recv().expect("group_updated event"));
        assert_eq!(update.kind, GroupUpdateKindOut::MembersAdded);
        assert_eq!(update.nostr_group_id, accepted.nostr_group_id_hex);
        assert_eq!(update.member_count, Some(3));
        assert_eq!(update.members.len(), 3);
        assert_eq!(
            update.profile.expect("profile").name,
            "Daemon remote membership event"
        );
    }

    #[tokio::test]
    async fn daemon_remote_group_profile_message_emits_group_updated_snapshot() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let created = inviter_mdk
            .create_group(
                &inviter_keys.public_key(),
                vec![invitee_kp],
                NostrGroupConfigData::new(
                    "Daemon remote profile event".to_string(),
                    "Remote profile fallback".to_string(),
                    None,
                    None,
                    None,
                    relay_urls.clone(),
                    vec![inviter_keys.public_key(), invitee_keys.public_key()],
                ),
            )
            .expect("create group");
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let welcome_rumor = created
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

        let invitee_client = Client::builder().signer(invitee_keys.clone()).build();
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
        .expect("accept welcome");

        let profile_event = make_group_message_event(
            &inviter_mdk,
            &inviter_keys,
            &created.group.mls_group_id,
            Kind::Metadata,
            r#"{"display_name":"Remote Name","about":"Remote About","picture":"https://example.com/remote.png"}"#,
            Tags::new(),
        );
        let invitee_host = test_host(&invitee_mdk, &invitee_keys, &invitee_client, &relay_urls);
        let processed = invitee_host
            .process_classified_inbound_group_message(InboundRelayEvent::GroupMessage {
                event: profile_event,
            })
            .expect("process classified inbound profile message")
            .expect("processed inbound profile message");
        let interpreted = invitee_host.interpret_conversation_event(
            processed
                .into_conversation_event()
                .expect("conversation event for remote profile"),
        );

        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let event_sinks: ProtocolEventSinks = Arc::new(Mutex::new(Vec::new()));
        match interpreted {
            RuntimeConversationEventInterpretation::Application { message } => {
                match invitee_host.interpret_runtime_application_message(*message) {
                    RuntimeApplicationMessageInterpretation::GroupProfile { message } => {
                        assert!(emit_remote_group_profile_updated(
                            &out_tx,
                            &event_sinks,
                            &invitee_host,
                            &invitee_keys.public_key(),
                            &message.nostr_group_id_hex,
                        ));
                    }
                    other => panic!("expected remote group profile interpretation, got {other:?}"),
                }
            }
            other => panic!("expected inbound application message, got {other:?}"),
        }

        let update = expect_group_updated(out_rx.try_recv().expect("group_updated event"));
        assert_eq!(update.kind, GroupUpdateKindOut::ProfileUpdated);
        assert_eq!(update.nostr_group_id, accepted.nostr_group_id_hex);
        assert_eq!(update.member_count, Some(2));
        assert_eq!(update.members.len(), 2);
        let profile = update.profile.expect("profile");
        assert_eq!(profile.owner_pubkey, inviter_keys.public_key().to_hex());
        assert_eq!(profile.name, "Remote Name");
        assert_eq!(profile.about, "Remote About");
        assert_eq!(
            profile.picture_url.as_deref(),
            Some("https://example.com/remote.png")
        );

        let reply = handle_get_group_profile_request(
            Some("req-remote-profile".to_string()),
            &invitee_host,
            &invitee_keys.public_key(),
            &accepted.nostr_group_id_hex,
        );
        let OutMsg::Ok {
            request_id,
            result: Some(result),
        } = reply
        else {
            panic!("expected successful get_group_profile reply");
        };
        assert_eq!(request_id.as_deref(), Some("req-remote-profile"));
        let result: GroupProfileOut =
            serde_json::from_value(result).expect("deserialize get_group_profile result");
        assert_eq!(result.owner_pubkey, inviter_keys.public_key().to_hex());
        assert_eq!(result.name, "Remote Name");
        assert_eq!(result.about, "Remote About");
        assert_eq!(
            result.picture_url.as_deref(),
            Some("https://example.com/remote.png")
        );
    }

    #[test]
    fn group_updated_events_fan_out_to_protocol_sinks() {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (sink_tx, mut sink_rx) = mpsc::unbounded_channel();
        let event_sinks: ProtocolEventSinks = Arc::new(Mutex::new(vec![sink_tx]));
        let event = OutMsg::GroupUpdated {
            update: GroupUpdatedOut {
                kind: GroupUpdateKindOut::MembersAdded,
                nostr_group_id: "aa".to_string(),
                member_count: Some(2),
                members: vec![GroupMemberOut {
                    pubkey: "owner".to_string(),
                    is_admin: true,
                }],
                profile: None,
            },
        };

        broadcast_protocol_event(&out_tx, &event_sinks, event);

        let out_event = out_rx.try_recv().expect("stdout event");
        let sink_event = sink_rx.try_recv().expect("socket event");
        let out_update = expect_group_updated(out_event);
        let sink_update = expect_group_updated(sink_event);
        assert_eq!(out_update.kind, GroupUpdateKindOut::MembersAdded);
        assert_eq!(sink_update.kind, GroupUpdateKindOut::MembersAdded);
        assert_eq!(out_update.nostr_group_id, "aa");
        assert_eq!(sink_update.nostr_group_id, "aa");
    }

    #[test]
    fn daemon_get_group_profile_request_returns_latest_profile_snapshot() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon get group profile".to_string(),
            "Fallback description".to_string(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let profile = UnsignedEvent::new(
            inviter_keys.public_key(),
            Timestamp::from(10_u64),
            Kind::Metadata,
            Tags::new(),
            r#"{"display_name":"Latest Name","about":"Latest About","picture":"https://example.com/group.png"}"#,
        );
        let wrapper = inviter_mdk
            .create_message(&created.group.mls_group_id, profile)
            .expect("create group profile");
        inviter_mdk
            .process_message(&wrapper)
            .expect("process group profile");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);

        let reply = handle_get_group_profile_request(
            Some("req-get-profile".to_string()),
            &host,
            &inviter_keys.public_key(),
            &hex::encode(created.group.nostr_group_id),
        );

        let OutMsg::Ok {
            request_id,
            result: Some(result),
        } = reply
        else {
            panic!("expected successful get_group_profile reply");
        };
        assert_eq!(request_id.as_deref(), Some("req-get-profile"));
        let result: GroupProfileOut =
            serde_json::from_value(result).expect("deserialize get_group_profile result");
        assert_eq!(
            result.nostr_group_id,
            hex::encode(created.group.nostr_group_id)
        );
        assert_eq!(result.owner_pubkey, inviter_keys.public_key().to_hex());
        assert_eq!(result.name, "Latest Name");
        assert_eq!(result.about, "Latest About");
        assert_eq!(
            result.picture_url.as_deref(),
            Some("https://example.com/group.png")
        );
    }

    #[test]
    fn daemon_get_group_profile_request_rejects_unknown_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        let keys = Keys::generate();
        let signer: Arc<dyn NostrSigner> = Arc::new(keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let mdk = crate::open_mdk(dir.path()).expect("open mdk");
        let host = test_host(&mdk, &keys, &client, &relay_urls);

        let reply = handle_get_group_profile_request(
            Some("req-get-missing".to_string()),
            &host,
            &keys.public_key(),
            "deadbeef",
        );

        let OutMsg::Error {
            request_id,
            code,
            message,
        } = reply
        else {
            panic!("expected get_group_profile error reply");
        };
        assert_eq!(request_id.as_deref(), Some("req-get-missing"));
        assert_eq!(code, "bad_group_id");
        assert!(message.contains("deadbeef") || message.contains("group"));
    }

    #[tokio::test]
    async fn daemon_upload_group_profile_image_request_succeeds_on_production_handler() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon image profile".to_string(),
            "Fallback about".to_string(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let existing_profile = UnsignedEvent::new(
            inviter_keys.public_key(),
            Timestamp::from(10_u64),
            Kind::Metadata,
            Tags::new(),
            r#"{"display_name":"Profile Name","about":"Profile About"}"#,
        );
        let existing_wrapper = inviter_mdk
            .create_message(&created.group.mls_group_id, existing_profile)
            .expect("create existing group profile");
        inviter_mdk
            .process_message(&existing_wrapper)
            .expect("process existing group profile");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let nostr_group_id_hex = hex::encode(created.group.nostr_group_id);
        let uploaded_url = "https://blossom.example.com/group-profile.jpg";
        let image_base64 = base64::engine::general_purpose::STANDARD
            .encode(include_bytes!("../../../fixtures/test-images/red.jpg"));

        let reply = handle_upload_group_profile_image_request_with(
            Some("req-upload-profile".to_string()),
            &host,
            &inviter_keys.public_key(),
            GroupProfileImageUploadInput {
                nostr_group_id: &nostr_group_id_hex,
                image_base64: &image_base64,
                mime_type: "image/jpeg",
            },
            |_encrypted_data, upload_mime_type, expected_hash_hex| async move {
                assert_eq!(upload_mime_type, "image/jpeg");
                Ok(UploadedBlob {
                    blossom_server: "https://blossom.example.com".to_string(),
                    uploaded_url: uploaded_url.to_string(),
                    descriptor_sha256_hex: expected_hash_hex,
                })
            },
            |prepared| {
                let mdk = &inviter_mdk;
                async move {
                    let processed = mdk
                        .process_message(&prepared.wrapper)
                        .expect("process prepared profile image");
                    match processed {
                        MessageProcessingResult::ApplicationMessage(message) => {
                            let metadata: Metadata =
                                serde_json::from_str(&message.content).expect("parse metadata");
                            assert_eq!(message.kind, Kind::Metadata);
                            assert_eq!(metadata.display_name.as_deref(), Some("Profile Name"));
                            assert_eq!(metadata.about.as_deref(), Some("Profile About"));
                            assert_eq!(metadata.picture.as_deref(), Some(uploaded_url));
                            assert!(
                                message
                                    .tags
                                    .iter()
                                    .any(pika_marmot_runtime::media::is_imeta_tag),
                                "profile image update should publish an imeta tag"
                            );
                        }
                        other => panic!("expected application message, got {other:?}"),
                    }
                    Ok(EventId::all_zeros())
                }
            },
        )
        .await;

        let (request_id, result) = match reply {
            OutMsg::Ok {
                request_id,
                result: Some(result),
            } => (request_id, result),
            other => panic!("expected successful upload_group_profile_image reply, got {other:?}"),
        };
        assert_eq!(request_id.as_deref(), Some("req-upload-profile"));
        let result: GroupProfileOut =
            serde_json::from_value(result).expect("deserialize upload_group_profile_image result");
        assert_eq!(result.nostr_group_id, nostr_group_id_hex);
        assert_eq!(result.owner_pubkey, inviter_keys.public_key().to_hex());
        assert_eq!(result.name, "Profile Name");
        assert_eq!(result.about, "Profile About");
        assert_eq!(result.picture_url.as_deref(), Some(uploaded_url));
    }

    #[tokio::test]
    async fn daemon_upload_group_profile_image_request_rejects_invalid_mime_type() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon invalid profile image".to_string(),
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
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let nostr_group_id_hex = hex::encode(created.group.nostr_group_id);
        let image_base64 = base64::engine::general_purpose::STANDARD
            .encode(include_bytes!("../../../fixtures/test-images/red.jpg"));

        let reply = handle_upload_group_profile_image_request_with(
            Some("req-upload-bad-mime".to_string()),
            &host,
            &inviter_keys.public_key(),
            GroupProfileImageUploadInput {
                nostr_group_id: &nostr_group_id_hex,
                image_base64: &image_base64,
                mime_type: "text/plain",
            },
            |_encrypted_data, _upload_mime_type, _expected_hash_hex| async move {
                panic!("upload should not run for invalid mime type");
            },
            |_prepared| async move {
                panic!("publish should not run for invalid mime type");
            },
        )
        .await;

        let OutMsg::Error {
            request_id,
            code,
            message,
        } = reply
        else {
            panic!("expected upload_group_profile_image error reply");
        };
        assert_eq!(request_id.as_deref(), Some("req-upload-bad-mime"));
        assert_eq!(code, "bad_mime_type");
        assert!(message.contains("image/"));
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
    fn daemon_group_message_processing_uses_shared_runtime_helper() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let keys = Keys::generate();
        let signer: Arc<dyn NostrSigner> = Arc::new(keys.clone());
        let client = Client::new(signer);
        let relay_urls: Vec<RelayUrl> = Vec::new();
        let mdk = crate::open_mdk(tempdir.path()).expect("open mdk");
        let config = NostrGroupConfigData::new(
            "daemon inbound group message".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![keys.public_key()],
        );
        let created = mdk
            .create_group(&keys.public_key(), vec![], config)
            .expect("create group");
        mdk.merge_pending_commit(&created.group.mls_group_id)
            .expect("merge pending commit");
        let event = make_group_message_event(
            &mdk,
            &keys,
            &created.group.mls_group_id,
            Kind::ChatMessage,
            "hello through daemon helper",
            Tags::new(),
        );
        let host = test_host(&mdk, &keys, &client, &relay_urls);

        let processed = host
            .process_classified_inbound_group_message(InboundRelayEvent::GroupMessage {
                event: event.clone(),
            })
            .expect("process classified group message")
            .expect("group message processing result");

        assert_eq!(processed.event_id(), event.id);
        match processed.into_conversation_event() {
            Some(ConversationEvent::Application(message)) => {
                assert_eq!(message.classification, MessageClassification::Chat);
                assert_eq!(
                    message.nostr_group_id_hex,
                    hex::encode(created.group.nostr_group_id)
                );
                assert_eq!(message.message.content, "hello through daemon helper");
            }
            other => panic!("expected processed application message, got {other:?}"),
        }
    }

    #[test]
    fn daemon_runtime_application_message_uses_shared_interpreter_for_call_signals() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let keys = Keys::generate();
        let signer: Arc<dyn NostrSigner> = Arc::new(keys.clone());
        let client = Client::new(signer);
        let relay_urls: Vec<RelayUrl> = Vec::new();
        let mdk = crate::open_mdk(tempdir.path()).expect("open mdk");
        let content = serde_json::json!({
            "v": 1,
            "ns": "pika.call",
            "type": "call.invite",
            "call_id": "550e8400-e29b-41d4-a716-446655440000",
            "ts_ms": 1730000000000i64,
            "body": {
                "moq_url": "https://moq.local/anon",
                "broadcast_base": "pika/calls/550e8400-e29b-41d4-a716-446655440000",
                "relay_auth": "capv1_test_token",
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
        let msg = make_test_message(CALL_SIGNAL_KIND, &content, Tags::new());
        let runtime_msg = pika_marmot_runtime::conversation::RuntimeApplicationMessage {
            mls_group_id: msg.mls_group_id.clone(),
            nostr_group_id_hex: "deadbeef".to_string(),
            classification: MessageClassification::CallSignal,
            message: msg,
        };

        let interpreted = test_host(&mdk, &keys, &client, &relay_urls)
            .interpret_runtime_application_message(runtime_msg);

        match interpreted {
            RuntimeApplicationMessageInterpretation::CallSignal {
                parsed_signal: Some(ParsedCallSignal::Invite { call_id, .. }),
                ..
            } => assert_eq!(call_id, "550e8400-e29b-41d4-a716-446655440000"),
            other => panic!("expected shared call-signal interpretation, got {other:?}"),
        }
    }

    #[test]
    fn daemon_conversation_event_uses_shared_interpreter_for_group_update_and_refresh() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let keys = Keys::generate();
        let signer: Arc<dyn NostrSigner> = Arc::new(keys.clone());
        let client = Client::new(signer);
        let relay_urls: Vec<RelayUrl> = Vec::new();
        let mdk = crate::open_mdk(tempdir.path()).expect("open mdk");
        let host = test_host(&mdk, &keys, &client, &relay_urls);
        let group_id = GroupId::from_slice(&[7, 7, 7]);
        let commit = ConversationEvent::GroupUpdate(RuntimeGroupUpdate {
            mls_group_id: group_id.clone(),
            nostr_group_id_hex: "deadbeef".to_string(),
            kind: RuntimeGroupUpdateKind::Commit,
        });
        let unresolved = ConversationEvent::UnresolvedGroup {
            mls_group_id: group_id.clone(),
        };
        let failed = ConversationEvent::PreviouslyFailed;

        let interpreted_commit = host.interpret_conversation_event(commit);
        let interpreted_unresolved = host.interpret_conversation_event(unresolved);
        let interpreted_failed = host.interpret_conversation_event(failed);

        match interpreted_commit {
            RuntimeConversationEventInterpretation::GroupUpdate { is_commit, .. } => {
                assert!(is_commit)
            }
            other => panic!("expected group-update interpretation, got {other:?}"),
        }
        match interpreted_unresolved {
            RuntimeConversationEventInterpretation::NeedsFullRefresh {
                reason:
                    pika_marmot_runtime::runtime::RuntimeConversationRefreshReason::UnresolvedGroup {
                        mls_group_id,
                    },
            } => assert_eq!(mls_group_id, group_id),
            other => panic!("expected unresolved-group refresh reason, got {other:?}"),
        }
        assert!(matches!(
            interpreted_failed,
            RuntimeConversationEventInterpretation::NeedsFullRefresh {
                reason:
                    pika_marmot_runtime::runtime::RuntimeConversationRefreshReason::PreviouslyFailed
            }
        ));
    }

    #[test]
    fn daemon_subscription_planning_uses_shared_runtime_targets() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let created = inviter_mdk
            .create_group(
                &inviter_keys.public_key(),
                vec![invitee_kp],
                NostrGroupConfigData::new(
                    "Daemon subscription planning".to_string(),
                    String::new(),
                    None,
                    None,
                    None,
                    relay_urls.clone(),
                    vec![inviter_keys.public_key(), invitee_keys.public_key()],
                ),
            )
            .expect("create group");
        let expected_group_id = hex::encode(created.group.nostr_group_id);
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);

        let plan = plan_daemon_group_subscriptions(&host, vec!["stale-group".to_string()])
            .expect("plan daemon group subscriptions");

        assert_eq!(
            plan.current.target_group_ids,
            vec![expected_group_id.clone()]
        );
        assert_eq!(plan.current.relay_urls, relay_urls);
        assert_eq!(plan.added_group_ids, vec![expected_group_id]);
        assert_eq!(plan.removed_group_ids, vec!["stale-group".to_string()]);
    }

    #[test]
    fn daemon_session_sync_planning_uses_shared_runtime_sync_plan() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://message-1.example").expect("relay url")];
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let created = inviter_mdk
            .create_group(
                &inviter_keys.public_key(),
                vec![invitee_kp],
                NostrGroupConfigData::new(
                    "Daemon session sync planning".to_string(),
                    String::new(),
                    None,
                    None,
                    None,
                    vec![RelayUrl::parse("wss://group-1.example").expect("group relay")],
                    vec![inviter_keys.public_key(), invitee_keys.public_key()],
                ),
            )
            .expect("create group");
        let expected_group_id = hex::encode(created.group.nostr_group_id);
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);

        let sync_plan = host
            .refresh_session_state(vec!["stale-group".to_string()], 90)
            .expect("refresh daemon session state")
            .sync_plan;

        assert_eq!(
            sync_plan.group_subscriptions.current.target_group_ids,
            vec![expected_group_id.clone()]
        );
        assert_eq!(
            sync_plan.group_subscriptions.added_group_ids,
            vec![expected_group_id]
        );
        assert_eq!(
            sync_plan.group_subscriptions.removed_group_ids,
            vec!["stale-group".to_string()]
        );
        assert_eq!(
            sync_plan.relay_roles.session_connect_relays,
            vec![
                RelayUrl::parse("wss://group-1.example").expect("group relay"),
                RelayUrl::parse("wss://message-1.example").expect("relay url"),
            ]
        );
        assert_eq!(sync_plan.welcome_inbox, daemon_welcome_inbox_intent(90));
    }

    #[tokio::test]
    async fn daemon_base_session_sync_uses_shared_runtime_executor() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        inviter_mdk
            .create_group(
                &inviter_keys.public_key(),
                vec![invitee_kp],
                NostrGroupConfigData::new(
                    "Daemon base session sync".to_string(),
                    String::new(),
                    None,
                    None,
                    None,
                    vec![RelayUrl::parse("wss://group-1.example").expect("group relay")],
                    vec![inviter_keys.public_key(), invitee_keys.public_key()],
                ),
            )
            .expect("create group");

        let bootstrapped = bootstrap_runtime_for_daemon(
            inviter_dir.path(),
            &inviter_keys,
            vec![RelayUrl::parse("wss://message-1.example").expect("message relay")],
            90,
        )
        .expect("bootstrap daemon runtime");
        let primary_relay_url = RelayUrl::parse("wss://message-1.example").expect("message relay");

        let execution = execute_daemon_base_session_sync(
            &bootstrapped.session,
            &bootstrapped.open.sync_plan,
            &primary_relay_url,
        )
        .await
        .expect("execute daemon base session sync");

        assert!(
            !execution.welcome_inbox_sub.as_str().is_empty(),
            "daemon startup should use the shared base session sync executor"
        );
    }

    #[test]
    fn daemon_runtime_refresh_uses_shared_query_boundary() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://message-1.example").expect("relay url")];
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let created = inviter_mdk
            .create_group(
                &inviter_keys.public_key(),
                vec![invitee_kp],
                NostrGroupConfigData::new(
                    "Daemon runtime refresh".to_string(),
                    String::new(),
                    None,
                    None,
                    None,
                    vec![RelayUrl::parse("wss://group-1.example").expect("group relay")],
                    vec![inviter_keys.public_key(), invitee_keys.public_key()],
                ),
            )
            .expect("create group");
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);

        let refreshed = host
            .refresh_session_state(vec!["stale-group".to_string()], 90)
            .expect("refresh daemon session state");

        assert_eq!(refreshed.joined_group_snapshots.len(), 1);
        assert!(refreshed.pending_welcome_snapshots.is_empty());
        assert_eq!(
            refreshed.current_group_subscriptions().target_group_ids,
            vec![hex::encode(created.group.nostr_group_id)]
        );
        assert_eq!(
            refreshed.sync_plan.group_subscriptions.removed_group_ids,
            vec!["stale-group".to_string()]
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

        let bootstrapped = bootstrap_runtime_for_daemon(
            inviter_dir.path(),
            &inviter_keys,
            vec![RelayUrl::parse("wss://message-1.example").expect("message relay")],
            90,
        )
        .expect("bootstrap");

        assert_eq!(bootstrapped.session.pubkey, inviter_keys.public_key());
        assert_eq!(bootstrapped.open.pubkey, inviter_keys.public_key());
        assert_eq!(
            bootstrapped.open.joined_group_snapshots.len(),
            1,
            "daemon bootstrap should surface joined groups through shared open state"
        );
        assert_eq!(
            bootstrapped
                .open
                .sync_plan
                .relay_roles
                .session_connect_relays,
            vec![
                RelayUrl::parse("wss://message-1.example").expect("message relay"),
                RelayUrl::parse("wss://test.relay").expect("relay url"),
            ]
        );
        assert_eq!(
            bootstrapped.open.sync_plan.welcome_inbox,
            daemon_welcome_inbox_intent(90)
        );
        assert_eq!(
            bootstrapped
                .open
                .current_group_subscriptions()
                .target_group_ids,
            vec![hex::encode(created.group.nostr_group_id)]
        );
        assert_eq!(
            bootstrapped.open.current_group_subscriptions().relay_urls,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")]
        );
        assert!(bootstrapped.open.seed_seen_welcomes().is_empty());
        assert!(bootstrapped.open.seed_seen_group_events().is_empty());
    }

    #[tokio::test]
    async fn init_group_uses_shared_group_create_and_publish_helper() {
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

    #[tokio::test]
    async fn init_group_follow_through_refreshes_shared_subscription_targets() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Daemon init_group refresh".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );

        let created = create_group_and_publish_welcomes_for_init_group(
            &inviter_keys,
            &inviter_mdk,
            invitee_kp,
            invitee_keys.public_key(),
            config,
            |_receiver, _giftwrap| async { Ok(()) },
        )
        .await
        .expect("init group create/publish");

        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let refreshed = host
            .refresh_session_state(Vec::new(), 90)
            .expect("refresh daemon session state");
        let created_group_id = hex::encode(created.group.nostr_group_id);

        assert_eq!(
            refreshed.current_group_subscriptions().target_group_ids,
            vec![created_group_id.clone()]
        );
        assert_eq!(
            refreshed.sync_plan.group_subscriptions.added_group_ids,
            vec![created_group_id]
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
    fn daemon_prepare_call_invite_uses_shared_command_boundary() {
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
    fn daemon_prepare_accept_call_uses_shared_command_boundary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mdk = crate::open_mdk(dir.path()).expect("open mdk");
        let keys = Keys::generate();
        let peer = Keys::generate();
        let created = mdk
            .create_group(
                &keys.public_key(),
                vec![],
                NostrGroupConfigData::new(
                    "Daemon call accept test".to_string(),
                    String::new(),
                    None,
                    None,
                    None,
                    vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
                    vec![keys.public_key()],
                ),
            )
            .expect("create group");
        mdk.merge_pending_commit(&created.group.mls_group_id)
            .expect("merge pending commit");

        let signer: Arc<dyn NostrSigner> = Arc::new(keys.clone());
        let client = Client::new(signer);
        let relay_urls = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let host = test_host(&mdk, &keys, &client, &relay_urls);
        let chat_id = hex::encode(created.group.nostr_group_id);
        let call_id = "550e8400-e29b-41d4-a716-446655440011";
        let peer_pubkey_hex = peer.public_key().to_hex();
        let mut session = default_audio_call_session(call_id);
        session.relay_auth = host
            .derive_relay_auth_token(&chat_id, call_id, &session, &peer_pubkey_hex)
            .expect("derive relay auth");

        let prepared = host
            .prepare_accept_call(&PendingIncomingCall {
                call_id: call_id.to_string(),
                target_id: chat_id,
                from_pubkey_hex: peer_pubkey_hex,
                session,
                is_video_call: false,
            })
            .expect("prepare daemon call accept");

        assert_eq!(prepared.incoming.call_id, call_id);
        assert!(prepared.signal.payload_json.contains("call.accept"));
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
    fn daemon_media_upload_commands_use_shared_command_boundary() {
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
        let signer: Arc<dyn NostrSigner> = Arc::new(inviter_keys.clone());
        let client = Client::new(signer);
        let relay_urls: Vec<RelayUrl> = Vec::new();
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let prepared = host
            .prepare_upload(
                &created.group.mls_group_id,
                b"daemon attachment",
                Some("text/plain"),
                Some("daemon.txt"),
            )
            .expect("prepare upload");
        let completed = host
            .complete_media_upload_operation(
                &created.group.mls_group_id,
                hex::encode(created.group.nostr_group_id),
                &prepared.upload,
                MediaUploadStatus::Uploaded(pika_marmot_runtime::media::UploadedBlob {
                    blossom_server: "https://example.com".to_string(),
                    uploaded_url: "https://example.com/blob".to_string(),
                    descriptor_sha256_hex: hex::encode(prepared.upload.encrypted_hash),
                }),
            )
            .into_media_upload_result()
            .expect("completed media upload");
        let message = make_test_message(
            Kind::ChatMessage,
            "hi",
            Tags::from_list(vec![completed.result.imeta_tag]),
        );

        let attachments = host.parse_message_media_attachments(&message);

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].attachment.filename, "daemon.txt");
        assert_eq!(attachments[0].attachment.mime_type, "text/plain");
    }

    #[test]
    fn daemon_media_batch_follow_through_uses_shared_completed_upload_state() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "daemon media batch".to_string(),
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
        let relay_urls: Vec<RelayUrl> = Vec::new();
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let first = host
            .prepare_upload(
                &created.group.mls_group_id,
                b"daemon first attachment",
                Some("text/plain"),
                Some("first.txt"),
            )
            .expect("prepare first upload");
        let second = host
            .prepare_upload(
                &created.group.mls_group_id,
                b"daemon second attachment",
                Some("text/plain"),
                Some("second.txt"),
            )
            .expect("prepare second upload");
        let completed_uploads = vec![
            host.complete_media_upload_operation(
                &created.group.mls_group_id,
                hex::encode(created.group.nostr_group_id),
                &first.upload,
                MediaUploadStatus::Uploaded(pika_marmot_runtime::media::UploadedBlob {
                    blossom_server: "https://example.com".to_string(),
                    uploaded_url: "https://example.com/blob/1".to_string(),
                    descriptor_sha256_hex: hex::encode(first.upload.encrypted_hash),
                }),
            )
            .into_media_upload_result()
            .expect("completed first upload"),
            host.complete_media_upload_operation(
                &created.group.mls_group_id,
                hex::encode(created.group.nostr_group_id),
                &second.upload,
                MediaUploadStatus::Uploaded(pika_marmot_runtime::media::UploadedBlob {
                    blossom_server: "https://example.com".to_string(),
                    uploaded_url: "https://example.com/blob/2".to_string(),
                    descriptor_sha256_hex: hex::encode(second.upload.encrypted_hash),
                }),
            )
            .into_media_upload_result()
            .expect("completed second upload"),
        ];
        let batch_fields = batch_media_fields_from_completed_uploads(&completed_uploads);
        let message = make_test_message(
            Kind::ChatMessage,
            "hi",
            Tags::from_list(batch_fields.imeta_tags.clone()),
        );

        let attachments = host.parse_message_media_attachments(&message);

        assert_eq!(
            batch_fields.uploaded_urls,
            vec![
                "https://example.com/blob/1".to_string(),
                "https://example.com/blob/2".to_string(),
            ]
        );
        assert_eq!(batch_fields.original_hashes.len(), 2);
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].attachment.filename, "first.txt");
        assert_eq!(attachments[1].attachment.filename, "second.txt");
    }

    #[test]
    fn daemon_media_upload_operation_result_uses_shared_runtime_event_boundary() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "daemon media op".to_string(),
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
        let relay_urls: Vec<RelayUrl> = Vec::new();
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let prepared = host
            .prepare_upload(
                &created.group.mls_group_id,
                b"daemon media op",
                Some("text/plain"),
                Some("daemon-op.txt"),
            )
            .expect("prepare upload");

        let operation = host.complete_media_upload_operation(
            &created.group.mls_group_id,
            hex::encode(created.group.nostr_group_id),
            &prepared.upload,
            pika_marmot_runtime::runtime::MediaUploadStatus::Uploaded(
                pika_marmot_runtime::media::UploadedBlob {
                    blossom_server: "https://example.com".to_string(),
                    uploaded_url: "https://example.com/blob".to_string(),
                    descriptor_sha256_hex: hex::encode(prepared.upload.encrypted_hash),
                },
            ),
        );
        let operation_id = operation.operation_id();
        let completed = operation
            .into_media_upload_result()
            .expect("completed media upload");

        assert_eq!(
            operation_id,
            EventId::from_byte_array(prepared.upload.encrypted_hash)
        );
        assert_eq!(completed.result.attachment.filename, "daemon-op.txt");
        assert_eq!(completed.result.attachment.mime_type, "text/plain");
    }

    #[tokio::test]
    async fn daemon_media_workflow_failure_uses_shared_upload_result_path() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let upload_dir = tempfile::tempdir().expect("upload tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = crate::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = crate::open_mdk(invitee_dir.path()).expect("open invitee mdk");
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "daemon media failure".to_string(),
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
        let relay_urls: Vec<RelayUrl> = Vec::new();
        let host = test_host(&inviter_mdk, &inviter_keys, &client, &relay_urls);
        let file_path = upload_dir.path().join("daemon-media.txt");
        std::fs::write(&file_path, b"daemon media workflow").expect("write media file");

        let error = upload_daemon_media_file(
            &host,
            &inviter_keys,
            &created.group.mls_group_id,
            &[],
            DaemonMediaUploadInput {
                nostr_group_id: &hex::encode(created.group.nostr_group_id),
                file_path: file_path.to_str().expect("file path"),
                mime_type: Some("text/plain"),
                filename: Some("daemon-media.txt"),
                include_path_in_validation_errors: false,
                require_uploaded_url: false,
            },
        )
        .await
        .expect_err("upload should fail without blossom servers");

        assert_eq!(error.code, "upload_failed");
        assert!(
            error
                .message
                .contains("no valid Blossom servers configured"),
            "real daemon media send flow should surface shared upload failure details"
        );
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
}
