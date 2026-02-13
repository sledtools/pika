use super::*;
use crate::state::CallStatus;
use serde::{Deserialize, Serialize};

const CALL_NS: &str = "pika.call";
const CALL_PROTOCOL_VERSION: u8 = 1;
const DEFAULT_CALL_BROADCAST_PREFIX: &str = "pika/calls";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CallTrackSpec {
    pub name: String,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_ms: u16,
}

impl CallTrackSpec {
    fn audio0_opus_default() -> Self {
        Self {
            name: "audio0".to_string(),
            codec: "opus".to_string(),
            sample_rate: 48_000,
            channels: 1,
            frame_ms: 20,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CallSessionParams {
    pub moq_url: String,
    pub broadcast_base: String,
    pub tracks: Vec<CallTrackSpec>,
}

#[derive(Debug, Clone)]
pub(super) enum ParsedCallSignal {
    Invite {
        call_id: String,
        session: CallSessionParams,
    },
    Accept {
        call_id: String,
        session: CallSessionParams,
    },
    Reject {
        call_id: String,
        reason: String,
    },
    End {
        call_id: String,
        reason: String,
    },
}

enum OutgoingCallSignal<'a> {
    Invite(&'a CallSessionParams),
    Accept(&'a CallSessionParams),
    Reject { reason: &'a str },
    End { reason: &'a str },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CallEnvelope {
    v: u8,
    ns: String,
    #[serde(rename = "type")]
    message_type: String,
    call_id: String,
    ts_ms: i64,
    #[serde(default)]
    from: Option<String>,
    body: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CallReasonBody {
    reason: String,
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub(super) fn parse_call_signal(content: &str) -> Option<ParsedCallSignal> {
    let env: CallEnvelope = serde_json::from_str(content).ok()?;
    if env.v != CALL_PROTOCOL_VERSION || env.ns != CALL_NS {
        return None;
    }

    match env.message_type.as_str() {
        "call.invite" => {
            let session: CallSessionParams = serde_json::from_value(env.body).ok()?;
            Some(ParsedCallSignal::Invite {
                call_id: env.call_id,
                session,
            })
        }
        "call.accept" => {
            let session: CallSessionParams = serde_json::from_value(env.body).ok()?;
            Some(ParsedCallSignal::Accept {
                call_id: env.call_id,
                session,
            })
        }
        "call.reject" => {
            let body: CallReasonBody = serde_json::from_value(env.body).ok()?;
            Some(ParsedCallSignal::Reject {
                call_id: env.call_id,
                reason: body.reason,
            })
        }
        "call.end" => {
            let body: CallReasonBody = serde_json::from_value(env.body).ok()?;
            Some(ParsedCallSignal::End {
                call_id: env.call_id,
                reason: body.reason,
            })
        }
        _ => None,
    }
}

pub(super) fn is_call_signal_payload(content: &str) -> bool {
    parse_call_signal(content).is_some()
}

fn build_call_signal_json(
    call_id: &str,
    outgoing: OutgoingCallSignal<'_>,
) -> Result<String, serde_json::Error> {
    let (message_type, body) = match outgoing {
        OutgoingCallSignal::Invite(session) => ("call.invite", serde_json::to_value(session)?),
        OutgoingCallSignal::Accept(session) => ("call.accept", serde_json::to_value(session)?),
        OutgoingCallSignal::Reject { reason } => (
            "call.reject",
            serde_json::to_value(CallReasonBody {
                reason: reason.to_string(),
            })?,
        ),
        OutgoingCallSignal::End { reason } => (
            "call.end",
            serde_json::to_value(CallReasonBody {
                reason: reason.to_string(),
            })?,
        ),
    };

    let env = CallEnvelope {
        v: CALL_PROTOCOL_VERSION,
        ns: CALL_NS.to_string(),
        message_type: message_type.to_string(),
        call_id: call_id.to_string(),
        ts_ms: now_millis(),
        from: None,
        body,
    };
    serde_json::to_string(&env)
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

    fn call_session_from_config(&self, call_id: &str) -> Option<CallSessionParams> {
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

        Some(CallSessionParams {
            moq_url,
            broadcast_base: format!("{prefix}/{call_id}"),
            tracks: vec![CallTrackSpec::audio0_opus_default()],
        })
    }

    fn current_peer_npub(&self, chat_id: &str) -> Option<String> {
        self.session
            .as_ref()
            .and_then(|s| s.groups.get(chat_id))
            .map(|g| g.peer_npub.clone())
    }

    fn current_pubkey_hex(&self) -> Option<String> {
        self.session.as_ref().map(|s| s.keys.public_key().to_hex())
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
                sess.keys.public_key(),
                Timestamp::from(now_seconds() as u64),
                Kind::Custom(9),
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
            let out = client.send_event_to(relays, &wrapper).await;
            let error = match out {
                Ok(output) if !output.success.is_empty() => None,
                Ok(output) => Some(
                    output
                        .failed
                        .values()
                        .next()
                        .cloned()
                        .unwrap_or_else(|| "no relay accepted event".to_string()),
                ),
                Err(e) => Some(e.to_string()),
            };
            if let Some(err) = error {
                let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::Toast(format!(
                    "{failure_context}: {err}",
                )))));
            }
        });
        Ok(())
    }

    fn update_call_status(&mut self, status: CallStatus) {
        if let Some(call) = self.state.active_call.as_mut() {
            call.status = status;
            self.emit_call_state();
        }
    }

    fn end_call_local(&mut self, reason: String) {
        if let Some(call) = self.state.active_call.as_mut() {
            call.status = CallStatus::Ended {
                reason: reason.clone(),
            };
            self.call_runtime.on_call_ended(&call.call_id);
            self.call_session_params = None;
            self.emit_call_state();
        }
    }

    pub(super) fn handle_start_call_action(&mut self, chat_id: &str) {
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

        let call_id = uuid::Uuid::new_v4().to_string();
        let Some(peer_npub) = self.current_peer_npub(chat_id) else {
            self.toast("Chat peer not found");
            return;
        };
        let Some(session) = self.call_session_from_config(&call_id) else {
            self.toast("Call config missing: set `call_moq_url` in pika_config.json");
            return;
        };

        self.state.active_call = Some(crate::state::CallState {
            call_id: call_id.clone(),
            chat_id: chat_id.to_string(),
            peer_npub,
            status: CallStatus::Offering,
            started_at: None,
            is_muted: false,
            debug: None,
        });
        self.call_session_params = Some(session.clone());
        self.emit_call_state();

        let payload = match build_call_signal_json(&call_id, OutgoingCallSignal::Invite(&session)) {
            Ok(v) => v,
            Err(e) => {
                self.toast(format!("Serialize invite failed: {e}"));
                self.end_call_local("serialize_failed".to_string());
                return;
            }
        };
        if let Err(e) = self.publish_call_signal(chat_id, payload, "Call invite publish failed") {
            self.toast(e);
            self.end_call_local("publish_failed".to_string());
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
        let session = self
            .call_session_params
            .clone()
            .or_else(|| self.call_session_from_config(&active.call_id));
        let Some(session) = session else {
            self.toast("Call config missing: set `call_moq_url` in pika_config.json");
            return;
        };

        let payload =
            match build_call_signal_json(&active.call_id, OutgoingCallSignal::Accept(&session)) {
                Ok(v) => v,
                Err(e) => {
                    self.toast(format!("Serialize accept failed: {e}"));
                    return;
                }
            };
        if let Err(e) = self.publish_call_signal(chat_id, payload, "Call accept publish failed") {
            self.toast(e);
            return;
        }

        let Some(local_pubkey_hex) = self.current_pubkey_hex() else {
            self.toast("No local pubkey for call runtime");
            self.end_call_local("runtime_error".to_string());
            return;
        };
        let peer_pubkey_hex = match PublicKey::parse(&active.peer_npub) {
            Ok(pk) => pk.to_hex(),
            Err(e) => {
                self.toast(format!("Peer pubkey parse failed: {e}"));
                self.end_call_local("runtime_error".to_string());
                return;
            }
        };
        if let Err(e) = self.call_runtime.on_call_connecting(
            &active.call_id,
            &session,
            &local_pubkey_hex,
            &peer_pubkey_hex,
            self.core_sender.clone(),
        ) {
            self.toast(format!("Call runtime start failed: {e}"));
            self.end_call_local("runtime_error".to_string());
            return;
        }
        self.call_session_params = Some(session);
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
        let payload = match build_call_signal_json(
            &active.call_id,
            OutgoingCallSignal::Reject { reason: "declined" },
        ) {
            Ok(v) => v,
            Err(e) => {
                self.toast(format!("Serialize reject failed: {e}"));
                return;
            }
        };
        if let Err(e) = self.publish_call_signal(chat_id, payload, "Call reject publish failed") {
            self.toast(e);
        }
        self.end_call_local("declined".to_string());
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
        let payload = match build_call_signal_json(
            &active.call_id,
            OutgoingCallSignal::End {
                reason: "user_hangup",
            },
        ) {
            Ok(v) => v,
            Err(e) => {
                self.toast(format!("Serialize end failed: {e}"));
                return;
            }
        };
        if let Err(e) =
            self.publish_call_signal(&active.chat_id, payload, "Call end publish failed")
        {
            self.toast(e);
        }
        self.end_call_local("user_hangup".to_string());
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

    fn send_busy_reject(&mut self, chat_id: &str, call_id: &str) {
        let payload =
            match build_call_signal_json(call_id, OutgoingCallSignal::Reject { reason: "busy" }) {
                Ok(v) => v,
                Err(_) => return,
            };
        let _ = self.publish_call_signal(chat_id, payload, "Busy reject publish failed");
    }

    pub(super) fn handle_incoming_call_signal(
        &mut self,
        chat_id: &str,
        sender_pubkey: &PublicKey,
        signal: ParsedCallSignal,
    ) {
        let peer_npub = sender_pubkey
            .to_bech32()
            .unwrap_or_else(|_| sender_pubkey.to_hex());

        match signal {
            ParsedCallSignal::Invite { call_id, session } => {
                if self.has_live_call() {
                    self.send_busy_reject(chat_id, &call_id);
                    return;
                }
                self.call_session_params = Some(session);
                self.state.active_call = Some(crate::state::CallState {
                    call_id,
                    chat_id: chat_id.to_string(),
                    peer_npub,
                    status: CallStatus::Ringing,
                    started_at: None,
                    is_muted: false,
                    debug: None,
                });
                self.emit_call_state();
            }
            ParsedCallSignal::Accept { call_id, session } => {
                let Some(active) = self.state.active_call.clone() else {
                    return;
                };
                if active.call_id != call_id
                    || active.chat_id != chat_id
                    || !matches!(active.status, CallStatus::Offering)
                {
                    return;
                }
                self.call_session_params = Some(session);
                let Some(local_pubkey_hex) = self.current_pubkey_hex() else {
                    self.toast("No local pubkey for call runtime");
                    self.end_call_local("runtime_error".to_string());
                    return;
                };
                let peer_pubkey_hex = match PublicKey::parse(&active.peer_npub) {
                    Ok(pk) => pk.to_hex(),
                    Err(e) => {
                        self.toast(format!("Peer pubkey parse failed: {e}"));
                        self.end_call_local("runtime_error".to_string());
                        return;
                    }
                };
                let Some(params) = self.call_session_params.as_ref() else {
                    self.toast("Missing call session parameters");
                    self.end_call_local("runtime_error".to_string());
                    return;
                };
                if let Err(e) = self.call_runtime.on_call_connecting(
                    &call_id,
                    params,
                    &local_pubkey_hex,
                    &peer_pubkey_hex,
                    self.core_sender.clone(),
                ) {
                    self.toast(format!("Call runtime start failed: {e}"));
                    self.end_call_local("runtime_error".to_string());
                    return;
                }
                self.update_call_status(CallStatus::Connecting);
            }
            ParsedCallSignal::Reject { call_id, reason } => {
                let Some(active) = self.state.active_call.as_ref() else {
                    return;
                };
                if active.call_id != call_id || active.chat_id != chat_id {
                    return;
                }
                self.end_call_local(reason);
            }
            ParsedCallSignal::End { call_id, reason } => {
                let Some(active) = self.state.active_call.as_ref() else {
                    return;
                };
                if active.call_id != call_id || active.chat_id != chat_id {
                    return;
                }
                self.end_call_local(reason);
            }
        }
    }

    pub(super) fn maybe_parse_call_signal(
        &self,
        sender_pubkey: &PublicKey,
        content: &str,
    ) -> Option<ParsedCallSignal> {
        let my_pubkey = self.session.as_ref().map(|s| s.keys.public_key());
        if my_pubkey.as_ref() == Some(sender_pubkey) {
            return None;
        }
        parse_call_signal(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_invite_signal() {
        let call_id = "550e8400-e29b-41d4-a716-446655440000";
        let session = CallSessionParams {
            moq_url: "https://moq.example.com/anon".to_string(),
            broadcast_base: format!("pika/calls/{call_id}"),
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
        assert!(!is_call_signal_payload(msg));
    }
}
