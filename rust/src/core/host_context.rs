use super::*;
use pika_marmot_runtime::runtime::MarmotRuntime;

pub(super) struct AppHostContext<'a> {
    session: &'a Session,
}

#[cfg(test)]
pub(super) fn runtime_for_mdk(mdk: &PikaMdk) -> MarmotRuntime<'_> {
    MarmotRuntime::new(mdk)
}

impl Session {
    pub(super) fn runtime(&self) -> MarmotRuntime<'_> {
        MarmotRuntime::with_client(&self.mdk, &self.client)
    }

    pub(super) fn host_context(&self) -> AppHostContext<'_> {
        AppHostContext { session: self }
    }
}

impl AppCore {
    pub(super) fn host_context(&self) -> anyhow::Result<AppHostContext<'_>> {
        self.session
            .as_ref()
            .map(Session::host_context)
            .context("not logged in")
    }
}

impl<'a> AppHostContext<'a> {
    fn runtime(&self) -> MarmotRuntime<'a> {
        self.session.runtime()
    }

    fn group_entry(&self, chat_id: &str) -> anyhow::Result<&'a GroupIndexEntry> {
        self.session.groups.get(chat_id).context("chat not found")
    }

    pub(super) fn current_pubkey_hex(&self) -> String {
        self.session.pubkey.to_hex()
    }

    pub(super) fn list_joined_group_snapshots(
        &self,
    ) -> anyhow::Result<Vec<pika_marmot_runtime::conversation::RuntimeJoinedGroupSnapshot>> {
        self.runtime().list_joined_group_snapshots()
    }

    pub(super) fn load_message_page(
        &self,
        chat_id: &str,
        query: pika_marmot_runtime::conversation::RuntimeMessagePageQuery,
    ) -> anyhow::Result<pika_marmot_runtime::conversation::RuntimeMessagePage> {
        self.runtime().load_message_page(chat_id, query)
    }

    #[cfg(test)]
    pub(super) fn list_pending_welcome_snapshots(
        &self,
    ) -> anyhow::Result<Vec<pika_marmot_runtime::welcome::PendingWelcomeSnapshot>> {
        self.runtime().list_pending_welcome_snapshots()
    }

    pub(super) fn lookup_pending_welcome(
        &self,
        target: &EventId,
    ) -> anyhow::Result<Option<mdk_storage_traits::welcomes::types::Welcome>> {
        self.runtime().lookup_pending_welcome(target)
    }

    pub(super) fn prepare_outbound_action_for_chat(
        &self,
        chat_id: &str,
        action: OutboundConversationAction,
    ) -> anyhow::Result<PreparedConversationAction> {
        let group = self.group_entry(chat_id)?;
        self.prepare_outbound_action_for_group_ids(
            group.mls_group_id.clone(),
            chat_id.to_string(),
            action,
        )
    }

    pub(super) fn prepare_outbound_action_for_group_ids(
        &self,
        mls_group_id: GroupId,
        nostr_group_id_hex: String,
        action: OutboundConversationAction,
    ) -> anyhow::Result<PreparedConversationAction> {
        self.runtime().prepare_outbound_action_for_group_ids(
            self.session.pubkey,
            mls_group_id,
            nostr_group_id_hex,
            action,
        )
    }

    pub(super) fn prepare_membership_evolution_for_chat(
        &self,
        chat_id: &str,
        key_package_events: &[Event],
    ) -> anyhow::Result<PreparedMembershipEvolution> {
        let group = self.group_entry(chat_id)?;
        self.runtime()
            .prepare_add_members(&group.mls_group_id, key_package_events)
    }

    pub(super) fn prepare_evolution(
        &self,
        mls_group_id: GroupId,
        evolution_event: Event,
        welcome_rumors: Option<Vec<UnsignedEvent>>,
        added_pubkeys: Vec<PublicKey>,
    ) -> anyhow::Result<PreparedMembershipEvolution> {
        self.runtime().prepare_evolution(
            mls_group_id,
            evolution_event,
            welcome_rumors,
            added_pubkeys,
        )
    }

    pub(super) fn finalize_published_evolution(
        &self,
        prepared: PreparedMembershipEvolution,
    ) -> MembershipUpdateResult {
        self.runtime().finalize_published_evolution(prepared)
    }

    pub(super) fn process_group_message_event(
        &self,
        event: Event,
    ) -> anyhow::Result<pika_marmot_runtime::runtime::InboundGroupMessageProcessing> {
        self.runtime().process_group_message_event(event)
    }

    pub(super) fn interpret_runtime_application_message(
        &self,
        runtime_msg: RuntimeApplicationMessage,
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

    #[cfg(test)]
    pub(super) fn plan_group_subscriptions(
        &self,
        subscribed_group_ids: Vec<String>,
    ) -> anyhow::Result<pika_marmot_runtime::runtime::RuntimeGroupSubscriptionPlan> {
        self.runtime()
            .plan_group_subscriptions(subscribed_group_ids)
    }

    pub(super) fn plan_session_sync(
        &self,
        subscribed_group_ids: Vec<String>,
        long_lived_session_relays: Vec<RelayUrl>,
        temporary_key_package_relays: Vec<RelayUrl>,
        welcome_inbox: pika_marmot_runtime::runtime::RuntimeWelcomeInboxSubscriptionIntent,
    ) -> anyhow::Result<pika_marmot_runtime::runtime::RuntimeSessionSyncPlan> {
        self.runtime().plan_session_sync(
            subscribed_group_ids,
            long_lived_session_relays,
            temporary_key_package_relays,
            welcome_inbox,
        )
    }

    pub(super) fn interpret_processing_result(
        &self,
        result: MessageProcessingResult,
    ) -> Option<ConversationEvent> {
        self.runtime().interpret_processing_result(result)
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
        upload: &EncryptedMediaUpload,
        uploaded_blob: pika_marmot_runtime::media::UploadedBlob,
    ) -> pika_marmot_runtime::media::RuntimeMediaUploadResult {
        self.runtime()
            .finish_upload(mls_group_id, upload, uploaded_blob)
    }

    pub(super) fn decrypt_downloaded_media(
        &self,
        mls_group_id: &GroupId,
        reference: &MediaReference,
        encrypted_data: &[u8],
        expected_encrypted_hash_hex: Option<&str>,
    ) -> anyhow::Result<pika_marmot_runtime::media::RuntimeDownloadedMedia> {
        self.runtime().decrypt_downloaded_media(
            mls_group_id,
            reference,
            encrypted_data,
            expected_encrypted_hash_hex,
        )
    }

    pub(super) fn prepare_outgoing_call_invite(
        &self,
        target_id: &str,
        peer_pubkey_hex: &str,
        call_id: &str,
        session: &call_control::CallSessionParams,
    ) -> Result<
        (
            pika_marmot_runtime::call_runtime::PendingOutgoingCall,
            pika_marmot_runtime::call_runtime::PreparedCallSignal,
        ),
        String,
    > {
        self.runtime()
            .prepare_outgoing_call_invite(target_id, peer_pubkey_hex, call_id, session)
    }

    pub(super) fn prepare_accept_incoming_call(
        &self,
        incoming: &pika_marmot_runtime::call_runtime::PendingIncomingCall,
        group: GroupCallContext<'_>,
    ) -> Result<PreparedAcceptedCall, String> {
        self.runtime().prepare_accept_incoming_call(incoming, group)
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

    pub(super) fn handle_inbound_call_signal(
        &self,
        ctx: InboundSignalContext<'_>,
        signal: ParsedCallSignal,
    ) -> InboundCallSignalOutcome {
        self.runtime().handle_inbound_call_signal(ctx, signal)
    }
}
