import {
  DEFAULT_ACCOUNT_ID,
  formatPairingApproveHint,
  type ChannelPlugin,
} from "openclaw/plugin-sdk";
import { getMarmotRuntime } from "./runtime.js";
import {
  listMarmotAccountIds,
  resolveDefaultMarmotAccountId,
  resolveMarmotAccount,
  type ResolvedMarmotAccount,
} from "./types.js";
import { MarmotSidecar, resolveAccountStateDir } from "./sidecar.js";
import { resolveMarmotSidecarCommand } from "./sidecar-install.js";
import { readFileSync } from "node:fs";
import path from "node:path";

type MarmotSidecarHandle = {
  sidecar: MarmotSidecar;
  pubkey: string;
  npub: string;
};

const activeSidecars = new Map<string, MarmotSidecarHandle>();

// Group chat pending history buffer (for context injection when mention-gated)
type PendingHistoryEntry = { sender: string; body: string; timestamp?: number };
const groupHistories = new Map<string, PendingHistoryEntry[]>();
const GROUP_HISTORY_LIMIT = 50;

// Cache group names from welcome events
const groupNames = new Map<string, string>();

function recordPendingHistory(historyKey: string, entry: PendingHistoryEntry): void {
  const history = groupHistories.get(historyKey) ?? [];
  history.push(entry);
  while (history.length > GROUP_HISTORY_LIMIT) {
    history.shift();
  }
  groupHistories.set(historyKey, history);
}

function flushPendingHistory(historyKey: string): PendingHistoryEntry[] {
  const history = groupHistories.get(historyKey) ?? [];
  groupHistories.set(historyKey, []);
  return history;
}

function detectMention(text: string, botPubkey: string, botNpub: string, cfg: any): boolean {
  const textLower = text.toLowerCase();
  const pubkeyLower = botPubkey.toLowerCase();
  const npubLower = botNpub.toLowerCase();

  // Check for nostr:npub mention (Pika/Nostr format)
  if (npubLower && textLower.includes(`nostr:${npubLower}`)) {
    return true;
  }
  // Check for raw npub mention
  if (npubLower && textLower.includes(npubLower)) {
    return true;
  }
  // Check for @pubkey or raw pubkey mention (fallback)
  if (textLower.includes(`@${pubkeyLower}`) || textLower.includes(pubkeyLower)) {
    return true;
  }
  // Check agent mentionPatterns from config (e.g. "kelaode", "@kelaode")
  try {
    const runtime = getMarmotRuntime();
    const mentionRegexes = runtime.channel.mentions.buildMentionRegexes(cfg);
    if (mentionRegexes.length > 0 && runtime.channel.mentions.matchesMentionPatterns(text, mentionRegexes)) {
      return true;
    }
  } catch {
    // SDK utilities not available, fall back to pubkey/npub detection
  }
  return false;
}

// --- Bech32 encoder for npub conversion ---
const BECH32_CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";

function bech32Polymod(values: number[]): number {
  const GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
  let chk = 1;
  for (const v of values) {
    const b = chk >> 25;
    chk = ((chk & 0x1ffffff) << 5) ^ v;
    for (let i = 0; i < 5; i++) {
      if ((b >> i) & 1) chk ^= GEN[i];
    }
  }
  return chk;
}

function bech32HrpExpand(hrp: string): number[] {
  const ret: number[] = [];
  for (let i = 0; i < hrp.length; i++) ret.push(hrp.charCodeAt(i) >> 5);
  ret.push(0);
  for (let i = 0; i < hrp.length; i++) ret.push(hrp.charCodeAt(i) & 31);
  return ret;
}

function bech32CreateChecksum(hrp: string, data: number[]): number[] {
  const values = bech32HrpExpand(hrp).concat(data).concat([0, 0, 0, 0, 0, 0]);
  const polymod = bech32Polymod(values) ^ 1;
  const ret: number[] = [];
  for (let i = 0; i < 6; i++) ret.push((polymod >> (5 * (5 - i))) & 31);
  return ret;
}

function hexToNpub(hex: string): string {
  const hrp = "npub";
  // Convert hex to bytes
  const bytes: number[] = [];
  for (let i = 0; i < hex.length; i += 2) {
    bytes.push(parseInt(hex.substring(i, i + 2), 16));
  }
  // Convert 8-bit bytes to 5-bit groups
  const data: number[] = [];
  let acc = 0;
  let bits = 0;
  for (const b of bytes) {
    acc = (acc << 8) | b;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      data.push((acc >> bits) & 31);
    }
  }
  if (bits > 0) data.push((acc << (5 - bits)) & 31);
  const checksum = bech32CreateChecksum(hrp, data);
  return hrp + "1" + data.concat(checksum).map(d => BECH32_CHARSET[d]).join("");
}

// --- Nostr profile name cache ---
interface CachedProfile {
  name: string | null;
  fetchedAt: number;
}
const profileCache = new Map<string, CachedProfile>();
const PROFILE_CACHE_TTL_MS = 60 * 60 * 1000; // 1 hour
const pendingFetches = new Set<string>();

async function fetchNostrProfileName(pubkeyHex: string, relays: string[]): Promise<string | null> {
  if (pendingFetches.has(pubkeyHex)) return null;
  pendingFetches.add(pubkeyHex);
  try {
    for (const relay of relays.slice(0, 3)) {
      try {
        const wsUrl = relay.replace(/\/$/, "");
        const result = await new Promise<string | null>((resolve) => {
          const ws = new WebSocket(wsUrl);
          const timeout = setTimeout(() => { try { ws.close(); } catch {} resolve(null); }, 5000);
          ws.addEventListener("open", () => {
            const subId = "profile_" + pubkeyHex.slice(0, 8);
            ws.send(JSON.stringify(["REQ", subId, { kinds: [0], authors: [pubkeyHex], limit: 1 }]));
          });
          ws.addEventListener("message", (event: any) => {
            try {
              const data = typeof event.data === "string" ? event.data : event.data.toString();
              const msg = JSON.parse(data);
              if (msg[0] === "EVENT" && msg[2]?.kind === 0) {
                const meta = JSON.parse(msg[2].content);
                const displayName = meta.display_name || meta.displayName || meta.name || null;
                clearTimeout(timeout);
                try { ws.close(); } catch {}
                resolve(displayName);
              } else if (msg[0] === "EOSE") {
                clearTimeout(timeout);
                try { ws.close(); } catch {}
                resolve(null);
              }
            } catch { /* ignore parse errors */ }
          });
          ws.addEventListener("error", () => { clearTimeout(timeout); try { ws.close(); } catch {} resolve(null); });
        });
        if (result) return result;
      } catch { /* try next relay */ }
    }
    return null;
  } finally {
    pendingFetches.delete(pubkeyHex);
  }
}

function getCachedProfileName(pubkeyHex: string): string | null | undefined {
  const cached = profileCache.get(pubkeyHex);
  if (!cached) return undefined; // not cached
  if (Date.now() - cached.fetchedAt > PROFILE_CACHE_TTL_MS) return undefined; // expired
  return cached.name;
}

function resolveMemberName(pubkey: string, cfg: any): string {
  // Check memberNames config (supports both hex and npub keys)
  const memberNames = cfg?.channels?.marmot?.memberNames ?? {};
  const pk = pubkey.toLowerCase();
  const npub = hexToNpub(pk);
  for (const [key, name] of Object.entries(memberNames)) {
    const keyLower = key.toLowerCase();
    if ((keyLower === pk || keyLower === npub) && typeof name === "string") {
      return name;
    }
  }
  // Check profile cache (synchronous — returns npub if not cached yet)
  const cached = getCachedProfileName(pk);
  if (cached) return cached;
  // Fall back to npub
  return npub;
}

/** Async version: fetches profile from relays if not cached, then caches it */
async function resolveMemberNameAsync(pubkey: string, cfg: any): Promise<string> {
  // Check memberNames config first
  const memberNames = cfg?.channels?.marmot?.memberNames ?? {};
  const pk = pubkey.toLowerCase();
  const npub = hexToNpub(pk);
  for (const [key, name] of Object.entries(memberNames)) {
    const keyLower = key.toLowerCase();
    if ((keyLower === pk || keyLower === npub) && typeof name === "string") {
      return name;
    }
  }
  // Check cache
  const cached = getCachedProfileName(pk);
  if (cached !== undefined) return cached || npub;
  // Fetch from relays
  const relays: string[] = cfg?.channels?.marmot?.relays ?? [];
  const profileName = await fetchNostrProfileName(pk, relays);
  profileCache.set(pk, { name: profileName, fetchedAt: Date.now() });
  return profileName || npub;
}

function isDmGroup(chatId: string, cfg: any): boolean {
  const dmGroups: string[] = cfg?.channels?.marmot?.dmGroups ?? [];
  return dmGroups.some((id: string) => id.toLowerCase() === chatId.toLowerCase());
}

/**
 * Check if a group is a 1:1 conversation (2 or fewer members).
 * Uses the MLS group membership via sqlite, with a fallback to the group name
 * from the welcome event (Pika names DM groups "DM").
 * Returns false on any error (fail-open: treat as multi-person group).
 */
function isOneOnOneGroup(nostrGroupId: string, stateDir: string): boolean {
  // Check in-memory cache first
  const cachedName = groupNames.get(nostrGroupId.toLowerCase());
  if (cachedName?.toLowerCase() === "dm") return true;

  // Query the MLS groups table for the group name (persists across restarts)
  try {
    const { execSync } = require("node:child_process");
    const dbPath = path.join(stateDir, "mdk.sqlite");
    const query = `SELECT name FROM groups WHERE nostr_group_id = x'${nostrGroupId}';`;
    const result = execSync(`sqlite3 "${dbPath}" "${query}"`, { encoding: "utf-8", timeout: 3000 }).trim();
    if (result.toLowerCase() === "dm") return true;
  } catch {
    // fall through
  }

  return false;
}

/**
 * Query the marmot sqlite DB for distinct member pubkeys in a group.
 * Returns an array of { pubkey, npub, name } for each member.
 */
async function getGroupMembers(
  nostrGroupId: string,
  stateDir: string,
  cfg: any,
): Promise<Array<{ pubkey: string; npub: string; name: string }>> {
  try {
    const dbPath = path.join(stateDir, "mdk.sqlite");
    // Use the groups table to find the mls_group_id for this nostr_group_id
    const { execSync } = await import("node:child_process");
    const query = `SELECT DISTINCT hex(m.pubkey) as pk FROM messages m JOIN groups g ON m.mls_group_id = g.mls_group_id WHERE g.nostr_group_id = x'${nostrGroupId}' ORDER BY pk;`;
    const result = execSync(`sqlite3 "${dbPath}" "${query}"`, { encoding: "utf-8", timeout: 3000 }).trim();
    if (!result) return [];
    const members = result.split("\n").map((hexPk) => {
      const pk = hexPk.trim().toLowerCase();
      const npub = hexToNpub(pk);
      const name = resolveMemberName(pk, cfg);
      return { pubkey: pk, npub, name };
    });
    return members;
  } catch {
    return [];
  }
}

function resolveRequireMention(chatId: string, cfg: any): boolean {
  // Check channels.marmot.groups config
  const groups = cfg?.channels?.marmot?.groups ?? {};
  const groupConfig = groups[chatId] ?? groups["*"];
  if (groupConfig && typeof groupConfig.requireMention === "boolean") {
    return groupConfig.requireMention;
  }
  // Default: require mention in groups
  return true;
}

const GROUP_SYSTEM_PROMPT = [
  "Trust model: You are in a group chat with your owner and their trusted friends.",
  "Be helpful and engaging with everyone — they are trusted, not strangers.",
  "Only the owner (CommandAuthorized=true) can run commands or access private information.",
  "If a friend asks for something sensitive, politely explain the boundary.",
  "Load GROUP_MEMORY.md for shared context. Never reference MEMORY.md or secrets in groups.",
  'To create a poll, send a pika-prompt code block: ```pika-prompt\n{"title":"Your question?","options":["Option A","Option B","Option C"]}\n```',
  'When users vote, you\'ll see messages like [Voted "Option A"]. Track votes to determine results.',
  'To send rich HTML content (forms, styled widgets, visualizations), use a pika-html code block with a short ID: ```pika-html my-widget\n<h1>Hello</h1>\n```',
  'Always include an ID after pika-html (e.g. "dashboard", "search-results"). To update it later, send: ```pika-html-update my-widget\n<h1>Updated</h1>\n``` The update replaces the original inline.',
  "The HTML renders in an inline WebView in the app. You can include CSS styles inline or in a <style> tag.",
  "To let users send a message back from the HTML, use the JS bridge: window.pika.send(\"message text\").",
  'For stateful widgets (3D avatars, dashboards), define window.pikaState in your HTML.',
  'To update state without reloading, send: ```pika-html-state-update my-widget\n{"key":"value"}\n```',
  'The app calls window.pikaState(body) on the live widget. Use pika-html-update for full HTML replacement, pika-html-state-update for JS state injection.',
].join(" ");

async function dispatchInboundToAgent(params: {
  runtime: ReturnType<typeof getMarmotRuntime>;
  accountId: string;
  chatId: string;
  senderId: string;
  text: string;
  isOwner: boolean;
  isGroupChat: boolean;
  wasMentioned?: boolean;
  inboundHistory?: PendingHistoryEntry[];
  groupName?: string;
  stateDir?: string;
  deliverText: (text: string) => Promise<void>;
  log?: { error?: (msg: string) => void };
}): Promise<void> {
  const { runtime, accountId, chatId, senderId, text, isOwner, isGroupChat, deliverText } = params;
  const cfg = runtime.config.loadConfig();

  // DM groups and owner-only 1:1 → main session. Multi-person groups → isolated session.
  const chatType = isGroupChat ? "group" : "dm";
  const senderName = await resolveMemberNameAsync(senderId, cfg);

  // Resolve agent binding — respects bindings config (e.g. channel: "marmot" → agentId)
  const route = runtime.channel.routing.resolveAgentRoute({
    cfg,
    channel: "marmot",
    accountId,
    peer: {
      kind: isGroupChat ? "group" : "direct",
      id: isGroupChat ? chatId : senderId,
    },
  });

  // Resolve group members for context (best effort)
  let groupMembersInfo: string | undefined;
  if (isGroupChat && params.stateDir) {
    const members = await getGroupMembers(chatId, params.stateDir, cfg);
    if (members.length > 0) {
      groupMembersInfo = members
        .map((m) => `${m.name} (nostr:${m.npub})`)
        .join(", ");
    }
  }

  const ctx = runtime.channel.reply.finalizeInboundContext({
    Body: text,
    RawBody: text,
    CommandBody: text,
    BodyForCommands: text,
    BodyForAgent: text,
    From: senderId,
    To: chatId,
    SessionKey: route.sessionKey,
    AccountId: route.accountId,
    Provider: "marmot",
    Surface: "marmot",
    ChatType: chatType,
    SenderId: senderId,
    SenderName: senderName,
    SenderUsername: hexToNpub(senderId.toLowerCase()),
    SenderTag: isOwner ? "owner" : "friend",
    CommandAuthorized: isOwner,
    WasMentioned: params.wasMentioned ?? !isGroupChat,
    ...(isGroupChat ? {
      GroupSubject: params.groupName || groupNames.get(chatId) || undefined,
      GroupSystemPrompt: GROUP_SYSTEM_PROMPT + (groupMembersInfo ? `\nGroup members: ${groupMembersInfo}\nTo mention someone, use their nostr:npub1... identifier.` : ""),
      InboundHistory: params.inboundHistory,
      ConversationLabel: params.groupName || chatId,
    } : {}),
  });

  await runtime.channel.reply.dispatchReplyWithBufferedBlockDispatcher({
    ctx,
    cfg,
    dispatcherOptions: {
      deliver: async (payload) => {
        const replyText = payload.text?.trim();
        if (!replyText) return;
        await deliverText(replyText);
      },
      onError: (err, info) => {
        params.log?.error?.(
          `[${accountId}] reply dispatch error kind=${info.kind}: ${String(err)}`,
        );
      },
    },
  });
}

function looksLikeGroupIdHex(input: string): boolean {
  return /^[0-9a-f]{64}$/i.test(input.trim());
}

function normalizeGroupId(input: string): string {
  const trimmed = input.trim();
  if (!trimmed) return trimmed;
  return trimmed
    .replace(/^marmot:/i, "")
    .replace(/^group:/i, "")
    .replace(/^marmot:group:/i, "")
    .trim()
    .toLowerCase();
}

function parseReplyExactly(text: string): string | null {
  const m = text.match(/^openclaw:\s*reply exactly\s*\"([^\"]*)\"\s*$/i);
  return m ? m[1] ?? "" : null;
}

function parseE2ePingNonce(text: string): string | null {
  // Deterministic E2E test hook (no LLM):
  //   inbound:  ping:<nonce>
  //   reply:   pong:<nonce>
  //
  // Keep this intentionally strict so it doesn't trigger accidentally in normal chats.
  const m = text.match(/^ping:([a-zA-Z0-9._-]{16,128})\s*$/);
  return m ? m[1] ?? "" : null;
}

function parseLegacyPikaE2eNonce(text: string): string | null {
  // Back-compat with older tests.
  const m = text.match(/^pika-e2e:([a-zA-Z0-9._-]{8,128})\s*$/);
  return m ? m[1] ?? "" : null;
}

function parsePikaPromptResponse(text: string): { promptId: string; selected: string } | null {
  const m = text.match(/```pika-prompt-response\n([\s\S]*?)```/);
  if (!m) return null;
  try {
    const json = JSON.parse(m[1].trim());
    if (typeof json.prompt_id === "string" && typeof json.selected === "string") {
      return { promptId: json.prompt_id, selected: json.selected };
    }
  } catch {}
  return null;
}

function resolveSidecarCmd(cfgCmd?: string | null): string | null {
  const env = process.env.MARMOT_SIDECAR_CMD?.trim();
  if (env) return env;
  const trimmed = String(cfgCmd ?? "").trim();
  return trimmed ? trimmed : null;
}

function resolveSidecarArgs(cfgArgs?: string[] | null): string[] | null {
  const env = process.env.MARMOT_SIDECAR_ARGS?.trim();
  if (env) {
    try {
      const parsed = JSON.parse(env);
      if (Array.isArray(parsed) && parsed.every((x) => typeof x === "string")) {
        return parsed;
      }
    } catch {
      // ignore
    }
  }
  if (Array.isArray(cfgArgs) && cfgArgs.every((x) => typeof x === "string")) {
    return cfgArgs;
  }
  return null;
}

export const marmotPlugin: ChannelPlugin<ResolvedMarmotAccount> = {
  id: "marmot",
  meta: {
    id: "marmot",
    label: "Marmot",
    selectionLabel: "Marmot (Rust)",
    docsPath: "/channels/marmot",
    docsLabel: "marmot",
    blurb: "MLS E2EE groups over Nostr (Rust sidecar).",
    order: 56,
    quickstartAllowFrom: true,
  },
  capabilities: {
    chatTypes: ["dm", "group"],
    media: false,
    nativeCommands: false,
  },
  reload: { configPrefixes: ["channels.marmot", "plugins.entries.marmot"] },

  config: {
    listAccountIds: (cfg) => listMarmotAccountIds(cfg),
    resolveAccount: (cfg, accountId) => resolveMarmotAccount({ cfg, accountId }),
    defaultAccountId: (cfg) => resolveDefaultMarmotAccountId(cfg),
    setAccountEnabled: async () => {
      throw new Error("marmot: multi-account enable/disable not implemented yet");
    },
    deleteAccount: async () => {
      throw new Error("marmot: multi-account delete not implemented yet");
    },
    isConfigured: (account) => account.configured,
    describeAccount: (account) => ({
      accountId: account.accountId,
      name: account.name,
      enabled: account.enabled,
      configured: account.configured,
    }),
    resolveAllowFrom: ({ cfg, accountId }) =>
      (resolveMarmotAccount({ cfg, accountId }).config.groupAllowFrom ?? []).map((x) => String(x)),
    formatAllowFrom: ({ allowFrom }) =>
      allowFrom
        .map((x) => String(x).trim().toLowerCase())
        .filter(Boolean),
  },

  // For now: no DMs, but keep the pairing surface stubbed so OpenClaw help output stays consistent.
  pairing: {
    idLabel: "marmotPubkey",
    normalizeAllowEntry: (entry) => entry.replace(/^marmot:/i, "").trim().toLowerCase(),
    notifyApproval: async () => {
      // Not implemented (DMs not implemented yet).
    },
  },
  security: {
    resolveDmPolicy: () => ({
      policy: "pairing",
      allowFrom: [],
      policyPath: "channels.marmot.dmPolicy",
      allowFromPath: "channels.marmot.allowFrom",
      approveHint: formatPairingApproveHint("marmot"),
      normalizeEntry: (raw) => raw.replace(/^marmot:/i, "").trim().toLowerCase(),
    }),
  },

  messaging: {
    normalizeTarget: (target) => normalizeGroupId(target),
    targetResolver: {
      looksLikeId: (input) => looksLikeGroupIdHex(normalizeGroupId(input)),
      hint: "<nostrGroupIdHex|marmot:group:<hex>>",
    },
  },

  outbound: {
    deliveryMode: "direct",
    textChunkLimit: 4000,
    sendText: async ({ to, text, accountId }) => {
      const aid = accountId ?? DEFAULT_ACCOUNT_ID;
      const handle = activeSidecars.get(aid);
      if (!handle) {
        throw new Error(`marmot sidecar not running for account ${aid}`);
      }
      const groupId = normalizeGroupId(to);
      if (!looksLikeGroupIdHex(groupId)) {
        throw new Error(`invalid marmot group id: ${to}`);
      }
      await handle.sidecar.sendMessage(groupId, text ?? "");
      return { channel: "marmot", to: groupId };
    },
    sendMedia: async () => {
      throw new Error("marmot does not support media");
    },
  },

  gateway: {
    startAccount: async (ctx) => {
      const account = ctx.account;
      const runtime = getMarmotRuntime();
      const cfg = runtime.config.loadConfig();
      const resolved = resolveMarmotAccount({ cfg, accountId: account.accountId });

      // Guard against duplicate startAccount calls for the same account.
      // Set sentinel immediately (before any awaits) to prevent races.
      if (activeSidecars.has(resolved.accountId)) {
        ctx.log?.info(
          `[${resolved.accountId}] sidecar already running, skipping duplicate startAccount`,
        );
        return { stop: () => {} };
      }
      activeSidecars.set(resolved.accountId, null as any);

      if (!resolved.enabled) {
        throw new Error("marmot account disabled");
      }
      if (!resolved.configured) {
        throw new Error("marmot relays not configured (channels.marmot.relays)");
      }

      const relays = resolved.config.relays.map((r) => String(r).trim()).filter(Boolean);
      const baseStateDir = resolveAccountStateDir({
        accountId: resolved.accountId,
        stateDirOverride: resolved.config.stateDir,
      });
      const requestedSidecarCmd = resolveSidecarCmd(resolved.config.sidecarCmd) ?? "marmotd";
      const sidecarCmd = await resolveMarmotSidecarCommand({
        requestedCmd: requestedSidecarCmd,
        log: ctx.log,
      });
      const relayArgs = (relays.length > 0 ? relays : ["ws://127.0.0.1:18080"]).flatMap((r) => ["--relay", r]);
      const sidecarArgs =
        resolveSidecarArgs(resolved.config.sidecarArgs) ??
        ["daemon", ...relayArgs, "--state-dir", baseStateDir];

      ctx.log?.info(
        `[${resolved.accountId}] 🦞 MOLTATHON MARMOT v0.2.0 — starting sidecar cmd=${JSON.stringify(sidecarCmd)} args=${JSON.stringify(sidecarArgs)}`,
      );

      const sidecar = new MarmotSidecar({ cmd: sidecarCmd, args: sidecarArgs });
      const ready = await sidecar.waitForReady(15_000);
      activeSidecars.set(resolved.accountId, {
        sidecar,
        pubkey: ready.pubkey,
        npub: ready.npub,
      });
      ctx.setStatus({
        accountId: resolved.accountId,
        publicKey: ready.pubkey,
      });

      // Ensure the daemon has the full relay list (even if started with a single relay).
      await sidecar.setRelays(relays);
      await sidecar.publishKeypackage(relays);

      const groupPolicy = resolved.config.groupPolicy ?? "allowlist";
      const groupAllowFrom =
        (resolved.config.groupAllowFrom ?? []).map((x) => String(x).trim().toLowerCase()).filter(Boolean);
      // Owner is explicitly configured, or falls back to first entry in groupAllowFrom
      const ownerPubkeys: string[] =
        (resolved.config.owner ? (Array.isArray(resolved.config.owner) ? resolved.config.owner : [resolved.config.owner]) : [])
          .map((x: any) => String(x).trim().toLowerCase())
          .filter(Boolean);
      const isOwnerPubkey = (pk: string): boolean => {
        if (ownerPubkeys.length > 0) return ownerPubkeys.includes(pk);
        // Legacy fallback: first entry in groupAllowFrom is owner
        return groupAllowFrom.length > 0 && groupAllowFrom[0] === pk;
      };
      const allowedGroups = resolved.config.groups ?? {};
      const activeCalls = new Map<string, { chatId: string; senderId: string }>();
      const callStartTtsText = String(process.env.MARMOT_CALL_START_TTS_TEXT ?? "").trim();
      const callStartTtsDelayMs = (() => {
        const raw = String(process.env.MARMOT_CALL_START_TTS_DELAY_MS ?? "").trim();
        if (!raw) return 1500;
        const n = Number(raw);
        if (!Number.isFinite(n)) return 1500;
        // Clamp: avoid footguns.
        return Math.max(0, Math.min(30_000, Math.floor(n)));
      })();

      const isGroupAllowed = (nostrGroupId: string): boolean => {
        if (groupPolicy === "open") return true;
        const gid = String(nostrGroupId).trim().toLowerCase();
        return Boolean(allowedGroups[gid]);
      };
      const isSenderAllowed = (pubkey: string): boolean => {
        if (groupAllowFrom.length === 0) return true;
        const pk = String(pubkey).trim().toLowerCase();
        return groupAllowFrom.includes(pk);
      };

      sidecar.onEvent(async (ev) => {
        if (ev.type === "welcome_received") {
          ctx.log?.info(
            `[${resolved.accountId}] welcome_received from=${ev.from_pubkey} group=${ev.nostr_group_id} name=${JSON.stringify(ev.group_name)}`,
          );
          // Cache group name for later use in GroupSubject
          if (ev.group_name && ev.nostr_group_id) {
            groupNames.set(ev.nostr_group_id.toLowerCase(), ev.group_name);
          }
          if (resolved.config.autoAcceptWelcomes) {
            try {
              await sidecar.acceptWelcome(ev.wrapper_event_id);
            } catch (err) {
              ctx.log?.error(
                `[${resolved.accountId}] failed to accept welcome wrapper=${ev.wrapper_event_id}: ${err}`,
              );
            }
          }
          return;
        }
        if (ev.type === "group_joined") {
          ctx.log?.info(
            `[${resolved.accountId}] group_joined nostr_group_id=${ev.nostr_group_id} mls_group_id=${ev.mls_group_id}`,
          );
          return;
        }
        if (ev.type === "call_invite_received") {
          if (!isGroupAllowed(ev.nostr_group_id)) {
            ctx.log?.debug(
              `[${resolved.accountId}] reject call invite (group not allowed) group=${ev.nostr_group_id} call_id=${ev.call_id}`,
            );
            await sidecar.rejectCall(ev.call_id, "group_not_allowed");
            return;
          }
          if (!isSenderAllowed(ev.from_pubkey)) {
            ctx.log?.debug(
              `[${resolved.accountId}] reject call invite (sender not allowed) sender=${ev.from_pubkey} call_id=${ev.call_id}`,
            );
            await sidecar.rejectCall(ev.call_id, "sender_not_allowed");
            return;
          }
          ctx.log?.info(
            `[${resolved.accountId}] accept call invite group=${ev.nostr_group_id} from=${ev.from_pubkey} call_id=${ev.call_id}`,
          );
          await sidecar.acceptCall(ev.call_id);
          return;
        }
	        if (ev.type === "call_session_started") {
	          activeCalls.set(ev.call_id, {
	            chatId: ev.nostr_group_id,
	            senderId: ev.from_pubkey,
	          });
	          ctx.log?.info(
	            `[${resolved.accountId}] call_session_started group=${ev.nostr_group_id} from=${ev.from_pubkey} call_id=${ev.call_id}`,
	          );
	          if (callStartTtsText) {
	            ctx.log?.info(
	              `[${resolved.accountId}] call_start_tts scheduled call_id=${ev.call_id} delay_ms=${callStartTtsDelayMs} text=${JSON.stringify(callStartTtsText)}`,
	            );
	            const callId = ev.call_id;
	            setTimeout(() => {
	              // Fire-and-forget: we don't want this to block the event loop.
	              void sidecar
	                .sendAudioResponse(callId, callStartTtsText)
	                .then((stats) => {
	                  const publish = stats.publish_path ? ` publish_path=${stats.publish_path}` : "";
	                  ctx.log?.info(
	                    `[${resolved.accountId}] call_start_tts ok call_id=${callId} frames_published=${stats.frames_published}${publish}`,
	                  );
	                })
	                .catch((err) => {
	                  ctx.log?.error(
	                    `[${resolved.accountId}] call_start_tts failed call_id=${callId}: ${err}`,
	                  );
	                });
	            }, callStartTtsDelayMs);
	          }
	          return;
	        }
        if (ev.type === "call_session_ended") {
          activeCalls.delete(ev.call_id);
          ctx.log?.info(
            `[${resolved.accountId}] call_session_ended call_id=${ev.call_id} reason=${ev.reason}`,
          );
          return;
        }
        if (ev.type === "call_debug") {
          ctx.log?.debug(
            `[${resolved.accountId}] call_debug call_id=${ev.call_id} tx=${ev.tx_frames} rx=${ev.rx_frames} drop=${ev.rx_dropped}`,
          );
          return;
        }
        if (ev.type === "call_transcript_partial") {
          ctx.log?.debug(
            `[${resolved.accountId}] call_transcript_partial call_id=${ev.call_id} text=${JSON.stringify(ev.text)}`,
          );
          return;
        }
        if (ev.type === "call_transcript_final") {
          ctx.log?.info(
            `[${resolved.accountId}] call_transcript_final call_id=${ev.call_id} text=${JSON.stringify(ev.text)}`,
          );
          const callCtx = activeCalls.get(ev.call_id);
          if (!callCtx) {
            ctx.log?.debug(
              `[${resolved.accountId}] call_transcript_final with no active call context call_id=${ev.call_id}`,
            );
            return;
          }
          const transcript = ev.text?.trim();
          if (!transcript) {
            return;
          }
          try {
            await dispatchInboundToAgent({
              runtime,
              accountId: resolved.accountId,
              senderId: callCtx.senderId,
              chatId: callCtx.chatId,
              text: transcript,
              deliverText: async (responseText: string) => {
                const stats = await sidecar.sendAudioResponse(ev.call_id, responseText);
                const publish = stats.publish_path ? ` publish_path=${stats.publish_path}` : "";
                ctx.log?.info(
                  `[${resolved.accountId}] call_tts ok call_id=${ev.call_id} frames_published=${stats.frames_published}${publish}`,
                );
              },
              log: ctx.log,
            });
          } catch (err) {
            ctx.log?.error(
              `[${resolved.accountId}] voice transcript dispatch failed call_id=${ev.call_id}: ${err}`,
            );
          }
          return;
        }
        if (ev.type === "message_received") {
          // Self-message filter: skip our own messages echoed back
          const handle = activeSidecars.get(resolved.accountId);
          if (handle && ev.from_pubkey.toLowerCase() === handle.pubkey.toLowerCase()) {
            ctx.log?.debug(
              `[${resolved.accountId}] skip self-message group=${ev.nostr_group_id}`,
            );
            return;
          }

          if (!isGroupAllowed(ev.nostr_group_id)) {
            ctx.log?.debug(
              `[${resolved.accountId}] drop message (group not allowed) group=${ev.nostr_group_id}`,
            );
            return;
          }
          if (!isSenderAllowed(ev.from_pubkey)) {
            ctx.log?.debug(
              `[${resolved.accountId}] drop message (sender not allowed) sender=${ev.from_pubkey}`,
            );
            return;
          }

          // Debug: if a call signal fails to parse in the sidecar, it will fall back to
          // `message_received` and the bot will treat it as plain text. Surface the raw
          // content (with basic redaction) so we can patch the sidecar parser.
          //
          // Note: we key off either explicit `pika.call` markers or "looks like JSON" to avoid
          // missing shape mismatches (e.g. JSON envelopes without expected substrings).
          if (
            typeof ev.content === "string" &&
            (ev.content.includes("pika.call") ||
              ev.content.includes("call.invite") ||
              ev.content.trim().startsWith("{"))
          ) {
            const redacted = ev.content.replace(/capv1_[0-9a-f]{64}/gi, "capv1_REDACTED");
            ctx.log?.warn(
              `[${resolved.accountId}] debug_message_received group=${ev.nostr_group_id} from=${ev.from_pubkey} content=${JSON.stringify(redacted.slice(0, 800))}`,
            );
          }

          const e2ePingNonce = parseE2ePingNonce(ev.content) ?? parseLegacyPikaE2eNonce(ev.content);
          if (e2ePingNonce !== null) {
            const ack = `pong:${e2ePingNonce}`;
            ctx.log?.info(
              `[${resolved.accountId}] e2e ping/pong hook reply group=${ev.nostr_group_id} from=${ev.from_pubkey} nonce=${e2ePingNonce}`,
            );
            await sidecar.sendMessage(ev.nostr_group_id, ack);
            return;
          }

          const directive = parseReplyExactly(ev.content);
          if (directive !== null) {
            await sidecar.sendMessage(ev.nostr_group_id, directive);
            return;
          }

          const pollResponse = parsePikaPromptResponse(ev.content);
          if (pollResponse !== null) {
            ev.content = `[Voted "${pollResponse.selected}"]`;
          }

          try {
            const senderPk = String(ev.from_pubkey).trim().toLowerCase();
            const senderIsOwner = isOwnerPubkey(senderPk);
            const currentCfg = runtime.config.loadConfig();
            const groupId = ev.nostr_group_id.toLowerCase();
            const historyKey = `marmot:${resolved.accountId}:${groupId}`;

            // Determine if this is a DM group (1:1 with bot)
            const isDm = isDmGroup(groupId, currentCfg);
            // Multi-person groups use group flow (even for owners); DM groups route to main session
            const isGroupChat = !isDm;

            if (isGroupChat) {
              // GROUP CHAT FLOW — mention gating + history buffering
              // Skip mention gating for 1:1 groups (2 members) — they should always trigger
              const oneOnOne = isOneOnOneGroup(groupId, baseStateDir);
              if (oneOnOne) {
                ctx.log?.debug(
                  `[${resolved.accountId}] 1:1 group detected, skipping mention gating group=${ev.nostr_group_id} from=${senderPk}`,
                );
              }
              const requireMention = oneOnOne ? false : resolveRequireMention(groupId, currentCfg);
              const wasMentioned = handle ? detectMention(ev.content, handle.pubkey, handle.npub, currentCfg) : false;
              const senderName = await resolveMemberNameAsync(senderPk, currentCfg);

              if (requireMention && !wasMentioned) {
                // Not mentioned — buffer for context, don't dispatch
                recordPendingHistory(historyKey, {
                  sender: senderName,
                  body: ev.content,
                  timestamp: ev.created_at ? ev.created_at * 1000 : Date.now(),
                });
                ctx.log?.debug(
                  `[${resolved.accountId}] group message buffered (no mention) group=${ev.nostr_group_id} from=${senderPk}`,
                );
                return;
              }

              // Mentioned (or mention not required) — dispatch with pending history
              const pendingHistory = flushPendingHistory(historyKey);
              ctx.log?.info(
                `[${resolved.accountId}] group message dispatching (mentioned=${wasMentioned}) group=${ev.nostr_group_id} from=${senderPk} pendingHistory=${pendingHistory.length}`,
              );

              await dispatchInboundToAgent({
                runtime,
                accountId: resolved.accountId,
                senderId: ev.from_pubkey,
                chatId: ev.nostr_group_id,
                text: ev.content,
                isOwner: senderIsOwner,
                isGroupChat: true,
                wasMentioned,
                inboundHistory: pendingHistory.length > 0 ? pendingHistory : undefined,
                groupName: groupNames.get(groupId),
                stateDir: baseStateDir,
                deliverText: async (responseText: string) => {
                  ctx.log?.info(
                    `[${resolved.accountId}] send group=${ev.nostr_group_id} len=${responseText.length}`,
                  );
                  await sidecar.sendMessage(ev.nostr_group_id, responseText);
                },
                log: ctx.log,
              });
            } else {
              // DM / OWNER FLOW — route to main session (existing behavior)
              await dispatchInboundToAgent({
                runtime,
                accountId: resolved.accountId,
                senderId: ev.from_pubkey,
                chatId: ev.nostr_group_id,
                text: ev.content,
                isOwner: senderIsOwner,
                isGroupChat: false,
                deliverText: async (responseText: string) => {
                  ctx.log?.info(
                    `[${resolved.accountId}] send dm=${ev.nostr_group_id} len=${responseText.length}`,
                  );
                  await sidecar.sendMessage(ev.nostr_group_id, responseText);
                },
                log: ctx.log,
              });
            }
          } catch (err) {
            ctx.log?.error(
              `[${resolved.accountId}] dispatchInboundToAgent failed: ${err}`,
            );
          }
        }
      });

      return {
        stop: () => {
          const handle = activeSidecars.get(resolved.accountId);
          if (handle) {
            activeSidecars.delete(resolved.accountId);
            void handle.sidecar.shutdown();
          }
          ctx.log?.info(`[${resolved.accountId}] marmot sidecar stopped`);
        },
      };
    },
  },
};
