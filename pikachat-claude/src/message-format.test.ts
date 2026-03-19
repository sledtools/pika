import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { augmentMessageText, detectMention, sanitizeMeta } from "./message-format.js";

describe("message formatting helpers", () => {
  it("appends attachment lines", () => {
    const result = augmentMessageText("check this out", [
      {
        filename: "photo.jpg",
        mime_type: "image/jpeg",
        width: 1920,
        height: 1080,
        local_path: "/tmp/photo.jpg",
      },
    ]);
    assert.equal(
      result,
      "check this out\n[Attachment: photo.jpg — image/jpeg (1920x1080) file:///tmp/photo.jpg]",
    );
  });

  it("sanitizes invalid meta keys", () => {
    assert.deepEqual(
      sanitizeMeta({
        chat_id: "abc",
        "bad-key": "nope",
        empty: " ",
      }),
      { chat_id: "abc" },
    );
  });

  it("detects mentions via npub, pubkey, and regex patterns", () => {
    assert.equal(
      detectMention({
        text: "hey nostr:npub1test",
        botPubkey: "abcdef",
        botNpub: "npub1test",
        mentionPatterns: [],
      }),
      true,
    );
    assert.equal(
      detectMention({
        text: "hello assistant",
        botPubkey: "abcdef",
        botNpub: "npub1test",
        mentionPatterns: ["\\bassistant\\b"],
      }),
      true,
    );
    assert.equal(
      detectMention({
        text: "plain message",
        botPubkey: "abcdef",
        botNpub: "npub1test",
        mentionPatterns: [],
      }),
      false,
    );
  });
});
