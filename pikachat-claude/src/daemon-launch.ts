import os from "node:os";
import path from "node:path";

import type { PikachatClaudeConfig } from "./config.js";
import { resolvePikachatDaemonCommand } from "./daemon-install.js";
import type { PikachatLogger } from "./daemon-client.js";

export type PikachatDaemonLaunchSpec = {
  cmd: string;
  args: string[];
  stateDir: string;
  backend: "native" | "acp";
  autoAcceptWelcomes: boolean;
};

function expandTilde(input: string): string {
  if (input === "~" || input.startsWith("~/")) {
    return path.join(os.homedir(), input.slice(1));
  }
  return input;
}

function defaultStateDir(): string {
  return path.join(os.homedir(), ".local", "state", "pikachat");
}

function defaultAcpCwd(stateDir: string, configured?: string): string {
  if (configured && configured.trim()) {
    return path.resolve(expandTilde(configured.trim()));
  }
  return path.join(stateDir, "acp");
}

function hasDaemonFlag(args: string[], flag: string): boolean {
  return args.includes(flag);
}

export async function buildPikachatDaemonLaunchSpec(params: {
  config: PikachatClaudeConfig;
  env?: NodeJS.ProcessEnv;
  log?: PikachatLogger;
}): Promise<PikachatDaemonLaunchSpec> {
  const stateDir = path.resolve(expandTilde(params.config.stateDir?.trim() || defaultStateDir()));
  const relays = params.config.relays.map((entry) => entry.trim()).filter(Boolean);
  const cmd = await resolvePikachatDaemonCommand({
    requestedCmd: params.config.daemonCmd?.trim() || "pikachat",
    pinnedVersion: params.config.daemonVersion,
    log: params.log,
  });

  const relayArgs = (relays.length > 0 ? relays : ["ws://127.0.0.1:18080"]).flatMap((relay) => [
    "--relay",
    relay,
  ]);
  let args = params.config.daemonArgs ?? ["daemon", ...relayArgs, "--state-dir", stateDir];
  const looksLikeDaemonCommand = args[0] === "daemon";

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
    autoAcceptWelcomes: hasDaemonFlag(args, "--auto-accept-welcomes"),
  };
}
