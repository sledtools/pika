import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { describe, it } from "node:test";

import { createInMemoryChannelForTests } from "./channel-runtime.js";
import type { PikachatDaemonEventHandler, PikachatDaemonOutMsg } from "./daemon-protocol.js";

class FakeDaemon {
  handler: PikachatDaemonEventHandler | null = null;
  sentMessages: Array<{ chatId: string; content: string }> = [];
  sentReactions: Array<{ chatId: string; eventId: string; emoji: string }> = [];
  mediaBatches: Array<{ chatId: string; files: string[] }> = [];
  ready = { type: "ready", protocol_version: 1, pubkey: "botpub", npub: "npub1bot" } as const;
  memberCountByGroup = new Map<string, number>();

  onEvent(handler: PikachatDaemonEventHandler): void {
    this.handler = handler;
  }

  async waitForReady() {
    return this.ready;
  }

  async waitForExit() {
    return await new Promise<void>(() => {});
  }

  async setRelays() {}

  async publishKeypackage() {}

  async listGroups() {
    return {
      groups: [...this.memberCountByGroup.entries()].map(([nostr_group_id, member_count]) => ({
        nostr_group_id,
        member_count,
      })),
    };
  }

  async listMembers(nostrGroupId: string) {
    return { member_count: this.memberCountByGroup.get(nostrGroupId) ?? 0 };
  }

  async listPendingWelcomes() {
    return { welcomes: [] };
  }

  async acceptWelcome() {}

  async sendMessage(chatId: string, content: string) {
    this.sentMessages.push({ chatId, content });
    return { event_id: `event-${this.sentMessages.length}` };
  }

  async sendReaction(chatId: string, eventId: string, emoji: string) {
    this.sentReactions.push({ chatId, eventId, emoji });
    return { event_id: "reaction-event" };
  }

  async sendMedia(chatId: string, filePath: string) {
    this.mediaBatches.push({ chatId, files: [filePath] });
    return {};
  }

  async sendMediaBatch(chatId: string, filePaths: string[]) {
    this.mediaBatches.push({ chatId, files: [...filePaths] });
    return {};
  }

  async sendTyping() {}

  async getMessages() {
    return { messages: [] };
  }

  async shutdown() {}

  pid() {
    return 1234;
  }

  async emit(event: PikachatDaemonOutMsg): Promise<void> {
    if (!this.handler) {
      throw new Error("missing event handler");
    }
    await this.handler(event);
  }
}

describe("PikachatClaudeChannel", () => {
  it("pairs unknown DM senders instead of delivering them", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "pikachat-claude-"));
    const daemon = new FakeDaemon();
    daemon.memberCountByGroup.set("dm1", 2);
    const notifications: Array<{ content: string; meta: Record<string, string> }> = [];
    const channel = createInMemoryChannelForTests({
      daemon,
      onNotification: (notification) => {
        notifications.push(notification);
      },
      config: {
        channelHome: tempDir,
        accessFile: path.join(tempDir, "access.json"),
        inboxDir: path.join(tempDir, "inbox"),
      },
    });
    try {
      await channel.start();
      await daemon.emit({
        type: "message_received",
        nostr_group_id: "dm1",
        from_pubkey: "sender1",
        content: "hello",
        kind: 9,
        created_at: 1,
        event_id: "ev1",
        message_id: "msg1",
      });
      assert.equal(notifications.length, 0);
      assert.equal(daemon.sentMessages.length, 1);
      assert.match(daemon.sentMessages[0].content, /Pairing code:/);
    } finally {
      await channel.stop();
      await rm(tempDir, { recursive: true, force: true });
    }
  });

  it("delivers allowlisted DMs", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "pikachat-claude-"));
    const daemon = new FakeDaemon();
    daemon.memberCountByGroup.set("dm1", 2);
    const notifications: Array<{ content: string; meta: Record<string, string> }> = [];
    const channel = createInMemoryChannelForTests({
      daemon,
      onNotification: (notification) => {
        notifications.push(notification);
      },
      config: {
        channelHome: tempDir,
        accessFile: path.join(tempDir, "access.json"),
        inboxDir: path.join(tempDir, "inbox"),
      },
    });
    try {
      await channel.start();
      await channel.allowSender("sender1");
      await daemon.emit({
        type: "message_received",
        nostr_group_id: "dm1",
        from_pubkey: "sender1",
        content: "hello",
        kind: 9,
        created_at: 1,
        event_id: "ev1",
        message_id: "msg1",
        media: [{ filename: "pic.jpg", mime_type: "image/jpeg", local_path: "/tmp/pic.jpg" }],
      });
      assert.equal(notifications.length, 1);
      assert.equal(notifications[0].meta.chat_type, "direct");
      assert.match(notifications[0].content, /\[Attachment: pic\.jpg/);
    } finally {
      await channel.stop();
      await rm(tempDir, { recursive: true, force: true });
    }
  });

  it("enforces group mention gating", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "pikachat-claude-"));
    const daemon = new FakeDaemon();
    daemon.memberCountByGroup.set("group1", 3);
    const notifications: Array<{ content: string; meta: Record<string, string> }> = [];
    const channel = createInMemoryChannelForTests({
      daemon,
      onNotification: (notification) => {
        notifications.push(notification);
      },
      config: {
        channelHome: tempDir,
        accessFile: path.join(tempDir, "access.json"),
        inboxDir: path.join(tempDir, "inbox"),
      },
    });
    try {
      await channel.start();
      await channel.enableGroup("group1", true);
      await daemon.emit({
        type: "message_received",
        nostr_group_id: "group1",
        from_pubkey: "sender1",
        content: "plain message",
        kind: 9,
        created_at: 1,
        event_id: "ev1",
        message_id: "msg1",
      });
      assert.equal(notifications.length, 0);
      await daemon.emit({
        type: "message_received",
        nostr_group_id: "group1",
        from_pubkey: "sender1",
        content: "hello npub1bot",
        kind: 9,
        created_at: 1,
        event_id: "ev2",
        message_id: "msg2",
      });
      assert.equal(notifications.length, 1);
      assert.equal(notifications[0].meta.chat_type, "group");
      assert.equal(notifications[0].meta.mentioned, "true");
    } finally {
      await channel.stop();
      await rm(tempDir, { recursive: true, force: true });
    }
  });
});
