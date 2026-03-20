import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { defaultPikachatRelays, resolvePikachatClaudeConfig } from "./config.js";

describe("resolvePikachatClaudeConfig", () => {
  it("falls back to the default pikachat relay profile", () => {
    const config = resolvePikachatClaudeConfig({});
    assert.deepEqual(config.relays, defaultPikachatRelays());
  });

  it("prefers explicit relay configuration", () => {
    const config = resolvePikachatClaudeConfig({
      PIKACHAT_RELAYS: '["wss://example.com","wss://example.org"]',
    });
    assert.deepEqual(config.relays, ["wss://example.com", "wss://example.org"]);
  });
});
