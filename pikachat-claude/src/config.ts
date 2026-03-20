import os from "node:os";
import path from "node:path";

const DEFAULT_MESSAGE_RELAYS = [
  "wss://relay.primal.net",
  "wss://nos.lol",
  "wss://relay.damus.io",
  "wss://us-east.nostr.pikachat.org",
  "wss://eu.nostr.pikachat.org",
];

export type PikachatClaudeConfig = {
  relays: string[];
  stateDir?: string;
  daemonCmd?: string;
  daemonArgs?: string[];
  daemonVersion?: string;
  daemonBackend: "native" | "acp";
  daemonAcpExec?: string;
  daemonAcpCwd?: string;
  autoAcceptWelcomes: boolean;
  channelSource: string;
  channelHome: string;
  accessFile: string;
  inboxDir: string;
};

function parseBoolean(value: string | undefined, fallback: boolean): boolean {
  if (value === undefined) return fallback;
  const normalized = value.trim().toLowerCase();
  if (!normalized) return fallback;
  return !["0", "false", "no", "off"].includes(normalized);
}

function parseStringArray(value: string | undefined): string[] {
  if (!value) return [];
  const trimmed = value.trim();
  if (!trimmed) return [];
  try {
    const parsed = JSON.parse(trimmed);
    if (Array.isArray(parsed)) {
      return parsed.map((entry) => String(entry).trim()).filter(Boolean);
    }
  } catch {
    // fall through
  }
  return trimmed
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function expandTilde(input: string): string {
  if (input === "~" || input.startsWith("~/")) {
    return path.join(os.homedir(), input.slice(1));
  }
  return input;
}

export function defaultClaudeChannelHome(): string {
  return path.join(os.homedir(), ".claude", "channels", "pikachat");
}

export function defaultPikachatRelays(): string[] {
  return [...DEFAULT_MESSAGE_RELAYS];
}

export function resolvePikachatClaudeConfig(env: NodeJS.ProcessEnv = process.env): PikachatClaudeConfig {
  const channelHome = path.resolve(
    expandTilde(env.PIKACHAT_CLAUDE_HOME?.trim() || defaultClaudeChannelHome()),
  );
  const configuredRelays = parseStringArray(env.PIKACHAT_RELAYS);
  return {
    relays: configuredRelays.length > 0 ? configuredRelays : defaultPikachatRelays(),
    stateDir: env.PIKACHAT_STATE_DIR?.trim() || undefined,
    daemonCmd: env.PIKACHAT_DAEMON_CMD?.trim() || env.PIKACHAT_SIDECAR_CMD?.trim() || undefined,
    daemonArgs: (() => {
      const values = parseStringArray(env.PIKACHAT_DAEMON_ARGS?.trim() || env.PIKACHAT_SIDECAR_ARGS?.trim());
      return values.length > 0 ? values : undefined;
    })(),
    daemonVersion: env.PIKACHAT_DAEMON_VERSION?.trim() || env.PIKACHAT_SIDECAR_VERSION?.trim() || undefined,
    daemonBackend: env.PIKACHAT_DAEMON_BACKEND === "acp" ? "acp" : "native",
    daemonAcpExec: env.PIKACHAT_DAEMON_ACP_EXEC?.trim() || undefined,
    daemonAcpCwd: env.PIKACHAT_DAEMON_ACP_CWD?.trim() || undefined,
    autoAcceptWelcomes: parseBoolean(env.PIKACHAT_AUTO_ACCEPT_WELCOMES, true),
    channelSource: env.PIKACHAT_CHANNEL_SOURCE?.trim() || "pikachat",
    channelHome,
    accessFile: path.join(channelHome, "access.json"),
    inboxDir: path.join(channelHome, "inbox"),
  };
}
