import { describe, it } from "node:test";
import assert from "node:assert/strict";
import os from "node:os";
import path from "node:path";
import { SendThrottle, resolveAccountStateDir } from "./sidecar.js";

// We can't import the PikachatSidecar class directly (it spawns a process),
// but we can test the type shapes and serde contracts by constructing the
// JSON payloads that flow between TypeScript and the Rust sidecar.

describe("SidecarInCmd send_media shape", () => {
  it("serializes full send_media command", () => {
    const cmd = {
      cmd: "send_media",
      request_id: "r1",
      nostr_group_id: "aabbccdd",
      file_path: "/tmp/photo.jpg",
      mime_type: "image/jpeg",
      filename: "photo.jpg",
      caption: "Check this out",
      blossom_servers: ["https://blossom.example.com"],
    };
    const json = JSON.stringify(cmd);
    const parsed = JSON.parse(json);
    assert.equal(parsed.cmd, "send_media");
    assert.equal(parsed.request_id, "r1");
    assert.equal(parsed.nostr_group_id, "aabbccdd");
    assert.equal(parsed.file_path, "/tmp/photo.jpg");
    assert.equal(parsed.mime_type, "image/jpeg");
    assert.equal(parsed.filename, "photo.jpg");
    assert.equal(parsed.caption, "Check this out");
    assert.deepStrictEqual(parsed.blossom_servers, ["https://blossom.example.com"]);
  });

  it("serializes minimal send_media command (optional fields omitted)", () => {
    const cmd = {
      cmd: "send_media",
      request_id: "r2",
      nostr_group_id: "aabbccdd",
      file_path: "/tmp/file.bin",
    };
    const json = JSON.stringify(cmd);
    const parsed = JSON.parse(json);
    assert.equal(parsed.cmd, "send_media");
    assert.equal(parsed.mime_type, undefined);
    assert.equal(parsed.filename, undefined);
    assert.equal(parsed.caption, undefined);
    assert.equal(parsed.blossom_servers, undefined);
  });
});

describe("SidecarInCmd reaction/hypernote-action shapes", () => {
  it("serializes react command", () => {
    const cmd = {
      cmd: "react",
      request_id: "r3",
      nostr_group_id: "aabbccdd",
      event_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      emoji: "🧇",
    };
    const parsed = JSON.parse(JSON.stringify(cmd));
    assert.equal(parsed.cmd, "react");
    assert.equal(parsed.event_id, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
    assert.equal(parsed.emoji, "🧇");
  });

  it("serializes submit_hypernote_action command", () => {
    const cmd = {
      cmd: "submit_hypernote_action",
      request_id: "r4",
      nostr_group_id: "aabbccdd",
      event_id: "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
      action: "vote_yes",
      form: { reason: "ship_it" },
    };
    const parsed = JSON.parse(JSON.stringify(cmd));
    assert.equal(parsed.cmd, "submit_hypernote_action");
    assert.equal(parsed.event_id, "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210");
    assert.equal(parsed.action, "vote_yes");
    assert.equal(parsed.form.reason, "ship_it");
  });
});

describe("SidecarOutMsg message_received with media", () => {
  it("parses message_received with media array", () => {
    const msg = {
      type: "message_received",
      nostr_group_id: "aabb",
      from_pubkey: "cc",
      content: "look at this",
      kind: 1,
      created_at: 1234567890,
      event_id: "ee",
      message_id: "dd",
      media: [
        {
          url: "https://blossom.example.com/abc123",
          mime_type: "image/png",
          filename: "screenshot.png",
          original_hash_hex: "deadbeef",
          nonce_hex: "cafebabe",
          scheme_version: "v1",
          width: 800,
          height: 600,
        },
      ],
    };
    const json = JSON.stringify(msg);
    const parsed = JSON.parse(json);
    assert.equal(parsed.type, "message_received");
    assert.equal(parsed.content, "look at this");
    assert.equal(parsed.kind, 1);
    assert.equal(parsed.event_id, "ee");
    assert.ok(Array.isArray(parsed.media));
    assert.equal(parsed.media.length, 1);
    assert.equal(parsed.media[0].url, "https://blossom.example.com/abc123");
    assert.equal(parsed.media[0].mime_type, "image/png");
    assert.equal(parsed.media[0].width, 800);
    assert.equal(parsed.media[0].height, 600);
  });

  it("parses message_received without media (field absent)", () => {
    const msg = {
      type: "message_received",
      nostr_group_id: "aabb",
      from_pubkey: "cc",
      content: "hello",
      kind: 9468,
      created_at: 1234567890,
      event_id: "ee",
      message_id: "dd",
    };
    const json = JSON.stringify(msg);
    const parsed = JSON.parse(json);
    assert.equal(parsed.type, "message_received");
    assert.equal(parsed.kind, 9468);
    assert.equal(parsed.event_id, "ee");
    assert.equal(parsed.media, undefined);
  });

  it("handles media with null dimensions", () => {
    const msg = {
      type: "message_received",
      nostr_group_id: "aabb",
      from_pubkey: "cc",
      content: "",
      kind: 1,
      created_at: 0,
      event_id: "ee",
      message_id: "dd",
      media: [
        {
          url: "https://example.com/file",
          mime_type: "application/pdf",
          filename: "doc.pdf",
          original_hash_hex: "aa",
          nonce_hex: "bb",
          scheme_version: "v1",
          width: null,
          height: null,
        },
      ],
    };
    const parsed = JSON.parse(JSON.stringify(msg));
    assert.equal(parsed.media[0].width, null);
    assert.equal(parsed.media[0].height, null);
    assert.equal(parsed.media[0].mime_type, "application/pdf");
  });
});

describe("media text augmentation", () => {
  // Mirrors the inline logic in channel.ts that appends attachment info to message text

  function augmentMessageText(
    content: string,
    media?: Array<{ filename: string; mime_type: string; width?: number | null; height?: number | null }>,
  ): string {
    if (!media || media.length === 0) return content;
    const mediaLines = media.map((m) => {
      const dims = m.width && m.height ? ` (${m.width}x${m.height})` : "";
      return `[Attachment: ${m.filename} — ${m.mime_type}${dims}]`;
    });
    const suffix = "\n" + mediaLines.join("\n");
    return content ? content + suffix : mediaLines.join("\n");
  }

  it("returns content unchanged when no media", () => {
    assert.equal(augmentMessageText("hello"), "hello");
    assert.equal(augmentMessageText("hello", []), "hello");
  });

  it("appends single attachment to content", () => {
    const result = augmentMessageText("check this out", [
      { filename: "photo.jpg", mime_type: "image/jpeg", width: 1920, height: 1080 },
    ]);
    assert.equal(result, "check this out\n[Attachment: photo.jpg — image/jpeg (1920x1080)]");
  });

  it("appends multiple attachments", () => {
    const result = augmentMessageText("files", [
      { filename: "a.png", mime_type: "image/png", width: 100, height: 200 },
      { filename: "b.pdf", mime_type: "application/pdf" },
    ]);
    const lines = result.split("\n");
    assert.equal(lines[0], "files");
    assert.equal(lines[1], "[Attachment: a.png — image/png (100x200)]");
    assert.equal(lines[2], "[Attachment: b.pdf — application/pdf]");
  });

  it("handles empty content with media (caption-less)", () => {
    const result = augmentMessageText("", [
      { filename: "doc.pdf", mime_type: "application/pdf" },
    ]);
    assert.equal(result, "[Attachment: doc.pdf — application/pdf]");
  });

  it("omits dimensions when null", () => {
    const result = augmentMessageText("hi", [
      { filename: "file.bin", mime_type: "application/octet-stream", width: null, height: null },
    ]);
    assert.equal(result, "hi\n[Attachment: file.bin — application/octet-stream]");
  });
});

describe("SendThrottle", () => {
  // Use a short interval (100ms) to keep tests fast while still verifiable.
  const INTERVAL = 100;

  /** Helper: wait for the internal chain to drain by enqueuing a sentinel. */
  function drain(throttle: SendThrottle): Promise<void> {
    return new Promise((resolve) => {
      throttle.enqueue(async () => { resolve(); });
    });
  }

  it("enqueue returns synchronously", () => {
    const throttle = new SendThrottle(INTERVAL);
    // enqueue returns void, not a Promise — caller is never blocked.
    const ret = throttle.enqueue(() => Promise.resolve());
    assert.equal(ret, undefined);
  });

  it("spaces rapid sends by at least the minimum interval", async () => {
    const throttle = new SendThrottle(INTERVAL);
    const timestamps: number[] = [];

    throttle.enqueue(async () => { timestamps.push(Date.now()); });
    throttle.enqueue(async () => { timestamps.push(Date.now()); });
    throttle.enqueue(async () => { timestamps.push(Date.now()); });

    await drain(throttle);

    assert.equal(timestamps.length, 3);
    // Each subsequent send should be >= INTERVAL after the previous one.
    for (let i = 1; i < timestamps.length; i++) {
      const gap = timestamps[i] - timestamps[i - 1];
      assert.ok(gap >= INTERVAL - 5, `gap between send ${i - 1} and ${i} was ${gap}ms, expected >= ${INTERVAL}ms`);
    }
  });

  it("preserves execution order", async () => {
    const throttle = new SendThrottle(INTERVAL);
    const order: number[] = [];

    for (let i = 0; i < 5; i++) {
      const n = i;
      throttle.enqueue(async () => { order.push(n); });
    }

    await drain(throttle);
    assert.deepStrictEqual(order, [0, 1, 2, 3, 4]);
  });

  it("calls onError and continues after a failure", async () => {
    const errors: string[] = [];
    const throttle = new SendThrottle(INTERVAL, (err) => { errors.push(err.message); });
    const results: string[] = [];

    throttle.enqueue(async () => { results.push("ok"); });
    throttle.enqueue(() => Promise.reject(new Error("boom")));
    throttle.enqueue(async () => { results.push("recovered"); });

    await drain(throttle);

    assert.deepStrictEqual(results, ["ok", "recovered"]);
    assert.deepStrictEqual(errors, ["boom"]);
  });

  it("skips delay when enough time has already passed", async () => {
    const throttle = new SendThrottle(INTERVAL);
    throttle.enqueue(() => Promise.resolve());
    await drain(throttle);

    // Wait longer than the interval
    await new Promise((r) => setTimeout(r, INTERVAL + 50));

    let executedAt = 0;
    const enqueuedAt = Date.now();
    throttle.enqueue(async () => { executedAt = Date.now(); });
    await drain(throttle);
    const delay = executedAt - enqueuedAt;
    assert.ok(delay < INTERVAL, `should not have delayed, took ${delay}ms`);
  });
});

describe("resolveAccountStateDir", () => {
  it("expands ~ in stateDirOverride", () => {
    const result = resolveAccountStateDir({
      accountId: "test-account",
      stateDirOverride: "~/.openclaw/.pikachat-state",
    });
    const expected = path.resolve(path.join(os.homedir(), ".openclaw/.pikachat-state"));
    assert.equal(result, expected);
  });

  it("expands bare ~ in stateDirOverride", () => {
    const result = resolveAccountStateDir({
      accountId: "test-account",
      stateDirOverride: "~",
    });
    assert.equal(result, os.homedir());
  });

  it("does not expand ~ in the middle of path", () => {
    const result = resolveAccountStateDir({
      accountId: "test-account",
      stateDirOverride: "/some/path/~stuff",
    });
    assert.equal(result, path.resolve("/some/path/~stuff"));
  });

  it("resolves absolute path without tilde unchanged", () => {
    const result = resolveAccountStateDir({
      accountId: "test-account",
      stateDirOverride: "/absolute/path/to/state",
    });
    assert.equal(result, "/absolute/path/to/state");
  });
});
