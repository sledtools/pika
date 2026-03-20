import { access, mkdir, realpath } from "node:fs/promises";
import path from "node:path";

import {
  allowSender,
  approvePairing,
  defaultAccessState,
  denyPairing,
  disableGroup,
  enableGroup,
  evaluateDmAccess,
  evaluateGroupAccess,
  ensurePendingPairing,
  loadAccessState,
  pruneExpiredPairings,
  removeSender,
  saveAccessState,
  setDmPolicy,
  type AccessState,
  type DmPolicy,
} from "./access.js";
import type { PikachatClaudeConfig } from "./config.js";
import { PikachatDaemonClient, type PikachatDaemonLike, type PikachatLogger } from "./daemon-client.js";
import { buildPikachatDaemonLaunchSpec } from "./daemon-launch.js";
import type { PikachatDaemonOutMsg } from "./daemon-protocol.js";
import { augmentMessageText, detectMention, sanitizeMeta } from "./message-format.js";

export type ChannelNotification = {
  content: string;
  meta: Record<string, string>;
};

export type ChannelStatus = {
  botPubkey: string | null;
  botNpub: string | null;
  knownGroups: string[];
};

export type ReplyRequest = {
  chatId: string;
  text?: string;
  replyTo?: string;
  files?: string[];
};

export type ReactRequest = {
  chatId: string;
  eventId: string;
  emoji: string;
};

export type ChannelRuntimeDeps = {
  daemonFactory?: (params: {
    cmd: string;
    args: string[];
    env?: NodeJS.ProcessEnv;
    logger?: PikachatLogger;
  }) => PikachatDaemonLike;
  now?: () => number;
};

export class PikachatClaudeChannel {
  #config: PikachatClaudeConfig;
  #logger: PikachatLogger | undefined;
  #onNotification: ((notification: ChannelNotification) => void | Promise<void>) | undefined;
  #deps: Required<ChannelRuntimeDeps>;
  #daemon: PikachatDaemonLike | null = null;
  #daemonLaunchAutoAccept = false;
  #botPubkey: string | null = null;
  #botNpub: string | null = null;
  #memberCounts = new Map<string, number>();

  constructor(params: {
    config: PikachatClaudeConfig;
    logger?: PikachatLogger;
    onNotification?: (notification: ChannelNotification) => void | Promise<void>;
    deps?: ChannelRuntimeDeps;
  }) {
    this.#config = params.config;
    this.#logger = params.logger;
    this.#onNotification = params.onNotification;
    this.#deps = {
      daemonFactory:
        params.deps?.daemonFactory ??
        ((spawnParams) => new PikachatDaemonClient(spawnParams)),
      now: params.deps?.now ?? (() => Date.now()),
    };
  }

  status(): ChannelStatus {
    return {
      botPubkey: this.#botPubkey,
      botNpub: this.#botNpub,
      knownGroups: [...this.#memberCounts.keys()].sort(),
    };
  }

  async start(): Promise<void> {
    if (this.#daemon) {
      return;
    }
    if (this.#config.relays.length === 0) {
      throw new Error("PIKACHAT_RELAYS is required");
    }

    await mkdir(this.#config.channelHome, { recursive: true });
    await mkdir(this.#config.inboxDir, { recursive: true });
    const initialState = pruneExpiredPairings(await loadAccessState(this.#config.accessFile), this.#deps.now());
    await saveAccessState(this.#config.accessFile, initialState);

    const launch = await buildPikachatDaemonLaunchSpec({
      config: this.#config,
      log: this.#logger,
    });
    this.#daemonLaunchAutoAccept = launch.autoAcceptWelcomes;
    const daemon = this.#deps.daemonFactory({
      cmd: launch.cmd,
      args: launch.args,
      logger: this.#logger,
    });
    daemon.onEvent(async (event) => {
        await this.#handleDaemonEvent(event);
      });
    this.#daemon = daemon;

    try {
      const ready = await daemon.waitForReady(15_000);
      this.#botPubkey = ready.pubkey.toLowerCase();
      this.#botNpub = ready.npub;
      this.#logger?.info?.(
        `[pikachat-claude] daemon ready pubkey=${this.#botPubkey} npub=${this.#botNpub} pid=${daemon.pid() ?? "unknown"}`,
      );

      daemon.waitForExit().then(() => {
        if (this.#daemon === daemon) {
          this.#daemon = null;
        }
      });

      await daemon.setRelays(this.#config.relays);
      await daemon.publishKeypackage(this.#config.relays);
      await this.#seedKnownGroups();
    } catch (err) {
      if (this.#daemon === daemon) {
        this.#daemon = null;
      }
      this.#botPubkey = null;
      this.#botNpub = null;
      this.#memberCounts.clear();
      try {
        await daemon.shutdown();
      } catch (shutdownErr) {
        this.#logger?.warn?.(
          `[pikachat-claude] failed to stop daemon after startup error: ${shutdownErr instanceof Error ? shutdownErr.message : String(shutdownErr)}`,
        );
      }
      throw err;
    }
  }

  async stop(): Promise<void> {
    if (!this.#daemon) return;
    const daemon = this.#daemon;
    this.#daemon = null;
    await daemon.shutdown();
  }

  async reply(request: ReplyRequest): Promise<{ eventIds: string[]; notes: string[] }> {
    const daemon = this.#requireDaemon();
    const notes: string[] = [];
    const eventIds: string[] = [];
    const text = request.text?.trim() ?? "";
    const files = await this.#resolveOutboundFiles(request.files ?? []);

    if (!text && files.length === 0) {
      throw new Error("reply requires text or files");
    }

    if (request.replyTo?.trim()) {
      notes.push("reply_to is not yet wired through the daemon; sent as a normal message");
    }

    if (text) {
      const result = await daemon.sendMessage(request.chatId, text);
      if (result.event_id) eventIds.push(result.event_id);
    }

    if (files.length > 0) {
      if (files.length === 1) {
        const result = await daemon.sendMedia(request.chatId, files[0]);
        if (result.event_id) eventIds.push(result.event_id);
      } else {
        const result = await daemon.sendMediaBatch(request.chatId, files);
        if (result.event_id) eventIds.push(result.event_id);
      }
    }

    return { eventIds, notes };
  }

  async react(request: ReactRequest): Promise<{ event_id?: string }> {
    const daemon = this.#requireDaemon();
    return await daemon.sendReaction(request.chatId, request.eventId, request.emoji);
  }

  async accessStatus(): Promise<AccessState> {
    return pruneExpiredPairings(await loadAccessState(this.#config.accessFile), this.#deps.now());
  }

  async approvePairing(code: string): Promise<{ senderId: string | null }> {
    const state = await this.accessStatus();
    const approved = approvePairing(state, code);
    await saveAccessState(this.#config.accessFile, approved.state);
    return { senderId: approved.pairing?.senderId ?? null };
  }

  async denyPairing(code: string): Promise<{ senderId: string | null }> {
    const state = await this.accessStatus();
    const denied = denyPairing(state, code);
    await saveAccessState(this.#config.accessFile, denied.state);
    return { senderId: denied.pairing?.senderId ?? null };
  }

  async setDmPolicy(dmPolicy: DmPolicy): Promise<AccessState> {
    const state = await this.accessStatus();
    const next = setDmPolicy(state, dmPolicy);
    await saveAccessState(this.#config.accessFile, next);
    return next;
  }

  async allowSender(senderId: string): Promise<AccessState> {
    const state = await this.accessStatus();
    const next = allowSender(state, senderId);
    await saveAccessState(this.#config.accessFile, next);
    return next;
  }

  async removeSender(senderId: string): Promise<AccessState> {
    const state = await this.accessStatus();
    const next = removeSender(state, senderId);
    await saveAccessState(this.#config.accessFile, next);
    return next;
  }

  async enableGroup(groupId: string, requireMention = true, allowFrom: string[] = []): Promise<AccessState> {
    const state = await this.accessStatus();
    const next = enableGroup(state, groupId, { requireMention, allowFrom });
    await saveAccessState(this.#config.accessFile, next);
    return next;
  }

  async disableGroup(groupId: string): Promise<AccessState> {
    const state = await this.accessStatus();
    const next = disableGroup(state, groupId);
    await saveAccessState(this.#config.accessFile, next);
    return next;
  }

  async #resolveOutboundFiles(files: string[]): Promise<string[]> {
    if (files.length === 0) {
      return [];
    }

    const trustedRoots = await this.#trustedOutboundRoots();
    const resolvedFiles: string[] = [];
    for (const file of files) {
      await access(file);
      const resolvedFile = await realpath(file);
      if (!trustedRoots.some((root) => this.#isWithinRoot(resolvedFile, root))) {
        throw new Error(`attachment path is outside trusted roots: ${file}`);
      }
      resolvedFiles.push(resolvedFile);
    }
    return resolvedFiles;
  }

  async #trustedOutboundRoots(): Promise<string[]> {
    const roots = new Set<string>();
    for (const candidate of [process.cwd(), this.#config.inboxDir]) {
      try {
        roots.add(await realpath(candidate));
      } catch {
        // ignore roots that do not exist yet
      }
    }
    return [...roots];
  }

  #isWithinRoot(candidate: string, root: string): boolean {
    const relative = path.relative(root, candidate);
    return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
  }

  async #seedKnownGroups(): Promise<void> {
    const daemon = this.#requireDaemon();
    try {
      const result = (await daemon.listGroups()) as { groups?: Array<{ nostr_group_id?: string; member_count?: number }> };
      for (const group of result.groups ?? []) {
        const groupId = String(group.nostr_group_id ?? "").trim().toLowerCase();
        if (!groupId) continue;
        this.#memberCounts.set(groupId, Number(group.member_count ?? 0));
      }
    } catch (err) {
      this.#logger?.warn?.(`[pikachat-claude] failed to seed groups: ${err}`);
    }
  }

  async #handleDaemonEvent(event: PikachatDaemonOutMsg): Promise<void> {
    if (!this.#daemon) return;
    switch (event.type) {
      case "welcome_received":
        if (this.#config.autoAcceptWelcomes && !this.#daemonLaunchAutoAccept) {
          try {
            await this.#daemon.acceptWelcome(event.wrapper_event_id);
          } catch (err) {
            const message = err instanceof Error ? err.message : String(err);
            if (!message.includes("not_found")) {
              throw err;
            }
          }
        }
        return;
      case "group_joined":
        this.#memberCounts.set(event.nostr_group_id.toLowerCase(), event.member_count);
        return;
      case "group_created":
        this.#memberCounts.set(event.nostr_group_id.toLowerCase(), event.member_count);
        return;
      case "group_updated":
        if (typeof event.update.member_count === "number") {
          this.#memberCounts.set(event.update.nostr_group_id.toLowerCase(), event.update.member_count);
        }
        return;
      case "message_received":
        await this.#handleInboundMessage(event);
        return;
      default:
        return;
    }
  }

  async #handleInboundMessage(event: Extract<PikachatDaemonOutMsg, { type: "message_received" }>): Promise<void> {
    if (!this.#botPubkey || !this.#botNpub) return;
    const senderId = event.from_pubkey.trim().toLowerCase();
    if (senderId === this.#botPubkey) {
      return;
    }

    const chatId = event.nostr_group_id.trim().toLowerCase();
    const chatType = await this.#resolveChatType(chatId);
    const messageText = augmentMessageText(event.content, event.media ?? []);
    const state = pruneExpiredPairings(await loadAccessState(this.#config.accessFile), this.#deps.now());

    if (chatType === "direct") {
      const decision = evaluateDmAccess(state, senderId);
      if (decision === "allowed") {
        await this.#emitNotification({
          content: messageText,
          meta: sanitizeMeta({
            chat_id: chatId,
            sender_id: senderId,
            sender_name: senderId,
            message_id: event.message_id,
            event_id: event.event_id,
            chat_type: "direct",
            mentioned: "true",
            has_attachments: String(Boolean(event.media?.length)),
          }),
        });
        return;
      }
      if (decision === "pairing") {
        const ensured = ensurePendingPairing(state, senderId, chatId, this.#deps.now());
        await saveAccessState(this.#config.accessFile, ensured.state);
        await this.#daemon!.sendMessage(
          chatId,
          `Pairing code: ${ensured.pairing.code}\nApprove it from Claude with the approve_pairing tool.`,
        );
      }
      return;
    }

    const groupDecision = evaluateGroupAccess(state, chatId, senderId);
    if (!groupDecision.enabled || !groupDecision.senderAllowed) {
      return;
    }
    const mentioned = detectMention({
      text: messageText,
      botPubkey: this.#botPubkey,
      botNpub: this.#botNpub,
      mentionPatterns: state.mentionPatterns,
    });
    if (groupDecision.requireMention && !mentioned) {
      return;
    }

    await this.#emitNotification({
      content: messageText,
      meta: sanitizeMeta({
        chat_id: chatId,
        sender_id: senderId,
        sender_name: senderId,
        message_id: event.message_id,
        event_id: event.event_id,
        chat_type: "group",
        group_name: chatId,
        mentioned: String(mentioned),
        has_attachments: String(Boolean(event.media?.length)),
      }),
    });
  }

  async #resolveChatType(chatId: string): Promise<"direct" | "group"> {
    const cached = this.#memberCounts.get(chatId);
    if (cached !== undefined) {
      return cached <= 2 ? "direct" : "group";
    }
    try {
      const daemon = this.#requireDaemon();
      const members = (await daemon.listMembers(chatId)) as { member_count?: number };
      const memberCount = Number(members.member_count ?? 0);
      if (memberCount > 0) {
        this.#memberCounts.set(chatId, memberCount);
      }
      return memberCount <= 2 ? "direct" : "group";
    } catch {
      return "group";
    }
  }

  async #emitNotification(notification: ChannelNotification): Promise<void> {
    if (!this.#onNotification) return;
    await this.#onNotification(notification);
  }

  #requireDaemon(): PikachatDaemonLike {
    if (!this.#daemon) {
      throw new Error("pikachat daemon is not running");
    }
    return this.#daemon;
  }
}

export function createInMemoryChannelForTests(params: {
  config?: Partial<PikachatClaudeConfig>;
  daemon: PikachatDaemonLike;
  onNotification?: (notification: ChannelNotification) => void | Promise<void>;
  now?: () => number;
}): PikachatClaudeChannel {
  const config: PikachatClaudeConfig = {
    relays: ["ws://127.0.0.1:18080"],
    daemonBackend: "native",
    daemonCmd: process.execPath,
    daemonArgs: ["daemon"],
    autoAcceptWelcomes: true,
    channelSource: "pikachat",
    channelHome: "/tmp/pikachat-claude-test",
    accessFile: "/tmp/pikachat-claude-test/access.json",
    inboxDir: "/tmp/pikachat-claude-test/inbox",
    ...params.config,
  };
  return new PikachatClaudeChannel({
    config,
    onNotification: params.onNotification,
    deps: {
      daemonFactory: () => params.daemon,
      now: params.now,
    },
  });
}
