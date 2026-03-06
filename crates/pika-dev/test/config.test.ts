import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { describe, it } from "node:test";

import { loadConfig, parseCliArgs } from "../src/config.js";
import { createTempDir } from "./helpers.js";

describe("config", () => {
  it("parses CLI options", () => {
    const parsed = parseCliArgs([
      "--config",
      "/tmp/pika-dev.toml",
      "--db",
      "/tmp/pika.db",
      "--max-sessions",
      "2",
      "--agent-backend",
      "fake",
      "--once",
    ]);

    assert.equal(parsed.configPath, path.resolve("/tmp/pika-dev.toml"));
    assert.equal(parsed.dbPath, path.resolve("/tmp/pika.db"));
    assert.equal(parsed.maxSessions, 2);
    assert.equal(parsed.backend, "fake");
    assert.equal(parsed.once, true);
  });

  it("loads config and applies overrides", () => {
    const dir = createTempDir("pika-dev-config-test");
    const configPath = path.join(dir, "pika-dev.toml");
    fs.writeFileSync(
      configPath,
      [
        'github_repo = "sledtools/pika"',
        'github_label = "pika-dev"',
        'github_token_env = "GITHUB_TOKEN"',
        "poll_interval_secs = 10",
        'provider = "anthropic"',
        'model = "claude-sonnet-4-5-20250929"',
        'thinking_level = "high"',
        "max_concurrent_sessions = 4",
        "session_timeout_mins = 15",
        'workspace_base = "work"',
        'session_base = "sessions"',
        'agent_backend = "pi"',
        'base_branch = "origin/main"',
        'bind_address = "127.0.0.1"',
        "bind_port = 8789",
        "allowed_npubs = []",
      ].join("\n"),
    );

    const cfg = loadConfig({
      configPath,
      dbPath: path.join(dir, "db.sqlite"),
      maxSessions: 2,
      backend: "fake",
      once: false,
    });

    assert.equal(cfg.max_concurrent_sessions, 2);
    assert.equal(cfg.agent_backend, "fake");
    assert.equal(cfg.repoOwner, "sledtools");
    assert.equal(cfg.repoName, "pika");
    assert.ok(path.isAbsolute(cfg.workspace_base));
    assert.ok(path.isAbsolute(cfg.session_base));
  });
});
