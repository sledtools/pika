export type PikachatGroupPolicy = "allowlist" | "open";
export type PikachatDaemonBackend = "native" | "acp";

export type PikachatGroupConfig = {
  name?: string;
  // Future: requireMention, toolPolicy, etc.
};

export type PikachatChannelConfig = {
  relays: string[];
  owner?: string | string[];
  stateDir?: string;
  daemonCmd?: string;
  daemonArgs?: string[];
  daemonVersion?: string;
  daemonBackend: PikachatDaemonBackend;
  daemonAcpExec?: string;
  daemonAcpCwd?: string;
  autoAcceptWelcomes: boolean;
  groupPolicy: PikachatGroupPolicy;
  groupAllowFrom: string[];
  groups: Record<string, PikachatGroupConfig>;
};

function asStringArray(value: unknown): string[] | null {
  if (!Array.isArray(value)) return null;
  const out: string[] = [];
  for (const v of value) {
    if (typeof v !== "string") continue;
    const t = v.trim();
    if (!t) continue;
    out.push(t);
  }
  return out;
}

export function resolvePikachatChannelConfig(raw: unknown): PikachatChannelConfig {
  const obj = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};

  const relays = asStringArray(obj.relays) ?? [];
  const owner = (() => {
    if (typeof obj.owner === "string" && obj.owner.trim()) {
      return obj.owner.trim().toLowerCase();
    }
    const ownerList = asStringArray(obj.owner);
    if (ownerList && ownerList.length > 0) {
      return ownerList.map((entry) => entry.toLowerCase());
    }
    return undefined;
  })();

  const stateDir = typeof obj.stateDir === "string" && obj.stateDir.trim() ? obj.stateDir.trim() : undefined;
  const daemonCmd = (() => {
    const raw = typeof obj.daemonCmd === "string" && obj.daemonCmd.trim()
      ? obj.daemonCmd
      : typeof obj.sidecarCmd === "string" && obj.sidecarCmd.trim()
        ? obj.sidecarCmd
        : undefined;
    return raw?.trim();
  })();
  const daemonArgs = asStringArray(obj.daemonArgs) ?? asStringArray(obj.sidecarArgs) ?? undefined;
  const daemonVersion = (() => {
    const raw = typeof obj.daemonVersion === "string" && obj.daemonVersion.trim()
      ? obj.daemonVersion
      : typeof obj.sidecarVersion === "string" && obj.sidecarVersion.trim()
        ? obj.sidecarVersion
        : undefined;
    return raw?.trim();
  })();
  const daemonBackend: PikachatDaemonBackend =
    obj.daemonBackend === "acp" || obj.daemonBackend === "native" ? obj.daemonBackend : "native";
  const daemonAcpExec =
    typeof obj.daemonAcpExec === "string" && obj.daemonAcpExec.trim()
      ? obj.daemonAcpExec.trim()
      : undefined;
  const daemonAcpCwd =
    typeof obj.daemonAcpCwd === "string" && obj.daemonAcpCwd.trim()
      ? obj.daemonAcpCwd.trim()
      : undefined;

  const autoAcceptWelcomes =
    typeof obj.autoAcceptWelcomes === "boolean" ? obj.autoAcceptWelcomes : true;

  const groupPolicy: PikachatGroupPolicy =
    obj.groupPolicy === "open" || obj.groupPolicy === "allowlist" ? obj.groupPolicy : "allowlist";

  const groupAllowFrom = (asStringArray(obj.groupAllowFrom) ?? []).map((x) => x.toLowerCase());

  const groupsRaw = obj.groups && typeof obj.groups === "object" ? (obj.groups as Record<string, unknown>) : {};
  const groups: Record<string, PikachatGroupConfig> = {};
  for (const [k, v] of Object.entries(groupsRaw)) {
    const gid = String(k).trim().toLowerCase();
    if (!gid) continue;
    const vv = v && typeof v === "object" ? (v as Record<string, unknown>) : {};
    const name = typeof vv.name === "string" && vv.name.trim() ? vv.name.trim() : undefined;
    groups[gid] = name ? { name } : {};
  }

  return {
    relays,
    owner,
    stateDir,
    daemonCmd,
    daemonArgs,
    daemonVersion,
    daemonBackend,
    daemonAcpExec,
    daemonAcpCwd,
    autoAcceptWelcomes,
    groupPolicy,
    groupAllowFrom,
    groups,
  };
}
