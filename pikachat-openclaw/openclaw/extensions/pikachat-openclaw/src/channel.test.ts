import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  buildReactionSystemEventText,
  resolveReactionEmoji,
  resolveReactionMessageId,
} from "./channel.js";

describe("reaction action helpers", () => {
  it("prefers explicit messageId over tool context", () => {
    const messageId = resolveReactionMessageId(
      { messageId: "abc123" },
      { currentMessageId: "from-context" },
    );
    assert.equal(messageId, "abc123");
  });

  it("falls back to toolContext.currentMessageId (number)", () => {
    const messageId = resolveReactionMessageId({}, { currentMessageId: 42 });
    assert.equal(messageId, "42");
  });

  it("returns empty string when no message id is available", () => {
    const messageId = resolveReactionMessageId({}, {});
    assert.equal(messageId, "");
  });

  it("defaults emoji to heart when empty", () => {
    assert.equal(resolveReactionEmoji(undefined), "❤️");
    assert.equal(resolveReactionEmoji(""), "❤️");
    assert.equal(resolveReactionEmoji("   "), "❤️");
  });

  it("keeps explicit reaction emoji", () => {
    assert.equal(resolveReactionEmoji("🔥"), "🔥");
  });
});

describe("reaction system event text", () => {
  it("formats direct-chat reaction events", () => {
    const text = buildReactionSystemEventText({
      senderName: "alice",
      emoji: "❤️",
      isGroupChat: false,
    });
    assert.equal(text, "Pikachat reaction added: ❤️ by alice.");
  });

  it("formats group reaction events with group name", () => {
    const text = buildReactionSystemEventText({
      senderName: "alice",
      emoji: "✅",
      isGroupChat: true,
      groupName: "Team Chat",
    });
    assert.equal(text, "Pikachat reaction added in Team Chat: ✅ by alice.");
  });
});
