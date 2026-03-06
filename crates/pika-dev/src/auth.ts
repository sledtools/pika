import crypto from "node:crypto";

import { nip19, verifyEvent, type Event as NostrEvent } from "nostr-tools";

const CHALLENGE_TTL_MS = 2 * 60 * 1000;
const TOKEN_TTL_MS = 24 * 60 * 60 * 1000;
const MAX_CLOCK_SKEW_SECONDS = 5 * 60;

interface ChallengeRecord {
  createdAtMs: number;
}

interface TokenRecord {
  npub: string;
  createdAtMs: number;
}

export class AuthState {
  private readonly challenges = new Map<string, ChallengeRecord>();
  private readonly tokens = new Map<string, TokenRecord>();
  private readonly allowedHexPubkeys: Set<string>;

  constructor(allowedNpubs: string[]) {
    this.allowedHexPubkeys = new Set<string>();
    for (const npub of allowedNpubs) {
      const decoded = decodeNpub(npub);
      if (decoded) {
        this.allowedHexPubkeys.add(decoded);
      }
    }
  }

  isEnabled(): boolean {
    return this.allowedHexPubkeys.size > 0;
  }

  createChallenge(): string {
    this.prune();
    const nonce = crypto.randomBytes(16).toString("hex");
    this.challenges.set(nonce, { createdAtMs: Date.now() });
    return nonce;
  }

  verifyEvent(eventJson: string): { token: string; npub: string } {
    this.prune();
    const event = JSON.parse(eventJson) as NostrEvent;

    if (!verifyEvent(event)) {
      throw new Error("invalid event signature");
    }

    if (event.kind !== 27235) {
      throw new Error(`expected NIP-98 kind 27235, got ${event.kind}`);
    }

    if (typeof event.content !== "string" || event.content.length === 0) {
      throw new Error("event.content must include challenge nonce");
    }

    const challenge = this.challenges.get(event.content);
    if (!challenge) {
      throw new Error("challenge nonce not found or expired");
    }
    this.challenges.delete(event.content);

    if (!this.isFresh(event.created_at)) {
      throw new Error("event created_at is stale");
    }

    if (this.isEnabled() && !this.allowedHexPubkeys.has(event.pubkey)) {
      throw new Error("pubkey not in allowlist");
    }

    const npub = toNpub(event.pubkey);
    const token = crypto.randomBytes(32).toString("hex");
    this.tokens.set(token, { npub, createdAtMs: Date.now() });

    return { token, npub };
  }

  validateToken(token: string): string | null {
    this.prune();
    const record = this.tokens.get(token);
    if (!record) {
      return null;
    }
    if (Date.now() - record.createdAtMs > TOKEN_TTL_MS) {
      this.tokens.delete(token);
      return null;
    }
    return record.npub;
  }

  private isFresh(createdAt: number): boolean {
    const nowSeconds = Math.floor(Date.now() / 1000);
    return Math.abs(nowSeconds - createdAt) <= MAX_CLOCK_SKEW_SECONDS;
  }

  private prune(): void {
    const now = Date.now();
    for (const [nonce, record] of this.challenges.entries()) {
      if (now - record.createdAtMs > CHALLENGE_TTL_MS) {
        this.challenges.delete(nonce);
      }
    }
    for (const [token, record] of this.tokens.entries()) {
      if (now - record.createdAtMs > TOKEN_TTL_MS) {
        this.tokens.delete(token);
      }
    }
  }
}

function decodeNpub(npub: string): string | null {
  try {
    const decoded = nip19.decode(npub);
    if (decoded.type !== "npub") {
      return null;
    }
    return decoded.data;
  } catch {
    return null;
  }
}

function toNpub(hexPubkey: string): string {
  try {
    return nip19.npubEncode(hexPubkey);
  } catch {
    return hexPubkey;
  }
}
