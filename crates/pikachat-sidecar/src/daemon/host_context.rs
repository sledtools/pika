use super::*;
use pika_marmot_runtime::membership::EvolutionPublishStatus;
#[cfg(test)]
use pika_marmot_runtime::membership::MembershipUpdateResult;
use pika_marmot_runtime::membership::PreparedMembershipEvolution;

#[derive(Debug)]
pub(super) enum DaemonPrepareError {
    BadGroup(anyhow::Error),
    Prepare(anyhow::Error),
}

pub(super) struct DaemonHostContext<'a> {
    client: &'a Client,
    relay_urls: &'a [RelayUrl],
    mdk: &'a MDK<MdkSqliteStorage>,
    keys: &'a Keys,
    pubkey_hex: String,
}

impl<'a> DaemonHostContext<'a> {
    pub(super) fn new(
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

    fn commands(&self) -> pika_marmot_runtime::runtime::RuntimeCommands<'a> {
        pika_marmot_runtime::runtime::RuntimeCommands::with_client(self.mdk, self.client)
    }

    fn queries(&self) -> pika_marmot_runtime::runtime::RuntimeQueries<'a> {
        pika_marmot_runtime::runtime::RuntimeQueries::new(self.mdk)
    }

    pub(super) fn lookup_joined_group_snapshot(
        &self,
        nostr_group_id: &str,
    ) -> anyhow::Result<pika_marmot_runtime::conversation::RuntimeJoinedGroupSnapshot> {
        self.runtime().lookup_joined_group_snapshot(nostr_group_id)
    }

    pub(super) fn resolve_group(&self, nostr_group_id: &str) -> anyhow::Result<GroupId> {
        Ok(self
            .lookup_joined_group_snapshot(nostr_group_id)?
            .mls_group_id)
    }

    pub(super) fn list_groups(
        &self,
    ) -> anyhow::Result<Vec<pika_marmot_runtime::conversation::RuntimeGroupSummary>> {
        self.runtime().list_groups()
    }

    #[cfg(test)]
    pub(super) fn list_joined_group_snapshots(
        &self,
    ) -> anyhow::Result<Vec<pika_marmot_runtime::conversation::RuntimeJoinedGroupSnapshot>> {
        self.runtime().list_joined_group_snapshots()
    }

    pub(super) fn load_message_page(
        &self,
        nostr_group_id: &str,
        query: pika_marmot_runtime::conversation::RuntimeMessagePageQuery,
    ) -> anyhow::Result<pika_marmot_runtime::conversation::RuntimeMessagePage> {
        self.runtime().load_message_page(nostr_group_id, query)
    }

    pub(super) fn list_pending_welcome_snapshots(
        &self,
    ) -> anyhow::Result<Vec<pika_marmot_runtime::welcome::PendingWelcomeSnapshot>> {
        self.queries().list_pending_welcome_snapshots()
    }

    pub(super) fn lookup_pending_welcome(
        &self,
        target: &EventId,
    ) -> anyhow::Result<Option<mdk_storage_traits::welcomes::types::Welcome>> {
        self.queries().lookup_pending_welcome(target)
    }

    pub(super) fn parse_message_media_attachments(
        &self,
        message: &mdk_storage_traits::messages::types::Message,
    ) -> Vec<ParsedMediaAttachment> {
        self.runtime().parse_message_attachments(message)
    }

    pub(super) async fn download_and_decrypt_media(
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

    pub(super) async fn publish_prepared(
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

    pub(super) fn complete_outbound_publish_operation(
        &self,
        prepared: PreparedConversationAction,
        publish_status: pika_marmot_runtime::outbound::OutboundConversationPublishStatus,
    ) -> pika_marmot_runtime::runtime::RuntimeOperationEvent {
        self.commands()
            .complete_outbound_publish_operation(prepared, publish_status)
    }

    pub(super) fn complete_call_signal_publish_operation(
        &self,
        kind: pika_marmot_runtime::runtime::CallSignalPublishKind,
        nostr_group_id_hex: String,
        prepared: pika_marmot_runtime::call_runtime::PreparedCallSignal,
        publish_status: pika_marmot_runtime::runtime::CallSignalPublishStatus,
    ) -> pika_marmot_runtime::runtime::RuntimeOperationEvent {
        self.commands().complete_call_signal_publish_operation(
            kind,
            nostr_group_id_hex,
            prepared,
            publish_status,
        )
    }

    pub(super) async fn sign_and_publish_rumor(
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

    pub(super) fn sign_call_payload(
        &self,
        nostr_group_id: &str,
        payload_json: String,
    ) -> anyhow::Result<Event> {
        let mls_group_id = self.resolve_group(nostr_group_id)?;
        let rumor = EventBuilder::new(CALL_SIGNAL_KIND, payload_json).build(self.keys.public_key());
        let msg_event = self
            .mdk
            .create_message(&mls_group_id, rumor)
            .context("create_message")?;
        resign_wrapper_without_protected_tags(self.keys, &msg_event)
    }

    pub(super) async fn publish_signed_call_payload(
        &self,
        signed: &Event,
        label: &str,
    ) -> anyhow::Result<()> {
        if self.relay_urls.is_empty() {
            anyhow::bail!("no relays configured");
        }
        publish_and_confirm_multi(self.client, self.relay_urls, signed, label)
            .await
            .map(|_| ())
    }

    pub(super) fn prepare_outbound_action(
        &self,
        nostr_group_id: &str,
        action: OutboundConversationAction,
    ) -> Result<PreparedConversationAction, DaemonPrepareError> {
        let target = self
            .commands()
            .resolve_outbound_target(nostr_group_id)
            .map_err(DaemonPrepareError::BadGroup)?;
        self.commands()
            .prepare_outbound_action_for_target(self.keys.public_key(), target, action)
            .map_err(DaemonPrepareError::Prepare)
    }

    pub(super) fn prepare_add_members(
        &self,
        nostr_group_id: &str,
        key_package_events: &[Event],
    ) -> Result<PreparedMembershipEvolution, DaemonPrepareError> {
        let mls_group_id = self
            .resolve_group(nostr_group_id)
            .map_err(DaemonPrepareError::BadGroup)?;
        self.commands()
            .prepare_add_members(&mls_group_id, key_package_events)
            .map_err(DaemonPrepareError::Prepare)
    }

    #[cfg(test)]
    pub(super) fn finalize_published_evolution(
        &self,
        prepared: PreparedMembershipEvolution,
    ) -> MembershipUpdateResult {
        self.commands().finalize_published_evolution(prepared)
    }

    pub(super) fn complete_membership_evolution_operation(
        &self,
        prepared: PreparedMembershipEvolution,
        publish_status: EvolutionPublishStatus,
    ) -> pika_marmot_runtime::runtime::RuntimeOperationEvent {
        self.commands()
            .complete_membership_evolution_operation(prepared, publish_status)
    }

    pub(super) fn derive_relay_auth_token(
        &self,
        nostr_group_id: &str,
        call_id: &str,
        session: &CallSessionParams,
        peer_pubkey_hex: &str,
    ) -> anyhow::Result<String> {
        let group = self.lookup_joined_group_snapshot(nostr_group_id)?;
        let derive_ctx = CallCryptoDeriveContext {
            mdk: self.mdk,
            mls_group_id: &group.mls_group_id,
            group_epoch: 0,
            call_id,
            session,
            local_pubkey_hex: &self.pubkey_hex,
            peer_pubkey_hex,
        };
        derive_shared_relay_auth_token(&derive_ctx).map_err(anyhow::Error::msg)
    }

    pub(super) fn prepare_call_invite(
        &self,
        nostr_group_id: &str,
        peer_pubkey_hex: &str,
        call_id: &str,
        session: &CallSessionParams,
    ) -> anyhow::Result<(
        PendingOutgoingCall,
        pika_marmot_runtime::call_runtime::PreparedCallSignal,
    )> {
        self.commands()
            .prepare_outgoing_call_invite(nostr_group_id, peer_pubkey_hex, call_id, session)
            .map_err(anyhow::Error::msg)
    }

    pub(super) fn prepare_accept_call(
        &self,
        invite: &PendingIncomingCall,
    ) -> Result<pika_marmot_runtime::call_runtime::PreparedAcceptedCall, String> {
        let mls_group_id = self
            .resolve_group(&invite.target_id)
            .map_err(|e| format!("resolve call group failed: {e:#}"))?;
        self.commands().prepare_accept_incoming_call(
            invite,
            GroupCallContext {
                mls_group_id: &mls_group_id,
                local_pubkey_hex: &self.pubkey_hex,
            },
        )
    }

    pub(super) fn prepare_reject_call_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<pika_marmot_runtime::call_runtime::PreparedCallSignal, String> {
        self.commands().prepare_reject_call_signal(call_id, reason)
    }

    pub(super) fn prepare_end_call_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<pika_marmot_runtime::call_runtime::PreparedCallSignal, String> {
        self.commands().prepare_end_call_signal(call_id, reason)
    }

    pub(super) fn process_classified_inbound_group_message(
        &self,
        inbound: InboundRelayEvent,
    ) -> anyhow::Result<Option<pika_marmot_runtime::runtime::InboundGroupMessageProcessing>> {
        self.runtime()
            .process_classified_inbound_group_message(inbound)
    }

    pub(super) fn interpret_runtime_application_message(
        &self,
        runtime_msg: pika_marmot_runtime::conversation::RuntimeApplicationMessage,
    ) -> pika_marmot_runtime::runtime::RuntimeApplicationMessageInterpretation {
        self.runtime()
            .interpret_runtime_application_message(runtime_msg)
    }

    pub(super) fn interpret_conversation_event(
        &self,
        event: ConversationEvent,
    ) -> pika_marmot_runtime::runtime::RuntimeConversationEventInterpretation {
        self.runtime().interpret_conversation_event(event)
    }

    pub(super) fn refresh_session_state(
        &self,
        subscribed_group_ids: Vec<String>,
        giftwrap_lookback_sec: u64,
    ) -> anyhow::Result<pika_marmot_runtime::runtime::RuntimeSessionOpenState> {
        self.queries().refresh_session_open_state(
            self.keys.public_key(),
            super::daemon_open_request(
                subscribed_group_ids,
                self.relay_urls.to_vec(),
                giftwrap_lookback_sec,
            ),
        )
    }

    pub(super) fn handle_inbound_call_signal(
        &self,
        ctx: pika_marmot_runtime::call_runtime::InboundSignalContext<'_>,
        signal: ParsedCallSignal,
    ) -> InboundCallSignalOutcome {
        self.runtime().handle_inbound_call_signal(ctx, signal)
    }

    pub(super) fn prepare_upload(
        &self,
        mls_group_id: &GroupId,
        bytes: &[u8],
        mime_type: Option<&str>,
        filename: Option<&str>,
    ) -> anyhow::Result<pika_marmot_runtime::media::PreparedMediaUpload> {
        self.commands()
            .prepare_upload(mls_group_id, bytes, mime_type, filename)
    }

    pub(super) fn complete_media_upload_operation(
        &self,
        mls_group_id: &GroupId,
        nostr_group_id_hex: String,
        upload: &mdk_core::encrypted_media::types::EncryptedMediaUpload,
        status: pika_marmot_runtime::runtime::MediaUploadStatus,
    ) -> pika_marmot_runtime::runtime::RuntimeOperationEvent {
        self.commands().complete_media_upload_operation(
            mls_group_id,
            nostr_group_id_hex,
            upload,
            status,
        )
    }
}
