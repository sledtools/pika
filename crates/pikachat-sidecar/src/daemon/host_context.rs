use super::*;

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

    pub(super) fn resolve_group(&self, nostr_group_id: &str) -> anyhow::Result<GroupId> {
        self.runtime()
            .mls_group_id_for_nostr_group_id(nostr_group_id)
    }

    pub(super) fn list_groups(
        &self,
    ) -> anyhow::Result<Vec<pika_marmot_runtime::conversation::RuntimeGroupSummary>> {
        self.runtime().list_groups()
    }

    pub(super) fn get_messages(
        &self,
        nostr_group_id: &str,
        pagination: Option<mdk_storage_traits::groups::Pagination>,
    ) -> anyhow::Result<Vec<mdk_storage_traits::messages::types::Message>> {
        self.runtime().get_messages(nostr_group_id, pagination)
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

    pub(super) async fn publish_call_payload(
        &self,
        nostr_group_id: &str,
        payload_json: String,
        label: &str,
    ) -> anyhow::Result<()> {
        self.publish_group_event(nostr_group_id, CALL_SIGNAL_KIND, payload_json, label)
            .await
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

    pub(super) fn derive_relay_auth_token(
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
        self.runtime()
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
    ) -> Result<pika_marmot_runtime::call_runtime::PreparedCallSignal, String> {
        self.runtime().prepare_reject_call_signal(call_id, reason)
    }

    pub(super) fn prepare_end_call_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<pika_marmot_runtime::call_runtime::PreparedCallSignal, String> {
        self.runtime().prepare_end_call_signal(call_id, reason)
    }

    pub(super) fn process_classified_inbound_group_message(
        &self,
        inbound: InboundRelayEvent,
    ) -> anyhow::Result<Option<pika_marmot_runtime::runtime::InboundGroupMessageProcessing>> {
        self.runtime()
            .process_classified_inbound_group_message(inbound)
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
        self.runtime()
            .prepare_upload(mls_group_id, bytes, mime_type, filename)
    }

    pub(super) fn finish_upload(
        &self,
        mls_group_id: &GroupId,
        upload: &mdk_core::encrypted_media::types::EncryptedMediaUpload,
        uploaded_blob: pika_marmot_runtime::media::UploadedBlob,
    ) -> pika_marmot_runtime::media::RuntimeMediaUploadResult {
        self.runtime()
            .finish_upload(mls_group_id, upload, uploaded_blob)
    }
}
