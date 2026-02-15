import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { once } from "node:events";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";
import { getMarmotRuntime } from "./runtime.js";

type SidecarOutMsg =
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
  | { type: "group_joined"; nostr_group_id: string; mls_group_id: string }
  | {
      type: "message_received";
      nostr_group_id: string;
      from_pubkey: string;
      content: string;
      created_at: number;
      message_id: string;
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
      type: "call_transcript_partial";
      call_id: string;
      text: string;
    }
  | {
      type: "call_transcript_final";
      call_id: string;
      text: string;
    };

type SidecarInCmd =
  | { cmd: "publish_keypackage"; request_id: string; relays: string[] }
  | { cmd: "set_relays"; request_id: string; relays: string[] }
  | { cmd: "list_pending_welcomes"; request_id: string }
  | { cmd: "accept_welcome"; request_id: string; wrapper_event_id: string }
  | { cmd: "list_groups"; request_id: string }
  | { cmd: "send_message"; request_id: string; nostr_group_id: string; content: string }
  | { cmd: "accept_call"; request_id: string; call_id: string }
  | { cmd: "reject_call"; request_id: string; call_id: string; reason?: string }
  | { cmd: "end_call"; request_id: string; call_id: string; reason?: string }
  | { cmd: "send_audio_response"; request_id: string; call_id: string; tts_text: string }
  | { cmd: "shutdown"; request_id: string };

type SidecarEventHandler = (msg: SidecarOutMsg) => void | Promise<void>;

function sanitizePathSegment(value: string): string {
  const cleaned = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "_")
    .replace(/^_+|_+$/g, "");
  return cleaned || "default";
}

export function resolveAccountStateDir(params: {
  accountId: string;
  stateDirOverride?: string | undefined;
  env?: NodeJS.ProcessEnv;
}): string {
  if (params.stateDirOverride && params.stateDirOverride.trim()) {
    return path.resolve(params.stateDirOverride.trim());
  }
  const env = params.env ?? process.env;
  const stateDir = getMarmotRuntime().state.resolveStateDir(env, os.homedir);
  return path.join(stateDir, "marmot", "accounts", sanitizePathSegment(params.accountId));
}

export class MarmotSidecar {
  #proc: ChildProcessWithoutNullStreams;
  #closed = false;
  #requestSeq = 0;
  #pending = new Map<
    string,
    {
      cmd: string;
      resolve: (v: unknown) => void;
      reject: (e: Error) => void;
      startedAt: number;
    }
  >();
  #onEvent: SidecarEventHandler | null = null;
  #readyResolve: ((msg: SidecarOutMsg & { type: "ready" }) => void) | null = null;
  #readyReject: ((err: Error) => void) | null = null;
  #readyPromise: Promise<SidecarOutMsg & { type: "ready" }>;

  constructor(params: { cmd: string; args: string[]; env?: NodeJS.ProcessEnv }) {
    this.#proc = spawn(params.cmd, params.args, {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, ...(params.env ?? {}) },
    });

    // Keep stdout strictly for JSONL; log sidecar stderr through OpenClaw logger.
    const rl = readline.createInterface({ input: this.#proc.stdout });
    rl.on("line", (line) => {
      this.#handleLine(line).catch((err) => {
        getMarmotRuntime().logger?.error(`[marmotd] unhandled error in handleLine: ${err}`);
      });
    });

    this.#proc.stderr.on("data", (buf) => {
      const s = String(buf);
      if (s.trim()) {
        // Avoid spamming logs with multi-line blobs; keep it line-ish.
        for (const ln of s.split(/\r?\n/)) {
          const trimmed = ln.trim();
          if (!trimmed) continue;
          // Sidecar parse/compat errors are critical for debugging interop, and OpenClaw's
          // default log level may hide debug output. Escalate likely call-signal errors.
          const looksLikeCallSignalIssue =
            trimmed.includes("pika.call") ||
            trimmed.includes("call.invite") ||
            trimmed.includes("call.accept") ||
            trimmed.includes("call signal");
          const log = getMarmotRuntime().logger;
          if (looksLikeCallSignalIssue) {
            log?.warn(`[marmotd] ${trimmed}`);
          } else {
            log?.debug(`[marmotd] ${trimmed}`);
          }
        }
      }
    });

    this.#readyPromise = new Promise((resolve, reject) => {
      this.#readyResolve = resolve;
      this.#readyReject = reject;
    });

    this.#proc.on("exit", (code, signal) => {
      this.#closed = true;
      const err = new Error(`marmot sidecar exited code=${code ?? "null"} signal=${signal ?? "null"}`);
      for (const { reject } of this.#pending.values()) {
        reject(err);
      }
      this.#pending.clear();
      // If we never got ready, unblock startup.
      const rr = this.#readyReject;
      if (rr) {
        this.#readyResolve = null;
        this.#readyReject = null;
        rr(err);
      }
    });
  }

  onEvent(handler: SidecarEventHandler): void {
    this.#onEvent = handler;
  }

  pid(): number | undefined {
    return this.#proc.pid;
  }

  async waitForReady(timeoutMs: number = 10_000): Promise<SidecarOutMsg & { type: "ready" }> {
    if (this.#closed) {
      throw new Error("sidecar already closed");
    }
    const timeoutPromise = new Promise<never>((_resolve, reject) => {
      const t = setTimeout(() => reject(new Error("timeout waiting for sidecar ready")), timeoutMs);
      // Avoid holding the event loop open if the caller abandons the promise.
      (t as any).unref?.();
    });
    const exitPromise = once(this.#proc, "exit").then(() => {
      throw new Error("sidecar exited before ready");
    });
    return await Promise.race([this.#readyPromise, timeoutPromise, exitPromise]);
  }

  async request(cmd: Omit<SidecarInCmd, "request_id">): Promise<unknown> {
    if (this.#closed) {
      throw new Error("sidecar is closed");
    }
    const requestId = `r${Date.now()}_${++this.#requestSeq}`;
    const payload = { ...cmd, request_id: requestId } as SidecarInCmd;
    const line = JSON.stringify(payload);

    const startedAt = Date.now();
    const cmdName = String((cmd as any).cmd ?? "unknown");
    const logRequests = String(process.env.MARMOT_SIDECAR_LOG_REQUESTS ?? "").trim() === "1";
    const p = new Promise<unknown>((resolve, reject) => {
      this.#pending.set(requestId, { cmd: cmdName, resolve, reject, startedAt });
    });
    if (logRequests) {
      getMarmotRuntime().logger?.info(
        `[marmot] sidecar_request_start cmd=${cmdName} request_id=${requestId}`,
      );
    }
    this.#proc.stdin.write(`${line}\n`);
    return await p;
  }

  async publishKeypackage(relays: string[]): Promise<void> {
    await this.request({ cmd: "publish_keypackage", relays } as any);
  }

  async setRelays(relays: string[]): Promise<void> {
    await this.request({ cmd: "set_relays", relays } as any);
  }

  async listPendingWelcomes(): Promise<unknown> {
    return await this.request({ cmd: "list_pending_welcomes" } as any);
  }

  async acceptWelcome(wrapperEventId: string): Promise<void> {
    await this.request({ cmd: "accept_welcome", wrapper_event_id: wrapperEventId } as any);
  }

  async listGroups(): Promise<unknown> {
    return await this.request({ cmd: "list_groups" } as any);
  }

  async sendMessage(nostrGroupId: string, content: string): Promise<void> {
    await this.request({ cmd: "send_message", nostr_group_id: nostrGroupId, content } as any);
  }

  async acceptCall(callId: string): Promise<void> {
    await this.request({ cmd: "accept_call", call_id: callId } as any);
  }

  async rejectCall(callId: string, reason?: string): Promise<void> {
    await this.request({ cmd: "reject_call", call_id: callId, reason } as any);
  }

  async endCall(callId: string, reason?: string): Promise<void> {
    await this.request({ cmd: "end_call", call_id: callId, reason } as any);
  }

  async sendAudioResponse(
    callId: string,
    ttsText: string,
  ): Promise<{
    call_id: string;
    frames_published: number;
    publish_path?: string;
    subscribe_path?: string;
    track?: string;
    local_label?: string;
    peer_label?: string;
  }> {
    const result = await this.request({
      cmd: "send_audio_response",
      call_id: callId,
      tts_text: ttsText,
    } as any);
    const framesPublished = (result as any)?.frames_published;
    if (typeof framesPublished !== "number" || !Number.isFinite(framesPublished)) {
      throw new Error("unexpected send_audio_response result (missing frames_published)");
    }
    const publishPath = (result as any)?.publish_path;
    const subscribePath = (result as any)?.subscribe_path;
    const track = (result as any)?.track;
    const localLabel = (result as any)?.local_label;
    const peerLabel = (result as any)?.peer_label;
    return {
      call_id: callId,
      frames_published: framesPublished,
      publish_path: typeof publishPath === "string" ? publishPath : undefined,
      subscribe_path: typeof subscribePath === "string" ? subscribePath : undefined,
      track: typeof track === "string" ? track : undefined,
      local_label: typeof localLabel === "string" ? localLabel : undefined,
      peer_label: typeof peerLabel === "string" ? peerLabel : undefined,
    };
  }

  async shutdown(): Promise<void> {
    if (this.#closed) return;
    try {
      await this.request({ cmd: "shutdown" } as any);
    } catch {
      // ignore
    }
    this.#proc.kill("SIGTERM");
  }

  async #handleLine(line: string): Promise<void> {
    const trimmed = line.trim();
    if (!trimmed) return;
    let msg: SidecarOutMsg;
    try {
      msg = JSON.parse(trimmed) as SidecarOutMsg;
    } catch {
      getMarmotRuntime().logger?.warn(`[marmot] invalid JSON from sidecar: ${trimmed}`);
      return;
    }

    if (msg.type === "ready") {
      const rr = this.#readyResolve;
      if (rr) {
        this.#readyResolve = null;
        this.#readyReject = null;
        rr(msg);
      }
      return;
    }

    if (msg.type === "ok" || msg.type === "error") {
      const requestId = (msg as any).request_id;
      if (typeof requestId === "string" && requestId) {
        const pending = this.#pending.get(requestId);
        if (pending) {
          this.#pending.delete(requestId);
          const logRequests = String(process.env.MARMOT_SIDECAR_LOG_REQUESTS ?? "").trim() === "1";
          const elapsedMs = Date.now() - pending.startedAt;
          if (msg.type === "ok") {
            if (logRequests) {
              getMarmotRuntime().logger?.info(
                `[marmot] sidecar_request_ok cmd=${pending.cmd} request_id=${requestId} elapsed_ms=${elapsedMs}`,
              );
            }
            pending.resolve((msg as any).result ?? null);
          } else {
            if (logRequests) {
              getMarmotRuntime().logger?.warn(
                `[marmot] sidecar_request_error cmd=${pending.cmd} request_id=${requestId} elapsed_ms=${elapsedMs} code=${msg.code} message=${JSON.stringify(msg.message)}`,
              );
            }
            pending.reject(new Error(`${msg.code}: ${msg.message}`));
          }
          return;
        }
      }
    }

    const handler = this.#onEvent;
    if (handler) {
      await handler(msg);
    }
  }
}
