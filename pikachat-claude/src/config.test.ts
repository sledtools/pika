import assert from "node:assert/strict";
import os from "node:os";
import path from "node:path";
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

  it("resolves channel home and derived paths", () => {
    const config = resolvePikachatClaudeConfig({
      PIKACHAT_CLAUDE_HOME: "~/tmp/pikachat-claude-home",
    });
    const expectedHome = path.join(os.homedir(), "tmp", "pikachat-claude-home");
    assert.equal(config.channelHome, expectedHome);
    assert.equal(config.accessFile, path.join(expectedHome, "access.json"));
    assert.equal(config.inboxDir, path.join(expectedHome, "inbox"));
  });

  it("parses autoAcceptWelcomes booleans", () => {
    assert.equal(resolvePikachatClaudeConfig({ PIKACHAT_AUTO_ACCEPT_WELCOMES: "true" }).autoAcceptWelcomes, true);
    assert.equal(resolvePikachatClaudeConfig({ PIKACHAT_AUTO_ACCEPT_WELCOMES: "false" }).autoAcceptWelcomes, false);
  });

  it("parses daemon args arrays", () => {
    const config = resolvePikachatClaudeConfig({
      PIKACHAT_DAEMON_ARGS: '["daemon","--verbose"]',
    });
    assert.deepEqual(config.daemonArgs, ["daemon", "--verbose"]);
  });
});
