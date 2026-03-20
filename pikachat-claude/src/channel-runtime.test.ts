import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
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
  failSetRelays = false;
  shutdownCalls = 0;

  onEvent(handler: PikachatDaemonEventHandler): void {
    this.handler = handler;
  }

  async waitForReady() {
    return this.ready;
  }

  async waitForExit() {
    return await new Promise<void>(() => {});
  }

  async setRelays() {
    if (this.failSetRelays) {
      throw new Error("set_relays failed");
    }
  }

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
    return { event_id: `media-${this.mediaBatches.length}` };
  }

  async sendMediaBatch(chatId: string, filePaths: string[]) {
    this.mediaBatches.push({ chatId, files: [...filePaths] });
    return { event_id: `media-batch-${this.mediaBatches.length}` };
  }

  async sendTyping() {}

  async getMessages() {
    return { messages: [] };
  }

  async shutdown() {
    this.shutdownCalls += 1;
  }

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

  it("preserves concurrent pending pairings", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "pikachat-claude-"));
    const accessFile = path.join(tempDir, "access.json");
    const daemon = new FakeDaemon();
    daemon.memberCountByGroup.set("dm1", 2);
    daemon.memberCountByGroup.set("dm2", 2);
    const channel = createInMemoryChannelForTests({
      daemon,
      config: {
        channelHome: tempDir,
        accessFile,
        inboxDir: path.join(tempDir, "inbox"),
      },
    });
    try {
      await channel.start();
      await Promise.all([
        daemon.emit({
          type: "message_received",
          nostr_group_id: "dm1",
          from_pubkey: "sender1",
          content: "hello one",
          kind: 9,
          created_at: 1,
          event_id: "ev1",
          message_id: "msg1",
        }),
        daemon.emit({
          type: "message_received",
          nostr_group_id: "dm2",
          from_pubkey: "sender2",
          content: "hello two",
          kind: 9,
          created_at: 2,
          event_id: "ev2",
          message_id: "msg2",
        }),
      ]);

      const persisted = JSON.parse(await readFile(accessFile, "utf8")) as {
        pendingPairings: Record<string, { senderId: string }>;
      };
      const senderIds = Object.values(persisted.pendingPairings)
        .map((pending) => pending.senderId)
        .sort();

      assert.deepEqual(senderIds, ["sender1", "sender2"]);
      assert.equal(daemon.sentMessages.length, 2);
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
        media: [{
          filename: "pic.jpg",
          mime_type: "image/jpeg",
          url: "https://example.com/pic.jpg",
          original_hash_hex: "abc123def456",
          nonce_hex: "fed654cba321",
          scheme_version: "1",
          local_path: "/tmp/pic.jpg",
        }],
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

  it("cleans up the daemon when startup fails", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "pikachat-claude-"));
    const daemon = new FakeDaemon();
    daemon.failSetRelays = true;
    const channel = createInMemoryChannelForTests({
      daemon,
      config: {
        channelHome: tempDir,
        accessFile: path.join(tempDir, "access.json"),
        inboxDir: path.join(tempDir, "inbox"),
      },
    });
    try {
      await assert.rejects(() => channel.start(), /set_relays failed/);
      assert.equal(daemon.shutdownCalls, 1);
    } finally {
      await channel.stop();
      await rm(tempDir, { recursive: true, force: true });
    }
  });

  it("returns media event ids for outbound attachments", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "pikachat-claude-"));
    const inboxDir = path.join(tempDir, "inbox");
    await mkdir(inboxDir, { recursive: true });
    const testFile = path.join(tempDir, "test.txt");
    await writeFile(testFile, "ok");

    const daemon = new FakeDaemon();
    const channel = createInMemoryChannelForTests({
      daemon,
      config: {
        channelHome: tempDir,
        accessFile: path.join(tempDir, "access.json"),
        inboxDir,
      },
    });
    try {
      await channel.start();
      const result = await channel.reply({ chatId: "dm1", files: [testFile] });
      assert.deepEqual(result.eventIds, ["media-1"]);
    } finally {
      await channel.stop();
      await rm(tempDir, { recursive: true, force: true });
    }
  });
});
