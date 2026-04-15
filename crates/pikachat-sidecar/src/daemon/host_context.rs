use super::*;
use crate::membership::EvolutionPublishStatus;
#[cfg(test)]
use crate::membership::MembershipUpdateResult;
use crate::membership::PreparedMembershipEvolution;

#[derive(Debug)]
pub(super) enum DaemonPrepareError {
    BadGroup(anyhow::Error),
    Prepare(anyhow::Error),
}

pub(super) struct DaemonHostContext<'a> {
    client: &'a Client,
    relay_urls: &'a [RelayUrl],
    mdk: &'a crate::PikaMdk,
    keys: &'a Keys,
    pubkey_hex: String,
}

impl<'a> DaemonHostContext<'a> {
    pub(super) fn new(
        client: &'a Client,
        relay_urls: &'a [RelayUrl],
        mdk: &'a crate::PikaMdk,
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

    fn runtime(&self) -> PikaRuntime<'a> {
        PikaRuntime::with_client(self.mdk, self.client)
    }

    pub(super) fn lookup_joined_group_snapshot(
        &self,
        nostr_group_id: &str,
    ) -> anyhow::Result<crate::conversation::RuntimeJoinedGroupSnapshot> {
        self.runtime().lookup_joined_group_snapshot(nostr_group_id)
    }

    pub(super) fn resolve_group(&self, nostr_group_id: &str) -> anyhow::Result<GroupId> {
        Ok(self
            .lookup_joined_group_snapshot(nostr_group_id)?
            .mls_group_id)
    }

    pub(super) fn list_groups(
        &self,
    ) -> anyhow::Result<Vec<crate::conversation::RuntimeGroupSummary>> {
        self.runtime().list_groups()
    }

    pub(super) fn lookup_group_profile_snapshot_for_owners(
        &self,
        nostr_group_id: &str,
        owner_pubkeys: &[PublicKey],
    ) -> anyhow::Result<Option<crate::conversation::RuntimeGroupProfileSnapshot>> {
        self.runtime()
            .lookup_group_profile_snapshot_for_owners(nostr_group_id, owner_pubkeys)
    }

    #[cfg(test)]
    pub(super) fn list_joined_group_snapshots(
        &self,
    ) -> anyhow::Result<Vec<crate::conversation::RuntimeJoinedGroupSnapshot>> {
        self.runtime().list_joined_group_snapshots()
    }

    pub(super) fn load_message_page(
        &self,
        nostr_group_id: &str,
        query: crate::conversation::RuntimeMessagePageQuery,
    ) -> anyhow::Result<crate::conversation::RuntimeMessagePage> {
        self.runtime().load_message_page(nostr_group_id, query)
    }

    pub(super) fn list_pending_welcome_snapshots(
        &self,
    ) -> anyhow::Result<Vec<crate::welcome::PendingWelcomeSnapshot>> {
        self.runtime().list_pending_welcome_snapshots()
    }

    pub(super) fn lookup_pending_welcome(
        &self,
        target: &EventId,
    ) -> anyhow::Result<Option<pika_mls::storage_traits::welcomes::types::Welcome>> {
        self.runtime().lookup_pending_welcome(target)
    }

    pub(super) fn parse_message_media_attachments(
        &self,
        message: &pika_mls::storage_traits::messages::types::Message,
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
        publish_status: crate::outbound::OutboundConversationPublishStatus,
    ) -> crate::runtime::RuntimeOperationEvent {
        self.runtime()
            .complete_outbound_publish_operation(prepared, publish_status)
    }

    pub(super) fn complete_call_signal_publish_operation(
        &self,
        kind: crate::runtime::CallSignalPublishKind,
        nostr_group_id_hex: String,
        prepared: crate::call_runtime::PreparedCallSignal,
        publish_status: crate::runtime::CallSignalPublishStatus,
    ) -> crate::runtime::RuntimeOperationEvent {
        self.runtime().complete_call_signal_publish_operation(
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
        let msg_event = pika_mls::conversation::wrap_rumor(self.mdk, mls_group_id, rumor)
            .context("create_message")?
            .wrapper;
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
        let msg_event = pika_mls::conversation::wrap_rumor(self.mdk, &mls_group_id, rumor)
            .context("create_message")?
            .wrapper;
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
            .runtime()
            .resolve_outbound_target(nostr_group_id)
            .map_err(DaemonPrepareError::BadGroup)?;
        self.runtime()
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
        self.runtime()
            .prepare_add_members(&mls_group_id, key_package_events)
            .map_err(DaemonPrepareError::Prepare)
    }

    pub(super) fn prepare_remove_members(
        &self,
        nostr_group_id: &str,
        removed_pubkeys: &[PublicKey],
    ) -> Result<PreparedMembershipEvolution, DaemonPrepareError> {
        let mls_group_id = self
            .resolve_group(nostr_group_id)
            .map_err(DaemonPrepareError::BadGroup)?;
        self.runtime()
            .prepare_remove_members(&mls_group_id, removed_pubkeys)
            .map_err(DaemonPrepareError::Prepare)
    }

    pub(super) fn prepare_leave_group(
        &self,
        nostr_group_id: &str,
    ) -> Result<PreparedMembershipEvolution, DaemonPrepareError> {
        let mls_group_id = self
            .resolve_group(nostr_group_id)
            .map_err(DaemonPrepareError::BadGroup)?;
        self.runtime()
            .prepare_leave_group(&mls_group_id)
            .map_err(DaemonPrepareError::Prepare)
    }

    #[cfg(test)]
    pub(super) fn finalize_published_evolution(
        &self,
        prepared: PreparedMembershipEvolution,
    ) -> MembershipUpdateResult {
        self.runtime().finalize_published_evolution(prepared)
    }

    pub(super) fn complete_membership_evolution_operation(
        &self,
        prepared: PreparedMembershipEvolution,
        publish_status: EvolutionPublishStatus,
    ) -> crate::runtime::RuntimeOperationEvent {
        self.runtime()
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
    ) -> anyhow::Result<(PendingOutgoingCall, crate::call_runtime::PreparedCallSignal)> {
        self.runtime()
            .prepare_outgoing_call_invite(nostr_group_id, peer_pubkey_hex, call_id, session)
            .map_err(anyhow::Error::msg)
    }

    pub(super) fn prepare_accept_call(
        &self,
        invite: &PendingIncomingCall,
    ) -> Result<crate::call_runtime::PreparedAcceptedCall, String> {
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

    pub(super) fn prepare_reject_call_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<crate::call_runtime::PreparedCallSignal, String> {
        self.runtime().prepare_reject_call_signal(call_id, reason)
    }

    pub(super) fn prepare_end_call_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<crate::call_runtime::PreparedCallSignal, String> {
        self.runtime().prepare_end_call_signal(call_id, reason)
    }

    pub(super) fn process_classified_inbound_group_message(
        &self,
        inbound: InboundRelayEvent,
    ) -> anyhow::Result<Option<crate::runtime::InboundGroupMessageProcessing>> {
        self.runtime()
            .process_classified_inbound_group_message(inbound)
    }

    pub(super) fn interpret_runtime_application_message(
        &self,
        runtime_msg: crate::conversation::RuntimeApplicationMessage,
    ) -> crate::runtime::RuntimeApplicationMessageInterpretation {
        self.runtime()
            .interpret_runtime_application_message(runtime_msg)
    }

    pub(super) fn interpret_conversation_event(
        &self,
        event: ConversationEvent,
    ) -> crate::runtime::RuntimeConversationEventInterpretation {
        self.runtime().interpret_conversation_event(event)
    }

    pub(super) fn refresh_session_state(
        &self,
        subscribed_group_ids: Vec<String>,
        giftwrap_lookback_sec: u64,
    ) -> anyhow::Result<crate::runtime::RuntimeSessionOpenState> {
        self.runtime().refresh_session_open_state(
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
        ctx: crate::call_runtime::InboundSignalContext<'_>,
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
    ) -> anyhow::Result<crate::media::PreparedMediaUpload> {
        self.runtime()
            .prepare_upload(mls_group_id, bytes, mime_type, filename)
    }

    pub(super) fn complete_media_upload_operation(
        &self,
        mls_group_id: &GroupId,
        nostr_group_id_hex: String,
        upload: &pika_mls::encrypted_media::types::EncryptedMediaUpload,
        status: crate::runtime::MediaUploadStatus,
    ) -> crate::runtime::RuntimeOperationEvent {
        self.runtime().complete_media_upload_operation(
            mls_group_id,
            nostr_group_id_hex,
            upload,
            status,
        )
    }
}
