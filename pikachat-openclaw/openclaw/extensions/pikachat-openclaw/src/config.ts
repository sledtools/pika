export type PikachatGroupPolicy = "allowlist" | "open";

export type PikachatGroupConfig = {
  name?: string;
  // Future: requireMention, toolPolicy, etc.
};

export type PikachatChannelConfig = {
  relays: string[];
  owner?: string | string[];
  stateDir?: string;
  sidecarCmd?: string;
  sidecarArgs?: string[];
  sidecarVersion?: string;
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
  const sidecarCmd =
    typeof obj.sidecarCmd === "string" && obj.sidecarCmd.trim() ? obj.sidecarCmd.trim() : undefined;
  const sidecarArgs = asStringArray(obj.sidecarArgs) ?? undefined;
  const sidecarVersion =
    typeof obj.sidecarVersion === "string" && obj.sidecarVersion.trim() ? obj.sidecarVersion.trim() : undefined;

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
    sidecarCmd,
    sidecarArgs,
    sidecarVersion,
    autoAcceptWelcomes,
    groupPolicy,
    groupAllowFrom,
    groups,
  };
}
