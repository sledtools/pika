import os from "node:os";
import path from "node:path";
import { resolvePikachatDaemonCommand } from "./sidecar-install.js";
import type { PikachatChannelConfig, PikachatDaemonBackend } from "./config.js";

type PikachatLog = {
  debug?: (msg: string) => void;
  info?: (msg: string) => void;
  warn?: (msg: string) => void;
  error?: (msg: string) => void;
};

export type PikachatDaemonLaunchSpec = {
  cmd: string;
  args: string[];
  stateDir: string;
  backend: PikachatDaemonBackend;
  autoAcceptWelcomes: boolean;
};

type ResolveDaemonCommand = (params: {
  requestedCmd: string;
  log?: PikachatLog;
  pinnedVersion?: string;
}) => Promise<string>;

function sanitizePathSegment(value: string): string {
  const cleaned = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "_")
    .replace(/^_+|_+$/g, "");
  return cleaned || "default";
}

function expandTilde(p: string): string {
  if (p === "~" || p.startsWith("~/")) {
    return path.join(os.homedir(), p.slice(1));
  }
  return p;
}

export function resolveAccountStateDir(params: {
  accountId: string;
  stateDirOverride?: string | undefined;
  env?: NodeJS.ProcessEnv;
}): string {
  if (params.stateDirOverride && params.stateDirOverride.trim()) {
    return path.resolve(expandTilde(params.stateDirOverride.trim()));
  }
  const env = params.env ?? process.env;
  const xdgStateHome = env.XDG_STATE_HOME?.trim();
  const baseStateDir =
    xdgStateHome && xdgStateHome.length > 0
      ? path.resolve(expandTilde(xdgStateHome))
      : path.join(os.homedir(), ".local", "state");
  return path.join(baseStateDir, "pikachat", "accounts", sanitizePathSegment(params.accountId));
}

function resolveConfiguredDaemonCmd(value: string | undefined, env: NodeJS.ProcessEnv = process.env): string {
  const overridden = env.PIKACHAT_DAEMON_CMD?.trim() || env.PIKACHAT_SIDECAR_CMD?.trim();
  if (overridden) return overridden;
  const trimmed = value?.trim();
  return trimmed && trimmed.length > 0 ? trimmed : "pikachat";
}

function resolveConfiguredDaemonArgs(
  value: string[] | undefined,
  env: NodeJS.ProcessEnv = process.env,
): string[] | undefined {
  const overridden = env.PIKACHAT_DAEMON_ARGS?.trim() || env.PIKACHAT_SIDECAR_ARGS?.trim();
  if (overridden) {
    try {
      const parsed = JSON.parse(overridden);
      if (Array.isArray(parsed) && parsed.every((entry) => typeof entry === "string")) {
        return parsed;
      }
    } catch {
      // ignore malformed env override and fall back to config/defaults
    }
  }
  if (!Array.isArray(value)) return undefined;
  const args = value.map((entry) => String(entry).trim()).filter(Boolean);
  return args.length > 0 ? args : undefined;
}

function hasDaemonFlag(args: string[], flag: string): boolean {
  return args.includes(flag);
}

function defaultAcpCwd(stateDir: string, configured?: string): string {
  if (configured && configured.trim()) {
    return path.resolve(expandTilde(configured.trim()));
  }
  return path.join(stateDir, "acp");
}

export async function buildPikachatDaemonLaunchSpec(
  params: {
    accountId: string;
    config: PikachatChannelConfig;
    log?: PikachatLog;
    env?: NodeJS.ProcessEnv;
  },
  deps: {
    resolveCommand?: ResolveDaemonCommand;
  } = {},
): Promise<PikachatDaemonLaunchSpec> {
  const resolveCommand = deps.resolveCommand ?? resolvePikachatDaemonCommand;
  const relays = params.config.relays.map((r) => String(r).trim()).filter(Boolean);
  const stateDir = resolveAccountStateDir({
    accountId: params.accountId,
    stateDirOverride: params.config.stateDir,
    env: params.env,
  });
  const cmd = await resolveCommand({
    requestedCmd: resolveConfiguredDaemonCmd(params.config.daemonCmd, params.env),
    log: params.log,
    pinnedVersion: params.config.daemonVersion,
  });

  const relayArgs = (relays.length > 0 ? relays : ["ws://127.0.0.1:18080"]).flatMap((r) => ["--relay", r]);
  let args = resolveConfiguredDaemonArgs(params.config.daemonArgs, params.env) ?? ["daemon", ...relayArgs, "--state-dir", stateDir];
  const looksLikeDaemonCommand = args.length > 0 && args[0] === "daemon";

  if (looksLikeDaemonCommand && params.config.autoAcceptWelcomes && !hasDaemonFlag(args, "--auto-accept-welcomes")) {
    args = [...args, "--auto-accept-welcomes"];
  }

  if (looksLikeDaemonCommand && params.config.daemonBackend === "acp") {
    if (!hasDaemonFlag(args, "--acp-exec")) {
      args = [...args, "--acp-exec", params.config.daemonAcpExec?.trim() || "npx -y pi-acp"];
    }
    if (!hasDaemonFlag(args, "--acp-cwd")) {
      args = [...args, "--acp-cwd", defaultAcpCwd(stateDir, params.config.daemonAcpCwd)];
    }
  }

  return {
    cmd,
    args,
    stateDir,
    backend: params.config.daemonBackend,
    autoAcceptWelcomes: looksLikeDaemonCommand && hasDaemonFlag(args, "--auto-accept-welcomes"),
  };
}
