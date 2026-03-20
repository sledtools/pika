import { randomBytes } from "node:crypto";
import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";

export type DmPolicy = "pairing" | "allowlist" | "disabled";

export type GroupAccess = {
  requireMention: boolean;
  allowFrom: string[];
};

export type PendingPairing = {
  code: string;
  senderId: string;
  chatId: string;
  createdAt: number;
};

export type AccessState = {
  dmPolicy: DmPolicy;
  allowFrom: string[];
  groups: Record<string, GroupAccess>;
  mentionPatterns: string[];
  pendingPairings: Record<string, PendingPairing>;
};

const PAIRING_TTL_MS = 24 * 60 * 60 * 1000;

export function defaultAccessState(): AccessState {
  return {
    dmPolicy: "pairing",
    allowFrom: [],
    groups: {},
    mentionPatterns: [],
    pendingPairings: {},
  };
}

function normalizeSenderId(senderId: string): string {
  return senderId.trim().toLowerCase();
}

function normalizeGroupId(groupId: string): string {
  return groupId.trim().toLowerCase();
}

export async function loadAccessState(filePath: string): Promise<AccessState> {
  try {
    const raw = await readFile(filePath, "utf8");
    const parsed = JSON.parse(raw) as Partial<AccessState>;
    return {
      dmPolicy:
        parsed.dmPolicy === "allowlist" || parsed.dmPolicy === "disabled"
          ? parsed.dmPolicy
          : "pairing",
      allowFrom: Array.isArray(parsed.allowFrom)
        ? parsed.allowFrom.map((entry) => normalizeSenderId(String(entry))).filter(Boolean)
        : [],
      groups: Object.fromEntries(
        Object.entries(parsed.groups ?? {}).map(([groupId, rawGroup]) => {
          const group = rawGroup as Partial<GroupAccess> | undefined;
          return [
            normalizeGroupId(groupId),
            {
              requireMention: group?.requireMention !== false,
              allowFrom: Array.isArray(group?.allowFrom)
                ? group!.allowFrom.map((entry) => normalizeSenderId(String(entry))).filter(Boolean)
                : [],
            },
          ];
        }),
      ),
      mentionPatterns: Array.isArray(parsed.mentionPatterns)
        ? parsed.mentionPatterns.map((entry) => String(entry)).filter(Boolean)
        : [],
      pendingPairings: Object.fromEntries(
        Object.entries(parsed.pendingPairings ?? {}).map(([code, pending]) => {
          const entry = pending as Partial<PendingPairing> | undefined;
          return [
            code.trim().toLowerCase(),
            {
              code: code.trim().toLowerCase(),
              senderId: normalizeSenderId(String(entry?.senderId ?? "")),
              chatId: normalizeGroupId(String(entry?.chatId ?? "")),
              createdAt: Number(entry?.createdAt ?? 0),
            },
          ];
        }),
      ),
    };
  } catch {
    return defaultAccessState();
  }
}

export async function saveAccessState(filePath: string, state: AccessState): Promise<void> {
  const directory = path.dirname(filePath);
  const tempPath = path.join(directory, `.access.${process.pid}.${Date.now()}.${Math.random().toString(16).slice(2)}.tmp`);
  await mkdir(directory, { recursive: true });
  await writeFile(tempPath, JSON.stringify(state, null, 2) + "\n", "utf8");
  await rename(tempPath, filePath);
}

export function pruneExpiredPairings(state: AccessState, now: number = Date.now()): AccessState {
  const pendingPairings = Object.fromEntries(
    Object.entries(state.pendingPairings).filter(([, pending]) => now - pending.createdAt < PAIRING_TTL_MS),
  );
  return { ...state, pendingPairings };
}

export function evaluateDmAccess(state: AccessState, senderId: string): "allowed" | "pairing" | "blocked" {
  const normalized = normalizeSenderId(senderId);
  if (state.dmPolicy === "disabled") return "blocked";
  if (state.allowFrom.includes(normalized)) return "allowed";
  return state.dmPolicy === "pairing" ? "pairing" : "blocked";
}

export function evaluateGroupAccess(
  state: AccessState,
  groupId: string,
  senderId: string,
): { enabled: boolean; requireMention: boolean; senderAllowed: boolean } {
  const normalizedGroupId = normalizeGroupId(groupId);
  const normalizedSenderId = normalizeSenderId(senderId);
  const group = state.groups[normalizedGroupId];
  if (!group) {
    return { enabled: false, requireMention: true, senderAllowed: false };
  }
  const senderAllowed = group.allowFrom.length === 0 || group.allowFrom.includes(normalizedSenderId);
  return {
    enabled: true,
    requireMention: group.requireMention !== false,
    senderAllowed,
  };
}

export function ensurePendingPairing(
  state: AccessState,
  senderId: string,
  chatId: string,
  now: number = Date.now(),
): { state: AccessState; pairing: PendingPairing } {
  const normalizedSenderId = normalizeSenderId(senderId);
  const normalizedChatId = normalizeGroupId(chatId);
  const existing = Object.values(state.pendingPairings).find(
    (pending) =>
      pending.senderId === normalizedSenderId &&
      pending.chatId === normalizedChatId &&
      now - pending.createdAt < PAIRING_TTL_MS,
  );
  if (existing) {
    return { state, pairing: existing };
  }
  const code = randomBytes(3).toString("hex");
  const pairing: PendingPairing = {
    code,
    senderId: normalizedSenderId,
    chatId: normalizedChatId,
    createdAt: now,
  };
  return {
    state: {
      ...state,
      pendingPairings: {
        ...state.pendingPairings,
        [code]: pairing,
      },
    },
    pairing,
  };
}

export function approvePairing(
  state: AccessState,
  code: string,
): { state: AccessState; pairing: PendingPairing | null } {
  const normalizedCode = code.trim().toLowerCase();
  const pairing = state.pendingPairings[normalizedCode] ?? null;
  if (!pairing) {
    return { state, pairing: null };
  }
  const pendingPairings = { ...state.pendingPairings };
  delete pendingPairings[normalizedCode];
  return {
    state: {
      ...state,
      allowFrom: Array.from(new Set([...state.allowFrom, pairing.senderId])).sort(),
      pendingPairings,
    },
    pairing,
  };
}

export function denyPairing(
  state: AccessState,
  code: string,
): { state: AccessState; pairing: PendingPairing | null } {
  const normalizedCode = code.trim().toLowerCase();
  const pairing = state.pendingPairings[normalizedCode] ?? null;
  if (!pairing) {
    return { state, pairing: null };
  }
  const pendingPairings = { ...state.pendingPairings };
  delete pendingPairings[normalizedCode];
  return { state: { ...state, pendingPairings }, pairing };
}

export function setDmPolicy(state: AccessState, dmPolicy: DmPolicy): AccessState {
  return { ...state, dmPolicy };
}

export function allowSender(state: AccessState, senderId: string): AccessState {
  const normalized = normalizeSenderId(senderId);
  return {
    ...state,
    allowFrom: Array.from(new Set([...state.allowFrom, normalized])).sort(),
  };
}

export function removeSender(state: AccessState, senderId: string): AccessState {
  const normalized = normalizeSenderId(senderId);
  return {
    ...state,
    allowFrom: state.allowFrom.filter((entry) => entry !== normalized),
  };
}

export function enableGroup(
  state: AccessState,
  groupId: string,
  options: Partial<GroupAccess> = {},
): AccessState {
  const normalizedGroupId = normalizeGroupId(groupId);
  return {
    ...state,
    groups: {
      ...state.groups,
      [normalizedGroupId]: {
        requireMention: options.requireMention !== false,
        allowFrom: (options.allowFrom ?? []).map((entry) => normalizeSenderId(entry)).filter(Boolean),
      },
    },
  };
}

export function disableGroup(state: AccessState, groupId: string): AccessState {
  const normalizedGroupId = normalizeGroupId(groupId);
  const groups = { ...state.groups };
  delete groups[normalizedGroupId];
  return { ...state, groups };
}
