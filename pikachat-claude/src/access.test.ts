import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  allowSender,
  approvePairing,
  defaultAccessState,
  denyPairing,
  enableGroup,
  ensurePendingPairing,
  evaluateDmAccess,
  evaluateGroupAccess,
  pruneExpiredPairings,
  removeSender,
  setDmPolicy,
} from "./access.js";

describe("access state helpers", () => {
  it("defaults to pairing policy", () => {
    const state = defaultAccessState();
    assert.equal(state.dmPolicy, "pairing");
    assert.deepEqual(state.allowFrom, []);
  });

  it("creates and approves pairings", () => {
    const created = ensurePendingPairing(defaultAccessState(), "AABB", "ccdd", 1000);
    assert.match(created.pairing.code, /^[0-9a-f]{6}$/);
    const approved = approvePairing(created.state, created.pairing.code);
    assert.equal(approved.pairing?.senderId, "aabb");
    assert.deepEqual(approved.state.allowFrom, ["aabb"]);
    assert.equal(Object.keys(approved.state.pendingPairings).length, 0);
  });

  it("reuses existing live pairings", () => {
    const first = ensurePendingPairing(defaultAccessState(), "AABB", "ccdd", 1000);
    const second = ensurePendingPairing(first.state, "aabb", "ccdd", 1500);
    assert.equal(first.pairing.code, second.pairing.code);
  });

  it("expires stale pairings", () => {
    const created = ensurePendingPairing(defaultAccessState(), "AABB", "ccdd", 1000);
    const pruned = pruneExpiredPairings(created.state, 1000 + 25 * 60 * 60 * 1000);
    assert.equal(Object.keys(pruned.pendingPairings).length, 0);
  });

  it("supports dm allowlist transitions", () => {
    const allowed = allowSender(defaultAccessState(), "AABB");
    assert.equal(evaluateDmAccess(allowed, "aabb"), "allowed");
    const removed = removeSender(allowed, "AABB");
    assert.equal(evaluateDmAccess(removed, "aabb"), "pairing");
    assert.equal(evaluateDmAccess(setDmPolicy(removed, "allowlist"), "aabb"), "blocked");
  });

  it("supports deny pairing", () => {
    const created = ensurePendingPairing(defaultAccessState(), "AABB", "ccdd", 1000);
    const denied = denyPairing(created.state, created.pairing.code);
    assert.equal(denied.pairing?.senderId, "aabb");
    assert.equal(Object.keys(denied.state.pendingPairings).length, 0);
  });

  it("applies per-group allowlist and mention defaults", () => {
    const state = enableGroup(defaultAccessState(), "GG", { allowFrom: ["aa"], requireMention: true });
    assert.deepEqual(evaluateGroupAccess(state, "gg", "aa"), {
      enabled: true,
      requireMention: true,
      senderAllowed: true,
    });
    assert.deepEqual(evaluateGroupAccess(state, "gg", "bb"), {
      enabled: true,
      requireMention: true,
      senderAllowed: false,
    });
    assert.deepEqual(evaluateGroupAccess(state, "missing", "aa"), {
      enabled: false,
      requireMention: true,
      senderAllowed: false,
    });
  });
});
