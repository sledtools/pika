use mdk_core::MDK;
use mdk_core::encrypted_media::crypto::{DEFAULT_SCHEME_VERSION, derive_encryption_key};
use mdk_core::prelude::GroupId;
use mdk_sqlite_storage::MdkSqliteStorage;
use nostr_sdk::hashes::{Hash as _, sha256};
use pika_media::crypto::{FrameKeyMaterial, opaque_participant_label};
use serde::{Deserialize, Serialize};

pub const DEFAULT_CALL_BROADCAST_PREFIX: &str = "pika/calls";
const CALL_NS: &str = "pika.call";
const CALL_PROTOCOL_VERSION: u8 = 1;
const RELAY_AUTH_CAP_PREFIX: &str = "capv1_";
const RELAY_AUTH_HEX_LEN: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallTrackSpec {
    pub name: String,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_ms: u16,
}

impl CallTrackSpec {
    pub fn audio0_opus_default() -> Self {
        Self {
            name: "audio0".to_string(),
            codec: "opus".to_string(),
            sample_rate: 48_000,
            channels: 1,
            frame_ms: 20,
        }
    }

    pub fn video0_h264_default() -> Self {
        Self {
            name: "video0".to_string(),
            codec: "h264".to_string(),
            sample_rate: 90_000,
            channels: 0,
            frame_ms: 33,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallSessionParams {
    pub moq_url: String,
    pub broadcast_base: String,
    pub relay_auth: String,
    pub tracks: Vec<CallTrackSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCallSignal {
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

pub enum OutgoingCallSignal<'a> {
    Invite(&'a CallSessionParams),
    Accept(&'a CallSessionParams),
    Reject { reason: &'a str },
    End { reason: &'a str },
}

#[derive(Debug, Clone)]
pub struct CallMediaCryptoContext {
    pub tx_keys: FrameKeyMaterial,
    pub rx_keys: FrameKeyMaterial,
    pub video_tx_keys: Option<FrameKeyMaterial>,
    pub video_rx_keys: Option<FrameKeyMaterial>,
    pub local_participant_label: String,
    pub peer_participant_label: String,
}

pub struct CallCryptoDeriveContext<'a> {
    pub mdk: &'a MDK<MdkSqliteStorage>,
    pub mls_group_id: &'a GroupId,
    pub group_epoch: u64,
    pub call_id: &'a str,
    pub session: &'a CallSessionParams,
    pub local_pubkey_hex: &'a str,
    pub peer_pubkey_hex: &'a str,
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

fn context_hash(parts: &[&[u8]]) -> [u8; 32] {
    let mut buf = Vec::new();
    for part in parts {
        let len: u32 = part.len().try_into().unwrap_or(u32::MAX);
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(part);
    }
    sha256::Hash::hash(&buf).to_byte_array()
}

pub fn key_id_for_sender(sender_id: &[u8]) -> u64 {
    let digest = context_hash(&[b"pika.call.media.keyid.v1", sender_id]);
    u64::from_be_bytes(digest[0..8].try_into().expect("hash width"))
}

pub fn call_shared_seed(
    call_id: &str,
    session: &CallSessionParams,
    local_pubkey_hex: &str,
    peer_pubkey_hex: &str,
) -> String {
    let (left, right) = if local_pubkey_hex <= peer_pubkey_hex {
        (local_pubkey_hex, peer_pubkey_hex)
    } else {
        (peer_pubkey_hex, local_pubkey_hex)
    };
    format!(
        "pika-call-media-v1|{call_id}|{}|{}|{}|{}",
        session.moq_url, session.broadcast_base, left, right
    )
}

pub fn valid_relay_auth_token(token: &str) -> bool {
    let trimmed = token.trim();
    let Some(hex_part) = trimmed.strip_prefix(RELAY_AUTH_CAP_PREFIX) else {
        return false;
    };
    hex_part.len() == RELAY_AUTH_HEX_LEN && hex_part.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn parse_call_signal(content: &str) -> Option<ParsedCallSignal> {
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

pub fn build_call_signal_json(
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

fn derive_track_keys(
    ctx: &CallCryptoDeriveContext<'_>,
    track: &str,
) -> Result<(FrameKeyMaterial, FrameKeyMaterial, [u8; 32]), String> {
    let shared_seed = call_shared_seed(
        ctx.call_id,
        ctx.session,
        ctx.local_pubkey_hex,
        ctx.peer_pubkey_hex,
    );
    let generation = 0u8;

    let tx_hash = context_hash(&[
        b"pika.call.media.base.v1",
        shared_seed.as_bytes(),
        ctx.local_pubkey_hex.as_bytes(),
        track.as_bytes(),
    ]);
    let rx_hash = context_hash(&[
        b"pika.call.media.base.v1",
        shared_seed.as_bytes(),
        ctx.peer_pubkey_hex.as_bytes(),
        track.as_bytes(),
    ]);
    let root_hash = context_hash(&[
        b"pika.call.media.root.v1",
        shared_seed.as_bytes(),
        track.as_bytes(),
    ]);

    let tx_filename = format!("call/{}/{track}/{}", ctx.call_id, ctx.local_pubkey_hex);
    let rx_filename = format!("call/{}/{track}/{}", ctx.call_id, ctx.peer_pubkey_hex);
    let root_filename = format!("call/{}/{track}/group-root", ctx.call_id);

    let tx_base = *derive_encryption_key(
        ctx.mdk,
        ctx.mls_group_id,
        DEFAULT_SCHEME_VERSION,
        &tx_hash,
        "application/pika-call",
        &tx_filename,
    )
    .map_err(|e| format!("derive tx media key for {track} failed: {e}"))?;

    let rx_base = *derive_encryption_key(
        ctx.mdk,
        ctx.mls_group_id,
        DEFAULT_SCHEME_VERSION,
        &rx_hash,
        "application/pika-call",
        &rx_filename,
    )
    .map_err(|e| format!("derive rx media key for {track} failed: {e}"))?;

    let group_root = *derive_encryption_key(
        ctx.mdk,
        ctx.mls_group_id,
        DEFAULT_SCHEME_VERSION,
        &root_hash,
        "application/pika-call",
        &root_filename,
    )
    .map_err(|e| format!("derive media group root for {track} failed: {e}"))?;

    let tx_keys = FrameKeyMaterial::from_base_key(
        tx_base,
        key_id_for_sender(ctx.local_pubkey_hex.as_bytes()),
        ctx.group_epoch,
        generation,
        track,
        group_root,
    );
    let rx_keys = FrameKeyMaterial::from_base_key(
        rx_base,
        key_id_for_sender(ctx.peer_pubkey_hex.as_bytes()),
        ctx.group_epoch,
        generation,
        track,
        group_root,
    );

    Ok((tx_keys, rx_keys, group_root))
}

pub fn derive_call_media_crypto_context(
    ctx: &CallCryptoDeriveContext<'_>,
    primary_track: &str,
    video_track: Option<&str>,
) -> Result<CallMediaCryptoContext, String> {
    let (tx_keys, rx_keys, group_root) = derive_track_keys(ctx, primary_track)?;

    let (video_tx_keys, video_rx_keys) = if let Some(track) = video_track {
        let (vtx, vrx, _) = derive_track_keys(ctx, track)?;
        (Some(vtx), Some(vrx))
    } else {
        (None, None)
    };

    Ok(CallMediaCryptoContext {
        tx_keys,
        rx_keys,
        video_tx_keys,
        video_rx_keys,
        local_participant_label: opaque_participant_label(
            &group_root,
            ctx.local_pubkey_hex.as_bytes(),
        ),
        peer_participant_label: opaque_participant_label(
            &group_root,
            ctx.peer_pubkey_hex.as_bytes(),
        ),
    })
}

pub fn derive_relay_auth_token(ctx: &CallCryptoDeriveContext<'_>) -> Result<String, String> {
    let shared_seed = call_shared_seed(
        ctx.call_id,
        ctx.session,
        ctx.local_pubkey_hex,
        ctx.peer_pubkey_hex,
    );
    let auth_hash = context_hash(&[
        b"pika.call.relay.auth.seed.v1",
        shared_seed.as_bytes(),
        ctx.call_id.as_bytes(),
    ]);
    let auth_key = *derive_encryption_key(
        ctx.mdk,
        ctx.mls_group_id,
        DEFAULT_SCHEME_VERSION,
        &auth_hash,
        "application/pika-call-auth",
        &format!("call/{}/relay-auth", ctx.call_id),
    )
    .map_err(|e| format!("derive relay auth token failed: {e}"))?;
    let token_hash = context_hash(&[
        b"pika.call.relay.auth.token.v1",
        &auth_key,
        ctx.call_id.as_bytes(),
        ctx.session.moq_url.as_bytes(),
        ctx.session.broadcast_base.as_bytes(),
    ]);
    Ok(format!("capv1_{}", hex::encode(token_hash)))
}

pub fn validate_relay_auth_token(ctx: &CallCryptoDeriveContext<'_>) -> Result<(), String> {
    if !valid_relay_auth_token(&ctx.session.relay_auth) {
        return Err("call relay auth token format invalid".to_string());
    }
    let expected = derive_relay_auth_token(ctx)?;
    if expected != ctx.session.relay_auth {
        return Err("call relay auth mismatch".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_invite_signal_round_trip() {
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
                assert_eq!(got_session, session);
            }
            _ => panic!("expected invite"),
        }
    }

    #[test]
    fn ignores_non_call_json() {
        assert!(parse_call_signal(r#"{"foo":"bar"}"#).is_none());
    }

    #[test]
    fn validates_relay_auth_token_shape() {
        assert!(valid_relay_auth_token(
            "capv1_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!valid_relay_auth_token("capv1_short"));
        assert!(!valid_relay_auth_token("notcap_0123456789abcdef"));
    }

    #[test]
    fn shared_seed_orders_pubkeys_stably() {
        let session = CallSessionParams {
            moq_url: "https://moq.example.com/anon".to_string(),
            broadcast_base: "pika/calls/test".to_string(),
            relay_auth: String::new(),
            tracks: vec![CallTrackSpec::audio0_opus_default()],
        };
        let a = call_shared_seed(
            "call-123",
            &session,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let b = call_shared_seed(
            "call-123",
            &session,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        assert_eq!(a, b);
    }
}
