import fs from "node:fs";
import path from "node:path";
import process from "node:process";

import { parse as parseToml } from "@iarna/toml";
import { z } from "zod";

const configSchema = z.object({
  github_repo: z.string().min(3).default("sledtools/pika"),
  github_label: z.string().min(1).default("pika-dev"),
  github_token_env: z.string().min(1).default("GITHUB_TOKEN"),
  poll_interval_secs: z.number().int().positive().default(60),

  provider: z.string().min(1).default("anthropic"),
  model: z.string().min(1).default("claude-sonnet-4-5-20250929"),
  thinking_level: z.string().min(1).default("high"),
  max_concurrent_sessions: z.number().int().min(1).max(20).default(5),
  session_timeout_mins: z.number().int().min(1).max(24 * 60).default(30),
  workspace_base: z.string().min(1).default(".tmp/pika-dev/workspaces"),
  session_base: z.string().min(1).default(".tmp/pika-dev/sessions"),
  agent_backend: z.enum(["pi", "fake"]).default("pi"),
  base_branch: z.string().min(1).default("origin/main"),

  bind_address: z.string().min(1).default("127.0.0.1"),
  bind_port: z.number().int().min(1).max(65535).default(8789),

  allowed_npubs: z.array(z.string().min(1)).default([]),
});

export type PikaDevConfig = z.infer<typeof configSchema> & {
  configPath: string;
  dbPath: string;
  repoOwner: string;
  repoName: string;
};

export interface CliOptions {
  configPath: string;
  dbPath: string;
  maxSessions?: number;
  backend?: "pi" | "fake";
  once: boolean;
}

export function parseCliArgs(argv: string[]): CliOptions {
  const defaults: CliOptions = {
    configPath: path.resolve("crates/pika-dev/pika-dev.toml"),
    dbPath: path.resolve(".tmp/pika-dev.db"),
    once: false,
  };

  const args = [...argv];
  while (args.length > 0) {
    const arg = args.shift();
    if (!arg) {
      break;
    }

    if (arg === "--config") {
      defaults.configPath = path.resolve(expectArgValue(args, "--config"));
      continue;
    }
    if (arg === "--db") {
      defaults.dbPath = path.resolve(expectArgValue(args, "--db"));
      continue;
    }
    if (arg === "--max-sessions") {
      defaults.maxSessions = parseInt(expectArgValue(args, "--max-sessions"), 10);
      continue;
    }
    if (arg === "--agent-backend") {
      const value = expectArgValue(args, "--agent-backend");
      if (value !== "pi" && value !== "fake") {
        throw new Error(`invalid --agent-backend value: ${value}`);
      }
      defaults.backend = value;
      continue;
    }
    if (arg === "--once") {
      defaults.once = true;
      continue;
    }

    throw new Error(`unknown argument: ${arg}`);
  }

  return defaults;
}

export function loadConfig(options: CliOptions): PikaDevConfig {
  if (!fs.existsSync(options.configPath)) {
    throw new Error(`config not found: ${options.configPath}`);
  }

  const raw = fs.readFileSync(options.configPath, "utf8");
  const parsed = parseToml(raw);
  const validated = configSchema.parse(parsed);

  if (options.maxSessions !== undefined) {
    validated.max_concurrent_sessions = options.maxSessions;
  }
  if (options.backend) {
    validated.agent_backend = options.backend;
  }

  const [repoOwner, repoName] = validated.github_repo.split("/");
  if (!repoOwner || !repoName) {
    throw new Error(`github_repo must be owner/repo, got: ${validated.github_repo}`);
  }

  return {
    ...validated,
    configPath: options.configPath,
    dbPath: options.dbPath,
    repoOwner,
    repoName,
    workspace_base: resolveMaybeRelative(validated.workspace_base),
    session_base: resolveMaybeRelative(validated.session_base),
  };
}

function resolveMaybeRelative(input: string): string {
  return path.isAbsolute(input) ? input : path.resolve(process.cwd(), input);
}

function expectArgValue(args: string[], flag: string): string {
  const value = args.shift();
  if (!value) {
    throw new Error(`missing value for ${flag}`);
  }
  return value;
}
