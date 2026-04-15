use crate::mdk_support::PikaMdk;
use mdk_storage_traits::GroupId;

use super::call_support::{
    build_call_signal_json, derive_call_media_crypto_context, validate_relay_auth_token,
    CallCryptoDeriveContext, CallMediaCryptoContext, CallSessionParams, OutgoingCallSignal,
    ParsedCallSignal,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingIncomingCall {
    pub call_id: String,
    pub target_id: String,
    pub from_pubkey_hex: String,
    pub session: CallSessionParams,
    pub is_video_call: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingOutgoingCall {
    pub call_id: String,
    pub target_id: String,
    pub peer_pubkey_hex: String,
    pub session: CallSessionParams,
    pub is_video_call: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedCallSignal {
    pub call_id: String,
    pub payload_json: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedAcceptedCall {
    pub incoming: PendingIncomingCall,
    pub signal: PreparedCallSignal,
    pub media_crypto: CallMediaCryptoContext,
}

#[derive(Debug, Clone)]
pub(crate) struct AcceptedOutgoingCall {
    pub pending: PendingOutgoingCall,
    pub session: CallSessionParams,
    pub media_crypto: CallMediaCryptoContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InboundCallPolicy {
    pub allow_group_calls: bool,
    pub allow_video_calls: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RejectedIncomingCall {
    pub call_id: String,
    pub reason_code: String,
    pub signal: PreparedCallSignal,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IncomingAcceptFailureKind {
    RelayAuth,
    MediaCrypto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IncomingAcceptFailure {
    pub call_id: String,
    pub kind: IncomingAcceptFailureKind,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteCallTermination {
    pub call_id: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub(crate) enum InboundCallSignalOutcome {
    Ignore,
    RejectIncoming(RejectedIncomingCall),
    IncomingInvite(Box<PendingIncomingCall>),
    OutgoingAccepted(Box<AcceptedOutgoingCall>),
    IncomingAcceptFailed(IncomingAcceptFailure),
    RemoteTermination(RemoteCallTermination),
}

pub(crate) struct CallWorkflowRuntime<'a> {
    mdk: &'a PikaMdk,
}

#[derive(Clone, Copy)]
pub(crate) struct GroupCallContext<'a> {
    pub mls_group_id: &'a GroupId,
    pub local_pubkey_hex: &'a str,
}

pub(crate) struct InboundSignalContext<'a> {
    pub target_id: &'a str,
    pub sender_pubkey_hex: &'a str,
    pub group: GroupCallContext<'a>,
    pub policy: InboundCallPolicy,
    pub has_live_call: bool,
    pub pending_outgoing: Option<&'a PendingOutgoingCall>,
}

impl<'a> CallWorkflowRuntime<'a> {
    pub(crate) fn new(mdk: &'a PikaMdk) -> Self {
        Self { mdk }
    }

    pub(crate) fn prepare_outgoing_invite(
        &self,
        target_id: &str,
        peer_pubkey_hex: &str,
        call_id: &str,
        session: &CallSessionParams,
    ) -> Result<(PendingOutgoingCall, PreparedCallSignal), String> {
        let signal = self.prepare_signal(call_id, OutgoingCallSignal::Invite(session))?;
        let pending = PendingOutgoingCall {
            call_id: call_id.to_string(),
            target_id: target_id.to_string(),
            peer_pubkey_hex: peer_pubkey_hex.to_string(),
            session: session.clone(),
            is_video_call: has_video_track(session),
        };
        Ok((pending, signal))
    }

    pub(crate) fn prepare_accept_incoming(
        &self,
        incoming: &PendingIncomingCall,
        group: GroupCallContext<'_>,
    ) -> Result<PreparedAcceptedCall, String> {
        self.validate_auth(
            group,
            &incoming.call_id,
            &incoming.session,
            &incoming.from_pubkey_hex,
        )?;
        let signal = self.prepare_signal(
            &incoming.call_id,
            OutgoingCallSignal::Accept(&incoming.session),
        )?;
        let media_crypto = self.derive_media_crypto(
            group,
            &incoming.call_id,
            &incoming.session,
            &incoming.from_pubkey_hex,
        )?;
        Ok(PreparedAcceptedCall {
            incoming: incoming.clone(),
            signal,
            media_crypto,
        })
    }

    pub(crate) fn prepare_reject_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<PreparedCallSignal, String> {
        self.prepare_signal(call_id, OutgoingCallSignal::Reject { reason })
    }

    pub(crate) fn prepare_end_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<PreparedCallSignal, String> {
        self.prepare_signal(call_id, OutgoingCallSignal::End { reason })
    }

    pub(crate) fn handle_inbound_signal(
        &self,
        ctx: InboundSignalContext<'_>,
        signal: ParsedCallSignal,
    ) -> InboundCallSignalOutcome {
        match signal {
            ParsedCallSignal::Invite { call_id, session } => {
                if !ctx.policy.allow_group_calls {
                    return self.reject_invite(&call_id, "unsupported_group", None);
                }
                if has_video_track(&session) && !ctx.policy.allow_video_calls {
                    return self.reject_invite(&call_id, "unsupported_video", None);
                }
                if ctx.has_live_call {
                    return self.reject_invite(&call_id, "busy", None);
                }
                match self.validate_auth(ctx.group, &call_id, &session, ctx.sender_pubkey_hex) {
                    Ok(()) => {
                        InboundCallSignalOutcome::IncomingInvite(Box::new(PendingIncomingCall {
                            call_id,
                            target_id: ctx.target_id.to_string(),
                            from_pubkey_hex: ctx.sender_pubkey_hex.to_string(),
                            is_video_call: has_video_track(&session),
                            session,
                        }))
                    }
                    Err(error) => self.reject_invite(&call_id, "auth_failed", Some(error)),
                }
            }
            ParsedCallSignal::Accept { call_id, session } => {
                let Some(pending) = ctx.pending_outgoing else {
                    return InboundCallSignalOutcome::Ignore;
                };
                if pending.call_id != call_id
                    || pending.target_id != ctx.target_id
                    || pending.peer_pubkey_hex != ctx.sender_pubkey_hex
                {
                    return InboundCallSignalOutcome::Ignore;
                }
                if pending.session.relay_auth != session.relay_auth {
                    return InboundCallSignalOutcome::IncomingAcceptFailed(IncomingAcceptFailure {
                        call_id,
                        kind: IncomingAcceptFailureKind::RelayAuth,
                        error: "call relay auth mismatch between invite and accept".to_string(),
                    });
                }
                if let Err(error) =
                    self.validate_auth(ctx.group, &call_id, &session, ctx.sender_pubkey_hex)
                {
                    return InboundCallSignalOutcome::IncomingAcceptFailed(IncomingAcceptFailure {
                        call_id,
                        kind: IncomingAcceptFailureKind::RelayAuth,
                        error,
                    });
                }
                match self.derive_media_crypto(ctx.group, &call_id, &session, ctx.sender_pubkey_hex)
                {
                    Ok(media_crypto) => {
                        InboundCallSignalOutcome::OutgoingAccepted(Box::new(AcceptedOutgoingCall {
                            pending: pending.clone(),
                            session,
                            media_crypto,
                        }))
                    }
                    Err(error) => {
                        InboundCallSignalOutcome::IncomingAcceptFailed(IncomingAcceptFailure {
                            call_id,
                            kind: IncomingAcceptFailureKind::MediaCrypto,
                            error,
                        })
                    }
                }
            }
            ParsedCallSignal::Reject { call_id, reason }
            | ParsedCallSignal::End { call_id, reason } => {
                InboundCallSignalOutcome::RemoteTermination(RemoteCallTermination {
                    call_id,
                    reason,
                })
            }
        }
    }

    fn prepare_signal(
        &self,
        call_id: &str,
        signal: OutgoingCallSignal<'_>,
    ) -> Result<PreparedCallSignal, String> {
        let payload_json = build_call_signal_json(call_id, signal)
            .map_err(|e| format!("serialize call signal failed: {e}"))?;
        Ok(PreparedCallSignal {
            call_id: call_id.to_string(),
            payload_json,
        })
    }

    fn reject_invite(
        &self,
        call_id: &str,
        reason_code: &str,
        error: Option<String>,
    ) -> InboundCallSignalOutcome {
        match self.prepare_reject_signal(call_id, reason_code) {
            Ok(signal) => InboundCallSignalOutcome::RejectIncoming(RejectedIncomingCall {
                call_id: call_id.to_string(),
                reason_code: reason_code.to_string(),
                signal,
                error,
            }),
            Err(err) => InboundCallSignalOutcome::IncomingAcceptFailed(IncomingAcceptFailure {
                call_id: call_id.to_string(),
                kind: IncomingAcceptFailureKind::RelayAuth,
                error: error.unwrap_or(err),
            }),
        }
    }

    fn validate_auth(
        &self,
        group: GroupCallContext<'_>,
        call_id: &str,
        session: &CallSessionParams,
        peer_pubkey_hex: &str,
    ) -> Result<(), String> {
        let derive_ctx = CallCryptoDeriveContext {
            mdk: self.mdk,
            mls_group_id: group.mls_group_id,
            group_epoch: 0,
            call_id,
            session,
            local_pubkey_hex: group.local_pubkey_hex,
            peer_pubkey_hex,
        };
        validate_relay_auth_token(&derive_ctx)
    }

    fn derive_media_crypto(
        &self,
        group: GroupCallContext<'_>,
        call_id: &str,
        session: &CallSessionParams,
        peer_pubkey_hex: &str,
    ) -> Result<CallMediaCryptoContext, String> {
        let group_epoch = self
            .mdk
            .get_group(group.mls_group_id)
            .map_err(|e| format!("load mls group failed: {e}"))?
            .ok_or_else(|| "mls group not found".to_string())?
            .epoch;
        let derive_ctx = CallCryptoDeriveContext {
            mdk: self.mdk,
            mls_group_id: group.mls_group_id,
            group_epoch,
            call_id,
            session,
            local_pubkey_hex: group.local_pubkey_hex,
            peer_pubkey_hex,
        };
        let video_track = has_video_track(session).then_some("video0");
        derive_call_media_crypto_context(&derive_ctx, "audio0", video_track)
    }
}

fn has_video_track(session: &CallSessionParams) -> bool {
    session.tracks.iter().any(|track| track.name == "video0")
}
