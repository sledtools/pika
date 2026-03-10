use super::*;
use crate::state::CallStatus;
use pika_marmot_runtime::call::{
    derive_relay_auth_token as derive_shared_relay_auth_token, DEFAULT_CALL_BROADCAST_PREFIX,
};
use pika_marmot_runtime::call_runtime::{
    GroupCallContext, InboundCallPolicy, InboundCallSignalOutcome, InboundSignalContext,
    PendingIncomingCall, PendingOutgoingCall, PreparedAcceptedCall,
};

pub(super) use pika_marmot_runtime::call::{
    CallCryptoDeriveContext, CallSessionParams, CallTrackSpec, ParsedCallSignal,
};

#[cfg(test)]
use pika_marmot_runtime::call::{parse_call_signal, valid_relay_auth_token};

/// Type-safe call end reasons. Converted to strings for the UniFFI `CallStatus::Ended { reason }`
/// and for the wire protocol (`call.end` / `call.reject` signals).
#[derive(Debug, Clone)]
pub(super) enum CallEndReason {
    UserHangup,
    Timeout,
    Declined,
    RuntimeError,
    AuthFailed,
    SerializeFailed,
    PublishFailed,
    /// Reason received from the network (reject/end signal from peer).
    Remote(String),
}

impl CallEndReason {
    pub fn as_str(&self) -> &str {
        match self {
            Self::UserHangup => "user_hangup",
            Self::Timeout => "timeout",
            Self::Declined => "declined",
            Self::RuntimeError => "runtime_error",
            Self::AuthFailed => "auth_failed",
            Self::SerializeFailed => "serialize_failed",
            Self::PublishFailed => "publish_failed",
            Self::Remote(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for CallEndReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AppCore {
    fn has_live_call(&self) -> bool {
        self.state
            .active_call
            .as_ref()
            .map(|c| {
                matches!(
                    c.status,
                    CallStatus::Offering
                        | CallStatus::Ringing
                        | CallStatus::Connecting
                        | CallStatus::Active
                )
            })
            .unwrap_or(false)
    }

    fn call_session_from_config(
        &self,
        call_id: &str,
        include_video: bool,
    ) -> Option<CallSessionParams> {
        let moq_url = self
            .config
            .call_moq_url
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)?;
        let prefix = self
            .config
            .call_broadcast_prefix
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_CALL_BROADCAST_PREFIX)
            .trim_matches('/')
            .to_string();
        if prefix.is_empty() {
            return None;
        }

        let mut tracks = vec![CallTrackSpec::audio0_opus_default()];
        if include_video {
            tracks.push(CallTrackSpec::video0_h264_default());
        }

        Some(CallSessionParams {
            moq_url,
            broadcast_base: format!("{prefix}/{call_id}"),
            relay_auth: String::new(),
            tracks,
        })
    }

    fn current_peer_npub(&self, chat_id: &str) -> Option<String> {
        let entry = self.session.as_ref()?.groups.get(chat_id)?;
        if entry.is_group {
            return None;
        }
        if let Some(peer) = entry.members.first() {
            return peer.pubkey.to_bech32().ok();
        }

        // "Note to self" DMs have no members besides self. Allow them to participate in
        // call UI/state-machine flows (useful for local/offline tests).
        self.session.as_ref()?.pubkey.to_bech32().ok()
    }

    fn current_pubkey_hex(&self) -> Option<String> {
        self.session.as_ref().map(|s| s.pubkey.to_hex())
    }

    pub(super) fn prepare_call_accept_for_chat(
        &self,
        chat_id: &str,
        active: &crate::state::CallState,
        session: &CallSessionParams,
    ) -> Result<PreparedAcceptedCall, String> {
        let sess = self
            .session
            .as_ref()
            .ok_or_else(|| "no active session".to_string())?;
        let group = sess
            .groups
            .get(chat_id)
            .ok_or_else(|| "chat not found".to_string())?;
        let local_pubkey_hex = sess.host_context().current_pubkey_hex();
        let peer_pubkey_hex = PublicKey::parse(&active.peer_npub)
            .map_err(|e| format!("Peer pubkey parse failed: {e}"))?
            .to_hex();
        let incoming = PendingIncomingCall {
            call_id: active.call_id.clone(),
            target_id: chat_id.to_string(),
            from_pubkey_hex: peer_pubkey_hex,
            session: session.clone(),
            is_video_call: active.is_video_call,
        };
        sess.host_context().prepare_accept_incoming_call(
            &incoming,
            GroupCallContext {
                mls_group_id: &group.mls_group_id,
                local_pubkey_hex: &local_pubkey_hex,
            },
        )
    }

    pub(super) fn derive_relay_auth_token(
        &self,
        chat_id: &str,
        call_id: &str,
        session: &CallSessionParams,
        local_pubkey_hex: &str,
        peer_pubkey_hex: &str,
    ) -> Result<String, String> {
        let sess = self
            .session
            .as_ref()
            .ok_or_else(|| "no active session".to_string())?;
        let group_entry = sess
            .groups
            .get(chat_id)
            .ok_or_else(|| "chat group not found".to_string())?;
        let derive_ctx = CallCryptoDeriveContext {
            mdk: &sess.mdk,
            mls_group_id: &group_entry.mls_group_id,
            group_epoch: 0,
            call_id,
            session,
            local_pubkey_hex,
            peer_pubkey_hex,
        };

        derive_shared_relay_auth_token(&derive_ctx)
    }

    fn publish_call_signal(
        &mut self,
        chat_id: &str,
        payload_json: String,
        failure_context: &'static str,
    ) -> Result<(), String> {
        let network_enabled = self.network_enabled();
        let fallback_relays = self.default_relays();

        let (client, wrapper, relays) = {
            let Some(sess) = self.session.as_mut() else {
                return Err("no active session".to_string());
            };
            let Some(group) = sess.groups.get(chat_id).cloned() else {
                return Err("chat not found".to_string());
            };

            let rumor = UnsignedEvent::new(
                sess.pubkey,
                Timestamp::from(now_seconds() as u64),
                super::CALL_SIGNAL_KIND,
                [],
                payload_json,
            );

            let wrapper = sess
                .mdk
                .create_message(&group.mls_group_id, rumor)
                .map_err(|e| format!("encrypt call signal failed: {e}"))?;

            let relays: Vec<RelayUrl> = if network_enabled {
                sess.mdk
                    .get_relays(&group.mls_group_id)
                    .ok()
                    .map(|s| s.into_iter().collect())
                    .filter(|v: &Vec<RelayUrl>| !v.is_empty())
                    .unwrap_or_else(|| fallback_relays.clone())
            } else {
                vec![]
            };

            (sess.client.clone(), wrapper, relays)
        };

        if !network_enabled {
            return Ok(());
        }

        let tx = self.core_sender.clone();
        self.runtime.spawn(async move {
            let outcome = super::relay_publish::publish_event_with_retry(
                &client,
                &relays,
                &wrapper,
                4,
                failure_context,
                false,
            )
            .await;
            if let super::relay_publish::PublishOutcome::Err(err) = outcome {
                let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::Toast(format!(
                    "{failure_context}: {err}",
                )))));
            }
        });
        Ok(())
    }

    fn update_call_status(&mut self, status: CallStatus) {
        let should_tick = matches!(status, CallStatus::Active);
        let cancel_offer_timeout = matches!(status, CallStatus::Connecting | CallStatus::Active);
        let previous = self.state.active_call.clone();
        let mut should_tick_after_update: Option<bool> = None;
        if let Some(call) = self.state.active_call.as_mut() {
            call.set_status(status);
            call.refresh_duration_display(now_seconds());
            should_tick_after_update = Some(should_tick && call.started_at.is_some());
        }
        let Some(should_tick_after_update) = should_tick_after_update else {
            return;
        };
        if cancel_offer_timeout {
            self.cancel_call_offer_timeout();
        }
        if should_tick_after_update {
            self.ensure_call_duration_ticks();
        } else {
            self.cancel_call_duration_ticks();
        }
        self.emit_call_state_with_previous(previous);
    }

    fn end_call_local(&mut self, reason: CallEndReason) {
        let previous = self.state.active_call.clone();
        let mut should_emit = false;
        if let Some(call) = self.state.active_call.as_mut() {
            call.set_status(CallStatus::Ended {
                reason: reason.to_string(),
            });
            call.refresh_duration_display(now_seconds());
            self.call_runtime.on_call_ended(&call.call_id);
            self.call_session_params = None;
            should_emit = true;
        }
        if !should_emit {
            return;
        }
        self.cancel_call_offer_timeout();
        self.cancel_call_duration_ticks();
        self.emit_call_state_with_previous(previous);
    }

    pub(super) fn handle_start_call_action(&mut self, chat_id: &str) {
        self.start_call_internal(chat_id, false);
    }

    pub(super) fn handle_start_video_call_action(&mut self, chat_id: &str) {
        self.start_call_internal(chat_id, true);
    }

    fn start_call_internal(&mut self, chat_id: &str, is_video_call: bool) {
        if !self.is_logged_in() {
            self.toast("Please log in first");
            return;
        }
        if !self.chat_exists(chat_id) {
            self.toast("Chat not found");
            return;
        }
        if self.has_live_call() {
            self.toast("Already in a call");
            return;
        }

        let network_enabled = self.network_enabled();
        let call_id = uuid::Uuid::new_v4().to_string();
        let Some(peer_npub) = self.current_peer_npub(chat_id) else {
            self.toast("Chat peer not found");
            return;
        };
        let Some(mut session) = self.call_session_from_config(&call_id, is_video_call) else {
            self.toast("Call config missing: set `call_moq_url` in pika_config.json");
            return;
        };

        if !network_enabled {
            let previous = self.state.active_call.clone();
            self.cancel_call_duration_ticks();
            self.state.active_call = Some(crate::state::CallState::new(
                call_id.clone(),
                chat_id.to_string(),
                peer_npub,
                CallStatus::Offering,
                None,
                false,
                is_video_call,
                None,
            ));
            self.call_session_params = Some(session);
            self.schedule_call_offer_timeout();
            self.emit_call_state_with_previous(previous);
            tracing::info!(call_id = %call_id, is_video_call, "call_start_offline");
            return;
        }

        let Some(local_pubkey_hex) = self.current_pubkey_hex() else {
            self.toast("No local pubkey for call setup");
            return;
        };
        let peer_pubkey_hex = match PublicKey::parse(&peer_npub) {
            Ok(pk) => pk.to_hex(),
            Err(e) => {
                self.toast(format!("Peer pubkey parse failed: {e}"));
                return;
            }
        };
        session.relay_auth = match self.derive_relay_auth_token(
            chat_id,
            &call_id,
            &session,
            &local_pubkey_hex,
            &peer_pubkey_hex,
        ) {
            Ok(token) => token,
            Err(err) => {
                self.toast(format!("Call relay auth setup failed: {err}"));
                return;
            }
        };

        let previous = self.state.active_call.clone();
        self.cancel_call_duration_ticks();
        self.state.active_call = Some(crate::state::CallState::new(
            call_id.clone(),
            chat_id.to_string(),
            peer_npub,
            CallStatus::Offering,
            None,
            false,
            is_video_call,
            None,
        ));
        self.call_session_params = Some(session.clone());
        self.schedule_call_offer_timeout();
        self.emit_call_state_with_previous(previous);

        let payload = match self.session.as_ref() {
            Some(sess) => match sess.host_context().prepare_outgoing_call_invite(
                chat_id,
                &peer_pubkey_hex,
                &call_id,
                &session,
            ) {
                Ok((_, signal)) => signal.payload_json,
                Err(err) => {
                    self.toast(format!("Serialize invite failed: {err}"));
                    self.end_call_local(CallEndReason::SerializeFailed);
                    return;
                }
            },
            None => {
                self.toast("No active session".to_string());
                self.end_call_local(CallEndReason::RuntimeError);
                return;
            }
        };
        // Never log the full invite payload: it includes `relay_auth` (cap token).
        tracing::info!(
            call_id = %call_id,
            moq_url = %session.moq_url,
            broadcast_base = %session.broadcast_base,
            tracks = session.tracks.len(),
            "call_invite"
        );
        if let Err(e) = self.publish_call_signal(chat_id, payload, "Call invite publish failed") {
            self.toast(e);
            self.end_call_local(CallEndReason::PublishFailed);
        }
    }

    pub(super) fn handle_accept_call_action(&mut self, chat_id: &str) {
        let Some(active) = self.state.active_call.clone() else {
            return;
        };
        if active.chat_id != chat_id {
            self.toast("Call/chat mismatch");
            return;
        }
        if !matches!(active.status, CallStatus::Ringing) {
            return;
        }
        let Some(session) = self.call_session_params.clone() else {
            self.toast("Missing call session parameters");
            return;
        };
        let prepared = match self.prepare_call_accept_for_chat(chat_id, &active, &session) {
            Ok(prepared) => prepared,
            Err(err) => {
                self.toast(format!("Call accept setup failed: {err}"));
                self.send_call_reject(chat_id, &active.call_id, "auth_failed");
                self.end_call_local(CallEndReason::AuthFailed);
                return;
            }
        };
        if let Err(e) = self.publish_call_signal(
            chat_id,
            prepared.signal.payload_json,
            "Call accept publish failed",
        ) {
            self.toast(e);
            return;
        }
        if let Err(e) = self.call_runtime.on_call_connecting(
            &active.call_id,
            &prepared.incoming.session,
            prepared.media_crypto,
            self.config.call_audio_backend.as_deref(),
            self.core_sender.clone(),
        ) {
            self.toast(format!("Call runtime start failed: {e}"));
            self.end_call_local(CallEndReason::RuntimeError);
            return;
        }
        self.call_session_params = Some(prepared.incoming.session);
        self.update_call_status(CallStatus::Connecting);
    }

    pub(super) fn handle_reject_call_action(&mut self, chat_id: &str) {
        let Some(active) = self.state.active_call.clone() else {
            return;
        };
        if active.chat_id != chat_id {
            return;
        }
        if !matches!(active.status, CallStatus::Ringing) {
            return;
        }
        // End locally first so the UI updates and audio stops immediately.
        // The signal to the peer is best-effort and publishes asynchronously.
        self.end_call_local(CallEndReason::Declined);
        let payload = match self.session.as_ref() {
            Some(sess) => match sess
                .host_context()
                .prepare_reject_call_signal(&active.call_id, "declined")
            {
                Ok(signal) => signal.payload_json,
                Err(err) => {
                    self.toast(format!("Serialize reject failed: {err}"));
                    return;
                }
            },
            None => return,
        };
        if let Err(e) = self.publish_call_signal(chat_id, payload, "Call reject publish failed") {
            self.toast(e);
        }
    }

    pub(super) fn handle_end_call_action(&mut self) {
        let Some(active) = self.state.active_call.clone() else {
            return;
        };
        if !matches!(
            active.status,
            CallStatus::Offering
                | CallStatus::Ringing
                | CallStatus::Connecting
                | CallStatus::Active
        ) {
            return;
        }
        // End locally first so the UI updates and audio stops immediately.
        // The signal to the peer is best-effort and publishes asynchronously.
        self.end_call_local(CallEndReason::UserHangup);
        let payload = match self.session.as_ref() {
            Some(sess) => match sess
                .host_context()
                .prepare_end_call_signal(&active.call_id, "user_hangup")
            {
                Ok(signal) => signal.payload_json,
                Err(err) => {
                    self.toast(format!("Serialize end failed: {err}"));
                    return;
                }
            },
            None => return,
        };
        if let Err(e) =
            self.publish_call_signal(&active.chat_id, payload, "Call end publish failed")
        {
            self.toast(e);
        }
    }

    pub(super) fn handle_call_offer_timeout(&mut self) {
        let Some(active) = self.state.active_call.clone() else {
            return;
        };
        if !matches!(active.status, CallStatus::Offering | CallStatus::Ringing) {
            return;
        }
        tracing::info!("call_offer_timeout: ending unanswered call");
        self.end_call_local(CallEndReason::Timeout);
        let payload = self.session.as_ref().and_then(|sess| {
            sess.host_context()
                .prepare_end_call_signal(&active.call_id, "timeout")
                .ok()
                .map(|signal| signal.payload_json)
        });
        if let Some(payload) = payload {
            let _ = self.publish_call_signal(
                &active.chat_id,
                payload,
                "Call timeout end publish failed",
            );
        }
    }

    pub(super) fn handle_toggle_mute_action(&mut self) {
        let Some(call) = self.state.active_call.as_mut() else {
            return;
        };
        if !matches!(
            call.status,
            CallStatus::Offering
                | CallStatus::Ringing
                | CallStatus::Connecting
                | CallStatus::Active
        ) {
            return;
        }
        call.is_muted = !call.is_muted;
        self.call_runtime.set_muted(&call.call_id, call.is_muted);
        self.emit_call_state();
    }

    pub(super) fn handle_toggle_camera_action(&mut self) {
        let Some(call) = self.state.active_call.as_mut() else {
            return;
        };
        if !call.is_video_call {
            return;
        }
        if !matches!(
            call.status,
            CallStatus::Offering | CallStatus::Connecting | CallStatus::Active
        ) {
            return;
        }
        call.is_camera_enabled = !call.is_camera_enabled;
        self.call_runtime
            .set_camera_enabled(&call.call_id, call.is_camera_enabled);
        self.emit_call_state();
    }

    fn send_call_reject(&mut self, chat_id: &str, call_id: &str, reason: &str) {
        let payload = match self.session.as_ref() {
            Some(sess) => {
                match sess
                    .host_context()
                    .prepare_reject_call_signal(call_id, reason)
                {
                    Ok(signal) => signal.payload_json,
                    Err(_) => return,
                }
            }
            None => return,
        };
        let _ = self.publish_call_signal(chat_id, payload, "Call reject publish failed");
    }

    pub(super) fn handle_incoming_call_signal(
        &mut self,
        chat_id: &str,
        sender_pubkey: &PublicKey,
        signal: ParsedCallSignal,
    ) {
        // Calls are MVP-only for 1:1 DMs. If a call invite arrives on a group chat,
        // reject it to avoid wedging state with no UI controls.
        let is_group_chat = self
            .session
            .as_ref()
            .and_then(|s| s.groups.get(chat_id))
            .map(|g| g.is_group)
            .unwrap_or(false);

        let peer_npub = sender_pubkey
            .to_bech32()
            .unwrap_or_else(|_| sender_pubkey.to_hex());
        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let Some(group) = sess.groups.get(chat_id) else {
            return;
        };
        let Some(local_pubkey_hex) = self.current_pubkey_hex() else {
            self.toast("No local pubkey for incoming call");
            return;
        };
        let peer_pubkey_hex = sender_pubkey.to_hex();
        let pending_outgoing = self
            .state
            .active_call
            .as_ref()
            .filter(|active| matches!(active.status, CallStatus::Offering))
            .and_then(|active| {
                self.call_session_params
                    .as_ref()
                    .map(|session| PendingOutgoingCall {
                        call_id: active.call_id.clone(),
                        target_id: active.chat_id.clone(),
                        peer_pubkey_hex: peer_pubkey_hex.clone(),
                        session: session.clone(),
                        is_video_call: active.is_video_call,
                    })
            });

        match sess.host_context().handle_inbound_call_signal(
            InboundSignalContext {
                target_id: chat_id,
                sender_pubkey_hex: &peer_pubkey_hex,
                group: GroupCallContext {
                    mls_group_id: &group.mls_group_id,
                    local_pubkey_hex: &local_pubkey_hex,
                },
                policy: InboundCallPolicy {
                    allow_group_calls: !is_group_chat,
                    allow_video_calls: true,
                },
                has_live_call: self.has_live_call(),
                pending_outgoing: pending_outgoing.as_ref(),
            },
            signal,
        ) {
            InboundCallSignalOutcome::Ignore => {}
            InboundCallSignalOutcome::RejectIncoming(rejected) => {
                if let Some(err) = rejected.error {
                    self.toast(format!("Rejected call invite: {err}"));
                }
                let _ = self.publish_call_signal(
                    chat_id,
                    rejected.signal.payload_json,
                    "Call reject publish failed",
                );
            }
            InboundCallSignalOutcome::IncomingInvite(incoming) => {
                self.call_session_params = Some(incoming.session.clone());
                let previous = self.state.active_call.clone();
                self.cancel_call_duration_ticks();
                self.state.active_call = Some(crate::state::CallState::new(
                    incoming.call_id.clone(),
                    chat_id.to_string(),
                    peer_npub,
                    CallStatus::Ringing,
                    None,
                    false,
                    incoming.is_video_call,
                    None,
                ));
                self.schedule_call_offer_timeout();
                self.emit_call_state_with_previous(previous);
            }
            InboundCallSignalOutcome::OutgoingAccepted(accepted) => {
                if let Err(e) = self.call_runtime.on_call_connecting(
                    &accepted.pending.call_id,
                    &accepted.session,
                    accepted.media_crypto,
                    self.config.call_audio_backend.as_deref(),
                    self.core_sender.clone(),
                ) {
                    self.toast(format!("Call runtime start failed: {e}"));
                    self.end_call_local(CallEndReason::RuntimeError);
                    return;
                }
                self.call_session_params = Some(accepted.session);
                self.update_call_status(CallStatus::Connecting);
            }
            InboundCallSignalOutcome::IncomingAcceptFailed(failure) => {
                self.toast(match failure.kind {
                    pika_marmot_runtime::call_runtime::IncomingAcceptFailureKind::RelayAuth => {
                        format!("Call relay auth verification failed: {}", failure.error)
                    }
                    pika_marmot_runtime::call_runtime::IncomingAcceptFailureKind::MediaCrypto => {
                        format!("Call media key setup failed: {}", failure.error)
                    }
                });
                self.end_call_local(match failure.kind {
                    pika_marmot_runtime::call_runtime::IncomingAcceptFailureKind::RelayAuth => {
                        CallEndReason::AuthFailed
                    }
                    pika_marmot_runtime::call_runtime::IncomingAcceptFailureKind::MediaCrypto => {
                        CallEndReason::RuntimeError
                    }
                });
            }
            InboundCallSignalOutcome::RemoteTermination(ended) => {
                let Some(active) = self.state.active_call.as_ref() else {
                    return;
                };
                if active.call_id != ended.call_id || active.chat_id != chat_id {
                    return;
                }
                self.end_call_local(CallEndReason::Remote(ended.reason));
            }
        }
    }

    pub(super) fn filter_incoming_call_signal(
        &self,
        sender_pubkey: &PublicKey,
        signal: Option<ParsedCallSignal>,
    ) -> Option<ParsedCallSignal> {
        let my_pubkey = self.session.as_ref().map(|s| s.pubkey);
        if my_pubkey.as_ref() == Some(sender_pubkey) {
            return None;
        }
        signal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pika_marmot_runtime::call::{build_call_signal_json, OutgoingCallSignal};

    #[test]
    fn parses_invite_signal() {
        let call_id = "550e8400-e29b-41d4-a716-446655440000";
        let session = CallSessionParams {
            moq_url: "https://moq.example.com/anon".to_string(),
            broadcast_base: format!("pika/calls/{call_id}"),
            relay_auth: "capv1_test_token".to_string(),
            tracks: vec![CallTrackSpec::audio0_opus_default()],
        };
        let json = build_call_signal_json(call_id, OutgoingCallSignal::Invite(&session)).unwrap();
        let parsed = parse_call_signal(&json);
        match parsed {
            Some(ParsedCallSignal::Invite {
                call_id: got_call_id,
                session: got_session,
            }) => {
                assert_eq!(got_call_id, call_id);
                assert_eq!(got_session.moq_url, "https://moq.example.com/anon");
                assert_eq!(got_session.broadcast_base, format!("pika/calls/{call_id}"));
                assert_eq!(got_session.relay_auth, "capv1_test_token");
                assert_eq!(got_session.tracks.len(), 1);
                assert_eq!(got_session.tracks[0].name, "audio0");
            }
            _ => panic!("expected invite"),
        }
    }

    #[test]
    fn ignores_non_call_json() {
        let msg = r#"{"foo":"bar"}"#;
        assert!(parse_call_signal(msg).is_none());
    }

    #[test]
    fn validates_relay_auth_token_shape() {
        assert!(valid_relay_auth_token(
            "capv1_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!valid_relay_auth_token("capv1_short"));
        assert!(!valid_relay_auth_token("notcap_0123456789abcdef"));
    }
}
