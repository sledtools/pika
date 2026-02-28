export type GroupPolicy = "allowlist" | "open";

export type AllowlistDecision =
  | { allowed: true }
  | {
    allowed: false;
    reason: "group_not_allowed" | "sender_not_allowed" | "sender_not_allowed_in_group";
  };

export function normalizeGroupId(input: string): string {
  const trimmed = input.trim();
  if (!trimmed) return trimmed;
  return trimmed
    .replace(/^pikachat-openclaw:/i, "")
    .replace(/^pikachat:/i, "")
    .replace(/^group:/i, "")
    .replace(/^pikachat-openclaw:group:/i, "")
    .replace(/^pikachat:group:/i, "")
    .trim()
    .toLowerCase();
}

export function parseReplyExactly(text: string): string | null {
  const m = text.match(/^openclaw:\s*reply exactly\s*\"([^\"]*)\"\s*$/i);
  return m ? m[1] ?? "" : null;
}

export function parseE2ePingNonce(text: string): string | null {
  const m = text.match(/^ping:([a-zA-Z0-9._-]{16,128})\s*$/);
  return m ? m[1] ?? "" : null;
}

export function parseLegacyPikaE2eNonce(text: string): string | null {
  const m = text.match(/^pika-e2e:([a-zA-Z0-9._-]{8,128})\s*$/);
  return m ? m[1] ?? "" : null;
}

export function parseDeterministicPingNonce(text: string): string | null {
  return parseE2ePingNonce(text) ?? parseLegacyPikaE2eNonce(text);
}

export function shouldBufferGroupMessage(params: {
  requireMention: boolean;
  wasMentioned: boolean;
  isHypernoteActionResponse: boolean;
}): boolean {
  return params.requireMention && !params.wasMentioned && !params.isHypernoteActionResponse;
}

export function evaluateAllowlistDecision(params: {
  groupPolicy: GroupPolicy;
  allowedGroups: Record<string, unknown>;
  groupAllowFrom: string[];
  groupUsers: string[] | null;
  nostrGroupId: string;
  senderPubkey: string;
}): AllowlistDecision {
  const groupId = normalizeGroupId(params.nostrGroupId);
  const sender = params.senderPubkey.trim().toLowerCase();

  if (params.groupPolicy !== "open" && !params.allowedGroups[groupId]) {
    return { allowed: false, reason: "group_not_allowed" };
  }
  if (params.groupAllowFrom.length > 0 && !params.groupAllowFrom.includes(sender)) {
    return { allowed: false, reason: "sender_not_allowed" };
  }
  if (params.groupUsers && params.groupUsers.length > 0 && !params.groupUsers.includes(sender)) {
    return { allowed: false, reason: "sender_not_allowed_in_group" };
  }
  return { allowed: true };
}

export type WelcomeBatchEntry = {
  from: string;
  group: string;
  name: string;
  wrapperId: string;
};

export function toWelcomeBatchEntry(ev: {
  from_pubkey: string;
  nostr_group_id: string;
  group_name: string;
  wrapper_event_id: string;
}): WelcomeBatchEntry {
  return {
    from: ev.from_pubkey,
    group: ev.nostr_group_id,
    name: ev.group_name,
    wrapperId: ev.wrapper_event_id,
  };
}
