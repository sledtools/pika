use super::*;
use crate::updates::ChatMediaUploadStatus;
#[cfg(test)]
use crate::updates::InternalEvent;

pub(super) struct AppHostContext<'a> {
    session: &'a Session,
}

#[derive(Debug, Clone)]
pub(super) enum AppApplicationMessageInterpretation {
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

#[derive(Debug, Clone)]
pub(super) enum AppConversationRefreshReason {
    UnresolvedGroup { mls_group_id: GroupId },
    PreviouslyFailed,
}

#[derive(Debug, Clone)]
pub(super) enum AppConversationEventInterpretation {
    Application {
        message: Box<RuntimeApplicationMessage>,
    },
    GroupUpdate {
        update: RuntimeGroupUpdate,
        is_commit: bool,
    },
    NeedsFullRefresh {
        reason: AppConversationRefreshReason,
    },
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
    ) -> anyhow::Result<super::conversation_support::JoinedGroupSnapshot> {
        super::conversation_support::lookup_joined_group_snapshot(&self.session.mdk, chat_id)
    }

    pub(super) fn current_pubkey_hex(&self) -> String {
        self.session.pubkey.to_hex()
    }

    pub(super) fn list_joined_group_snapshots(
        &self,
    ) -> anyhow::Result<Vec<super::conversation_support::JoinedGroupSnapshot>> {
        super::conversation_support::list_joined_group_snapshots(&self.session.mdk)
    }

    pub(super) fn load_message_page(
        &self,
        chat_id: &str,
        query: super::conversation_support::MessagePageQuery,
    ) -> anyhow::Result<super::conversation_support::MessagePage> {
        super::conversation_support::load_message_page(&self.session.mdk, chat_id, query)
    }

    #[cfg(test)]
    pub(super) fn list_pending_welcome_snapshots(
        &self,
    ) -> anyhow::Result<Vec<super::welcome_support::PendingWelcomeSnapshot>> {
        super::welcome_support::list_pending_welcome_snapshots(&self.session.mdk)
    }

    pub(super) fn lookup_pending_welcome(
        &self,
        target: &EventId,
    ) -> anyhow::Result<Option<mdk_storage_traits::welcomes::types::Welcome>> {
        super::welcome_support::lookup_pending_welcome(&self.session.mdk, target)
    }

    pub(super) fn prepare_outbound_action_for_chat(
        &self,
        chat_id: &str,
        action: OutboundConversationAction,
    ) -> anyhow::Result<PreparedConversationAction> {
        super::outbound_support::prepare_action(
            &self.session.mdk,
            self.session.pubkey,
            chat_id,
            action,
        )
    }

    pub(super) fn prepare_outbound_action_for_group_ids(
        &self,
        mls_group_id: GroupId,
        nostr_group_id_hex: String,
        action: OutboundConversationAction,
    ) -> anyhow::Result<PreparedConversationAction> {
        super::outbound_support::prepare_action_for_group_ids(
            &self.session.mdk,
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
        publish_status: super::outbound_support::OutboundConversationPublishStatus,
    ) -> InternalEvent {
        match publish_status {
            super::outbound_support::OutboundConversationPublishStatus::Published => {
                InternalEvent::PublishMessageResult {
                    chat_id: prepared.target.nostr_group_id_hex,
                    rumor_id: prepared.rumor_id.to_hex(),
                    ok: true,
                    error: None,
                }
            }
            super::outbound_support::OutboundConversationPublishStatus::PublishFailed(error) => {
                InternalEvent::PublishMessageResult {
                    chat_id: prepared.target.nostr_group_id_hex,
                    rumor_id: prepared.rumor_id.to_hex(),
                    ok: false,
                    error: Some(error),
                }
            }
        }
    }

    #[cfg(test)]
    pub(super) fn complete_call_signal_publish_operation(
        &self,
        kind: CallSignalPublishKind,
        error: Option<String>,
    ) -> InternalEvent {
        InternalEvent::CallSignalPublishResult { kind, error }
    }

    pub(super) fn prepare_membership_evolution_for_chat(
        &self,
        chat_id: &str,
        key_package_events: &[Event],
    ) -> anyhow::Result<PreparedMembershipEvolution> {
        let snapshot =
            super::conversation_support::lookup_joined_group_snapshot(&self.session.mdk, chat_id)?;
        super::membership_support::prepare_add_members(
            &self.session.mdk,
            &snapshot.mls_group_id,
            key_package_events,
        )
    }

    pub(super) fn prepare_evolution(
        &self,
        mls_group_id: GroupId,
        evolution_event: Event,
        welcome_rumors: Option<Vec<UnsignedEvent>>,
        added_pubkeys: Vec<PublicKey>,
    ) -> anyhow::Result<PreparedMembershipEvolution> {
        super::membership_support::prepare_evolution(
            &self.session.mdk,
            mls_group_id,
            evolution_event,
            welcome_rumors,
            added_pubkeys,
        )
    }

    pub(super) fn complete_membership_evolution_operation(
        &self,
        prepared: PreparedMembershipEvolution,
        publish_status: EvolutionPublishStatus,
    ) -> Result<super::membership_support::MembershipUpdateResult, String> {
        match publish_status {
            EvolutionPublishStatus::Published => {
                Ok(super::membership_support::finalize_published_evolution(
                    &self.session.mdk,
                    prepared,
                ))
            }
            EvolutionPublishStatus::PublishFailed(error) => Err(error),
        }
    }

    pub(super) fn process_group_message_event(
        &self,
        event: Event,
    ) -> anyhow::Result<Option<ConversationEvent>> {
        super::conversation_support::process_event(&self.session.mdk, &event)
    }

    pub(super) fn interpret_application_message(
        &self,
        runtime_msg: RuntimeApplicationMessage,
    ) -> AppApplicationMessageInterpretation {
        match runtime_msg.classification {
            super::message_support::MessageClassification::TypingIndicator => {
                AppApplicationMessageInterpretation::TypingIndicator {
                    message: runtime_msg,
                }
            }
            super::message_support::MessageClassification::CallSignal => {
                AppApplicationMessageInterpretation::CallSignal {
                    parsed_signal: super::call_support::parse_call_signal(
                        &runtime_msg.message.content,
                    ),
                    message: runtime_msg,
                }
            }
            super::message_support::MessageClassification::Chat
            | super::message_support::MessageClassification::Reaction
            | super::message_support::MessageClassification::Hypernote
            | super::message_support::MessageClassification::HypernoteResponse => {
                AppApplicationMessageInterpretation::Content {
                    message: runtime_msg,
                }
            }
            super::message_support::MessageClassification::GroupProfile => {
                AppApplicationMessageInterpretation::GroupProfile {
                    message: runtime_msg,
                }
            }
        }
    }

    pub(super) fn interpret_conversation_event(
        &self,
        event: ConversationEvent,
    ) -> AppConversationEventInterpretation {
        match event {
            ConversationEvent::Application(message) => {
                AppConversationEventInterpretation::Application { message }
            }
            ConversationEvent::GroupUpdate(update) => {
                let is_commit = matches!(
                    update.kind,
                    super::conversation_support::RuntimeGroupUpdateKind::Commit
                );
                AppConversationEventInterpretation::GroupUpdate { update, is_commit }
            }
            ConversationEvent::UnresolvedGroup { mls_group_id } => {
                AppConversationEventInterpretation::NeedsFullRefresh {
                    reason: AppConversationRefreshReason::UnresolvedGroup { mls_group_id },
                }
            }
            ConversationEvent::PreviouslyFailed => {
                AppConversationEventInterpretation::NeedsFullRefresh {
                    reason: AppConversationRefreshReason::PreviouslyFailed,
                }
            }
        }
    }

    pub(super) fn interpret_processing_result(
        &self,
        result: MessageProcessingResult,
    ) -> Option<ConversationEvent> {
        super::conversation_support::interpret_processing_result(&self.session.mdk, result)
    }

    pub(super) fn prepare_upload(
        &self,
        mls_group_id: &GroupId,
        bytes: &[u8],
        mime_type: Option<&str>,
        filename: Option<&str>,
    ) -> anyhow::Result<super::media_support::PreparedMediaUpload> {
        super::media_support::prepare_upload(
            &self.session.mdk,
            mls_group_id,
            bytes,
            mime_type,
            filename,
        )
    }

    pub(super) fn complete_media_upload_operation(
        &self,
        mls_group_id: &GroupId,
        upload: &EncryptedMediaUpload,
        status: ChatMediaUploadStatus,
    ) -> Result<super::media_support::MediaUploadResult, String> {
        match status {
            ChatMediaUploadStatus::Uploaded(uploaded_blob) => {
                Ok(super::media_support::finish_upload(
                    &self.session.mdk,
                    mls_group_id,
                    upload,
                    uploaded_blob,
                ))
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
    ) -> anyhow::Result<super::media_support::DownloadedMedia> {
        super::media_support::decrypt_downloaded_media(
            &self.session.mdk,
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
            super::call_workflow::PendingOutgoingCall,
            super::call_workflow::PreparedCallSignal,
        ),
        String,
    > {
        super::call_workflow::CallWorkflowRuntime::new(&self.session.mdk).prepare_outgoing_invite(
            target_id,
            peer_pubkey_hex,
            call_id,
            session,
        )
    }

    pub(super) fn prepare_accept_incoming_call(
        &self,
        incoming: &super::call_workflow::PendingIncomingCall,
        group: GroupCallContext<'_>,
    ) -> Result<PreparedAcceptedCall, String> {
        super::call_workflow::CallWorkflowRuntime::new(&self.session.mdk)
            .prepare_accept_incoming(incoming, group)
    }

    pub(super) fn prepare_reject_call_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<super::call_workflow::PreparedCallSignal, String> {
        super::call_workflow::CallWorkflowRuntime::new(&self.session.mdk)
            .prepare_reject_signal(call_id, reason)
    }

    pub(super) fn prepare_end_call_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<super::call_workflow::PreparedCallSignal, String> {
        super::call_workflow::CallWorkflowRuntime::new(&self.session.mdk)
            .prepare_end_signal(call_id, reason)
    }

    pub(super) fn handle_inbound_call_signal(
        &self,
        ctx: InboundSignalContext<'_>,
        signal: ParsedCallSignal,
    ) -> InboundCallSignalOutcome {
        super::call_workflow::CallWorkflowRuntime::new(&self.session.mdk)
            .handle_inbound_signal(ctx, signal)
    }
}
