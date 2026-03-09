/**
 * TypeScript mirror of the native pikachat daemon JSONL protocol.
 *
 * Keep this aligned with crates/pikachat-sidecar/src/protocol.rs. OpenClaw
 * should treat this module as its daemon integration contract, not daemon
 * implementation detail.
 */

export type PikachatDaemonOutMsg =
  | { type: "ready"; protocol_version: number; pubkey: string; npub: string }
  | { type: "ok"; request_id?: string | null; result?: unknown }
  | { type: "error"; request_id?: string | null; code: string; message: string }
  | { type: "keypackage_published"; event_id: string }
  | {
      type: "welcome_received";
      wrapper_event_id: string;
      welcome_event_id: string;
      from_pubkey: string;
      nostr_group_id: string;
      group_name: string;
    }
  | { type: "group_joined"; nostr_group_id: string; mls_group_id: string; member_count: number }
  | {
      type: "message_received";
      nostr_group_id: string;
      from_pubkey: string;
      content: string;
      kind: number;
      created_at: number;
      event_id: string;
      message_id: string;
      media?: Array<{
        url: string;
        mime_type: string;
        filename: string;
        original_hash_hex: string;
        nonce_hex: string;
        scheme_version: string;
        width?: number | null;
        height?: number | null;
        local_path?: string | null;
      }>;
    }
  | {
      type: "call_invite_received";
      call_id: string;
      from_pubkey: string;
      nostr_group_id: string;
    }
  | {
      type: "call_session_started";
      call_id: string;
      nostr_group_id: string;
      from_pubkey: string;
    }
  | { type: "call_session_ended"; call_id: string; reason: string }
  | {
      type: "call_debug";
      call_id: string;
      tx_frames: number;
      rx_frames: number;
      rx_dropped: number;
    }
  | {
      type: "call_audio_chunk";
      call_id: string;
      audio_path: string;
      sample_rate: number;
      channels: number;
    }
  | {
      type: "group_created";
      nostr_group_id: string;
      mls_group_id: string;
      peer_pubkey: string;
      member_count: number;
    };

export type PikachatDaemonInCmd =
  | { cmd: "publish_keypackage"; request_id: string; relays: string[] }
  | { cmd: "set_relays"; request_id: string; relays: string[] }
  | { cmd: "list_pending_welcomes"; request_id: string }
  | { cmd: "accept_welcome"; request_id: string; wrapper_event_id: string }
  | { cmd: "list_groups"; request_id: string }
  | { cmd: "hypernote_catalog"; request_id: string }
  | { cmd: "send_message"; request_id: string; nostr_group_id: string; content: string }
  | {
      cmd: "send_hypernote";
      request_id: string;
      nostr_group_id: string;
      content: string;
      title?: string;
      state?: string;
    }
  | { cmd: "react"; request_id: string; nostr_group_id: string; event_id: string; emoji: string }
  | {
      cmd: "submit_hypernote_action";
      request_id: string;
      nostr_group_id: string;
      event_id: string;
      action: string;
      form?: Record<string, string>;
    }
  | { cmd: "send_typing"; request_id: string; nostr_group_id: string }
  | { cmd: "accept_call"; request_id: string; call_id: string }
  | { cmd: "reject_call"; request_id: string; call_id: string; reason?: string }
  | { cmd: "end_call"; request_id: string; call_id: string; reason?: string }
  | { cmd: "send_audio_response"; request_id: string; call_id: string; tts_text: string }
  | {
      cmd: "send_audio_file";
      request_id: string;
      call_id: string;
      audio_path: string;
      sample_rate: number;
      channels?: number;
    }
  | { cmd: "init_group"; request_id: string; peer_pubkey: string; group_name?: string }
  | {
      cmd: "send_media";
      request_id: string;
      nostr_group_id: string;
      file_path: string;
      mime_type?: string;
      filename?: string;
      caption?: string;
      blossom_servers?: string[];
    }
  | { cmd: "shutdown"; request_id: string };

export type PikachatDaemonEventHandler = (msg: PikachatDaemonOutMsg) => void | Promise<void>;
