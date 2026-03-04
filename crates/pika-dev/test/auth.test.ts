import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { finalizeEvent, generateSecretKey, getPublicKey, nip19 } from "nostr-tools";

import { AuthState } from "../src/auth.js";

describe("auth", () => {
  it("issues token for valid allowlisted NIP-98 event", () => {
    const secret = generateSecretKey();
    const pubkeyHex = getPublicKey(secret);
    const npub = nip19.npubEncode(pubkeyHex);

    const auth = new AuthState([npub]);
    const challenge = auth.createChallenge();

    const event = finalizeEvent(
      {
        kind: 27235,
        created_at: Math.floor(Date.now() / 1000),
        tags: [["u", "https://dev.pikachat.org/session/1"]],
        content: challenge,
      },
      secret,
    );

    const verified = auth.verifyEvent(JSON.stringify(event));
    assert.equal(verified.npub, npub);
    assert.ok(verified.token.length > 0);
    assert.equal(auth.validateToken(verified.token), npub);
  });

  it("rejects non-allowlisted pubkeys when auth is enabled", () => {
    const allowedSecret = generateSecretKey();
    const allowedNpub = nip19.npubEncode(getPublicKey(allowedSecret));

    const deniedSecret = generateSecretKey();
    const auth = new AuthState([allowedNpub]);
    const challenge = auth.createChallenge();

    const event = finalizeEvent(
      {
        kind: 27235,
        created_at: Math.floor(Date.now() / 1000),
        tags: [],
        content: challenge,
      },
      deniedSecret,
    );

    assert.throws(() => auth.verifyEvent(JSON.stringify(event)), /allowlist/);
  });
});
