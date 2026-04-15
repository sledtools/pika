use super::*;
use crate::updates::ChatMediaUploadStatus;
#[cfg(test)]
use crate::updates::InternalEvent;

pub(super) struct AppHostContext<'a> {
    session: &'a Session,
}

impl Session {
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
    pub(super) fn lookup_joined_group_snapshot(
        &self,
        chat_id: &str,
    ) -> anyhow::Result<pika_marmot_runtime::conversation::RuntimeJoinedGroupSnapshot> {
        pika_marmot_runtime::conversation::ConversationRuntime::new(&self.session.mdk)
            .lookup_joined_group_snapshot(chat_id)
    }

    pub(super) fn current_pubkey_hex(&self) -> String {
        self.session.pubkey.to_hex()
    }

    pub(super) fn list_joined_group_snapshots(
        &self,
    ) -> anyhow::Result<Vec<pika_marmot_runtime::conversation::RuntimeJoinedGroupSnapshot>> {
        pika_marmot_runtime::conversation::ConversationRuntime::new(&self.session.mdk)
            .list_joined_group_snapshots()
    }

    pub(super) fn load_message_page(
        &self,
        chat_id: &str,
        query: pika_marmot_runtime::conversation::RuntimeMessagePageQuery,
    ) -> anyhow::Result<pika_marmot_runtime::conversation::RuntimeMessagePage> {
        pika_marmot_runtime::conversation::ConversationRuntime::new(&self.session.mdk)
            .load_message_page(chat_id, query)
    }

    #[cfg(test)]
    pub(super) fn list_pending_welcome_snapshots(
        &self,
    ) -> anyhow::Result<Vec<pika_marmot_runtime::welcome::PendingWelcomeSnapshot>> {
        pika_marmot_runtime::welcome::list_pending_welcome_snapshots(&self.session.mdk)
    }

    pub(super) fn lookup_pending_welcome(
        &self,
        target: &EventId,
    ) -> anyhow::Result<Option<mdk_storage_traits::welcomes::types::Welcome>> {
        pika_marmot_runtime::welcome::lookup_pending_welcome(&self.session.mdk, target)
    }

    pub(super) fn prepare_outbound_action_for_chat(
        &self,
        chat_id: &str,
        action: OutboundConversationAction,
    ) -> anyhow::Result<PreparedConversationAction> {
        pika_marmot_runtime::outbound::OutboundConversationRuntime::new(&self.session.mdk)
            .prepare_action(self.session.pubkey, chat_id, action)
    }

    pub(super) fn prepare_outbound_action_for_group_ids(
        &self,
        mls_group_id: GroupId,
        nostr_group_id_hex: String,
        action: OutboundConversationAction,
    ) -> anyhow::Result<PreparedConversationAction> {
        pika_marmot_runtime::outbound::OutboundConversationRuntime::new(&self.session.mdk)
            .prepare_action_for_group_ids(
                self.session.pubkey,
                mls_group_id,
                nostr_group_id_hex,
                action,
            )
    }

    #[cfg(test)]
    pub(super) fn complete_outbound_publish_operation(
        &self,
        prepared: PreparedConversationAction,
        publish_status: pika_marmot_runtime::outbound::OutboundConversationPublishStatus,
    ) -> InternalEvent {
        match publish_status {
            pika_marmot_runtime::outbound::OutboundConversationPublishStatus::Published {
                ..
            } => InternalEvent::PublishMessageResult {
                chat_id: prepared.target.nostr_group_id_hex,
                rumor_id: prepared.rumor_id.to_hex(),
                ok: true,
                error: None,
            },
            pika_marmot_runtime::outbound::OutboundConversationPublishStatus::PublishFailed(
                error,
            ) => InternalEvent::PublishMessageResult {
                chat_id: prepared.target.nostr_group_id_hex,
                rumor_id: prepared.rumor_id.to_hex(),
                ok: false,
                error: Some(error),
            },
        }
    }

    #[cfg(test)]
    pub(super) fn complete_call_signal_publish_operation(
        &self,
        kind: pika_marmot_runtime::runtime::CallSignalPublishKind,
        publish_status: pika_marmot_runtime::runtime::CallSignalPublishStatus,
    ) -> InternalEvent {
        let error = match publish_status {
            pika_marmot_runtime::runtime::CallSignalPublishStatus::Published { .. } => None,
            pika_marmot_runtime::runtime::CallSignalPublishStatus::PublishFailed {
                error, ..
            } => Some(error),
        };
        InternalEvent::CallSignalPublishResult { kind, error }
    }

    pub(super) fn prepare_membership_evolution_for_chat(
        &self,
        chat_id: &str,
        key_package_events: &[Event],
    ) -> anyhow::Result<PreparedMembershipEvolution> {
        let snapshot =
            pika_marmot_runtime::conversation::ConversationRuntime::new(&self.session.mdk)
                .lookup_joined_group_snapshot(chat_id)?;
        pika_marmot_runtime::membership::MembershipRuntime::new(&self.session.mdk)
            .prepare_add_members(&snapshot.mls_group_id, key_package_events)
    }

    pub(super) fn prepare_evolution(
        &self,
        mls_group_id: GroupId,
        evolution_event: Event,
        welcome_rumors: Option<Vec<UnsignedEvent>>,
        added_pubkeys: Vec<PublicKey>,
    ) -> anyhow::Result<PreparedMembershipEvolution> {
        pika_marmot_runtime::membership::MembershipRuntime::new(&self.session.mdk)
            .prepare_evolution(mls_group_id, evolution_event, welcome_rumors, added_pubkeys)
    }

    pub(super) fn complete_membership_evolution_operation(
        &self,
        prepared: PreparedMembershipEvolution,
        publish_status: EvolutionPublishStatus,
    ) -> Result<pika_marmot_runtime::membership::MembershipUpdateResult, String> {
        match publish_status {
            EvolutionPublishStatus::Published => Ok(
                pika_marmot_runtime::membership::MembershipRuntime::new(&self.session.mdk)
                    .finalize_published_evolution(prepared),
            ),
            EvolutionPublishStatus::PublishFailed(error) => Err(error),
        }
    }

    pub(super) fn process_group_message_event(
        &self,
        event: Event,
    ) -> anyhow::Result<pika_marmot_runtime::runtime::InboundGroupMessageProcessing> {
        let event_id = event.id;
        match pika_marmot_runtime::conversation::ConversationRuntime::new(&self.session.mdk)
            .process_event(&event)?
        {
            Some(conversation_event) => Ok(
                pika_marmot_runtime::runtime::InboundGroupMessageProcessing::Processed {
                    event_id,
                    conversation_event,
                },
            ),
            None => Ok(
                pika_marmot_runtime::runtime::InboundGroupMessageProcessing::Ignored { event_id },
            ),
        }
    }

    pub(super) fn interpret_runtime_application_message(
        &self,
        runtime_msg: RuntimeApplicationMessage,
    ) -> pika_marmot_runtime::runtime::RuntimeApplicationMessageInterpretation {
        match runtime_msg.classification {
            pika_marmot_runtime::message::MessageClassification::TypingIndicator => {
                pika_marmot_runtime::runtime::RuntimeApplicationMessageInterpretation::TypingIndicator {
                    message: runtime_msg,
                }
            }
            pika_marmot_runtime::message::MessageClassification::CallSignal => {
                pika_marmot_runtime::runtime::RuntimeApplicationMessageInterpretation::CallSignal {
                    parsed_signal: pika_marmot_runtime::call::parse_call_signal(
                        &runtime_msg.message.content,
                    ),
                    message: runtime_msg,
                }
            }
            pika_marmot_runtime::message::MessageClassification::Chat
            | pika_marmot_runtime::message::MessageClassification::Reaction
            | pika_marmot_runtime::message::MessageClassification::Hypernote
            | pika_marmot_runtime::message::MessageClassification::HypernoteResponse => {
                pika_marmot_runtime::runtime::RuntimeApplicationMessageInterpretation::Content {
                    message: runtime_msg,
                }
            }
            pika_marmot_runtime::message::MessageClassification::GroupProfile => {
                pika_marmot_runtime::runtime::RuntimeApplicationMessageInterpretation::GroupProfile {
                    message: runtime_msg,
                }
            }
        }
    }

    pub(super) fn interpret_conversation_event(
        &self,
        event: ConversationEvent,
    ) -> pika_marmot_runtime::runtime::RuntimeConversationEventInterpretation {
        match event {
            ConversationEvent::Application(message) => {
                pika_marmot_runtime::runtime::RuntimeConversationEventInterpretation::Application {
                    message,
                }
            }
            ConversationEvent::GroupUpdate(update) => {
                let is_commit = matches!(
                    update.kind,
                    pika_marmot_runtime::conversation::RuntimeGroupUpdateKind::Commit
                );
                pika_marmot_runtime::runtime::RuntimeConversationEventInterpretation::GroupUpdate {
                    update,
                    is_commit,
                }
            }
            ConversationEvent::UnresolvedGroup { mls_group_id } => {
                pika_marmot_runtime::runtime::RuntimeConversationEventInterpretation::NeedsFullRefresh {
                    reason: pika_marmot_runtime::runtime::RuntimeConversationRefreshReason::UnresolvedGroup {
                        mls_group_id,
                    },
                }
            }
            ConversationEvent::PreviouslyFailed => {
                pika_marmot_runtime::runtime::RuntimeConversationEventInterpretation::NeedsFullRefresh {
                    reason: pika_marmot_runtime::runtime::RuntimeConversationRefreshReason::PreviouslyFailed,
                }
            }
        }
    }

    pub(super) fn interpret_processing_result(
        &self,
        result: MessageProcessingResult,
    ) -> Option<ConversationEvent> {
        pika_marmot_runtime::conversation::ConversationRuntime::new(&self.session.mdk)
            .interpret_processing_result(result)
    }

    pub(super) fn prepare_upload(
        &self,
        mls_group_id: &GroupId,
        bytes: &[u8],
        mime_type: Option<&str>,
        filename: Option<&str>,
    ) -> anyhow::Result<pika_marmot_runtime::media::PreparedMediaUpload> {
        pika_marmot_runtime::media::MediaRuntime::new(&self.session.mdk).prepare_upload(
            mls_group_id,
            bytes,
            mime_type,
            filename,
        )
    }

    pub(super) fn complete_media_upload_operation(
        &self,
        mls_group_id: &GroupId,
        nostr_group_id_hex: String,
        upload: &EncryptedMediaUpload,
        status: ChatMediaUploadStatus,
    ) -> Result<pika_marmot_runtime::runtime::CompletedMediaUpload, String> {
        match status {
            ChatMediaUploadStatus::Uploaded(uploaded_blob) => {
                Ok(pika_marmot_runtime::runtime::CompletedMediaUpload {
                    nostr_group_id_hex,
                    result: pika_marmot_runtime::media::MediaRuntime::new(&self.session.mdk)
                        .finish_upload(mls_group_id, upload, uploaded_blob),
                })
            }
            ChatMediaUploadStatus::UploadFailed(error) => Err(error),
        }
    }

    pub(super) fn decrypt_downloaded_media(
        &self,
        mls_group_id: &GroupId,
        reference: &MediaReference,
        encrypted_data: &[u8],
        expected_encrypted_hash_hex: Option<&str>,
    ) -> anyhow::Result<pika_marmot_runtime::media::RuntimeDownloadedMedia> {
        pika_marmot_runtime::media::MediaRuntime::new(&self.session.mdk).decrypt_downloaded_media(
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
        pika_marmot_runtime::call_runtime::CallWorkflowRuntime::new(&self.session.mdk)
            .prepare_outgoing_invite(target_id, peer_pubkey_hex, call_id, session)
    }

    pub(super) fn prepare_accept_incoming_call(
        &self,
        incoming: &pika_marmot_runtime::call_runtime::PendingIncomingCall,
        group: GroupCallContext<'_>,
    ) -> Result<PreparedAcceptedCall, String> {
        pika_marmot_runtime::call_runtime::CallWorkflowRuntime::new(&self.session.mdk)
            .prepare_accept_incoming(incoming, group)
    }

    pub(super) fn prepare_reject_call_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<pika_marmot_runtime::call_runtime::PreparedCallSignal, String> {
        pika_marmot_runtime::call_runtime::CallWorkflowRuntime::new(&self.session.mdk)
            .prepare_reject_signal(call_id, reason)
    }

    pub(super) fn prepare_end_call_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<pika_marmot_runtime::call_runtime::PreparedCallSignal, String> {
        pika_marmot_runtime::call_runtime::CallWorkflowRuntime::new(&self.session.mdk)
            .prepare_end_signal(call_id, reason)
    }

    pub(super) fn handle_inbound_call_signal(
        &self,
        ctx: InboundSignalContext<'_>,
        signal: ParsedCallSignal,
    ) -> InboundCallSignalOutcome {
        pika_marmot_runtime::call_runtime::CallWorkflowRuntime::new(&self.session.mdk)
            .handle_inbound_signal(ctx, signal)
    }
}
