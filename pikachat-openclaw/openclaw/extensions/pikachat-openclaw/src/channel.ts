import {
  DEFAULT_ACCOUNT_ID,
  formatPairingApproveHint,
  type ChannelPlugin,
} from "openclaw/plugin-sdk";
import { getPikachatRuntime } from "./runtime.js";
import {
  listPikachatAccountIds,
  resolveDefaultPikachatAccountId,
  resolvePikachatAccount,
  type ResolvedPikachatAccount,
} from "./types.js";
import { PikachatSidecar, resolveAccountStateDir } from "./sidecar.js";
import { resolvePikachatSidecarCommand } from "./sidecar-install.js";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

/**
 * Transcribe a WAV audio chunk using openclaw's media-understanding framework
 * via runtime.stt.transcribeAudioFile. Falls back to direct OpenAI-compatible
 * fetch if the runtime method is not available (older openclaw versions).
 */
async function transcribeAudioChunk(params: {
  audioPath: string;
  runtime: ReturnType<typeof getPikachatRuntime>;
  log?: { info?: (msg: string) => void; error?: (msg: string) => void; debug?: (msg: string) => void; warn?: (msg: string) => void };
  accountId: string;
  callId: string;
}): Promise<string | undefined> {
  const { audioPath, runtime, log, accountId, callId } = params;
  const cfg = runtime.config.loadConfig();

  // Prefer runtime.stt if available (openclaw >= 2026.2.x with STT support)
  const stt = (runtime as any).stt;
  if (stt?.transcribeAudioFile) {
    const result = await stt.transcribeAudioFile({ filePath: audioPath, cfg });
    const text = result?.text?.trim();
    if (text) {
      log?.info?.(`[${accountId}] transcribed call_id=${callId} text_len=${text.length}`);
    }
    return text || undefined;
  }

  // Fallback: direct fetch to OpenAI-compatible endpoint
  log?.debug?.(`[${accountId}] runtime.stt not available, using direct fetch fallback`);
  const apiKey = process.env.OPENAI_API_KEY?.trim() ?? process.env.GROQ_API_KEY?.trim();
  if (!apiKey) {
    log?.warn?.(`[${accountId}] no STT provider configured (set OPENAI_API_KEY or upgrade openclaw for runtime.stt) call_id=${callId}`);
    return undefined;
  }

  const isGroq = !process.env.OPENAI_API_KEY?.trim() && !!process.env.GROQ_API_KEY?.trim();
  const baseUrl = isGroq ? "https://api.groq.com/openai/v1" : "https://api.openai.com/v1";
  const model = isGroq ? "whisper-large-v3-turbo" : "gpt-4o-mini-transcribe";

  const wavBuffer = readFileSync(audioPath);
  const formData = new FormData();
  formData.append("file", new Blob([wavBuffer], { type: "audio/wav" }), "audio.wav");
  formData.append("model", model);
  formData.append("response_format", "json");

  const resp = await fetch(`${baseUrl}/audio/transcriptions`, {
    method: "POST",
    headers: { "Authorization": `Bearer ${apiKey}` },
    body: formData,
    signal: AbortSignal.timeout(30_000),
  });

  if (!resp.ok) {
    const body = await resp.text().catch(() => "");
    throw new Error(`STT failed status=${resp.status} body=${body.slice(0, 200)}`);
  }

  const json: any = await resp.json();
  const text = typeof json.text === "string" ? json.text.trim() : "";
  if (text) {
    log?.info?.(`[${accountId}] transcribed call_id=${callId} provider=${isGroq ? "groq" : "openai"} model=${model} text_len=${text.length}`);
  }
  return text || undefined;
}

type PikachatSidecarHandle = {
  sidecar: PikachatSidecar;
  pubkey: string;
  npub: string;
};

const activeSidecars = new Map<string, PikachatSidecarHandle>();

// Map session keys to group/account context so tool factories can resolve targets.
const sessionGroupMap = new Map<string, { groupId: string; accountId: string }>();

/** Find the most recently mapped group ID for a given account (used by message actions). */
function findGroupIdForAccount(accountId: string): string | null {
  for (const entry of sessionGroupMap.values()) {
    if (entry.accountId === accountId) return entry.groupId;
  }
  return null;
}

// Per-account hypernote catalog text fetched from sidecar at startup.
const hypernoteCatalogs = new Map<string, string>();

// Group chat pending history buffer (for context injection when mention-gated)
type PendingHistoryEntry = { sender: string; body: string; timestamp?: number };
const groupHistories = new Map<string, PendingHistoryEntry[]>();
const GROUP_HISTORY_LIMIT = 50;

// Cache group names from welcome events
const groupNames = new Map<string, string>();

// Cache group member counts from sidecar events (group_joined, group_created, list_groups).
// Used to auto-detect 1:1 DM groups without relying on the group name.
const groupMemberCounts = new Map<string, number>();

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
    const runtime = getPikachatRuntime();
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
const HYPERNOTE_ACTION_RESPONSE_KIND = 9468;
const REACTION_KIND = 7; // NIP-25

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
  const memberNames = cfg?.channels?.["pikachat-openclaw"]?.memberNames ?? {};
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
  const memberNames = cfg?.channels?.["pikachat-openclaw"]?.memberNames ?? {};
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
  const relays: string[] = cfg?.channels?.["pikachat-openclaw"]?.relays ?? [];
  const profileName = await fetchNostrProfileName(pk, relays);
  profileCache.set(pk, { name: profileName, fetchedAt: Date.now() });
  return profileName || npub;
}

function isDmGroup(chatId: string, cfg: any): boolean {
  const dmGroups: string[] = cfg?.channels?.["pikachat-openclaw"]?.dmGroups ?? [];
  return dmGroups.some((id: string) => id.toLowerCase() === chatId.toLowerCase());
}

/**
 * Check if a group is a 1:1 conversation (2 or fewer members).
 * Uses the member count cache populated from sidecar events (group_joined,
 * group_created, list_groups).
 * Returns false if unknown (fail-open: treat as multi-person group).
 */
function isOneOnOneGroup(nostrGroupId: string): boolean {
  const count = groupMemberCounts.get(nostrGroupId.toLowerCase());
  return count !== undefined && count <= 2;
}

/**
 * Query the pikachat sqlite DB for distinct member pubkeys in a group.
 * Returns an array of { pubkey, npub, name } for each member.
 */
async function getGroupMembers(
  nostrGroupId: string,
  stateDir: string,
  cfg: any,
): Promise<Array<{ pubkey: string; npub: string; name: string }>> {
  try {
    const dbPath = path.join(stateDir, "mdk.sqlite");
    const safeGroupId = nostrGroupId.replace(/[^0-9a-fA-F]/g, "");
    if (!safeGroupId) return [];
    const { DatabaseSync } = await import("node:sqlite");
    const db = new DatabaseSync(dbPath, { open: true, readOnly: true });
    try {
      const stmt = db.prepare(
        `SELECT DISTINCT hex(m.pubkey) as pk
         FROM messages m
         JOIN groups g ON m.mls_group_id = g.mls_group_id
         WHERE g.nostr_group_id = x'${safeGroupId}'
         ORDER BY pk`,
      );
      const rows = stmt.all() as Array<{ pk: string }>;
      return rows.map((row) => {
        const pk = row.pk.trim().toLowerCase();
        const npub = hexToNpub(pk);
        const name = resolveMemberName(pk, cfg);
        return { pubkey: pk, npub, name };
      });
    } finally {
      db.close();
    }
  } catch {
    return [];
  }
}

type GroupConfig = {
  requireMention?: boolean;
  users?: string[];
  systemPrompt?: string;
};

function resolveGroupConfig(chatId: string, cfg: any): GroupConfig | null {
  const groups = cfg?.channels?.["pikachat-openclaw"]?.groups ?? {};
  const gid = chatId.toLowerCase();
  // Exact match first, then wildcard fallback
  return groups[gid] ?? groups["*"] ?? null;
}

function resolveRequireMention(chatId: string, cfg: any): boolean {
  const groupConfig = resolveGroupConfig(chatId, cfg);
  if (groupConfig && typeof groupConfig.requireMention === "boolean") {
    return groupConfig.requireMention;
  }
  // Default: require mention in groups
  return true;
}

function resolveGroupUsers(chatId: string, cfg: any): string[] | null {
  const groupConfig = resolveGroupConfig(chatId, cfg);
  if (groupConfig?.users && Array.isArray(groupConfig.users) && groupConfig.users.length > 0) {
    return groupConfig.users.map((u: string) => String(u).trim().toLowerCase()).filter(Boolean);
  }
  return null;
}

function resolveGroupSystemPrompt(chatId: string, cfg: any): string | null {
  const groupConfig = resolveGroupConfig(chatId, cfg);
  if (groupConfig?.systemPrompt && typeof groupConfig.systemPrompt === "string") {
    return groupConfig.systemPrompt.trim();
  }
  return null;
}

function isSenderAllowedInGroup(senderPk: string, chatId: string, cfg: any): boolean {
  const groupUsers = resolveGroupUsers(chatId, cfg);
  if (!groupUsers) return true; // No per-group restriction
  return groupUsers.includes(senderPk.toLowerCase());
}

const GROUP_SYSTEM_PROMPT = [
  "Trust model: You are in a group chat with your owner and their trusted friends.",
  "Be helpful and engaging with everyone — they are trusted, not strangers.",
  "Only the owner (CommandAuthorized=true) can run commands or access private information.",
  "If a friend asks for something sensitive, politely explain the boundary.",
  "Load GROUP_MEMORY.md for shared context. Never reference MEMORY.md or secrets in groups.",
  "To send interactive UI cards, use the send_hypernote tool with MDX content.",
  'User submissions arrive as structured action text like [Hypernote action "action_name" submitted] with optional form fields below.',
  "To react to a message, use the send_reaction tool with the message's event_id and an emoji.",
  "To interact with another user's hypernote (e.g. vote in a poll), use the submit_hypernote_action tool.",
  'To send rich HTML content (forms, styled widgets, visualizations), use a pika-html code block with a short ID: ```pika-html my-widget\n<h1>Hello</h1>\n```',
  'Always include an ID after pika-html (e.g. "dashboard", "search-results"). To update it later, send: ```pika-html-update my-widget\n<h1>Updated</h1>\n``` The update replaces the original inline.',
  "The HTML renders in an inline WebView in the app. You can include CSS styles inline or in a <style> tag.",
  "To let users send a message back from the HTML, use the JS bridge: window.pika.send(\"message text\").",
  'For stateful widgets (3D avatars, dashboards), define window.pikaState in your HTML.',
  'To update state without reloading, send: ```pika-html-state-update my-widget\n{"key":"value"}\n```',
  'The app calls window.pikaState(body) on the live widget. Use pika-html-update for full HTML replacement, pika-html-state-update for JS state injection.',
].join(" ");

async function dispatchInboundToAgent(params: {
  runtime: ReturnType<typeof getPikachatRuntime>;
  accountId: string;
  chatId: string;
  senderId: string;
  eventId?: string;
  text: string;
  isOwner: boolean;
  isGroupChat: boolean;
  wasMentioned?: boolean;
  inboundHistory?: PendingHistoryEntry[];
  groupName?: string;
  stateDir?: string;
  deliverText: (text: string) => Promise<void>;
  sendTyping?: () => Promise<void>;
  log?: { error?: (msg: string) => void };
}): Promise<void> {
  const { runtime, accountId, chatId, senderId, text, isOwner, isGroupChat, deliverText } = params;
  const cfg = runtime.config.loadConfig();

  // DM groups and owner-only 1:1 → main session. Multi-person groups → isolated session.
  const chatType = isGroupChat ? "group" : "dm";
  const senderName = await resolveMemberNameAsync(senderId, cfg);

  // Resolve agent binding — respects bindings config (e.g. channel: "pikachat" → agentId)
  const route = runtime.channel.routing.resolveAgentRoute({
    cfg,
    channel: "pikachat-openclaw",
    accountId,
    peer: {
      kind: isGroupChat ? "group" : "direct",
      id: isGroupChat ? chatId : senderId,
    },
  });

  // Record session → group mapping so tool factories can resolve targets.
  sessionGroupMap.set(route.sessionKey, { groupId: chatId, accountId: route.accountId ?? accountId });

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
    BodyForAgent: params.eventId ? `${text}\n\n[pikachat_event_id: ${params.eventId}]` : text,
    From: senderId,
    To: chatId,
    SessionKey: route.sessionKey,
    AccountId: route.accountId,
    Provider: "pikachat-openclaw",
    Surface: "pikachat-openclaw",
    ChatType: chatType,
    SenderId: senderId,
    SenderName: senderName,
    SenderUsername: hexToNpub(senderId.toLowerCase()),
    SenderTag: isOwner ? "owner" : "friend",
    MessageSid: params.eventId,
    CommandAuthorized: isOwner,
    WasMentioned: params.wasMentioned ?? !isGroupChat,
    ...(isGroupChat ? {
      GroupSubject: params.groupName || groupNames.get(chatId) || undefined,
      GroupSystemPrompt: (resolveGroupSystemPrompt(chatId, cfg) ?? GROUP_SYSTEM_PROMPT)
        + (groupMembersInfo ? `\nGroup members: ${groupMembersInfo}\nTo mention someone, use their nostr:npub1... identifier.` : "")
        + (hypernoteCatalogs.get(accountId) ? `\n\nHypernote components available via send_hypernote tool:\n${hypernoteCatalogs.get(accountId)}` : ""),
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
      onReplyStart: params.sendTyping,
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

function resolveOutboundTarget(to: string, accountId?: string | null): { handle: PikachatSidecarHandle; groupId: string } {
  const aid = accountId ?? DEFAULT_ACCOUNT_ID;
  const handle = activeSidecars.get(aid);
  if (!handle) {
    throw new Error(`pikachat sidecar not running for account ${aid}`);
  }
  const groupId = normalizeGroupId(to);
  if (!looksLikeGroupIdHex(groupId)) {
    throw new Error(`invalid pikachat group id: ${to}`);
  }
  return { handle, groupId };
}

function normalizeGroupId(input: string): string {
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

function parseHypernoteActionResponse(text: string): { action: string; form: Record<string, string> } | null {
  try {
    const parsed = JSON.parse(text);
    if (!parsed || typeof parsed !== "object") return null;
    const action = typeof parsed.action === "string" ? parsed.action.trim() : "";
    if (!action) return null;
    const formRaw = parsed.form;
    const form: Record<string, string> = {};
    if (formRaw && typeof formRaw === "object" && !Array.isArray(formRaw)) {
      for (const [key, value] of Object.entries(formRaw)) {
        if (value == null) continue;
        form[String(key)] = String(value);
      }
    }
    return { action, form };
  } catch {
    return null;
  }
}

function formatHypernoteActionResponse(response: { action: string; form: Record<string, string> }): string {
  const lines = Object.entries(response.form).map(([k, v]) => `- ${k}: ${v}`);
  if (lines.length === 0) {
    return `[Hypernote action "${response.action}" submitted]`;
  }
  return [`[Hypernote action "${response.action}" submitted]`, ...lines].join("\n");
}

function resolveSidecarCmd(cfgCmd?: string | null): string | null {
  const env = process.env.PIKACHAT_SIDECAR_CMD?.trim();
  if (env) return env;
  const trimmed = String(cfgCmd ?? "").trim();
  return trimmed ? trimmed : null;
}

function resolveSidecarArgs(cfgArgs?: string[] | null): string[] | null {
  const env = process.env.PIKACHAT_SIDECAR_ARGS?.trim();
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

export const pikachatPlugin: ChannelPlugin<ResolvedPikachatAccount> = {
  id: "pikachat-openclaw",
  meta: {
    id: "pikachat-openclaw",
    label: "Pikachat",
    selectionLabel: "Pikachat (Rust)",
    docsPath: "/channels/pikachat-openclaw",
    docsLabel: "pikachat-openclaw",
    blurb: "MLS E2EE groups over Nostr (Rust sidecar).",
    order: 56,
    quickstartAllowFrom: true,
  },
  capabilities: {
    chatTypes: ["dm", "group"],
    media: true,
    reactions: true,
    nativeCommands: false,
  },
  reload: { configPrefixes: ["channels.pikachat-openclaw", "plugins.entries.pikachat-openclaw"] },

  config: {
    listAccountIds: (cfg) => listPikachatAccountIds(cfg),
    resolveAccount: (cfg, accountId) => resolvePikachatAccount({ cfg, accountId }),
    defaultAccountId: (cfg) => resolveDefaultPikachatAccountId(cfg),
    setAccountEnabled: async () => {
      throw new Error("pikachat: multi-account enable/disable not implemented yet");
    },
    deleteAccount: async () => {
      throw new Error("pikachat: multi-account delete not implemented yet");
    },
    isConfigured: (account) => account.configured,
    describeAccount: (account) => ({
      accountId: account.accountId,
      name: account.name ?? undefined,
      enabled: account.enabled,
      configured: account.configured,
      publicKey: activeSidecars.get(account.accountId)?.pubkey ?? undefined,
    }),
    resolveAllowFrom: ({ cfg, accountId }) =>
      (resolvePikachatAccount({ cfg, accountId }).config.groupAllowFrom ?? []).map((x) => String(x)),
    formatAllowFrom: ({ allowFrom }) =>
      allowFrom
        .map((x) => String(x).trim().toLowerCase())
        .filter(Boolean),
  },

  // For now: no DMs, but keep the pairing surface stubbed so OpenClaw help output stays consistent.
  pairing: {
    idLabel: "pikachatPubkey",
    normalizeAllowEntry: (entry) => entry.replace(/^pikachat-openclaw:/i, "").replace(/^pikachat:/i, "").trim().toLowerCase(),
    notifyApproval: async () => {
      // Not implemented (DMs not implemented yet).
    },
  },
  security: {
    resolveDmPolicy: () => ({
      policy: "pairing",
      allowFrom: [],
      policyPath: "channels.pikachat-openclaw.dmPolicy",
      allowFromPath: "channels.pikachat-openclaw.allowFrom",
      approveHint: formatPairingApproveHint("pikachat-openclaw"),
      normalizeEntry: (raw) => raw.replace(/^pikachat-openclaw:/i, "").replace(/^pikachat:/i, "").trim().toLowerCase(),
    }),
  },

  messaging: {
    normalizeTarget: (target) => normalizeGroupId(target),
    targetResolver: {
      looksLikeId: (input) => looksLikeGroupIdHex(normalizeGroupId(input)),
      hint: "<nostrGroupIdHex|pikachat-openclaw:group:<hex>>",
    },
  },

  actions: {
    listActions: () => ["react"] as any[],
    handleAction: async ({ action, params, accountId }): Promise<any> => {
      if (action === "react") {
        const messageId = typeof params.messageId === "string" ? params.messageId : "";
        const emoji = (typeof params.emoji === "string" && params.emoji.trim()) ? params.emoji.trim() : "❤️";
        if (!messageId) return { content: [{ type: "text", text: "messageId is required." }] };
        const handle = activeSidecars.get(accountId ?? DEFAULT_ACCOUNT_ID);
        if (!handle) return { content: [{ type: "text", text: "Sidecar not running." }] };
        const groupId = findGroupIdForAccount(accountId ?? DEFAULT_ACCOUNT_ID);
        if (!groupId) return { content: [{ type: "text", text: "No target group." }] };
        handle.sidecar.sendReaction(groupId, messageId, emoji);
        return { content: [{ type: "text", text: "Reaction sent." }] };
      }
      return { content: [{ type: "text", text: `Unknown action: ${action}` }] };
    },
  },

  agentPrompt: {
    messageToolHints: () => [
      "- Use `send_hypernote` to send interactive UI cards. Compose MDX content using components: Card, VStack, HStack, Heading, Body, Caption, TextInput, ChecklistItem, SubmitButton. The tool returns a `[pikachat_event_id: <id>]` you can use immediately with `submit_hypernote_action` to vote on your own poll.",
      '- User responses to hypernote buttons arrive as structured text: [Hypernote action "action_name" submitted] with optional form fields.',
      "- Use `submit_hypernote_action` to interact with another user's or bot's hypernote (e.g. vote in a poll). Requires the event_id and action name.",
      "- Each inbound message includes a `[pikachat_event_id: <id>]` line. Use this id with the `react` action or `submit_hypernote_action` to reference that message.",
    ],
  },

  status: {
    buildAccountSnapshot: ({ account }) => ({
      accountId: account.accountId,
      name: account.name ?? undefined,
      enabled: account.enabled,
      configured: account.configured,
      publicKey: activeSidecars.get(account.accountId)?.pubkey ?? undefined,
    }),
  },

  outbound: {
    deliveryMode: "direct",
    textChunkLimit: 4000,
    sendText: async ({ to, text, accountId }) => {
      const { handle, groupId } = resolveOutboundTarget(to, accountId);
      const result = await handle.sidecar.sendMessage(groupId, text ?? "");
      return { channel: "pikachat-openclaw", to: groupId, messageId: result?.event_id ?? "" };
    },
    sendMedia: async ({ to, text, mediaUrl, accountId }) => {
      const { handle, groupId } = resolveOutboundTarget(to, accountId);
      if (!mediaUrl) {
        throw new Error("sendMedia requires a mediaUrl");
      }

      // Download media to a temp file so the sidecar can read it
      const tempDir = mkdtempSync(path.join(tmpdir(), "pikachat-media-"));
      const urlObj = new URL(mediaUrl);
      const basename = path.basename(urlObj.pathname) || "file.bin";
      const tempFile = path.join(tempDir, basename);
      try {
        const resp = await fetch(mediaUrl, { signal: AbortSignal.timeout(60_000) });
        if (!resp.ok) {
          throw new Error(`download failed: HTTP ${resp.status}`);
        }
        const arrayBuf = await resp.arrayBuffer();
        writeFileSync(tempFile, Buffer.from(arrayBuf));
      } catch (err) {
        rmSync(tempDir, { recursive: true, force: true });
        throw new Error(`failed to download media from ${mediaUrl}: ${err}`);
      }

      try {
        const result = (await handle.sidecar.sendMedia(groupId, tempFile, {
          caption: text ?? "",
        })) as any;
        return { channel: "pikachat-openclaw" as const, to: groupId, messageId: result?.event_id ?? "" };
      } finally {
        // Clean up temp file after a delay to ensure sidecar has read it
        const timer = setTimeout(() => {
          rmSync(tempDir, { recursive: true, force: true });
        }, 30_000);
        (timer as any).unref?.();
      }
    },
  },

  gateway: {
    startAccount: async (ctx) => {
      const account = ctx.account;
      const runtime = getPikachatRuntime();
      const cfg = runtime.config.loadConfig();
      const resolved = resolvePikachatAccount({ cfg, accountId: account.accountId });

      // Guard against duplicate startAccount calls for the same account.
      // Return a long-lived Promise tied to the existing sidecar so the
      // framework considers this channel alive (prevents auto-restart loops).
      const existingHandle = activeSidecars.get(resolved.accountId);
      if (existingHandle) {
        ctx.log?.info(
          `[${resolved.accountId}] sidecar already running, skipping duplicate startAccount`,
        );
        ctx.setStatus({
          accountId: resolved.accountId,
          publicKey: existingHandle.pubkey,
        });
        return new Promise<void>((resolve) => {
          const finish = () => resolve();
          ctx.abortSignal.addEventListener("abort", () => {
            const handle = activeSidecars.get(resolved.accountId);
            if (handle) {
              activeSidecars.delete(resolved.accountId);
              void handle.sidecar.shutdown();
            }
            ctx.log?.info(`[${resolved.accountId}] pikachat sidecar stopped`);
            finish();
          }, { once: true });
          existingHandle.sidecar.waitForExit().then(finish);
        });
      }
      activeSidecars.set(resolved.accountId, null as any);

      if (!resolved.enabled) {
        throw new Error("pikachat account disabled");
      }
      if (!resolved.configured) {
        throw new Error("pikachat relays not configured (channels.pikachat-openclaw.relays)");
      }

      const relays = resolved.config.relays.map((r) => String(r).trim()).filter(Boolean);
      const baseStateDir = resolveAccountStateDir({
        accountId: resolved.accountId,
        stateDirOverride: resolved.config.stateDir,
      });
      const requestedSidecarCmd = resolveSidecarCmd(resolved.config.sidecarCmd) ?? "pikachat";
      const sidecarCmd = await resolvePikachatSidecarCommand({
        requestedCmd: requestedSidecarCmd,
        log: ctx.log,
        pinnedVersion: resolved.config.sidecarVersion,
      });
      const relayArgs = (relays.length > 0 ? relays : ["ws://127.0.0.1:18080"]).flatMap((r) => ["--relay", r]);
      const configuredSidecarArgs = resolveSidecarArgs(resolved.config.sidecarArgs);
      let sidecarArgs = configuredSidecarArgs ?? ["daemon", ...relayArgs, "--state-dir", baseStateDir];
      const sidecarArgsLookLikeDaemon = sidecarArgs.length > 0 && sidecarArgs[0] === "daemon";
      const sidecarHasAutoAcceptFlag = sidecarArgs.includes("--auto-accept-welcomes");
      if (resolved.config.autoAcceptWelcomes && sidecarArgsLookLikeDaemon && !sidecarHasAutoAcceptFlag) {
        sidecarArgs = [...sidecarArgs, "--auto-accept-welcomes"];
      }
      const sidecarAutoAcceptWelcomes =
        sidecarArgsLookLikeDaemon && sidecarArgs.includes("--auto-accept-welcomes");

      ctx.log?.info(
        `[${resolved.accountId}] 🦞 MOLTATHON PIKACHAT v0.2.0 — starting sidecar cmd=${JSON.stringify(sidecarCmd)} args=${JSON.stringify(sidecarArgs)}`,
      );

      const sidecar = new PikachatSidecar({ cmd: sidecarCmd, args: sidecarArgs });
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

      // Fetch hypernote component catalog for agent prompt injection.
      try {
        const catalog = await sidecar.hypernoteCatalog();
        const catalogText = typeof catalog === "string" ? catalog : JSON.stringify(catalog);
        if (catalogText) hypernoteCatalogs.set(resolved.accountId, catalogText);
      } catch {
        ctx.log?.debug?.(`[${resolved.accountId}] hypernote catalog fetch failed (optional)`);
      }

      // Seed member counts from existing groups (so isOneOnOneGroup works
      // immediately) and create an owner DM if no groups exist yet.
      // Fire-and-forget so it doesn't block startup.
      {
        const ownerCfg = resolved.config.owner;
        const ownerPk: string | undefined = ownerCfg
          ? String(Array.isArray(ownerCfg) ? ownerCfg[0] : ownerCfg).trim().toLowerCase()
          : undefined;
        void (async () => {
          try {
            const groupsResult = (await sidecar.listGroups()) as any;
            const groups: any[] = groupsResult?.groups ?? [];
            // Seed member count cache for all groups
            for (const g of groups) {
              const gid = String(g.nostr_group_id ?? "").toLowerCase();
              const mc = typeof g.member_count === "number" ? g.member_count : 0;
              if (gid) groupMemberCounts.set(gid, mc);
            }
            if (ownerPk && groups.length === 0) {
              ctx.log?.info(
                `[${resolved.accountId}] no groups found, creating DM with owner ${ownerPk}`,
              );
              const created = await sidecar.initGroup(ownerPk);
              ctx.log?.info(
                `[${resolved.accountId}] owner DM created nostr_group_id=${created.nostr_group_id}`,
              );
            }
          } catch (err) {
            ctx.log?.warn(
              `[${resolved.accountId}] failed to seed groups / init owner DM: ${err}`,
            );
          }
        })();
      }

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
      const activeCalls = new Map<string, { chatId: string; senderId: string; responding: boolean }>();
      const callStartTtsText = String(process.env.PIKACHAT_CALL_START_TTS_TEXT ?? "").trim();
      const callStartTtsDelayMs = (() => {
        const raw = String(process.env.PIKACHAT_CALL_START_TTS_DELAY_MS ?? "").trim();
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

      // Batch welcome processing: collect welcomes over a short window,
      // then log a single summary line with accept/fail counts.
      let welcomeBatch: Array<{ from: string; group: string; name: string; wrapperId: string }> = [];
      let welcomeFlushTimer: ReturnType<typeof setTimeout> | null = null;
      const WELCOME_BATCH_DELAY_MS = 500;

      const flushWelcomeBatch = async () => {
        welcomeFlushTimer = null;
        const batch = welcomeBatch;
        welcomeBatch = [];
        if (batch.length === 0) return;

        const uniqueSenders = new Set(batch.map((w) => w.from));
        ctx.log?.info(
          `[${resolved.accountId}] welcome_received count=${batch.length} senders=${uniqueSenders.size}`,
        );

        if (!resolved.config.autoAcceptWelcomes) return;
        if (sidecarAutoAcceptWelcomes) {
          ctx.log?.debug(
            `[${resolved.accountId}] auto-accept welcomes handled by sidecar count=${batch.length}`,
          );
          return;
        }

        let accepted = 0;
        let failed = 0;
        for (const w of batch) {
          try {
            await sidecar.acceptWelcome(w.wrapperId);
            accepted++;
          } catch {
            failed++;
          }
        }
        if (failed > 0) {
          ctx.log?.debug(
            `[${resolved.accountId}] auto-accept welcomes: accepted=${accepted} stale=${failed}`,
          );
        } else if (accepted > 0) {
          ctx.log?.info(
            `[${resolved.accountId}] auto-accept welcomes: accepted=${accepted}`,
          );
        }
      };

      sidecar.onEvent(async (ev) => {
        if (ev.type === "welcome_received") {
          // Cache group name for later use in GroupSubject
          if (ev.group_name && ev.nostr_group_id) {
            groupNames.set(ev.nostr_group_id.toLowerCase(), ev.group_name);
          }
          welcomeBatch.push({
            from: ev.from_pubkey,
            group: ev.nostr_group_id,
            name: ev.group_name,
            wrapperId: ev.wrapper_event_id,
          });
          if (!welcomeFlushTimer) {
            welcomeFlushTimer = setTimeout(() => { void flushWelcomeBatch(); }, WELCOME_BATCH_DELAY_MS);
          }
          return;
        }
        if (ev.type === "group_joined") {
          groupMemberCounts.set(ev.nostr_group_id.toLowerCase(), ev.member_count);
          ctx.log?.info(
            `[${resolved.accountId}] group_joined nostr_group_id=${ev.nostr_group_id} mls_group_id=${ev.mls_group_id} members=${ev.member_count}`,
          );
          return;
        }
        if (ev.type === "group_created") {
          groupMemberCounts.set(ev.nostr_group_id.toLowerCase(), ev.member_count);
          ctx.log?.info(
            `[${resolved.accountId}] group_created nostr_group_id=${ev.nostr_group_id} mls_group_id=${ev.mls_group_id} peer=${ev.peer_pubkey} members=${ev.member_count}`,
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
          // Per-group user allowlist check (same layered logic as messages — see below).
          // groups[id].users is an additional filter on top of groupAllowFrom.
          {
            const currentCfgForCall = runtime.config.loadConfig();
            if (!isSenderAllowedInGroup(ev.from_pubkey, ev.nostr_group_id, currentCfgForCall)) {
              ctx.log?.debug(
                `[${resolved.accountId}] reject call invite (sender not in group users) sender=${ev.from_pubkey} group=${ev.nostr_group_id} call_id=${ev.call_id}`,
              );
              await sidecar.rejectCall(ev.call_id, "sender_not_allowed_in_group");
              return;
            }
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
	            responding: false,
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
        if (ev.type === "call_audio_chunk") {
          ctx.log?.info(
            `[${resolved.accountId}] call_audio_chunk call_id=${ev.call_id} path=${ev.audio_path}`,
          );
          const callCtx = activeCalls.get(ev.call_id);
          if (!callCtx) {
            ctx.log?.debug(
              `[${resolved.accountId}] call_audio_chunk with no active call context call_id=${ev.call_id}`,
            );
            return;
          }
          if (callCtx.responding) {
            ctx.log?.debug(
              `[${resolved.accountId}] skip audio chunk while responding call_id=${ev.call_id}`,
            );
            return;
          }
          // Transcribe the audio chunk using openclaw's media-understanding provider
          let transcript: string | undefined;
          try {
            transcript = await transcribeAudioChunk({
              audioPath: ev.audio_path,
              runtime,
              log: ctx.log,
              accountId: resolved.accountId,
              callId: ev.call_id,
            });
          } catch (err) {
            ctx.log?.error(
              `[${resolved.accountId}] audio transcription failed call_id=${ev.call_id}: ${err}`,
            );
            return;
          } finally {
            // Clean up temp WAV file
            try { rmSync(ev.audio_path, { force: true }); } catch {}
          }
          if (!transcript?.trim()) {
            return;
          }
          callCtx.responding = true;
          try {
            const currentCfg = runtime.config.loadConfig();
            await dispatchInboundToAgent({
              runtime,
              accountId: resolved.accountId,
              senderId: callCtx.senderId,
              chatId: callCtx.chatId,
              text: transcript.trim(),
              isOwner: isOwnerPubkey(callCtx.senderId.toLowerCase()),
              isGroupChat: false,
              deliverText: async (responseText: string) => {
                ctx.log?.info(
                  `[${resolved.accountId}] call TTS response call_id=${ev.call_id} text_len=${responseText.length}`,
                );
                // Use openclaw's config-driven TTS (OpenAI, ElevenLabs, Edge)
                // to get raw PCM, write to temp file, send via sendAudioFile.
                // Falls back to sidecar's built-in TTS on failure.
                try {
                  const ttsResult = await runtime.tts.textToSpeechTelephony({
                    text: responseText,
                    cfg: currentCfg,
                  });
                  if (!ttsResult.success || !ttsResult.audioBuffer) {
                    throw new Error(ttsResult.error ?? "TTS returned no audio");
                  }
                  const tempDir = mkdtempSync(path.join(tmpdir(), "pikachat-tts-"));
                  const pcmPath = path.join(tempDir, `tts-${Date.now()}.pcm`);
                  writeFileSync(pcmPath, ttsResult.audioBuffer);
                  const timer = setTimeout(() => {
                    rmSync(tempDir, { recursive: true, force: true });
                  }, 5 * 60 * 1000);
                  (timer as any).unref?.();
                  const sampleRate = ttsResult.sampleRate ?? 24000;
                  await sidecar.sendAudioFile(ev.call_id, pcmPath, sampleRate);
                  ctx.log?.info(
                    `[${resolved.accountId}] call audio sent call_id=${ev.call_id} path=${pcmPath} sample_rate=${sampleRate} provider=${ttsResult.provider ?? "unknown"}`,
                  );
                } catch (openclawTtsErr) {
                  ctx.log?.info(
                    `[${resolved.accountId}] openclaw_tts error call_id=${ev.call_id}: ${openclawTtsErr}, falling back to sidecar TTS`,
                  );
                  await sidecar.sendAudioResponse(ev.call_id, responseText);
                }
              },
              log: ctx.log,
            });
          } catch (err) {
            ctx.log?.error(
              `[${resolved.accountId}] voice transcript dispatch failed call_id=${ev.call_id}: ${err}`,
            );
          } finally {
            callCtx.responding = false;
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

          // Per-group user allowlist check.
          // Note: groups[id].users is an *additional* filter on top of groupAllowFrom.
          // A sender who passes groupAllowFrom can still be silently dropped here if they
          // are not listed in the group's users array. This is intentional for fine-grained
          // per-group control, but the drop is silent so it's easy to miss if misconfigured.
          {
            const currentCfgForGroupCheck = runtime.config.loadConfig();
            if (!isSenderAllowedInGroup(ev.from_pubkey, ev.nostr_group_id, currentCfgForGroupCheck)) {
              ctx.log?.debug(
                `[${resolved.accountId}] drop message (sender not in group users allowlist) sender=${ev.from_pubkey} group=${ev.nostr_group_id}`,
              );
              return;
            }
          }

          // NIP-25 reactions arrive as kind=7 message_received events.
          // Surface as a brief contextual message so the agent is aware, but
          // don't trigger a full reply (wasMentioned=false keeps it passive context).
          if (ev.kind === REACTION_KIND) {
            ctx.log?.debug(
              `[${resolved.accountId}] reaction received from=${ev.from_pubkey} emoji=${JSON.stringify(ev.content)} target=${ev.event_id}`,
            );
            const senderName = await resolveMemberNameAsync(ev.from_pubkey, runtime.config.loadConfig());
            const reactionText = `${senderName} reacted ${ev.content || "?"} to a message`;
            await dispatchInboundToAgent({
              runtime,
              accountId: resolved.accountId,
              senderId: ev.from_pubkey,
              eventId: ev.event_id,
              chatId: ev.nostr_group_id,
              text: reactionText,
              isOwner: isOwnerPubkey(String(ev.from_pubkey).trim().toLowerCase()),
              isGroupChat: !isOneOnOneGroup(ev.nostr_group_id.toLowerCase()),
              wasMentioned: false,
              stateDir: baseStateDir,
              deliverText: async (responseText: string) => {
                sidecar.sendMessage(ev.nostr_group_id, responseText);
              },
              log: ctx.log,
            });
            return;
          }

          // Debug: if a call signal fails to parse in the sidecar, it will fall back to
          // `message_received` and the bot will treat it as plain text. Surface the raw
          // content (with basic redaction) so we can patch the sidecar parser.
          //
          // Note: we key off either explicit `pika.call` markers or "looks like JSON" to avoid
          // missing shape mismatches (e.g. JSON envelopes without expected substrings).
          if (
            ev.kind !== HYPERNOTE_ACTION_RESPONSE_KIND &&
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
          } else if (ev.kind === HYPERNOTE_ACTION_RESPONSE_KIND) {
            const actionResponse = parseHypernoteActionResponse(ev.content);
            if (actionResponse) {
              ev.content = formatHypernoteActionResponse(actionResponse);
            }
          }

          // Augment content with media attachment descriptions so the agent can see them
          let messageText = ev.content;
          if (ev.media && ev.media.length > 0) {
            const mediaLines = ev.media.map((m) => {
              const dims = m.width && m.height ? ` (${m.width}x${m.height})` : "";
              const localFile = m.local_path ? ` file://${m.local_path}` : "";
              return `[Attachment: ${m.filename} — ${m.mime_type}${dims}${localFile}]`;
            });
            const suffix = "\n" + mediaLines.join("\n");
            messageText = messageText ? messageText + suffix : mediaLines.join("\n");
          }

          try {
            const senderPk = String(ev.from_pubkey).trim().toLowerCase();
            const senderIsOwner = isOwnerPubkey(senderPk);
            const currentCfg = runtime.config.loadConfig();
            const groupId = ev.nostr_group_id.toLowerCase();
            const historyKey = `pikachat-openclaw:${resolved.accountId}:${groupId}`;

            // Determine if this is a DM group (1:1 with bot)
            const isDm = isOneOnOneGroup(groupId);
            // Multi-person groups use group flow (even for owners); DM groups route to main session
            const isGroupChat = !isDm;

            if (isGroupChat) {
              // GROUP CHAT FLOW — mention gating + history buffering
              const requireMention = resolveRequireMention(groupId, currentCfg);
              const wasMentioned = handle ? detectMention(messageText, handle.pubkey, handle.npub, currentCfg) : false;
              const isHypernoteActionResponse = ev.kind === HYPERNOTE_ACTION_RESPONSE_KIND;

              if (requireMention && !wasMentioned && !isHypernoteActionResponse) {
                // Not mentioned — buffer for context, don't dispatch.
                // Use sync resolveMemberName (returns cached name or npub) to
                // avoid the slow relay fetch just for history buffering.
                const senderName = resolveMemberName(senderPk, currentCfg);
                recordPendingHistory(historyKey, {
                  sender: senderName,
                  body: messageText,
                  timestamp: ev.created_at ? ev.created_at * 1000 : Date.now(),
                });
                ctx.log?.debug(
                  `[${resolved.accountId}] group message buffered (no mention) group=${ev.nostr_group_id} from=${senderPk}`,
                );
                return;
              }

              // Mentioned (or mention not required) — fire typing indicator
              // eagerly before the expensive profile fetch + agent dispatch.
              // Brief delay so it doesn't feel instantaneous / robotic.
              setTimeout(() => { sidecar.sendTyping(ev.nostr_group_id).catch(() => {}); }, 500);

              const pendingHistory = flushPendingHistory(historyKey);
              ctx.log?.info(
                `[${resolved.accountId}] group message dispatching (mentioned=${wasMentioned}) group=${ev.nostr_group_id} from=${senderPk} pendingHistory=${pendingHistory.length}`,
              );

              await dispatchInboundToAgent({
                runtime,
                accountId: resolved.accountId,
                senderId: ev.from_pubkey,
                eventId: ev.event_id,
                chatId: ev.nostr_group_id,
                text: messageText,
                isOwner: senderIsOwner,
                isGroupChat: true,
                wasMentioned: wasMentioned || isHypernoteActionResponse,
                inboundHistory: pendingHistory.length > 0 ? pendingHistory : undefined,
                groupName: groupNames.get(groupId),
                stateDir: baseStateDir,
                deliverText: async (responseText: string) => {
                  ctx.log?.info(
                    `[${resolved.accountId}] send group=${ev.nostr_group_id} len=${responseText.length}`,
                  );
                  sidecar.sendMessage(ev.nostr_group_id, responseText);
                },
                sendTyping: async () => {
                  await sidecar.sendTyping(ev.nostr_group_id).catch((err) => {
                    ctx.log?.debug?.(`[${resolved.accountId}] typing indicator failed group=${ev.nostr_group_id}: ${err}`);
                  });
                },
                log: ctx.log,
              });
            } else {
              // DM / OWNER FLOW — route to main session (existing behavior)
              // Fire typing eagerly before the expensive profile fetch + agent dispatch.
              // Brief delay so it doesn't feel instantaneous / robotic.
              setTimeout(() => { sidecar.sendTyping(ev.nostr_group_id).catch(() => {}); }, 500);
              await dispatchInboundToAgent({
                runtime,
                accountId: resolved.accountId,
                senderId: ev.from_pubkey,
                eventId: ev.event_id,
                chatId: ev.nostr_group_id,
                text: messageText,
                isOwner: senderIsOwner,
                isGroupChat: false,
                deliverText: async (responseText: string) => {
                  ctx.log?.info(
                    `[${resolved.accountId}] send dm=${ev.nostr_group_id} len=${responseText.length}`,
                  );
                  sidecar.sendMessage(ev.nostr_group_id, responseText);
                },
                sendTyping: async () => {
                  await sidecar.sendTyping(ev.nostr_group_id).catch((err) => {
                    ctx.log?.debug?.(`[${resolved.accountId}] typing indicator failed dm=${ev.nostr_group_id}: ${err}`);
                  });
                },
                log: ctx.log,
              });
            }
          } catch (err) {
            ctx.log?.error(
              `[${resolved.accountId}] dispatchInboundToAgent failed: ${err}`,
            );
          } finally {
            // Clean up decrypted media temp files after the agent has had time to read them
            if (ev.media && ev.media.length > 0) {
              const paths = ev.media.map((m) => m.local_path).filter(Boolean) as string[];
              if (paths.length > 0) {
                const timer = setTimeout(() => {
                  for (const p of paths) {
                    try { rmSync(p, { force: true }); } catch {}
                  }
                }, 5 * 60 * 1000); // 5 minutes
                (timer as any).unref?.();
              }
            }
          }
        }
      });

      // Return a long-lived Promise so the framework keeps the channel
      // as "running". Resolves when the sidecar exits or abort fires.
      return new Promise<void>((resolve) => {
        const finish = () => resolve();
        ctx.abortSignal.addEventListener("abort", () => {
          const handle = activeSidecars.get(resolved.accountId);
          if (handle) {
            activeSidecars.delete(resolved.accountId);
            void handle.sidecar.shutdown();
          }
          ctx.log?.info(`[${resolved.accountId}] pikachat sidecar stopped`);
          finish();
        }, { once: true });
        sidecar.waitForExit().then(() => {
          activeSidecars.delete(resolved.accountId);
          ctx.log?.info(`[${resolved.accountId}] pikachat sidecar exited`);
          finish();
        });
      });
    },
  },
};

// ---------------------------------------------------------------------------
// Agent tool factories — registered via api.registerTool() in index.ts.
// Each factory receives OpenClawPluginToolContext and uses sessionGroupMap
// (populated by dispatchInboundToAgent) to resolve the target group.
// ---------------------------------------------------------------------------

type ToolContext = {
  sessionKey?: string;
  agentAccountId?: string;
  [key: string]: unknown;
};

function resolveToolTarget(ctx: ToolContext): { handle: PikachatSidecarHandle; groupId: string } | null {
  const session = ctx.sessionKey ? sessionGroupMap.get(ctx.sessionKey) : null;
  if (!session) return null;
  const handle = activeSidecars.get(session.accountId);
  if (!handle) return null;
  return { handle, groupId: session.groupId };
}

export function createSendHypernoteToolFactory(): (ctx: ToolContext) => any {
  return (ctx) => {
    return {
      name: "send_hypernote",
      description: "Send interactive UI (hypernote) to the current chat. Use MDX components from the hypernote catalog.",
      parameters: {
        type: "object",
        properties: {
          content: { type: "string", description: "MDX content using hypernote components" },
          title: { type: "string", description: "Optional card title" },
          state: { type: "string", description: "Optional JSON state for interactive components" },
        },
        required: ["content"],
      },
      async execute(_id: string, params: { content: string; title?: string; state?: string }) {
        const target = resolveToolTarget(ctx);
        if (!target) return { content: [{ type: "text" as const, text: "Sidecar not running or no target group." }] };
        const result = await target.handle.sidecar.sendHypernote(target.groupId, params.content, {
          title: params.title,
          state: params.state,
        });
        const eventIdNote = result?.event_id ? ` [pikachat_event_id: ${result.event_id}]` : "";
        return { content: [{ type: "text" as const, text: `Hypernote sent.${eventIdNote}` }] };
      },
    };
  };
}

export function createSubmitHypernoteActionToolFactory(): (ctx: ToolContext) => any {
  return (ctx) => {
    return {
      name: "submit_hypernote_action",
      description: "Submit a hypernote action response (e.g. vote in a poll or interact with a form). Requires the event_id of the hypernote.",
      parameters: {
        type: "object",
        properties: {
          event_id: { type: "string", description: "Nostr event ID of the hypernote to respond to" },
          action: { type: "string", description: "Action name to submit (matches a SubmitButton action prop)" },
          form: { type: "object", description: "Form field values as key-value pairs" },
        },
        required: ["event_id", "action"],
      },
      async execute(_id: string, params: { event_id: string; action: string; form?: Record<string, string> }) {
        const target = resolveToolTarget(ctx);
        if (!target) return { content: [{ type: "text" as const, text: "Sidecar not running or no target group." }] };
        const result = await target.handle.sidecar.submitHypernoteAction(target.groupId, params.event_id, params.action, params.form ?? {});
        const eventIdNote = result?.event_id ? ` [pikachat_event_id: ${result.event_id}]` : "";
        return { content: [{ type: "text" as const, text: `Hypernote action submitted.${eventIdNote}` }] };
      },
    };
  };
}
