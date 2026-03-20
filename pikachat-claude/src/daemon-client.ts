import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { once } from "node:events";
import readline from "node:readline";
import type {
  PikachatDaemonEventHandler,
  PikachatDaemonInCmd,
  PikachatDaemonOutMsg,
} from "./daemon-protocol.js";

export type PikachatLogger = {
  debug?: (message: string) => void;
  info?: (message: string) => void;
  warn?: (message: string) => void;
  error?: (message: string) => void;
};

export class SendThrottle {
  #lastSendAt = 0;
  #chain: Promise<void> = Promise.resolve();
  #minIntervalMs: number;
  #onError: (err: Error) => void;

  constructor(minIntervalMs = 1000, onError: (err: Error) => void = () => {}) {
    this.#minIntervalMs = minIntervalMs;
    this.#onError = onError;
  }

  enqueue(fn: () => Promise<unknown>): Promise<void> {
    const task = this.#chain.catch(() => undefined).then(async () => {
      const now = Date.now();
      const elapsed = now - this.#lastSendAt;
      if (elapsed < this.#minIntervalMs) {
        await new Promise((resolve) => setTimeout(resolve, this.#minIntervalMs - elapsed));
      }
      try {
        await fn();
        this.#lastSendAt = Date.now();
      } catch (err) {
        this.#onError(err as Error);
        throw err;
      }
    });
    this.#chain = task.catch(() => undefined);
    return task;
  }
}

type SendMediaResult = {
  event_id?: string;
  uploaded_url?: string;
  uploaded_urls?: string[];
  original_hash_hex?: string;
  original_hashes?: string[];
};

export type DaemonSpawnParams = {
  cmd: string;
  args: string[];
  env?: NodeJS.ProcessEnv;
  logger?: PikachatLogger;
};

export interface PikachatDaemonLike {
  onEvent(handler: PikachatDaemonEventHandler): void;
  waitForReady(timeoutMs?: number): Promise<PikachatDaemonOutMsg & { type: "ready" }>;
  waitForExit(): Promise<void>;
  setRelays(relays: string[]): Promise<void>;
  publishKeypackage(relays: string[]): Promise<void>;
  listGroups(): Promise<unknown>;
  listMembers(nostrGroupId: string): Promise<unknown>;
  listPendingWelcomes(): Promise<unknown>;
  acceptWelcome(wrapperEventId: string): Promise<void>;
  sendMessage(nostrGroupId: string, content: string): Promise<{ event_id?: string }>;
  sendReaction(nostrGroupId: string, eventId: string, emoji: string): Promise<{ event_id?: string }>;
  sendMedia(
    nostrGroupId: string,
    filePath: string,
    opts?: { mimeType?: string; filename?: string; caption?: string; blossomServers?: string[] },
  ): Promise<SendMediaResult>;
  sendMediaBatch(
    nostrGroupId: string,
    filePaths: string[],
    opts?: { caption?: string; blossomServers?: string[] },
  ): Promise<SendMediaResult>;
  sendTyping(nostrGroupId: string): Promise<void>;
  getMessages(nostrGroupId: string, limit?: number): Promise<unknown>;
  shutdown(): Promise<void>;
  pid(): number | undefined;
}

export class PikachatDaemonClient implements PikachatDaemonLike {
  static readonly DEFAULT_REQUEST_TIMEOUT_MS = 30_000;
  #proc: ChildProcessWithoutNullStreams;
  #closed = false;
  #readySeen = false;
  #requestSeq = 0;
  #pending = new Map<
    string,
    {
      cmd: string;
      resolve: (value: unknown) => void;
      reject: (err: Error) => void;
      startedAt: number;
      timeoutId: NodeJS.Timeout;
    }
  >();
  #onEvent: PikachatDaemonEventHandler | null = null;
  #readyResolve: ((msg: PikachatDaemonOutMsg & { type: "ready" }) => void) | null = null;
  #readyReject: ((err: Error) => void) | null = null;
  #readyPromise: Promise<PikachatDaemonOutMsg & { type: "ready" }>;
  #exitResolve: (() => void) | null = null;
  #exitPromise: Promise<void>;
  #stderrTail: string[] = [];
  #logger: PikachatLogger | undefined;
  #sendThrottle: SendThrottle;
  #requestTimeoutMs = PikachatDaemonClient.DEFAULT_REQUEST_TIMEOUT_MS;

  constructor(params: DaemonSpawnParams) {
    this.#logger = params.logger;
    this.#sendThrottle = new SendThrottle(1000, (err) => {
      this.#logger?.error?.(`[pikachat-claude] throttled send failed: ${err}`);
    });
    this.#proc = spawn(params.cmd, params.args, {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, ...(params.env ?? {}) },
    });

    const rl = readline.createInterface({ input: this.#proc.stdout });
    rl.on("line", (line: string) => {
      void this.#handleLine(line).catch((err) => {
        const message = err instanceof Error ? err.message : String(err);
        this.#logger?.error?.(`[pikachat-claude] daemon event handling failed: ${message}`);
      });
    });

    this.#proc.stderr.on("data", (buf: Buffer) => {
      const lines = String(buf).split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
      for (const line of lines) {
        this.#stderrTail.push(line);
        if (this.#stderrTail.length > 40) {
          this.#stderrTail.shift();
        }
        if (!this.#readySeen) {
          this.#logger?.info?.(`[pikachat] ${line}`);
        } else {
          this.#logger?.debug?.(`[pikachat] ${line}`);
        }
      }
    });

    this.#readyPromise = new Promise((resolve, reject) => {
      this.#readyResolve = resolve;
      this.#readyReject = reject;
    });
    this.#exitPromise = new Promise((resolve) => {
      this.#exitResolve = resolve;
    });

    this.#proc.on("exit", (code: number | null, signal: NodeJS.Signals | null) => {
      this.#closed = true;
      if ((code ?? 0) !== 0 && this.#stderrTail.length > 0) {
        this.#logger?.error?.(
          `[pikachat-claude] daemon exited with recent stderr=${JSON.stringify(this.#stderrTail.slice(-12))}`,
        );
      }
      const err = new Error(`pikachat daemon exited code=${code ?? "null"} signal=${signal ?? "null"}`);
      for (const { reject, timeoutId } of this.#pending.values()) {
        clearTimeout(timeoutId);
        reject(err);
      }
      this.#pending.clear();
      this.#readyReject?.(err);
      this.#readyResolve = null;
      this.#readyReject = null;
      this.#exitResolve?.();
      this.#exitResolve = null;
    });
  }

  onEvent(handler: PikachatDaemonEventHandler): void {
    this.#onEvent = handler;
  }

  pid(): number | undefined {
    return this.#proc.pid;
  }

  waitForExit(): Promise<void> {
    if (this.#closed) return Promise.resolve();
    return this.#exitPromise;
  }

  async waitForReady(timeoutMs = 10_000): Promise<PikachatDaemonOutMsg & { type: "ready" }> {
    if (this.#closed) {
      throw new Error("daemon already closed");
    }
    const timeoutPromise = new Promise<never>((_resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("timeout waiting for daemon ready")), timeoutMs);
      (timer as any).unref?.();
    });
    const exitPromise = once(this.#proc, "exit").then(() => {
      throw new Error("daemon exited before ready");
    });
    return await Promise.race([this.#readyPromise, timeoutPromise, exitPromise]);
  }

  async request(cmd: Omit<PikachatDaemonInCmd, "request_id">): Promise<unknown> {
    if (this.#closed) {
      throw new Error("daemon is closed");
    }
    const requestId = `r${Date.now()}_${++this.#requestSeq}`;
    const payload = { ...cmd, request_id: requestId } as PikachatDaemonInCmd;
    const line = JSON.stringify(payload);
    const startedAt = Date.now();
    const cmdName = String((cmd as { cmd?: string }).cmd ?? "unknown");
    const promise = new Promise<unknown>((resolve, reject) => {
      const timeoutId = setTimeout(() => {
        if (!this.#pending.delete(requestId)) {
          return;
        }
        reject(new Error(`request timeout cmd=${cmdName} request_id=${requestId}`));
      }, this.#requestTimeoutMs);
      (timeoutId as any).unref?.();
      this.#pending.set(requestId, { cmd: cmdName, resolve, reject, startedAt, timeoutId });
    });
    this.#proc.stdin.write(`${line}\n`);
    return await promise;
  }

  async publishKeypackage(relays: string[]): Promise<void> {
    await this.request({ cmd: "publish_keypackage", relays } as any);
  }

  async setRelays(relays: string[]): Promise<void> {
    await this.request({ cmd: "set_relays", relays } as any);
  }

  async listPendingWelcomes(): Promise<unknown> {
    return await this.request({ cmd: "list_pending_welcomes" } as const);
  }

  async acceptWelcome(wrapperEventId: string): Promise<void> {
    await this.request({ cmd: "accept_welcome", wrapper_event_id: wrapperEventId } as any);
  }

  async listGroups(): Promise<unknown> {
    return await this.request({ cmd: "list_groups" } as const);
  }

  async listMembers(nostrGroupId: string): Promise<unknown> {
    return await this.request({ cmd: "list_members", nostr_group_id: nostrGroupId } as any);
  }

  async sendMessage(nostrGroupId: string, content: string): Promise<{ event_id?: string }> {
    let result: unknown;
    await this.#sendThrottle.enqueue(async () => {
      result = await this.request({ cmd: "send_message", nostr_group_id: nostrGroupId, content } as any);
    });
    return (result as { event_id?: string }) ?? {};
  }

  async sendReaction(nostrGroupId: string, eventId: string, emoji: string): Promise<{ event_id?: string }> {
    let result: unknown;
    await this.#sendThrottle.enqueue(async () => {
      result = await this.request({
        cmd: "react",
        nostr_group_id: nostrGroupId,
        event_id: eventId,
        emoji,
      } as any);
    });
    return (result as { event_id?: string }) ?? {};
  }

  async sendMedia(
    nostrGroupId: string,
    filePath: string,
    opts?: { mimeType?: string; filename?: string; caption?: string; blossomServers?: string[] },
  ): Promise<SendMediaResult> {
    let result: unknown;
    await this.#sendThrottle.enqueue(async () => {
      result = await this.request({
        cmd: "send_media",
        nostr_group_id: nostrGroupId,
        file_path: filePath,
        mime_type: opts?.mimeType,
        filename: opts?.filename,
        caption: opts?.caption,
        blossom_servers: opts?.blossomServers,
      } as any);
    });
    return (result as SendMediaResult) ?? {};
  }

  async sendMediaBatch(
    nostrGroupId: string,
    filePaths: string[],
    opts?: { caption?: string; blossomServers?: string[] },
  ): Promise<SendMediaResult> {
    let result: unknown;
    await this.#sendThrottle.enqueue(async () => {
      result = await this.request({
        cmd: "send_media_batch",
        nostr_group_id: nostrGroupId,
        file_paths: filePaths,
        caption: opts?.caption,
        blossom_servers: opts?.blossomServers,
      } as any);
    });
    return (result as SendMediaResult) ?? {};
  }

  async sendTyping(nostrGroupId: string): Promise<void> {
    await this.request({ cmd: "send_typing", nostr_group_id: nostrGroupId } as any);
  }

  async getMessages(nostrGroupId: string, limit = 50): Promise<unknown> {
    return await this.request({ cmd: "get_messages", nostr_group_id: nostrGroupId, limit } as any);
  }

  async shutdown(): Promise<void> {
    if (this.#closed) return;
    try {
      await this.request({ cmd: "shutdown" } as const);
    } catch {
      // ignore
    }
    this.#proc.kill("SIGTERM");
  }

  async #handleLine(line: string): Promise<void> {
    const trimmed = line.trim();
    if (!trimmed) return;
    let msg: PikachatDaemonOutMsg;
    try {
      msg = JSON.parse(trimmed) as PikachatDaemonOutMsg;
    } catch {
      this.#logger?.warn?.(`[pikachat-claude] invalid JSON from daemon: ${trimmed}`);
      return;
    }

    if (msg.type === "ready") {
      this.#readySeen = true;
      this.#readyResolve?.(msg);
      this.#readyResolve = null;
      this.#readyReject = null;
      return;
    }

    if (msg.type === "ok" || msg.type === "error") {
      const requestId = (msg as { request_id?: string | null }).request_id;
      if (typeof requestId === "string" && requestId) {
        const pending = this.#pending.get(requestId);
        if (pending) {
          this.#pending.delete(requestId);
          clearTimeout(pending.timeoutId);
          const elapsedMs = Date.now() - pending.startedAt;
          if (msg.type === "ok") {
            this.#logger?.debug?.(
              `[pikachat-claude] request ok cmd=${pending.cmd} request_id=${requestId} elapsed_ms=${elapsedMs}`,
            );
            pending.resolve((msg as { result?: unknown }).result ?? null);
          } else {
            this.#logger?.warn?.(
              `[pikachat-claude] request error cmd=${pending.cmd} request_id=${requestId} elapsed_ms=${elapsedMs} code=${msg.code}`,
            );
            pending.reject(new Error(`${msg.code}: ${msg.message}`));
          }
          return;
        }
      }
    }

    if (this.#onEvent) {
      await this.#onEvent(msg);
    }
  }
}
