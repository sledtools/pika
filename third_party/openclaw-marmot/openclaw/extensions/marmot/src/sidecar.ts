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
    };

type SidecarInCmd =
  | { cmd: "publish_keypackage"; request_id: string; relays: string[] }
  | { cmd: "set_relays"; request_id: string; relays: string[] }
  | { cmd: "list_pending_welcomes"; request_id: string }
  | { cmd: "accept_welcome"; request_id: string; wrapper_event_id: string }
  | { cmd: "list_groups"; request_id: string }
  | { cmd: "send_message"; request_id: string; nostr_group_id: string; content: string }
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
    { resolve: (v: unknown) => void; reject: (e: Error) => void; startedAt: number }
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
          getMarmotRuntime().logger?.debug(`[marmotd] ${trimmed}`);
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
    const p = new Promise<unknown>((resolve, reject) => {
      this.#pending.set(requestId, { resolve, reject, startedAt });
    });
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
          if (msg.type === "ok") {
            pending.resolve((msg as any).result ?? null);
          } else {
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
