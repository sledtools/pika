import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  evaluateAllowlistDecision,
  normalizeGroupId,
  parseDeterministicPingNonce,
  parseReplyExactly,
  shouldBufferGroupMessage,
  toWelcomeBatchEntry,
} from "./channel-behavior.js";

describe("channel behavior helpers", () => {
  it("normalizes outbound target ids", () => {
    assert.equal(normalizeGroupId("pikachat-openclaw:group:AA11"), "aa11");
    assert.equal(normalizeGroupId("pikachat:bb22"), "bb22");
    assert.equal(normalizeGroupId(" group:CC33 "), "cc33");
  });

  it("parses deterministic ping and reply-exact hooks", () => {
    assert.equal(parseDeterministicPingNonce("ping:abcDEF1234567890"), "abcDEF1234567890");
    assert.equal(parseDeterministicPingNonce("pika-e2e:legacyNonce"), "legacyNonce");
    assert.equal(parseDeterministicPingNonce("hello"), null);

    assert.equal(parseReplyExactly('openclaw: reply exactly "ship this"'), "ship this");
    assert.equal(parseReplyExactly("openclaw: reply exactly"), null);
  });

  it("applies layered allowlist gating for call/message events", () => {
    const base = {
      groupPolicy: "allowlist" as const,
      allowedGroups: { "abc123": {} },
      groupAllowFrom: ["owner-pk"],
      groupUsers: ["owner-pk"],
      nostrGroupId: "abc123",
      senderPubkey: "owner-pk",
    };
    assert.deepEqual(evaluateAllowlistDecision(base), { allowed: true });

    assert.deepEqual(
      evaluateAllowlistDecision({ ...base, nostrGroupId: "other-group" }),
      { allowed: false, reason: "group_not_allowed" },
    );
    assert.deepEqual(
      evaluateAllowlistDecision({ ...base, senderPubkey: "intruder" }),
      { allowed: false, reason: "sender_not_allowed" },
    );
    assert.deepEqual(
      evaluateAllowlistDecision({ ...base, groupAllowFrom: [], groupUsers: ["owner-pk"], senderPubkey: "friend-pk" }),
      { allowed: false, reason: "sender_not_allowed_in_group" },
    );
    assert.deepEqual(
      evaluateAllowlistDecision({
        ...base,
        groupPolicy: "open",
        allowedGroups: {},
        groupAllowFrom: [],
        groupUsers: null,
        senderPubkey: "anyone",
      }),
      { allowed: true },
    );
  });

  it("encodes welcome event batch entries used by sidecar event handling", () => {
    assert.deepEqual(
      toWelcomeBatchEntry({
        from_pubkey: "sender",
        nostr_group_id: "group-a",
        group_name: "Test Group",
        wrapper_event_id: "wrap-1",
      }),
      {
        from: "sender",
        group: "group-a",
        name: "Test Group",
        wrapperId: "wrap-1",
      },
    );
  });

  it("applies mention-gating buffer policy", () => {
    assert.equal(
      shouldBufferGroupMessage({
        requireMention: true,
        wasMentioned: false,
        isHypernoteActionResponse: false,
      }),
      true,
    );
    assert.equal(
      shouldBufferGroupMessage({
        requireMention: true,
        wasMentioned: true,
        isHypernoteActionResponse: false,
      }),
      false,
    );
    assert.equal(
      shouldBufferGroupMessage({
        requireMention: true,
        wasMentioned: false,
        isHypernoteActionResponse: true,
      }),
      false,
    );
  });
});
