//! Versioned daemon protocol surface for stdio/socket/remote hosts.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Versioned request surface for the daemon's native JSONL/socket protocol.
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum InCmd {
    PublishKeypackage {
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default)]
        relays: Vec<String>,
    },
    SetRelays {
        #[serde(default)]
        request_id: Option<String>,
        relays: Vec<String>,
    },
    ListPendingWelcomes {
        #[serde(default)]
        request_id: Option<String>,
    },
    AcceptWelcome {
        #[serde(default)]
        request_id: Option<String>,
        wrapper_event_id: String,
    },
    ListGroups {
        #[serde(default)]
        request_id: Option<String>,
    },
    HypernoteCatalog {
        #[serde(default)]
        request_id: Option<String>,
    },
    SendMessage {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        content: String,
    },
    SendHypernote {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        content: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        state: Option<String>,
    },
    React {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        event_id: String,
        emoji: String,
    },
    SubmitHypernoteAction {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        event_id: String,
        action: String,
        #[serde(default)]
        form: HashMap<String, String>,
    },
    SendMedia {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        file_path: String,
        #[serde(default)]
        mime_type: Option<String>,
        #[serde(default)]
        filename: Option<String>,
        #[serde(default)]
        caption: String,
        #[serde(default)]
        blossom_servers: Vec<String>,
    },
    SendMediaBatch {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        file_paths: Vec<String>,
        #[serde(default)]
        caption: String,
        #[serde(default)]
        blossom_servers: Vec<String>,
    },
    SendTyping {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
    },
    InviteCall {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        peer_pubkey: String,
        #[serde(default)]
        call_id: Option<String>,
        moq_url: String,
        #[serde(default)]
        broadcast_base: Option<String>,
        #[serde(default)]
        track_name: Option<String>,
        #[serde(default)]
        track_codec: Option<String>,
        #[serde(default)]
        relay_auth: Option<String>,
    },
    AcceptCall {
        #[serde(default)]
        request_id: Option<String>,
        call_id: String,
    },
    RejectCall {
        #[serde(default)]
        request_id: Option<String>,
        call_id: String,
        #[serde(default = "default_reject_reason")]
        reason: String,
    },
    EndCall {
        #[serde(default)]
        request_id: Option<String>,
        call_id: String,
        #[serde(default = "default_end_reason")]
        reason: String,
    },
    SendAudioResponse {
        #[serde(default)]
        request_id: Option<String>,
        call_id: String,
        tts_text: String,
    },
    SendAudioFile {
        #[serde(default)]
        request_id: Option<String>,
        call_id: String,
        audio_path: String,
        sample_rate: u32,
        #[serde(default = "default_channels")]
        channels: u16,
    },
    SendCallData {
        #[serde(default)]
        request_id: Option<String>,
        call_id: String,
        payload_hex: String,
        #[serde(default)]
        track_name: Option<String>,
    },
    InitGroup {
        #[serde(default)]
        request_id: Option<String>,
        peer_pubkey: String,
        #[serde(default = "default_group_name")]
        group_name: String,
    },
    GetMessages {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        #[serde(default = "default_get_messages_limit")]
        limit: usize,
    },
    Shutdown {
        #[serde(default)]
        request_id: Option<String>,
    },
}

/// Versioned event/response surface for the daemon's native JSONL/socket protocol.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutMsg {
    Ready {
        protocol_version: u32,
        pubkey: String,
        npub: String,
    },
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        code: String,
        message: String,
    },
    KeypackagePublished {
        event_id: String,
    },
    WelcomeReceived {
        wrapper_event_id: String,
        welcome_event_id: String,
        from_pubkey: String,
        nostr_group_id: String,
        group_name: String,
    },
    GroupJoined {
        nostr_group_id: String,
        mls_group_id: String,
        member_count: u32,
    },
    MessageReceived {
        nostr_group_id: String,
        from_pubkey: String,
        content: String,
        kind: u16,
        created_at: u64,
        event_id: String,
        message_id: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        media: Vec<MediaAttachmentOut>,
    },
    CallInviteReceived {
        call_id: String,
        from_pubkey: String,
        nostr_group_id: String,
    },
    CallSessionStarted {
        call_id: String,
        nostr_group_id: String,
        from_pubkey: String,
    },
    CallSessionEnded {
        call_id: String,
        reason: String,
    },
    CallDebug {
        call_id: String,
        tx_frames: u64,
        rx_frames: u64,
        rx_dropped: u64,
    },
    CallAudioChunk {
        call_id: String,
        audio_path: String,
        sample_rate: u32,
        channels: u8,
    },
    CallData {
        call_id: String,
        payload_hex: String,
        track_name: String,
    },
    GroupCreated {
        nostr_group_id: String,
        mls_group_id: String,
        peer_pubkey: String,
        member_count: u32,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MediaAttachmentOut {
    pub url: String,
    pub mime_type: String,
    pub filename: String,
    pub original_hash_hex: String,
    pub nonce_hex: String,
    pub scheme_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
}

/// Wrapper that optionally carries a per-command response sender for socket connections.
pub struct DaemonCmd {
    pub cmd: InCmd,
    pub response_tx: Option<mpsc::UnboundedSender<OutMsg>>,
}

pub fn out_error(request_id: Option<String>, code: &str, message: impl Into<String>) -> OutMsg {
    OutMsg::Error {
        request_id,
        code: code.to_string(),
        message: message.into(),
    }
}

pub fn out_ok(request_id: Option<String>, result: Option<serde_json::Value>) -> OutMsg {
    OutMsg::Ok { request_id, result }
}

fn default_channels() -> u16 {
    1
}

fn default_reject_reason() -> String {
    "declined".to_string()
}

fn default_end_reason() -> String {
    "user_hangup".to_string()
}

fn default_get_messages_limit() -> usize {
    50
}

fn default_group_name() -> String {
    "DM".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::Kind;

    #[test]
    fn deserialize_send_media_full() {
        let json = r#"{
            "cmd": "send_media",
            "request_id": "r1",
            "nostr_group_id": "aa",
            "file_path": "/tmp/photo.jpg",
            "mime_type": "image/jpeg",
            "filename": "photo.jpg",
            "caption": "Check this out",
            "blossom_servers": ["https://blossom.example.com"]
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::SendMedia {
                request_id,
                nostr_group_id,
                file_path,
                mime_type,
                filename,
                caption,
                blossom_servers,
            } => {
                assert_eq!(request_id.as_deref(), Some("r1"));
                assert_eq!(nostr_group_id, "aa");
                assert_eq!(file_path, "/tmp/photo.jpg");
                assert_eq!(mime_type.as_deref(), Some("image/jpeg"));
                assert_eq!(filename.as_deref(), Some("photo.jpg"));
                assert_eq!(caption, "Check this out");
                assert_eq!(blossom_servers, vec!["https://blossom.example.com"]);
            }
            other => panic!("expected SendMedia, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_send_media_minimal() {
        let json = r#"{
            "cmd": "send_media",
            "nostr_group_id": "bb",
            "file_path": "/tmp/file.bin"
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::SendMedia {
                request_id,
                mime_type,
                filename,
                caption,
                blossom_servers,
                ..
            } => {
                assert!(request_id.is_none());
                assert!(mime_type.is_none());
                assert!(filename.is_none());
                assert_eq!(caption, "");
                assert!(blossom_servers.is_empty());
            }
            other => panic!("expected SendMedia, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_hypernote_catalog_cmd() {
        let json = r#"{"cmd":"hypernote_catalog","request_id":"r2"}"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::HypernoteCatalog { request_id } => {
                assert_eq!(request_id.as_deref(), Some("r2"));
            }
            other => panic!("expected HypernoteCatalog, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_react_cmd() {
        let json = r#"{
            "cmd": "react",
            "request_id": "r3",
            "nostr_group_id": "aa",
            "event_id": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "emoji": "🧇"
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::React {
                request_id,
                nostr_group_id,
                event_id,
                emoji,
            } => {
                assert_eq!(request_id.as_deref(), Some("r3"));
                assert_eq!(nostr_group_id, "aa");
                assert_eq!(
                    event_id,
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                );
                assert_eq!(emoji, "🧇");
            }
            other => panic!("expected React, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_submit_hypernote_action_cmd() {
        let json = r#"{
            "cmd": "submit_hypernote_action",
            "request_id": "r4",
            "nostr_group_id": "aa",
            "event_id": "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
            "action": "vote_yes",
            "form": {"note":"ship it"}
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::SubmitHypernoteAction {
                request_id,
                nostr_group_id,
                event_id,
                action,
                form,
            } => {
                assert_eq!(request_id.as_deref(), Some("r4"));
                assert_eq!(nostr_group_id, "aa");
                assert_eq!(
                    event_id,
                    "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
                );
                assert_eq!(action, "vote_yes");
                assert_eq!(form.get("note").map(String::as_str), Some("ship it"));
            }
            other => panic!("expected SubmitHypernoteAction, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_send_audio_response_cmd() {
        let json = r#"{
            "cmd": "send_audio_response",
            "request_id": "r5",
            "call_id": "550e8400-e29b-41d4-a716-446655440010",
            "tts_text": "hello from sidecar"
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize send_audio_response");
        match cmd {
            InCmd::SendAudioResponse {
                request_id,
                call_id,
                tts_text,
            } => {
                assert_eq!(request_id.as_deref(), Some("r5"));
                assert_eq!(call_id, "550e8400-e29b-41d4-a716-446655440010");
                assert_eq!(tts_text, "hello from sidecar");
            }
            other => panic!("expected SendAudioResponse, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_send_audio_file_cmd_defaults_channels() {
        let json = r#"{
            "cmd": "send_audio_file",
            "request_id": "r6",
            "call_id": "550e8400-e29b-41d4-a716-446655440011",
            "audio_path": "/tmp/reply.pcm",
            "sample_rate": 24000
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize send_audio_file");
        match cmd {
            InCmd::SendAudioFile {
                request_id,
                call_id,
                audio_path,
                sample_rate,
                channels,
            } => {
                assert_eq!(request_id.as_deref(), Some("r6"));
                assert_eq!(call_id, "550e8400-e29b-41d4-a716-446655440011");
                assert_eq!(audio_path, "/tmp/reply.pcm");
                assert_eq!(sample_rate, 24_000);
                assert_eq!(channels, 1);
            }
            other => panic!("expected SendAudioFile, got {other:?}"),
        }
    }

    #[test]
    fn serialize_message_received_without_media() {
        let msg = OutMsg::MessageReceived {
            nostr_group_id: "aabb".into(),
            from_pubkey: "cc".into(),
            content: "hello".into(),
            kind: Kind::ChatMessage.as_u16(),
            created_at: 123,
            event_id: "ee".into(),
            message_id: "dd".into(),
            media: vec![],
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "message_received");
        assert_eq!(json["content"], "hello");
        assert_eq!(json["kind"], Kind::ChatMessage.as_u16());
        assert_eq!(json["event_id"], "ee");
        assert!(json.get("media").is_none());
    }

    #[test]
    fn serialize_message_received_with_media() {
        let msg = OutMsg::MessageReceived {
            nostr_group_id: "aabb".into(),
            from_pubkey: "cc".into(),
            content: "look at this".into(),
            kind: Kind::ChatMessage.as_u16(),
            created_at: 456,
            event_id: "ee".into(),
            message_id: "dd".into(),
            media: vec![MediaAttachmentOut {
                url: "https://blossom.example.com/abc123".into(),
                mime_type: "image/png".into(),
                filename: "screenshot.png".into(),
                original_hash_hex: "deadbeef".into(),
                nonce_hex: "cafebabe".into(),
                scheme_version: "v1".into(),
                width: Some(800),
                height: Some(600),
                local_path: Some("/tmp/decrypted.png".into()),
            }],
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["kind"], Kind::ChatMessage.as_u16());
        assert_eq!(json["event_id"], "ee");
        let media = json["media"].as_array().expect("media should be array");
        assert_eq!(media.len(), 1);
        assert_eq!(media[0]["url"], "https://blossom.example.com/abc123");
        assert_eq!(media[0]["mime_type"], "image/png");
        assert_eq!(media[0]["filename"], "screenshot.png");
        assert_eq!(media[0]["width"], 800);
        assert_eq!(media[0]["height"], 600);
    }

    #[test]
    fn serialize_message_received_media_omits_null_dimensions() {
        let msg = OutMsg::MessageReceived {
            nostr_group_id: "aabb".into(),
            from_pubkey: "cc".into(),
            content: "".into(),
            kind: Kind::ChatMessage.as_u16(),
            created_at: 0,
            event_id: "ee".into(),
            message_id: "dd".into(),
            media: vec![MediaAttachmentOut {
                url: "https://example.com/file".into(),
                mime_type: "application/pdf".into(),
                filename: "doc.pdf".into(),
                original_hash_hex: "aa".into(),
                nonce_hex: "bb".into(),
                scheme_version: "v1".into(),
                width: None,
                height: None,
                local_path: None,
            }],
        };
        let json = serde_json::to_value(&msg).unwrap();
        let media = &json["media"][0];
        assert!(media.get("width").is_none());
        assert!(media.get("height").is_none());
    }

    #[test]
    fn deserialize_send_media_batch_full() {
        let json = r#"{
            "cmd": "send_media_batch",
            "request_id": "r1",
            "nostr_group_id": "bb",
            "file_paths": ["/tmp/a.jpg", "/tmp/b.png"],
            "caption": "Two photos",
            "blossom_servers": ["https://blossom.example.com"]
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::SendMediaBatch {
                request_id,
                nostr_group_id,
                file_paths,
                caption,
                blossom_servers,
            } => {
                assert_eq!(request_id.as_deref(), Some("r1"));
                assert_eq!(nostr_group_id, "bb");
                assert_eq!(file_paths, vec!["/tmp/a.jpg", "/tmp/b.png"]);
                assert_eq!(caption, "Two photos");
                assert_eq!(blossom_servers, vec!["https://blossom.example.com"]);
            }
            other => panic!("expected SendMediaBatch, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_send_media_batch_defaults() {
        let json = r#"{
            "cmd": "send_media_batch",
            "nostr_group_id": "cc",
            "file_paths": ["/tmp/x.jpg"]
        }"#;
        let cmd: InCmd = serde_json::from_str(json).expect("deserialize");
        match cmd {
            InCmd::SendMediaBatch {
                request_id,
                caption,
                blossom_servers,
                file_paths,
                ..
            } => {
                assert!(request_id.is_none());
                assert_eq!(caption, "");
                assert!(blossom_servers.is_empty());
                assert_eq!(file_paths.len(), 1);
            }
            other => panic!("expected SendMediaBatch, got {other:?}"),
        }
    }
}
