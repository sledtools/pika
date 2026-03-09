use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use mdk_core::prelude::{GroupId, MessageProcessingResult};
use mdk_storage_traits::groups::{Pagination, types::Group};
use mdk_storage_traits::messages::types::Message;
use nostr_sdk::prelude::*;

use crate::PikaMdk;
use crate::call::{CallSessionParams, ParsedCallSignal};
use crate::call_runtime::{
    CallWorkflowRuntime, GroupCallContext, InboundCallSignalOutcome, InboundSignalContext,
    PendingIncomingCall, PreparedAcceptedCall, PreparedCallSignal,
};
use crate::conversation::{ConversationEvent, ConversationRuntime, RuntimeGroupSummary};
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

pub struct RuntimeSession {
    pub pubkey: PublicKey,
    pub client: Client,
    pub mdk: PikaMdk,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuntimeStartupState {
    pub existing_group_ids: Vec<String>,
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
        subscribe_group_messages_individual(&self.client, &existing_group_ids_from_mdk(&self.mdk)?)
            .await
    }

    pub fn existing_group_ids(&self) -> Result<Vec<String>> {
        existing_group_ids_from_mdk(&self.mdk)
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
    subscribe_group_messages_individual(client, &existing_group_ids_from_mdk(mdk)?).await
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

pub fn existing_group_ids_from_mdk(mdk: &PikaMdk) -> Result<Vec<String>> {
    Ok(mdk
        .get_groups()?
        .into_iter()
        .map(|group| hex::encode(group.nostr_group_id))
        .collect())
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
        existing_group_ids: existing_group_ids_from_mdk(&session.mdk)?,
        seen_welcomes: HashSet::new(),
        seen_group_events: HashSet::new(),
    };
    Ok(BootstrappedRuntimeSession { session, startup })
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn bootstrap_runtime_session_surfaces_existing_group_ids() {
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
            bootstrapped.startup.existing_group_ids,
            vec![expected_group_id]
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
