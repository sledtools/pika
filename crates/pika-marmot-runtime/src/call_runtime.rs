use crate::PikaMdk;
use crate::call::{
    CallCryptoDeriveContext, CallMediaCryptoContext, CallSessionParams, OutgoingCallSignal,
    ParsedCallSignal, build_call_signal_json, derive_call_media_crypto_context,
    validate_relay_auth_token,
};
use mdk_core::prelude::GroupId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingIncomingCall {
    pub call_id: String,
    pub target_id: String,
    pub from_pubkey_hex: String,
    pub session: CallSessionParams,
    pub is_video_call: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOutgoingCall {
    pub call_id: String,
    pub target_id: String,
    pub peer_pubkey_hex: String,
    pub session: CallSessionParams,
    pub is_video_call: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCallSignal {
    pub call_id: String,
    pub payload_json: String,
}

#[derive(Debug, Clone)]
pub struct PreparedAcceptedCall {
    pub incoming: PendingIncomingCall,
    pub signal: PreparedCallSignal,
    pub media_crypto: CallMediaCryptoContext,
}

#[derive(Debug, Clone)]
pub struct AcceptedOutgoingCall {
    pub pending: PendingOutgoingCall,
    pub session: CallSessionParams,
    pub media_crypto: CallMediaCryptoContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboundCallPolicy {
    pub allow_group_calls: bool,
    pub allow_video_calls: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedIncomingCall {
    pub call_id: String,
    pub reason_code: String,
    pub signal: PreparedCallSignal,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncomingAcceptFailureKind {
    RelayAuth,
    MediaCrypto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingAcceptFailure {
    pub call_id: String,
    pub kind: IncomingAcceptFailureKind,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCallTermination {
    pub call_id: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub enum InboundCallSignalOutcome {
    Ignore,
    RejectIncoming(RejectedIncomingCall),
    IncomingInvite(Box<PendingIncomingCall>),
    OutgoingAccepted(Box<AcceptedOutgoingCall>),
    IncomingAcceptFailed(IncomingAcceptFailure),
    RemoteTermination(RemoteCallTermination),
}

pub struct CallWorkflowRuntime<'a> {
    mdk: &'a PikaMdk,
}

#[derive(Clone, Copy)]
pub struct GroupCallContext<'a> {
    pub mls_group_id: &'a GroupId,
    pub local_pubkey_hex: &'a str,
}

pub struct InboundSignalContext<'a> {
    pub target_id: &'a str,
    pub sender_pubkey_hex: &'a str,
    pub group: GroupCallContext<'a>,
    pub policy: InboundCallPolicy,
    pub has_live_call: bool,
    pub pending_outgoing: Option<&'a PendingOutgoingCall>,
}

impl<'a> CallWorkflowRuntime<'a> {
    pub fn new(mdk: &'a PikaMdk) -> Self {
        Self { mdk }
    }

    pub fn prepare_outgoing_invite(
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

    pub fn prepare_accept_incoming(
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

    pub fn prepare_reject_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<PreparedCallSignal, String> {
        self.prepare_signal(call_id, OutgoingCallSignal::Reject { reason })
    }

    pub fn prepare_end_signal(
        &self,
        call_id: &str,
        reason: &str,
    ) -> Result<PreparedCallSignal, String> {
        self.prepare_signal(call_id, OutgoingCallSignal::End { reason })
    }

    pub fn handle_inbound_signal(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::{CallTrackSpec, derive_relay_auth_token};
    use crate::membership::MembershipRuntime;
    use crate::open_mdk;
    use mdk_core::prelude::NostrGroupConfigData;
    use nostr_sdk::prelude::{Event, EventBuilder, Keys, Kind, RelayUrl};

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

    fn make_group() -> (
        &'static PikaMdk,
        &'static PikaMdk,
        GroupId,
        Keys,
        Keys,
        CallSessionParams,
        CallWorkflowRuntime<'static>,
    ) {
        let inviter_dir = Box::leak(Box::new(tempfile::tempdir().expect("inviter tempdir")));
        let invitee_dir = Box::leak(Box::new(tempfile::tempdir().expect("invitee tempdir")));
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Call runtime test".to_string(),
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
        let welcome_rumor = created
            .welcome_rumors
            .into_iter()
            .next()
            .expect("welcome rumor");
        let wrapper = tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                EventBuilder::gift_wrap(
                    &inviter_keys,
                    &invitee_keys.public_key(),
                    welcome_rumor,
                    [],
                )
                .await
                .expect("build giftwrap")
            });
        tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(async {
                crate::welcome::ingest_welcome_from_giftwrap(
                    &invitee_mdk,
                    &invitee_keys,
                    &wrapper,
                    |_| true,
                )
                .await
                .expect("ingest welcome")
                .expect("welcome should ingest");
            });
        let pending = invitee_mdk
            .get_pending_welcomes(None)
            .expect("get pending welcomes");
        invitee_mdk
            .accept_welcome(pending.first().expect("pending welcome"))
            .expect("accept welcome");

        let call_id = "call-runtime-test";
        let mut session = CallSessionParams {
            moq_url: "https://moq.example.com/anon".to_string(),
            broadcast_base: format!("pika/calls/{call_id}"),
            relay_auth: String::new(),
            tracks: vec![CallTrackSpec::audio0_opus_default()],
        };
        let relay_auth = derive_relay_auth_token(&CallCryptoDeriveContext {
            mdk: &inviter_mdk,
            mls_group_id: &created.group.mls_group_id,
            group_epoch: 0,
            call_id,
            session: &session,
            local_pubkey_hex: &inviter_keys.public_key().to_hex(),
            peer_pubkey_hex: &invitee_keys.public_key().to_hex(),
        })
        .expect("derive relay auth");
        session.relay_auth = relay_auth;

        let leaked_inviter_mdk = Box::leak(Box::new(inviter_mdk));
        let leaked_mdk = Box::leak(Box::new(invitee_mdk));
        (
            leaked_inviter_mdk,
            leaked_mdk,
            created.group.mls_group_id,
            inviter_keys,
            invitee_keys,
            session,
            CallWorkflowRuntime::new(leaked_mdk),
        )
    }

    #[test]
    fn prepare_accept_incoming_validates_and_derives_media_crypto() {
        let (inviter_mdk, invitee_mdk, group_id, inviter_keys, invitee_keys, mut session, runtime) =
            make_group();
        let peer_dir = tempfile::tempdir().expect("peer tempdir");
        let peer_keys = Keys::generate();
        let peer_mdk = open_mdk(peer_dir.path()).expect("open peer mdk");
        let peer_kp = make_key_package_event(&peer_mdk, &peer_keys);
        let prepared_evolution = MembershipRuntime::new(inviter_mdk)
            .prepare_add_members(&group_id, &[peer_kp])
            .expect("prepare add member");
        inviter_mdk
            .merge_pending_commit(&group_id)
            .expect("merge pending commit");
        invitee_mdk
            .process_message(&prepared_evolution.evolution_event)
            .expect("process evolution");
        let group_epoch = invitee_mdk
            .get_group(&group_id)
            .expect("load group")
            .expect("group should exist")
            .epoch;
        assert!(group_epoch > 0, "test should exercise non-zero group epoch");
        session.relay_auth = derive_relay_auth_token(&CallCryptoDeriveContext {
            mdk: invitee_mdk,
            mls_group_id: &group_id,
            group_epoch: 0,
            call_id: "call-runtime-test",
            session: &session,
            local_pubkey_hex: &invitee_keys.public_key().to_hex(),
            peer_pubkey_hex: &inviter_keys.public_key().to_hex(),
        })
        .expect("derive relay auth after evolution");
        let incoming = PendingIncomingCall {
            call_id: "call-runtime-test".to_string(),
            target_id: "chat1".to_string(),
            from_pubkey_hex: inviter_keys.public_key().to_hex(),
            is_video_call: false,
            session,
        };

        let prepared = runtime
            .prepare_accept_incoming(
                &incoming,
                GroupCallContext {
                    mls_group_id: &group_id,
                    local_pubkey_hex: &invitee_keys.public_key().to_hex(),
                },
            )
            .expect("prepare accept");

        assert_eq!(prepared.incoming.call_id, "call-runtime-test");
        assert!(!prepared.signal.payload_json.is_empty());
        assert_eq!(prepared.media_crypto.tx_keys.epoch, group_epoch);
        assert_eq!(prepared.media_crypto.rx_keys.epoch, group_epoch);
        assert!(prepared.media_crypto.video_tx_keys.is_none());
    }

    #[test]
    fn handle_inbound_signal_rejects_video_when_policy_disabled() {
        let (_inviter_mdk, _mdk, group_id, inviter_keys, invitee_keys, mut session, runtime) =
            make_group();
        session.tracks.push(CallTrackSpec::video0_h264_default());
        let outcome = runtime.handle_inbound_signal(
            InboundSignalContext {
                target_id: "chat1",
                sender_pubkey_hex: &inviter_keys.public_key().to_hex(),
                group: GroupCallContext {
                    mls_group_id: &group_id,
                    local_pubkey_hex: &invitee_keys.public_key().to_hex(),
                },
                policy: InboundCallPolicy {
                    allow_group_calls: true,
                    allow_video_calls: false,
                },
                has_live_call: false,
                pending_outgoing: None,
            },
            ParsedCallSignal::Invite {
                call_id: "call-runtime-test".to_string(),
                session,
            },
        );

        match outcome {
            InboundCallSignalOutcome::RejectIncoming(rejected) => {
                assert_eq!(rejected.reason_code, "unsupported_video");
            }
            _ => panic!("expected video reject"),
        }
    }

    #[test]
    fn handle_inbound_signal_accepts_matching_pending_outgoing() {
        let (_inviter_mdk, _mdk, group_id, inviter_keys, invitee_keys, session, runtime) =
            make_group();
        let pending = PendingOutgoingCall {
            call_id: "call-runtime-test".to_string(),
            target_id: "chat1".to_string(),
            peer_pubkey_hex: inviter_keys.public_key().to_hex(),
            is_video_call: false,
            session: session.clone(),
        };

        let outcome = runtime.handle_inbound_signal(
            InboundSignalContext {
                target_id: "chat1",
                sender_pubkey_hex: &inviter_keys.public_key().to_hex(),
                group: GroupCallContext {
                    mls_group_id: &group_id,
                    local_pubkey_hex: &invitee_keys.public_key().to_hex(),
                },
                policy: InboundCallPolicy {
                    allow_group_calls: true,
                    allow_video_calls: true,
                },
                has_live_call: false,
                pending_outgoing: Some(&pending),
            },
            ParsedCallSignal::Accept {
                call_id: "call-runtime-test".to_string(),
                session,
            },
        );

        match outcome {
            InboundCallSignalOutcome::OutgoingAccepted(accepted) => {
                assert_eq!(accepted.pending.call_id, "call-runtime-test");
            }
            _ => panic!("expected outgoing accept"),
        }
    }
}
