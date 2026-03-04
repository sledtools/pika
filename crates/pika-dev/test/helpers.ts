import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import type { PikaDevConfig } from "../src/config.js";

export function createTempDir(prefix: string): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), `${prefix}-`));
}

export function createTestConfig(overrides: Partial<PikaDevConfig> = {}): PikaDevConfig {
  const root = createTempDir("pika-dev-config");
  return {
    github_repo: "sledtools/pika",
    github_label: "pika-dev",
    github_token_env: "GITHUB_TOKEN",
    poll_interval_secs: 60,
    provider: "anthropic",
    model: "claude-sonnet-4-5-20250929",
    thinking_level: "high",
    max_concurrent_sessions: 5,
    session_timeout_mins: 30,
    workspace_base: path.join(root, "workspaces"),
    session_base: path.join(root, "sessions"),
    agent_backend: "fake",
    base_branch: "origin/main",
    bind_address: "127.0.0.1",
    bind_port: 8789,
    allowed_npubs: [],
    configPath: path.join(root, "pika-dev.toml"),
    dbPath: path.join(root, "pika-dev.db"),
    repoOwner: "sledtools",
    repoName: "pika",
    ...overrides,
  };
}
