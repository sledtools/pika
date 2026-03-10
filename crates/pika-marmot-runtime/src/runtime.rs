use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use mdk_core::prelude::{GroupId, MessageProcessingResult};
use mdk_storage_traits::groups::{Pagination, types::Group};
use mdk_storage_traits::messages::types::Message;
use nostr_sdk::prelude::*;

use crate::PikaMdk;
use crate::call::{CallSessionParams, ParsedCallSignal, parse_call_signal};
use crate::call_runtime::{
    CallWorkflowRuntime, GroupCallContext, InboundCallSignalOutcome, InboundSignalContext,
    PendingIncomingCall, PreparedAcceptedCall, PreparedCallSignal,
};
use crate::conversation::{
    ConversationEvent, ConversationRuntime, RuntimeApplicationMessage, RuntimeGroupSummary,
};
use crate::media::{
    MediaRuntime, ParsedMediaAttachment, PreparedMediaUpload, RuntimeDownloadedMedia,
    RuntimeMediaUploadResult,
};
use crate::membership::{MembershipRuntime, MembershipUpdateResult, PreparedMembershipEvolution};
use crate::outbound::{
    OutboundConversationAction, OutboundConversationRuntime, PreparedConversationAction,
    PublishedConversationAction, ResolvedConversationTarget,
};
use crate::relay::subscribe_group_msgs;

pub const DEFAULT_INBOUND_RELAY_SEEN_CAP: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundRelayEvent {
    Welcome {
        wrapper: Event,
        sender: PublicKey,
        rumor: UnsignedEvent,
    },
    GroupMessage {
        event: Event,
    },
}

#[derive(Debug, Clone)]
pub enum InboundGroupMessageProcessing {
    Ignored {
        event_id: EventId,
    },
    Processed {
        event_id: EventId,
        conversation_event: ConversationEvent,
    },
}

impl InboundGroupMessageProcessing {
    pub fn event_id(&self) -> EventId {
        match self {
            Self::Ignored { event_id } | Self::Processed { event_id, .. } => *event_id,
        }
    }

    pub fn into_conversation_event(self) -> Option<ConversationEvent> {
        match self {
            Self::Ignored { .. } => None,
            Self::Processed {
                conversation_event, ..
            } => Some(conversation_event),
        }
    }
}

#[derive(Debug, Clone)]
pub enum RuntimeApplicationMessageInterpretation {
    TypingIndicator {
        message: RuntimeApplicationMessage,
    },
    CallSignal {
        message: RuntimeApplicationMessage,
        parsed_signal: Option<ParsedCallSignal>,
    },
    Content {
        message: RuntimeApplicationMessage,
    },
    GroupProfile {
        message: RuntimeApplicationMessage,
    },
}

impl RuntimeApplicationMessageInterpretation {
    pub fn message(&self) -> &RuntimeApplicationMessage {
        match self {
            Self::TypingIndicator { message }
            | Self::CallSignal { message, .. }
            | Self::Content { message }
            | Self::GroupProfile { message } => message,
        }
    }
}

#[derive(Debug, Clone)]
pub enum RuntimeConversationRefreshReason {
    UnresolvedGroup { mls_group_id: GroupId },
    PreviouslyFailed,
}

#[derive(Debug, Clone)]
pub enum RuntimeConversationEventInterpretation {
    Application {
        message: Box<RuntimeApplicationMessage>,
    },
    GroupUpdate {
        update: crate::conversation::RuntimeGroupUpdate,
        is_commit: bool,
    },
    NeedsFullRefresh {
        reason: RuntimeConversationRefreshReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundRelaySeenCache {
    seen: HashSet<EventId>,
    order: VecDeque<EventId>,
    cap: Option<usize>,
}

impl Default for InboundRelaySeenCache {
    fn default() -> Self {
        Self::bounded(DEFAULT_INBOUND_RELAY_SEEN_CAP)
    }
}

impl InboundRelaySeenCache {
    pub fn bounded(cap: usize) -> Self {
        Self {
            seen: HashSet::new(),
            order: VecDeque::new(),
            cap: Some(cap),
        }
    }

    pub fn unbounded() -> Self {
        Self {
            seen: HashSet::new(),
            order: VecDeque::new(),
            cap: None,
        }
    }

    pub fn extend<I>(&mut self, ids: I)
    where
        I: IntoIterator<Item = EventId>,
    {
        for id in ids {
            let _ = self.record(id);
        }
    }

    pub fn record(&mut self, id: EventId) -> bool {
        if !self.seen.insert(id) {
            return false;
        }
        self.order.push_back(id);
        if let Some(cap) = self.cap {
            while self.order.len() > cap {
                if let Some(old) = self.order.pop_front() {
                    self.seen.remove(&old);
                }
            }
        }
        true
    }
}

pub async fn classify_inbound_relay_event(
    client: &Client,
    seen: &mut InboundRelaySeenCache,
    event: Event,
) -> Result<Option<InboundRelayEvent>> {
    if !seen.record(event.id) {
        return Ok(None);
    }

    match event.kind {
        Kind::GiftWrap => {
            let unwrapped = client
                .unwrap_gift_wrap(&event)
                .await
                .context("unwrap giftwrap rumor")?;
            if unwrapped.rumor.kind != Kind::MlsWelcome {
                return Ok(None);
            }
            Ok(Some(InboundRelayEvent::Welcome {
                wrapper: event,
                sender: unwrapped.sender,
                rumor: unwrapped.rumor,
            }))
        }
        Kind::MlsGroupMessage => Ok(Some(InboundRelayEvent::GroupMessage { event })),
        _ => Ok(None),
    }
}

pub struct RuntimeSession {
    pub pubkey: PublicKey,
    pub client: Client,
    pub mdk: PikaMdk,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuntimeGroupSubscriptionState {
    pub target_group_ids: Vec<String>,
    pub relay_urls: Vec<RelayUrl>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuntimeGroupSubscriptionPlan {
    pub current: RuntimeGroupSubscriptionState,
    pub added_group_ids: Vec<String>,
    pub removed_group_ids: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuntimeRelayRolePlan {
    pub long_lived_session_relays: Vec<RelayUrl>,
    pub active_group_relays: Vec<RelayUrl>,
    pub temporary_key_package_relays: Vec<RelayUrl>,
    pub session_connect_relays: Vec<RelayUrl>,
    pub key_package_operation_relays: Vec<RelayUrl>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuntimeWelcomeInboxSubscriptionIntent {
    pub lookback: Option<Duration>,
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuntimeSessionSyncPlan {
    pub relay_roles: RuntimeRelayRolePlan,
    pub welcome_inbox: RuntimeWelcomeInboxSubscriptionIntent,
    pub group_subscriptions: RuntimeGroupSubscriptionPlan,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuntimeStartupState {
    pub group_subscriptions: RuntimeGroupSubscriptionState,
    pub seen_welcomes: HashSet<EventId>,
    pub seen_group_events: HashSet<EventId>,
}

pub struct BootstrappedRuntimeSession {
    pub session: RuntimeSession,
    pub startup: RuntimeStartupState,
}

pub struct MarmotRuntime<'a> {
    mdk: &'a PikaMdk,
    client: Option<&'a Client>,
}

impl RuntimeSession {
    pub fn runtime(&self) -> MarmotRuntime<'_> {
        MarmotRuntime::with_client(&self.mdk, &self.client)
    }

    pub async fn connect_relays(
        &self,
        relays: &[RelayUrl],
        reconnect: bool,
        wait_timeout: Option<Duration>,
    ) {
        connect_runtime_relays(&self.client, relays, reconnect, wait_timeout).await;
    }

    pub async fn subscribe_welcome_inbox(
        &self,
        lookback: Option<Duration>,
        limit: Option<usize>,
    ) -> Result<SubscriptionId> {
        subscribe_welcome_inbox(&self.client, self.pubkey, lookback, limit).await
    }

    pub async fn subscribe_group_messages_combined(
        &self,
        nostr_group_ids: &[String],
    ) -> Result<Option<SubscriptionId>> {
        subscribe_group_messages_combined(&self.client, nostr_group_ids).await
    }

    pub async fn subscribe_existing_groups_individual(
        &self,
    ) -> Result<HashMap<SubscriptionId, String>> {
        let state = group_subscription_state_from_mdk(&self.mdk)?;
        subscribe_group_messages_individual(&self.client, &state.target_group_ids).await
    }

    pub fn existing_group_ids(&self) -> Result<Vec<String>> {
        Ok(group_subscription_state_from_mdk(&self.mdk)?.target_group_ids)
    }

    pub fn group_subscription_state(&self) -> Result<RuntimeGroupSubscriptionState> {
        group_subscription_state_from_mdk(&self.mdk)
    }

    pub fn plan_group_subscriptions<I>(
        &self,
        subscribed_group_ids: I,
    ) -> Result<RuntimeGroupSubscriptionPlan>
    where
        I: IntoIterator<Item = String>,
    {
        plan_group_subscriptions_from_mdk(&self.mdk, subscribed_group_ids)
    }

    pub fn plan_session_sync<I, J, K>(
        &self,
        subscribed_group_ids: I,
        long_lived_session_relays: J,
        temporary_key_package_relays: K,
        welcome_inbox: RuntimeWelcomeInboxSubscriptionIntent,
    ) -> Result<RuntimeSessionSyncPlan>
    where
        I: IntoIterator<Item = String>,
        J: IntoIterator<Item = RelayUrl>,
        K: IntoIterator<Item = RelayUrl>,
    {
        plan_runtime_session_sync_from_mdk(
            &self.mdk,
            subscribed_group_ids,
            long_lived_session_relays,
            temporary_key_package_relays,
            welcome_inbox,
        )
    }
}

impl BootstrappedRuntimeSession {
    pub fn runtime(&self) -> MarmotRuntime<'_> {
        self.session.runtime()
    }
}

impl<'a> MarmotRuntime<'a> {
    pub fn new(mdk: &'a PikaMdk) -> Self {
        Self { mdk, client: None }
    }

    pub fn with_client(mdk: &'a PikaMdk, client: &'a Client) -> Self {
        Self {
            mdk,
            client: Some(client),
        }
    }

    pub fn from_session(session: &'a RuntimeSession) -> Self {
        session.runtime()
    }

    pub fn mdk(&self) -> &'a PikaMdk {
        self.mdk
    }

    pub fn client(&self) -> Option<&'a Client> {
        self.client
    }

    pub fn conversation(&self) -> ConversationRuntime<'a> {
        ConversationRuntime::new(self.mdk)
    }

    pub fn outbound(&self) -> OutboundConversationRuntime<'a> {
        OutboundConversationRuntime::new(self.mdk)
    }

    pub fn media(&self) -> MediaRuntime<'a> {
        MediaRuntime::new(self.mdk)
    }

    pub fn membership(&self) -> MembershipRuntime<'a> {
        MembershipRuntime::new(self.mdk)
    }

    pub fn calls(&self) -> CallWorkflowRuntime<'a> {
        CallWorkflowRuntime::new(self.mdk)
    }

    pub fn process_event(&self, event: &Event) -> Result<Option<ConversationEvent>> {
        self.conversation().process_event(event)
    }

    pub fn process_group_message_event(
        &self,
        event: Event,
    ) -> Result<InboundGroupMessageProcessing> {
        let event_id = event.id;
        match self.conversation().process_event(&event)? {
            Some(conversation_event) => Ok(InboundGroupMessageProcessing::Processed {
                event_id,
                conversation_event,
            }),
            None => Ok(InboundGroupMessageProcessing::Ignored { event_id }),
        }
    }

    pub fn process_classified_inbound_group_message(
        &self,
        inbound: InboundRelayEvent,
    ) -> Result<Option<InboundGroupMessageProcessing>> {
        let InboundRelayEvent::GroupMessage { event } = inbound else {
            return Ok(None);
        };
        self.process_group_message_event(event).map(Some)
    }

    pub fn interpret_runtime_application_message(
        &self,
        runtime_msg: RuntimeApplicationMessage,
    ) -> RuntimeApplicationMessageInterpretation {
        match runtime_msg.classification {
            crate::message::MessageClassification::TypingIndicator => {
                RuntimeApplicationMessageInterpretation::TypingIndicator {
                    message: runtime_msg,
                }
            }
            crate::message::MessageClassification::CallSignal => {
                RuntimeApplicationMessageInterpretation::CallSignal {
                    parsed_signal: parse_call_signal(&runtime_msg.message.content),
                    message: runtime_msg,
                }
            }
            crate::message::MessageClassification::Chat
            | crate::message::MessageClassification::Reaction
            | crate::message::MessageClassification::Hypernote
            | crate::message::MessageClassification::HypernoteResponse => {
                RuntimeApplicationMessageInterpretation::Content {
                    message: runtime_msg,
                }
            }
            crate::message::MessageClassification::GroupProfile => {
                RuntimeApplicationMessageInterpretation::GroupProfile {
                    message: runtime_msg,
                }
            }
        }
    }

    pub fn interpret_conversation_event(
        &self,
        event: ConversationEvent,
    ) -> RuntimeConversationEventInterpretation {
        match event {
            ConversationEvent::Application(message) => {
                RuntimeConversationEventInterpretation::Application { message }
            }
            ConversationEvent::GroupUpdate(update) => {
                let is_commit = matches!(
                    update.kind,
                    crate::conversation::RuntimeGroupUpdateKind::Commit
                );
                RuntimeConversationEventInterpretation::GroupUpdate { update, is_commit }
            }
            ConversationEvent::UnresolvedGroup { mls_group_id } => {
                RuntimeConversationEventInterpretation::NeedsFullRefresh {
                    reason: RuntimeConversationRefreshReason::UnresolvedGroup { mls_group_id },
                }
            }
            ConversationEvent::PreviouslyFailed => {
                RuntimeConversationEventInterpretation::NeedsFullRefresh {
                    reason: RuntimeConversationRefreshReason::PreviouslyFailed,
                }
            }
        }
    }

    pub fn group_subscription_state(&self) -> Result<RuntimeGroupSubscriptionState> {
        group_subscription_state_from_mdk(self.mdk)
    }

    pub fn plan_group_subscriptions<I>(
        &self,
        subscribed_group_ids: I,
    ) -> Result<RuntimeGroupSubscriptionPlan>
    where
        I: IntoIterator<Item = String>,
    {
        plan_group_subscriptions_from_mdk(self.mdk, subscribed_group_ids)
    }

    pub fn plan_session_sync<I, J, K>(
        &self,
        subscribed_group_ids: I,
        long_lived_session_relays: J,
        temporary_key_package_relays: K,
        welcome_inbox: RuntimeWelcomeInboxSubscriptionIntent,
    ) -> Result<RuntimeSessionSyncPlan>
    where
        I: IntoIterator<Item = String>,
        J: IntoIterator<Item = RelayUrl>,
        K: IntoIterator<Item = RelayUrl>,
    {
        plan_runtime_session_sync_from_mdk(
            self.mdk,
            subscribed_group_ids,
            long_lived_session_relays,
            temporary_key_package_relays,
            welcome_inbox,
        )
    }

    pub fn interpret_processing_result(
        &self,
        result: MessageProcessingResult,
    ) -> Option<ConversationEvent> {
        self.conversation().interpret_processing_result(result)
    }

    pub fn find_group(&self, nostr_group_id_hex: &str) -> Result<Group> {
        self.conversation().find_group(nostr_group_id_hex)
    }

    pub fn mls_group_id_for_nostr_group_id(&self, nostr_group_id_hex: &str) -> Result<GroupId> {
        self.conversation()
            .mls_group_id_for_nostr_group_id(nostr_group_id_hex)
    }

    pub fn list_groups(&self) -> Result<Vec<RuntimeGroupSummary>> {
        self.conversation().list_groups()
    }

    pub fn get_messages(
        &self,
        nostr_group_id_hex: &str,
        pagination: Option<Pagination>,
    ) -> Result<Vec<Message>> {
        self.conversation()
            .get_messages(nostr_group_id_hex, pagination)
    }

    pub async fn ingest_group_backlog(
        &self,
        relay_urls: &[RelayUrl],
        nostr_group_id_hex: &str,
        seen: &mut HashSet<EventId>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let client = self
            .client
            .context("runtime client not configured for backlog ingest")?;
        self.conversation()
            .ingest_backlog_messages(client, relay_urls, nostr_group_id_hex, seen, limit)
            .await
    }

    pub fn resolve_outbound_target(
        &self,
        nostr_group_id_hex: &str,
    ) -> Result<ResolvedConversationTarget> {
        self.outbound().resolve_target(nostr_group_id_hex)
    }

    pub fn prepare_outbound_action(
        &self,
        sender: PublicKey,
        nostr_group_id_hex: &str,
        action: OutboundConversationAction,
    ) -> Result<PreparedConversationAction> {
        self.outbound()
            .prepare_action(sender, nostr_group_id_hex, action)
    }

    pub fn prepare_outbound_action_for_group(
        &self,
        sender: PublicKey,
        group: Group,
        action: OutboundConversationAction,
    ) -> Result<PreparedConversationAction> {
        self.outbound()
            .prepare_action_for_group(sender, group, action)
    }

    pub fn prepare_outbound_action_for_group_ids(
        &self,
        sender: PublicKey,
        mls_group_id: GroupId,
        nostr_group_id_hex: String,
        action: OutboundConversationAction,
    ) -> Result<PreparedConversationAction> {
        self.outbound().prepare_action_for_group_ids(
            sender,
            mls_group_id,
            nostr_group_id_hex,
            action,
        )
    }

    pub fn prepare_outbound_action_for_target(
        &self,
        sender: PublicKey,
        target: ResolvedConversationTarget,
        action: OutboundConversationAction,
    ) -> Result<PreparedConversationAction> {
        self.outbound()
            .prepare_action_for_target(sender, target, action)
    }

    pub async fn publish_prepared_action(
        &self,
        relay_urls: &[RelayUrl],
        prepared: &PreparedConversationAction,
        label: &str,
    ) -> Result<PublishedConversationAction> {
        let client = self
            .client
            .context("runtime client not configured for publish")?;
        self.outbound()
            .publish_prepared_with_confirm(client, relay_urls, prepared, label)
            .await
    }

    pub fn prepare_upload(
        &self,
        mls_group_id: &GroupId,
        bytes: &[u8],
        mime_type: Option<&str>,
        filename: Option<&str>,
    ) -> Result<PreparedMediaUpload> {
        self.media()
            .prepare_upload(mls_group_id, bytes, mime_type, filename)
    }

    pub fn finish_upload(
        &self,
        mls_group_id: &GroupId,
        upload: &mdk_core::encrypted_media::types::EncryptedMediaUpload,
        uploaded_blob: crate::media::UploadedBlob,
    ) -> RuntimeMediaUploadResult {
        self.media()
            .finish_upload(mls_group_id, upload, uploaded_blob)
    }

    pub fn parse_message_attachments(&self, message: &Message) -> Vec<ParsedMediaAttachment> {
        self.media().parse_message_attachments(message)
    }

    pub async fn download_media(
        &self,
        mls_group_id: &GroupId,
        reference: &mdk_core::encrypted_media::types::MediaReference,
        expected_encrypted_hash_hex: Option<&str>,
    ) -> Result<RuntimeDownloadedMedia> {
        self.media()
            .download_media(mls_group_id, reference, expected_encrypted_hash_hex)
            .await
    }

    pub fn decrypt_downloaded_media(
        &self,
        mls_group_id: &GroupId,
        reference: &mdk_core::encrypted_media::types::MediaReference,
        encrypted_data: &[u8],
        expected_encrypted_hash_hex: Option<&str>,
    ) -> Result<RuntimeDownloadedMedia> {
        self.media().decrypt_downloaded_media(
            mls_group_id,
            reference,
            encrypted_data,
            expected_encrypted_hash_hex,
        )
    }

    pub fn prepare_add_members(
        &self,
        mls_group_id: &GroupId,
        key_package_events: &[Event],
    ) -> Result<PreparedMembershipEvolution> {
        self.membership()
            .prepare_add_members(mls_group_id, key_package_events)
    }

    pub fn prepare_evolution(
        &self,
        mls_group_id: GroupId,
        evolution_event: Event,
        welcome_rumors: Option<Vec<UnsignedEvent>>,
        added_pubkeys: Vec<PublicKey>,
    ) -> Result<PreparedMembershipEvolution> {
        self.membership().prepare_evolution(
            mls_group_id,
            evolution_event,
            welcome_rumors,
            added_pubkeys,
        )
    }

    pub fn finalize_published_evolution(
        &self,
        prepared: PreparedMembershipEvolution,
    ) -> MembershipUpdateResult {
        self.membership().finalize_published_evolution(prepared)
    }

    pub fn prepare_outgoing_call_invite(
        &self,
        target_id: &str,
        peer_pubkey_hex: &str,
        call_id: &str,
        session: &CallSessionParams,
    ) -> Result<
        (
            crate::call_runtime::PendingOutgoingCall,
            crate::call_runtime::PreparedCallSignal,
        ),
        String,
    > {
        self.calls()
            .prepare_outgoing_invite(target_id, peer_pubkey_hex, call_id, session)
    }

    pub fn prepare_accept_incoming_call(
        &self,
        incoming: &PendingIncomingCall,
        group: GroupCallContext<'_>,
    ) -> Result<PreparedAcceptedCall, String> {
        self.calls().prepare_accept_incoming(incoming, group)
    }

    pub fn prepare_reject_call_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<PreparedCallSignal, String> {
        self.calls().prepare_reject_signal(call_id, reason)
    }

    pub fn prepare_end_call_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<PreparedCallSignal, String> {
        self.calls().prepare_end_signal(call_id, reason)
    }

    pub fn handle_inbound_call_signal(
        &self,
        ctx: InboundSignalContext<'_>,
        signal: ParsedCallSignal,
    ) -> InboundCallSignalOutcome {
        self.calls().handle_inbound_signal(ctx, signal)
    }
}

pub async fn connect_runtime_relays(
    client: &Client,
    relays: &[RelayUrl],
    reconnect: bool,
    wait_timeout: Option<Duration>,
) {
    for relay in relays {
        let _ = client.add_relay(relay.clone()).await;
    }
    if reconnect {
        client.disconnect().await;
    }
    client.connect().await;
    if let Some(timeout) = wait_timeout {
        client.wait_for_connection(timeout).await;
    }
}

pub async fn temporary_client_from_session_signer(
    session_client: &Client,
    purpose: &str,
) -> Result<Client> {
    let signer = session_client
        .signer()
        .await
        .with_context(|| format!("{purpose} signer unavailable"))?;
    Ok(Client::new(signer))
}

pub async fn subscribe_welcome_inbox(
    client: &Client,
    pubkey: PublicKey,
    lookback: Option<Duration>,
    limit: Option<usize>,
) -> Result<SubscriptionId> {
    let mut filter = Filter::new().kind(Kind::GiftWrap).custom_tag(
        SingleLetterTag::lowercase(Alphabet::P),
        pubkey.to_hex().to_lowercase(),
    );
    if let Some(limit) = limit {
        filter = filter.limit(limit);
    }
    if let Some(lookback) = lookback {
        filter = filter.since(Timestamp::now() - lookback);
    }
    let out = client.subscribe(filter, None).await?;
    Ok(out.val)
}

pub async fn subscribe_group_messages_combined(
    client: &Client,
    nostr_group_ids: &[String],
) -> Result<Option<SubscriptionId>> {
    if nostr_group_ids.is_empty() {
        return Ok(None);
    }
    let filter = Filter::new().kind(Kind::MlsGroupMessage).custom_tags(
        SingleLetterTag::lowercase(Alphabet::H),
        nostr_group_ids.to_vec(),
    );
    let out = client.subscribe(filter, None).await?;
    Ok(Some(out.val))
}

pub async fn subscribe_existing_groups_individual(
    client: &Client,
    mdk: &PikaMdk,
) -> Result<HashMap<SubscriptionId, String>> {
    let state = group_subscription_state_from_mdk(mdk)?;
    subscribe_group_messages_individual(client, &state.target_group_ids).await
}

pub async fn subscribe_group_messages_individual(
    client: &Client,
    nostr_group_ids: &[String],
) -> Result<HashMap<SubscriptionId, String>> {
    let mut out = HashMap::new();
    for nostr_group_id_hex in nostr_group_ids {
        match subscribe_group_msgs(client, nostr_group_id_hex).await {
            Ok(subscription_id) => {
                out.insert(subscription_id, nostr_group_id_hex.clone());
            }
            Err(err) => {
                tracing::warn!(
                    nostr_group_id = nostr_group_id_hex,
                    error = %err,
                    "subscribe existing group failed"
                );
            }
        }
    }
    Ok(out)
}

pub fn group_subscription_state_from_mdk(mdk: &PikaMdk) -> Result<RuntimeGroupSubscriptionState> {
    let groups = mdk.get_groups()?;
    let mut target_group_ids = BTreeSet::new();
    let mut relay_urls = BTreeSet::new();
    for group in groups {
        target_group_ids.insert(hex::encode(group.nostr_group_id));
        if let Ok(group_relays) = mdk.get_relays(&group.mls_group_id) {
            relay_urls.extend(group_relays);
        }
    }
    Ok(RuntimeGroupSubscriptionState {
        target_group_ids: target_group_ids.into_iter().collect(),
        relay_urls: relay_urls.into_iter().collect(),
    })
}

pub fn plan_group_subscriptions_from_mdk<I>(
    mdk: &PikaMdk,
    subscribed_group_ids: I,
) -> Result<RuntimeGroupSubscriptionPlan>
where
    I: IntoIterator<Item = String>,
{
    let current = group_subscription_state_from_mdk(mdk)?;
    let subscribed_group_ids: BTreeSet<String> = subscribed_group_ids.into_iter().collect();
    let current_group_ids: BTreeSet<String> = current.target_group_ids.iter().cloned().collect();
    Ok(RuntimeGroupSubscriptionPlan {
        added_group_ids: current_group_ids
            .difference(&subscribed_group_ids)
            .cloned()
            .collect(),
        removed_group_ids: subscribed_group_ids
            .difference(&current_group_ids)
            .cloned()
            .collect(),
        current,
    })
}

pub fn plan_runtime_relay_roles<I, J, K>(
    long_lived_session_relays: I,
    active_group_relays: J,
    temporary_key_package_relays: K,
) -> RuntimeRelayRolePlan
where
    I: IntoIterator<Item = RelayUrl>,
    J: IntoIterator<Item = RelayUrl>,
    K: IntoIterator<Item = RelayUrl>,
{
    let long_lived_session_relays: BTreeSet<RelayUrl> =
        long_lived_session_relays.into_iter().collect();
    let active_group_relays: BTreeSet<RelayUrl> = active_group_relays.into_iter().collect();
    let temporary_key_package_relays: BTreeSet<RelayUrl> =
        temporary_key_package_relays.into_iter().collect();

    let mut session_connect_relays = long_lived_session_relays.clone();
    session_connect_relays.extend(active_group_relays.iter().cloned());

    let mut key_package_operation_relays = long_lived_session_relays.clone();
    key_package_operation_relays.extend(temporary_key_package_relays.iter().cloned());

    RuntimeRelayRolePlan {
        long_lived_session_relays: long_lived_session_relays.into_iter().collect(),
        active_group_relays: active_group_relays.into_iter().collect(),
        temporary_key_package_relays: temporary_key_package_relays.into_iter().collect(),
        session_connect_relays: session_connect_relays.into_iter().collect(),
        key_package_operation_relays: key_package_operation_relays.into_iter().collect(),
    }
}

pub fn plan_runtime_session_sync_from_mdk<I, J, K>(
    mdk: &PikaMdk,
    subscribed_group_ids: I,
    long_lived_session_relays: J,
    temporary_key_package_relays: K,
    welcome_inbox: RuntimeWelcomeInboxSubscriptionIntent,
) -> Result<RuntimeSessionSyncPlan>
where
    I: IntoIterator<Item = String>,
    J: IntoIterator<Item = RelayUrl>,
    K: IntoIterator<Item = RelayUrl>,
{
    let group_subscriptions = plan_group_subscriptions_from_mdk(mdk, subscribed_group_ids)?;
    let relay_roles = plan_runtime_relay_roles(
        long_lived_session_relays,
        group_subscriptions.current.relay_urls.clone(),
        temporary_key_package_relays,
    );
    Ok(RuntimeSessionSyncPlan {
        relay_roles,
        welcome_inbox,
        group_subscriptions,
    })
}

pub fn bootstrap_runtime_session<F>(
    pubkey: PublicKey,
    signer: Arc<dyn NostrSigner>,
    open_mdk: F,
) -> Result<BootstrappedRuntimeSession>
where
    F: FnOnce() -> Result<PikaMdk>,
{
    let mdk = open_mdk()?;
    let client = Client::new(signer);
    let session = RuntimeSession {
        pubkey,
        client,
        mdk,
    };
    let startup = RuntimeStartupState {
        group_subscriptions: group_subscription_state_from_mdk(&session.mdk)?,
        seen_welcomes: HashSet::new(),
        seen_group_events: HashSet::new(),
    };
    Ok(BootstrappedRuntimeSession { session, startup })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::{CallTrackSpec, OutgoingCallSignal, build_call_signal_json};
    use crate::conversation::RuntimeApplicationMessage;
    use mdk_core::prelude::NostrGroupConfigData;

    fn open_test_mdk(dir: &tempfile::TempDir) -> PikaMdk {
        crate::open_mdk(dir.path()).expect("open test mdk")
    }

    fn make_key_package_event(mdk: &PikaMdk, keys: &Keys) -> Event {
        let relay = RelayUrl::parse("wss://test.relay").expect("relay url");
        let (content, tags, _hash_ref) = mdk
            .create_key_package_for_event(&keys.public_key(), vec![relay])
            .expect("create key package");
        EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign key package")
    }

    fn make_group_message_event(
        mdk: &PikaMdk,
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

    fn make_runtime_application_message(
        classification: crate::message::MessageClassification,
        kind: Kind,
        content: &str,
    ) -> RuntimeApplicationMessage {
        let created_at = Timestamp::from(123_u64);
        let pubkey = Keys::generate().public_key();
        let tags = Tags::new();
        let mls_group_id = GroupId::from_slice(&[1, 2, 3]);
        RuntimeApplicationMessage {
            mls_group_id: mls_group_id.clone(),
            nostr_group_id_hex: "deadbeef".to_string(),
            classification,
            message: mdk_storage_traits::messages::types::Message {
                id: EventId::all_zeros(),
                mls_group_id,
                pubkey,
                kind,
                created_at,
                processed_at: created_at,
                content: content.to_string(),
                tags: tags.clone(),
                event: UnsignedEvent::new(pubkey, created_at, kind, tags, content.to_string()),
                wrapper_event_id: EventId::all_zeros(),
                epoch: None,
                state: mdk_core::prelude::message_types::MessageState::Processed,
            },
        }
    }

    #[tokio::test]
    async fn classify_inbound_relay_event_returns_welcome_for_new_giftwrap() {
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let client = Client::builder().signer(invitee_keys.clone()).build();
        let rumor = UnsignedEvent::new(
            inviter_keys.public_key(),
            Timestamp::from(1_u64),
            Kind::MlsWelcome,
            Tags::new(),
            "{}".to_string(),
        );
        let wrapper = EventBuilder::gift_wrap(
            &inviter_keys,
            &invitee_keys.public_key(),
            rumor.clone(),
            Vec::<Tag>::new(),
        )
        .await
        .expect("gift wrap");
        let mut seen = InboundRelaySeenCache::default();

        let classified = classify_inbound_relay_event(&client, &mut seen, wrapper.clone())
            .await
            .expect("classify inbound event")
            .expect("welcome event");

        match classified {
            InboundRelayEvent::Welcome {
                wrapper: classified_wrapper,
                sender,
                rumor: classified_rumor,
            } => {
                assert_eq!(classified_wrapper.id, wrapper.id);
                assert_eq!(sender, inviter_keys.public_key());
                assert_eq!(classified_rumor.pubkey, rumor.pubkey);
                assert_eq!(classified_rumor.kind, rumor.kind);
                assert_eq!(classified_rumor.content, rumor.content);
            }
            other => panic!("expected welcome ingress event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn classify_inbound_relay_event_dedupes_event_ids_and_keeps_group_messages() {
        let keys = Keys::generate();
        let client = Client::builder().signer(keys.clone()).build();
        let event = EventBuilder::new(Kind::MlsGroupMessage, "hello")
            .sign_with_keys(&keys)
            .expect("sign event");
        let mut seen = InboundRelaySeenCache::bounded(8);

        let first = classify_inbound_relay_event(&client, &mut seen, event.clone())
            .await
            .expect("classify first event");
        let duplicate = classify_inbound_relay_event(&client, &mut seen, event.clone())
            .await
            .expect("classify duplicate event");

        assert_eq!(first, Some(InboundRelayEvent::GroupMessage { event }));
        assert!(
            duplicate.is_none(),
            "duplicate event id should be suppressed"
        );
    }

    #[test]
    fn process_classified_inbound_group_message_returns_processed_application_outcome() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let keys = Keys::generate();
        let mdk = open_test_mdk(&tempdir);
        let config = NostrGroupConfigData::new(
            "runtime inbound group message".to_string(),
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
            "hello through shared runtime",
            Tags::new(),
        );
        let runtime = MarmotRuntime::new(&mdk);

        let processed = runtime
            .process_classified_inbound_group_message(InboundRelayEvent::GroupMessage {
                event: event.clone(),
            })
            .expect("process classified group message")
            .expect("group message processing result");

        assert_eq!(processed.event_id(), event.id);
        match processed.into_conversation_event() {
            Some(ConversationEvent::Application(message)) => {
                let RuntimeApplicationMessage {
                    nostr_group_id_hex,
                    classification,
                    message,
                    ..
                } = *message;
                assert_eq!(classification, crate::message::MessageClassification::Chat);
                assert_eq!(
                    nostr_group_id_hex,
                    hex::encode(created.group.nostr_group_id)
                );
                assert_eq!(message.content, "hello through shared runtime");
            }
            other => panic!("expected processed application message, got {other:?}"),
        }
    }

    #[test]
    fn process_group_message_event_ignores_non_group_events() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let keys = Keys::generate();
        let mdk = open_test_mdk(&tempdir);
        let runtime = MarmotRuntime::new(&mdk);
        let event = EventBuilder::new(Kind::TextNote, "not a group message")
            .sign_with_keys(&keys)
            .expect("sign text note");

        let processed = runtime
            .process_group_message_event(event.clone())
            .expect("process non-group event");

        match processed {
            InboundGroupMessageProcessing::Ignored { event_id } => {
                assert_eq!(event_id, event.id);
            }
            other => panic!("expected ignored non-group event, got {other:?}"),
        }
    }

    #[test]
    fn interpret_runtime_application_message_parses_shared_call_signal() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let mdk = open_test_mdk(&tempdir);
        let runtime = MarmotRuntime::new(&mdk);
        let call_id = "550e8400-e29b-41d4-a716-446655440000";
        let session = CallSessionParams {
            moq_url: "https://moq.example.com/anon".to_string(),
            broadcast_base: format!("pika/calls/{call_id}"),
            relay_auth: "capv1_test_token".to_string(),
            tracks: vec![CallTrackSpec::audio0_opus_default()],
        };
        let content = build_call_signal_json(call_id, OutgoingCallSignal::Invite(&session))
            .expect("build call signal");
        let runtime_msg = make_runtime_application_message(
            crate::message::MessageClassification::CallSignal,
            crate::message::CALL_SIGNAL_KIND,
            &content,
        );

        let interpreted = runtime.interpret_runtime_application_message(runtime_msg);

        match interpreted {
            RuntimeApplicationMessageInterpretation::CallSignal {
                message,
                parsed_signal: Some(ParsedCallSignal::Invite { call_id, session }),
            } => {
                assert_eq!(
                    message.classification,
                    crate::message::MessageClassification::CallSignal
                );
                assert_eq!(call_id, "550e8400-e29b-41d4-a716-446655440000");
                assert_eq!(session.moq_url, "https://moq.example.com/anon");
            }
            other => panic!("expected parsed call-signal interpretation, got {other:?}"),
        }
    }

    #[test]
    fn interpret_runtime_application_message_marks_typing_content_and_group_profile() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let mdk = open_test_mdk(&tempdir);
        let runtime = MarmotRuntime::new(&mdk);
        let typing = make_runtime_application_message(
            crate::message::MessageClassification::TypingIndicator,
            crate::message::TYPING_INDICATOR_KIND,
            "typing",
        );
        let content = make_runtime_application_message(
            crate::message::MessageClassification::Chat,
            Kind::ChatMessage,
            "hello",
        );
        let group_profile = make_runtime_application_message(
            crate::message::MessageClassification::GroupProfile,
            Kind::Metadata,
            "{\"name\":\"Pika\"}",
        );

        assert!(matches!(
            runtime.interpret_runtime_application_message(typing),
            RuntimeApplicationMessageInterpretation::TypingIndicator { .. }
        ));
        assert!(matches!(
            runtime.interpret_runtime_application_message(content),
            RuntimeApplicationMessageInterpretation::Content { .. }
        ));
        assert!(matches!(
            runtime.interpret_runtime_application_message(group_profile),
            RuntimeApplicationMessageInterpretation::GroupProfile { .. }
        ));
    }

    #[test]
    fn interpret_conversation_event_surfaces_group_update_commit_state() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let mdk = open_test_mdk(&tempdir);
        let runtime = MarmotRuntime::new(&mdk);
        let update = crate::conversation::RuntimeGroupUpdate {
            mls_group_id: GroupId::from_slice(&[1, 2, 3]),
            nostr_group_id_hex: "deadbeef".to_string(),
            kind: crate::conversation::RuntimeGroupUpdateKind::Commit,
        };

        let interpreted =
            runtime.interpret_conversation_event(ConversationEvent::GroupUpdate(update.clone()));

        match interpreted {
            RuntimeConversationEventInterpretation::GroupUpdate {
                update: interpreted_update,
                is_commit,
            } => {
                assert!(is_commit);
                assert_eq!(
                    interpreted_update.nostr_group_id_hex,
                    update.nostr_group_id_hex
                );
                assert_eq!(interpreted_update.kind, update.kind);
            }
            other => panic!("expected group-update interpretation, got {other:?}"),
        }
    }

    #[test]
    fn interpret_conversation_event_surfaces_refresh_reasons() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let mdk = open_test_mdk(&tempdir);
        let runtime = MarmotRuntime::new(&mdk);
        let group_id = GroupId::from_slice(&[9, 9, 9]);

        let unresolved = runtime.interpret_conversation_event(ConversationEvent::UnresolvedGroup {
            mls_group_id: group_id.clone(),
        });
        let failed = runtime.interpret_conversation_event(ConversationEvent::PreviouslyFailed);

        match unresolved {
            RuntimeConversationEventInterpretation::NeedsFullRefresh {
                reason: RuntimeConversationRefreshReason::UnresolvedGroup { mls_group_id },
            } => assert_eq!(mls_group_id, group_id),
            other => panic!("expected unresolved-group refresh reason, got {other:?}"),
        }
        assert!(matches!(
            failed,
            RuntimeConversationEventInterpretation::NeedsFullRefresh {
                reason: RuntimeConversationRefreshReason::PreviouslyFailed
            }
        ));
    }

    #[test]
    fn plan_group_subscriptions_surfaces_current_added_removed_and_relays() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir);
        let invitee_mdk = open_test_mdk(&invitee_dir);
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let relay_url = RelayUrl::parse("wss://test.relay").expect("relay url");
        let config = NostrGroupConfigData::new(
            "runtime subscription plan test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![relay_url.clone()],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let expected_group_id = hex::encode(created.group.nostr_group_id);
        let runtime = MarmotRuntime::new(&inviter_mdk);

        let plan = runtime
            .plan_group_subscriptions(vec!["stale-group".to_string()])
            .expect("plan group subscriptions");

        assert_eq!(
            plan.current.target_group_ids,
            vec![expected_group_id.clone()]
        );
        assert_eq!(plan.current.relay_urls, vec![relay_url]);
        assert_eq!(plan.added_group_ids, vec![expected_group_id]);
        assert_eq!(plan.removed_group_ids, vec!["stale-group".to_string()]);
    }

    #[test]
    fn plan_runtime_relay_roles_separates_session_group_and_temporary_relays() {
        let plan = plan_runtime_relay_roles(
            vec![
                RelayUrl::parse("wss://message-1.example").expect("message relay"),
                RelayUrl::parse("wss://message-2.example").expect("message relay"),
            ],
            vec![
                RelayUrl::parse("wss://group-1.example").expect("group relay"),
                RelayUrl::parse("wss://message-1.example").expect("message relay"),
            ],
            vec![
                RelayUrl::parse("wss://kp-1.example").expect("kp relay"),
                RelayUrl::parse("wss://message-2.example").expect("message relay"),
            ],
        );

        assert_eq!(
            plan.long_lived_session_relays,
            vec![
                RelayUrl::parse("wss://message-1.example").expect("message relay"),
                RelayUrl::parse("wss://message-2.example").expect("message relay"),
            ]
        );
        assert_eq!(
            plan.active_group_relays,
            vec![
                RelayUrl::parse("wss://group-1.example").expect("group relay"),
                RelayUrl::parse("wss://message-1.example").expect("message relay"),
            ]
        );
        assert_eq!(
            plan.temporary_key_package_relays,
            vec![
                RelayUrl::parse("wss://kp-1.example").expect("kp relay"),
                RelayUrl::parse("wss://message-2.example").expect("message relay"),
            ]
        );
        assert_eq!(
            plan.session_connect_relays,
            vec![
                RelayUrl::parse("wss://group-1.example").expect("group relay"),
                RelayUrl::parse("wss://message-1.example").expect("message relay"),
                RelayUrl::parse("wss://message-2.example").expect("message relay"),
            ]
        );
        assert_eq!(
            plan.key_package_operation_relays,
            vec![
                RelayUrl::parse("wss://kp-1.example").expect("kp relay"),
                RelayUrl::parse("wss://message-1.example").expect("message relay"),
                RelayUrl::parse("wss://message-2.example").expect("message relay"),
            ]
        );
    }

    #[test]
    fn plan_runtime_session_sync_composes_relay_roles_group_diffs_and_welcome_intent() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir);
        let invitee_mdk = open_test_mdk(&invitee_dir);
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let group_relay = RelayUrl::parse("wss://group-1.example").expect("group relay");
        let created = inviter_mdk
            .create_group(
                &inviter_keys.public_key(),
                vec![invitee_kp],
                NostrGroupConfigData::new(
                    "runtime session sync plan".to_string(),
                    String::new(),
                    None,
                    None,
                    None,
                    vec![group_relay.clone()],
                    vec![inviter_keys.public_key(), invitee_keys.public_key()],
                ),
            )
            .expect("create group");
        let expected_group_id = hex::encode(created.group.nostr_group_id);
        let runtime = MarmotRuntime::new(&inviter_mdk);
        let welcome_inbox = RuntimeWelcomeInboxSubscriptionIntent {
            lookback: Some(Duration::from_secs(30)),
            limit: Some(25),
        };

        let plan = runtime
            .plan_session_sync(
                vec!["stale-group".to_string()],
                vec![RelayUrl::parse("wss://message-1.example").expect("message relay")],
                vec![RelayUrl::parse("wss://kp-1.example").expect("kp relay")],
                welcome_inbox.clone(),
            )
            .expect("plan runtime session sync");

        assert_eq!(
            plan.group_subscriptions.current.target_group_ids,
            vec![expected_group_id.clone()]
        );
        assert_eq!(
            plan.group_subscriptions.added_group_ids,
            vec![expected_group_id]
        );
        assert_eq!(
            plan.group_subscriptions.removed_group_ids,
            vec!["stale-group".to_string()]
        );
        assert_eq!(plan.welcome_inbox, welcome_inbox);
        assert_eq!(
            plan.relay_roles.session_connect_relays,
            vec![
                RelayUrl::parse("wss://group-1.example").expect("group relay"),
                RelayUrl::parse("wss://message-1.example").expect("message relay"),
            ]
        );
        assert_eq!(
            plan.relay_roles.key_package_operation_relays,
            vec![
                RelayUrl::parse("wss://kp-1.example").expect("kp relay"),
                RelayUrl::parse("wss://message-1.example").expect("message relay"),
            ]
        );
    }

    #[test]
    fn bootstrap_runtime_session_surfaces_group_subscription_state() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir);
        let invitee_mdk = open_test_mdk(&invitee_dir);
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "runtime bootstrap test".to_string(),
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
        let expected_group_id = hex::encode(created.group.nostr_group_id);

        let bootstrapped = bootstrap_runtime_session(
            inviter_keys.public_key(),
            Arc::new(inviter_keys.clone()),
            || Ok(inviter_mdk),
        )
        .expect("bootstrap runtime session");

        assert_eq!(bootstrapped.session.pubkey, inviter_keys.public_key());
        assert_eq!(
            bootstrapped.startup.group_subscriptions.target_group_ids,
            vec![expected_group_id]
        );
        assert_eq!(
            bootstrapped.startup.group_subscriptions.relay_urls,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")]
        );
        assert!(bootstrapped.startup.seen_welcomes.is_empty());
        assert!(bootstrapped.startup.seen_group_events.is_empty());
    }

    #[test]
    fn runtime_facade_lists_groups_through_bootstrapped_session() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir);
        let invitee_mdk = open_test_mdk(&invitee_dir);
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "runtime facade test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        let bootstrapped = bootstrap_runtime_session(
            inviter_keys.public_key(),
            Arc::new(inviter_keys.clone()),
            || Ok(inviter_mdk),
        )
        .expect("bootstrap runtime session");

        let groups = bootstrapped.runtime().list_groups().expect("list groups");

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "runtime facade test");
    }
}
