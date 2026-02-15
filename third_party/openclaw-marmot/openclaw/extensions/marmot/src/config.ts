export type MarmotGroupPolicy = "allowlist" | "open";

export type MarmotGroupConfig = {
  name?: string;
  // Future: requireMention, toolPolicy, etc.
};

export type MarmotChannelConfig = {
  relays: string[];
  stateDir?: string;
  sidecarCmd?: string;
  sidecarArgs?: string[];
  autoAcceptWelcomes: boolean;
  groupPolicy: MarmotGroupPolicy;
  groupAllowFrom: string[];
  groups: Record<string, MarmotGroupConfig>;
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

export function resolveMarmotChannelConfig(raw: unknown): MarmotChannelConfig {
  const obj = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};

  const relays = asStringArray(obj.relays) ?? [];

  const stateDir = typeof obj.stateDir === "string" && obj.stateDir.trim() ? obj.stateDir.trim() : undefined;
  const sidecarCmd =
    typeof obj.sidecarCmd === "string" && obj.sidecarCmd.trim() ? obj.sidecarCmd.trim() : undefined;
  const sidecarArgs = asStringArray(obj.sidecarArgs) ?? undefined;

  const autoAcceptWelcomes =
    typeof obj.autoAcceptWelcomes === "boolean" ? obj.autoAcceptWelcomes : true;

  const groupPolicy: MarmotGroupPolicy =
    obj.groupPolicy === "open" || obj.groupPolicy === "allowlist" ? obj.groupPolicy : "allowlist";

  const groupAllowFrom = (asStringArray(obj.groupAllowFrom) ?? []).map((x) => x.toLowerCase());

  const groupsRaw = obj.groups && typeof obj.groups === "object" ? (obj.groups as Record<string, unknown>) : {};
  const groups: Record<string, MarmotGroupConfig> = {};
  for (const [k, v] of Object.entries(groupsRaw)) {
    const gid = String(k).trim().toLowerCase();
    if (!gid) continue;
    const vv = v && typeof v === "object" ? (v as Record<string, unknown>) : {};
    const name = typeof vv.name === "string" && vv.name.trim() ? vv.name.trim() : undefined;
    groups[gid] = name ? { name } : {};
  }

  return {
    relays,
    stateDir,
    sidecarCmd,
    sidecarArgs,
    autoAcceptWelcomes,
    groupPolicy,
    groupAllowFrom,
    groups,
  };
}

