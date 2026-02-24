import {
  type RuntimeSnapshot,
  defaultRuntimeSnapshot,
  normalizeRuntimeSnapshot,
} from "./runtime_contract";
import { WasmRuntimeContractStateMachine } from "./runtime_wasm";
import { finalizeEvent, generateSecretKey, getPublicKey } from "nostr-tools/pure";

export interface Env {
  AGENTS: DurableObjectNamespace;
  AGENT_API_TOKEN?: string;
  PI_ADAPTER_BASE_URL?: string;
  PI_ADAPTER_TOKEN?: string;
  PI_ADAPTER_TIMEOUT_MS?: string;
}

type AgentBrain = "pi";

type ChatTurn = {
  role: "user" | "assistant";
  content: string;
  at_ms: number;
};

type RelayProbe = {
  relay: string;
  ok: boolean;
  status_code?: number;
  error?: string;
  checked_at_ms: number;
};

type RelaySession = {
  active: boolean;
  relay?: string;
  connected_at_ms?: number;
  rate_limited_until_ms?: number;
  last_req_at_ms?: number;
  last_event_at_ms?: number;
  last_notice?: string;
  inbound_events: number;
  notices: number;
  oks: number;
  errors: number;
  published_events: number;
  publish_failures: number;
  acked_events: number;
  inbound_applied_messages: number;
  auto_replies: number;
  duplicate_events_ignored: number;
  last_published_event_id?: string;
  last_ok_event_id?: string;
  last_ok_message?: string;
  pending_event_ids: string[];
  seen_event_ids: string[];
  seen_event_fingerprints: string[];
  subscription_id?: string;
};

type PendingRelayEvent = {
  id: string;
  frame_json: string;
  attempts: number;
  last_attempt_ms: number;
};

type AgentRecord = {
  schema_version: number;
  id: string;
  name: string;
  brain: AgentBrain;
  status: "booting" | "ready";
  created_at_ms: number;
  updated_at_ms: number;
  ready_at_ms: number;
  relay_urls: string[];
  bot_pubkey: string;
  relay_pubkey?: string;
  key_package_published_at_ms?: number;
  relay_probe?: RelayProbe;
  relay_session?: RelaySession;
  session_active_until_ms?: number;
  history: ChatTurn[];
  runtime_snapshot: RuntimeSnapshot;
  last_reply?: {
    at_ms: number;
    source?: "pi-adapter";
    latency_ms: number;
    error?: string;
  };
};

type ReplyGeneration = {
  reply: string;
  source: "pi-adapter";
};

type CreateAgentRequest = {
  id?: string;
  name?: string;
  brain?: string;
  relay_urls?: string[];
  bot_secret_key_hex?: string;
};

type RuntimeProcessWelcomeRequest = {
  group_id?: string;
  wrapper_event_id_hex?: string;
  welcome_event_json?: string;
};

type PiRpcEvent = {
  type?: string;
  success?: boolean;
  reply?: string;
  text?: string;
  delta?: string;
  message?: string;
  assistantMessageEvent?: {
    type?: string;
    delta?: string;
  };
  event?: unknown;
  data?: unknown;
  result?: {
    reply?: string;
  };
};

type RelayMlsEvent = {
  id: string;
  pubkey: string;
  content: string;
  tags: string[][];
  event_json: string;
};

type RelaySignedEvent = {
  id: string;
  pubkey: string;
  sig: string;
  kind: number;
  created_at: number;
  tags: unknown[];
  content: string;
};

type RuntimeEngine = {
  snapshot(): RuntimeSnapshot;
  initOrLoadIdentity(secretSeedHint?: string): {
    created: boolean;
    pubkey_hint: string;
    key_package_hint: string;
  };
  publishKeypackagePayload(): {
    pubkey_hint: string;
    key_package_hint: string;
    payload_json: string;
    tags_json: string;
    event_json: string;
    event_id: string;
  };
  processWelcome(groupId: string): {
    group_id: string;
    created_group: boolean;
    processed_welcomes: number;
    mls_group_id_hex: string;
    nostr_group_id_hex: string;
  };
  processWelcomeEventJson(
    groupId: string,
    wrapperEventIdHex: string,
    welcomeEventJson: string,
  ): {
    group_id: string;
    created_group: boolean;
    processed_welcomes: number;
    mls_group_id_hex: string;
    nostr_group_id_hex: string;
  };
  processGroupMessage(groupId: string, eventId: string, ciphertextB64: string): {
    group_id: string;
    event_id: string;
    applied_messages: number;
    message_kind: string;
    plaintext: string | null;
  };
  processGroupMessageEventJson(groupId: string, eventJson: string): {
    group_id: string;
    event_id: string;
    applied_messages: number;
    message_kind: string;
    plaintext: string | null;
  };
  createOutboundGroupMessage(groupId: string, plaintext: string): {
    group_id: string;
    event_id: string;
    sequence: number;
    ciphertext_b64: string;
    event_json: string;
  };
};

const DEFAULT_RELAY_URLS = [
  "wss://us-east.nostr.pikachat.org",
  "wss://eu.nostr.pikachat.org",
];
const BOOT_DELAY_MS = 2000;
const SESSION_ACTIVE_TTL_MS = 5 * 60 * 1000;
const RELAY_POLL_INTERVAL_MS = 30 * 1000;
const AGENT_RECORD_SCHEMA_VERSION = 1;
const RELAY_SECRET_STORAGE_KEY = "relay_secret_hex_v1";
const RELAY_PENDING_EVENTS_STORAGE_KEY = "relay_pending_events_v1";
const RELAY_PENDING_RETRY_INTERVAL_MS = 2000;
const RELAY_PENDING_MAX_ATTEMPTS = 5;
const RELAY_SEEN_EVENT_IDS_MAX = 96;
const RELAY_SEEN_EVENT_FINGERPRINTS_MAX = 96;
const NOSTR_KIND_MLS_KEY_PACKAGE = 443;
const NOSTR_KIND_MLS_GROUP_MESSAGE = 445;
const DEFAULT_PI_ADAPTER_TIMEOUT_MS = 15_000;
const RELAY_RATE_LIMIT_BACKOFF_MS = 10_000;

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

function hexToBytes(hex: string): Uint8Array {
  const normalized = hex.trim().toLowerCase();
  if (!/^[0-9a-f]+$/.test(normalized) || normalized.length % 2 !== 0) {
    throw new Error("invalid hex");
  }
  const out = new Uint8Array(normalized.length / 2);
  for (let i = 0; i < out.length; i += 1) {
    out[i] = Number.parseInt(normalized.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

function parseRelaySignedEvent(jsonText: string): RelaySignedEvent | null {
  try {
    const parsed = JSON.parse(jsonText) as Partial<RelaySignedEvent> | null;
    if (!parsed || typeof parsed !== "object") {
      return null;
    }
    const id = String(parsed.id ?? "").trim();
    const pubkey = String(parsed.pubkey ?? "").trim().toLowerCase();
    const sig = String(parsed.sig ?? "").trim();
    const kind = Number(parsed.kind);
    const createdAt = Number(parsed.created_at);
    const content = String(parsed.content ?? "");
    const tags = Array.isArray(parsed.tags) ? parsed.tags : [];
    if (
      !id ||
      !pubkey ||
      !sig ||
      !Number.isFinite(kind) ||
      !Number.isFinite(createdAt) ||
      typeof content !== "string"
    ) {
      return null;
    }
    return {
      id,
      pubkey,
      sig,
      kind: Math.floor(kind),
      created_at: Math.floor(createdAt),
      tags,
      content,
    };
  } catch {
    return null;
  }
}

function toNostrGroupTagValue(groupId: string): string {
  const trimmed = groupId.trim();
  if (!trimmed) {
    return "";
  }
  if (/^[0-9a-fA-F]+$/.test(trimmed) && trimmed.length % 2 === 0) {
    return trimmed.toLowerCase();
  }
  return bytesToHex(new TextEncoder().encode(trimmed));
}

function nowMs(): number {
  return Date.now();
}

function fnv1aHashHex(input: string): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < input.length; i += 1) {
    hash ^= input.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(16).padStart(8, "0");
}

function parseTimeoutMs(raw: string | undefined, fallbackMs: number): number {
  const parsed = Number(raw?.trim() ?? "");
  if (!Number.isFinite(parsed) || parsed <= 0) {
    return fallbackMs;
  }
  return Math.min(120_000, Math.max(100, Math.floor(parsed)));
}

function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data, null, 2), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
    },
  });
}

function sanitizeAgentId(raw?: string): string {
  const normalized = (raw ?? "")
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/^-+/, "")
    .replace(/-+$/, "")
    .slice(0, 64);
  return normalized || crypto.randomUUID();
}

function asBrain(value?: string): AgentBrain {
  if (value && value !== "pi") {
    throw new Error("workers demo supports brain=pi only");
  }
  return "pi";
}

function toPubkeyHint(agentId: string): string {
  const normalized = agentId.trim().toLowerCase() || "agent";
  const hexAlphabet = "0123456789abcdef";
  let out = "";
  for (let i = 0; i < normalized.length && out.length < 64; i += 1) {
    const code = normalized.charCodeAt(i);
    out += hexAlphabet[code % 16];
    out += hexAlphabet[(code >> 4) % 16];
  }
  if (out.length < 64) {
    out = `${out}${"0".repeat(64)}`;
  }
  return out.slice(0, 64);
}

function normalizeRelays(input?: string[]): string[] {
  if (!Array.isArray(input) || input.length === 0) {
    return [...DEFAULT_RELAY_URLS];
  }

  const seen = new Set<string>();
  const out: string[] = [];
  for (const raw of input) {
    const relay = String(raw ?? "").trim();
    if (!relay || seen.has(relay)) {
      continue;
    }
    seen.add(relay);
    out.push(relay);
  }

  return out.length > 0 ? out : [...DEFAULT_RELAY_URLS];
}

function unauthorized(): Response {
  return new Response("unauthorized", {
    status: 401,
    headers: { "www-authenticate": "Bearer" },
  });
}

function isAuthorized(req: Request, env: Env): boolean {
  const expected = env.AGENT_API_TOKEN?.trim();
  if (!expected) {
    return true;
  }

  const auth = req.headers.get("authorization") ?? "";
  if (auth.toLowerCase().startsWith("bearer ")) {
    const token = auth.slice("bearer ".length).trim();
    return token === expected;
  }

  const apiKey = req.headers.get("x-api-key")?.trim();
  return apiKey === expected;
}

async function probeRelay(relayUrl: string): Promise<RelayProbe> {
  const checked_at_ms = nowMs();
  const normalizedBase = relayUrl
    .replace(/^wss:\/\//i, "https://")
    .replace(/^ws:\/\//i, "http://");

  const candidates: Array<{ url: string; method: "HEAD" | "GET" }> = [
    { url: normalizedBase, method: "HEAD" },
  ];
  if (!normalizedBase.endsWith("/health")) {
    const healthUrl = normalizedBase.endsWith("/")
      ? `${normalizedBase}health`
      : `${normalizedBase}/health`;
    candidates.push({ url: healthUrl, method: "GET" });
  }

  let lastStatus: number | undefined;
  let lastError: string | undefined;
  for (const candidate of candidates) {
    try {
      const res = await fetch(candidate.url, {
        method: candidate.method,
        redirect: "follow",
        cf: { cacheTtl: 0, cacheEverything: false },
      });
      lastStatus = res.status;
      if (res.ok) {
        return {
          relay: relayUrl,
          ok: true,
          status_code: res.status,
          checked_at_ms,
        };
      }
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
  }

  return {
    relay: relayUrl,
    ok: false,
    status_code: lastStatus,
    error: lastError,
    checked_at_ms,
  };
}

function parsePath(pathname: string): string[] {
  return pathname.split("/").filter((segment) => segment.length > 0);
}

async function parseJson<T>(req: Request): Promise<T> {
  return (await req.json()) as T;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (!isAuthorized(request, env)) {
      return unauthorized();
    }

    const url = new URL(request.url);
    const parts = parsePath(url.pathname);

    if (request.method === "GET" && parts.length === 1 && parts[0] === "health") {
      return json({ ok: true, service: "pika-workers-agent-demo" });
    }

    if (request.method === "POST" && parts.length === 1 && parts[0] === "agents") {
      const body = await parseJson<CreateAgentRequest>(request);
      const agentId = sanitizeAgentId(body.id ?? body.name);
      const name = (body.name ?? agentId).trim() || agentId;
      const relayUrls = normalizeRelays(body.relay_urls);
      let brain: AgentBrain;
      try {
        brain = asBrain(body.brain);
      } catch (error) {
        return json({ error: error instanceof Error ? error.message : String(error) }, 400);
      }

      const id = env.AGENTS.idFromName(agentId);
      const stub = env.AGENTS.get(id);
      const initResp = await stub.fetch("https://agent.internal/init", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          id: agentId,
          name,
          brain,
          relay_urls: relayUrls,
          bot_secret_key_hex: body.bot_secret_key_hex,
        }),
      });

      if (!initResp.ok) {
        const errText = await initResp.text();
        return json({ error: errText || "failed to initialize agent" }, initResp.status);
      }

      return new Response(await initResp.text(), {
        status: 201,
        headers: { "content-type": "application/json; charset=utf-8" },
      });
    }

    if (parts.length === 2 && parts[0] === "agents" && request.method === "GET") {
      const agentId = sanitizeAgentId(parts[1]);
      const id = env.AGENTS.idFromName(agentId);
      const stub = env.AGENTS.get(id);
      return stub.fetch("https://agent.internal/status", { method: "GET" });
    }

    if (
      parts.length === 4 &&
      parts[0] === "agents" &&
      parts[2] === "runtime" &&
      parts[3] === "process-welcome" &&
      request.method === "POST"
    ) {
      const agentId = sanitizeAgentId(parts[1]);
      const id = env.AGENTS.idFromName(agentId);
      const stub = env.AGENTS.get(id);
      return stub.fetch("https://agent.internal/runtime/process-welcome", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: await request.text(),
      });
    }

    return json({ error: "not found" }, 404);
  },
};

export class AgentDurableObject {
  private state: DurableObjectState;
  private record: AgentRecord | null = null;
  private relaySocket: WebSocket | null = null;
  private relaySocketRelay: string | null = null;
  private relaySubscriptionId: string | null = null;
  private relayMessageQueue: Promise<void> = Promise.resolve();

  constructor(state: DurableObjectState, env: Env) {
    this.state = state;
    this.env = env;
  }

  private env: Env;

  async fetch(request: Request): Promise<Response> {
    try {
      const url = new URL(request.url);

      if (url.pathname === "/init" && request.method === "POST") {
        return this.handleInit(request);
      }

      if (url.pathname === "/status" && request.method === "GET") {
        return this.handleStatus();
      }

      if (url.pathname === "/runtime/process-welcome" && request.method === "POST") {
        return this.handleRuntimeProcessWelcome(request);
      }

      return json({ error: "not found" }, 404);
    } catch (error) {
      console.error(
        JSON.stringify({
          event: "agent.do.error",
          error: error instanceof Error ? error.message : String(error),
        }),
      );
      return json(
        { error: error instanceof Error ? error.message : "internal error" },
        500,
      );
    }
  }

  async alarm(): Promise<void> {
    const existing = await this.loadRecord();
    if (!existing) {
      return;
    }

    await this.maybeMarkReady(existing);
    const now = nowMs();
    if (!existing.session_active_until_ms || existing.session_active_until_ms <= now) {
      this.closeRelaySocket("session-inactive");
      const session = this.ensureRelaySessionState(existing);
      if (session.active) {
        session.active = false;
        existing.updated_at_ms = now;
        await this.saveRecord(existing);
      }
      return;
    }

    const hasRelaySocket = this.ensureRelaySocket(existing);
    if (!hasRelaySocket) {
      await this.refreshRelayProbe(existing);
    } else {
      await this.flushPendingRelayEvents(existing);
      const sent = this.sendRelayObserveRequest(existing);
      if (sent) {
        existing.updated_at_ms = now;
        await this.saveRecord(existing);
      }
      console.log(
        JSON.stringify({
          event: "agent.relay.ws.active",
          agent_id: existing.id,
          relay: this.relaySocketRelay,
        }),
      );
    }

    await this.scheduleRelayAlarm(existing);
  }

  private async loadRecord(): Promise<AgentRecord | null> {
    if (this.record) {
      return this.record;
    }
    const persisted = await this.state.storage.get<AgentRecord>("agent_record");
    if (persisted) {
      const schemaVersion = persisted.schema_version ?? 0;
      if (schemaVersion > AGENT_RECORD_SCHEMA_VERSION) {
        throw new Error(
          `agent record schema too new: expected <= ${AGENT_RECORD_SCHEMA_VERSION}, got ${schemaVersion}`,
        );
      }
      if (schemaVersion < AGENT_RECORD_SCHEMA_VERSION) {
        persisted.schema_version = AGENT_RECORD_SCHEMA_VERSION;
      }

      if (!persisted.runtime_snapshot) {
        persisted.runtime_snapshot = defaultRuntimeSnapshot();
      } else {
        persisted.runtime_snapshot = normalizeRuntimeSnapshot(
          persisted.runtime_snapshot as RuntimeSnapshot,
        );
      }
      if (!persisted.relay_session) {
        persisted.relay_session = {
          active: false,
          inbound_events: 0,
          notices: 0,
          oks: 0,
          errors: 0,
          published_events: 0,
          publish_failures: 0,
          acked_events: 0,
          inbound_applied_messages: 0,
          auto_replies: 0,
          duplicate_events_ignored: 0,
          pending_event_ids: [],
          seen_event_ids: [],
          seen_event_fingerprints: [],
        };
      } else {
        persisted.relay_session.inbound_events = persisted.relay_session.inbound_events ?? 0;
        persisted.relay_session.notices = persisted.relay_session.notices ?? 0;
        persisted.relay_session.oks = persisted.relay_session.oks ?? 0;
        persisted.relay_session.errors = persisted.relay_session.errors ?? 0;
        persisted.relay_session.published_events =
          persisted.relay_session.published_events ?? 0;
        persisted.relay_session.publish_failures =
          persisted.relay_session.publish_failures ?? 0;
        persisted.relay_session.acked_events = persisted.relay_session.acked_events ?? 0;
        persisted.relay_session.inbound_applied_messages =
          persisted.relay_session.inbound_applied_messages ?? 0;
        persisted.relay_session.auto_replies = persisted.relay_session.auto_replies ?? 0;
        persisted.relay_session.duplicate_events_ignored =
          persisted.relay_session.duplicate_events_ignored ?? 0;
        persisted.relay_session.pending_event_ids =
          persisted.relay_session.pending_event_ids ?? [];
        persisted.relay_session.seen_event_ids = (
          persisted.relay_session.seen_event_ids ?? []
        ).slice(-RELAY_SEEN_EVENT_IDS_MAX);
        persisted.relay_session.seen_event_fingerprints = (
          persisted.relay_session.seen_event_fingerprints ?? []
        ).slice(-RELAY_SEEN_EVENT_FINGERPRINTS_MAX);
        const rateLimitUntil = Number(persisted.relay_session.rate_limited_until_ms);
        persisted.relay_session.rate_limited_until_ms =
          Number.isFinite(rateLimitUntil) && rateLimitUntil > 0
            ? Math.floor(rateLimitUntil)
            : undefined;
        persisted.relay_session.active = persisted.relay_session.active ?? false;
      }
      await this.state.storage.put("agent_record", persisted);
      this.record = persisted;
      return persisted;
    }
    return null;
  }

  private async saveRecord(record: AgentRecord): Promise<void> {
    this.record = record;
    await this.state.storage.put("agent_record", record);
  }

  private markSessionActive(record: AgentRecord): void {
    record.session_active_until_ms = nowMs() + SESSION_ACTIVE_TTL_MS;
  }

  private async scheduleRelayAlarm(record: AgentRecord): Promise<void> {
    if (!record.session_active_until_ms) {
      return;
    }
    const now = nowMs();
    const until = record.session_active_until_ms;
    if (until <= now) {
      return;
    }
    const nextAt = Math.min(until, now + RELAY_POLL_INTERVAL_MS);
    const current = await this.state.storage.getAlarm();
    if (!current || current > nextAt || current < now) {
      await this.state.storage.setAlarm(nextAt);
    }
  }

  private ensureRelaySessionState(record: AgentRecord): RelaySession {
    if (!record.relay_session) {
      record.relay_session = {
        active: false,
        relay: record.relay_urls[0],
        inbound_events: 0,
        notices: 0,
        oks: 0,
        errors: 0,
        published_events: 0,
        publish_failures: 0,
        acked_events: 0,
        inbound_applied_messages: 0,
        auto_replies: 0,
        duplicate_events_ignored: 0,
        pending_event_ids: [],
        seen_event_ids: [],
        seen_event_fingerprints: [],
      };
    }
    return record.relay_session;
  }

  private runtimeGroupIds(record: AgentRecord): string[] {
    const groups = record.runtime_snapshot?.groups ?? {};
    return Object.keys(groups).filter((groupId) => groupId.trim().length > 0);
  }

  private runtimeGroupTagValueForGroup(record: AgentRecord, groupId: string): string {
    const groupState = record.runtime_snapshot?.groups?.[groupId];
    const fromSnapshot = String(groupState?.nostr_group_id_hex ?? "")
      .trim()
      .toLowerCase();
    if (/^[0-9a-f]+$/.test(fromSnapshot) && fromSnapshot.length % 2 === 0) {
      return fromSnapshot;
    }
    return toNostrGroupTagValue(groupId);
  }

  private runtimeGroupTagValues(record: AgentRecord): string[] {
    const out: string[] = [];
    const seen = new Set<string>();
    for (const groupId of this.runtimeGroupIds(record)) {
      const tagValue = this.runtimeGroupTagValueForGroup(record, groupId);
      if (!tagValue || seen.has(tagValue)) {
        continue;
      }
      seen.add(tagValue);
      out.push(tagValue);
    }
    return out;
  }

  private extractRelayGroupTagValue(tags: string[][]): string | null {
    for (const tag of tags) {
      if (!Array.isArray(tag) || tag.length < 2) {
        continue;
      }
      if (String(tag[0] ?? "").trim().toLowerCase() !== "h") {
        continue;
      }
      const value = String(tag[1] ?? "").trim().toLowerCase();
      if (value) {
        return value;
      }
    }
    return null;
  }

  private extractRelayRoleTagValue(tags: string[][]): string | null {
    for (const tag of tags) {
      if (!Array.isArray(tag) || tag.length < 2) {
        continue;
      }
      if (String(tag[0] ?? "").trim().toLowerCase() !== "role") {
        continue;
      }
      const value = String(tag[1] ?? "").trim().toLowerCase();
      if (value) {
        return value;
      }
    }
    return null;
  }

  private resolveRuntimeGroupIdForRelayEvent(record: AgentRecord, tags: string[][]): string | null {
    const tagValue = this.extractRelayGroupTagValue(tags);
    if (!tagValue) {
      return null;
    }
    for (const groupId of this.runtimeGroupIds(record)) {
      if (this.runtimeGroupTagValueForGroup(record, groupId) === tagValue) {
        return groupId;
      }
    }
    return null;
  }

  private async patchRelaySession(
    agentId: string,
    mutate: (session: RelaySession, record: AgentRecord) => void,
  ): Promise<void> {
    const existing = await this.loadRecord();
    if (!existing || existing.id !== agentId) {
      return;
    }
    const session = this.ensureRelaySessionState(existing);
    mutate(session, existing);
    existing.updated_at_ms = nowMs();
    await this.saveRecord(existing);
  }

  private async loadPendingRelayEvents(): Promise<PendingRelayEvent[]> {
    const pending = await this.state.storage.get<PendingRelayEvent[]>(
      RELAY_PENDING_EVENTS_STORAGE_KEY,
    );
    if (!Array.isArray(pending)) {
      return [];
    }
    return pending.filter(
      (item) =>
        !!item &&
        typeof item.id === "string" &&
        typeof item.frame_json === "string" &&
        Number.isFinite(item.attempts) &&
        Number.isFinite(item.last_attempt_ms),
    );
  }

  private async savePendingRelayEvents(events: PendingRelayEvent[]): Promise<void> {
    await this.state.storage.put(RELAY_PENDING_EVENTS_STORAGE_KEY, events);
  }

  private async queuePendingRelayEvent(item: PendingRelayEvent): Promise<void> {
    const pending = await this.loadPendingRelayEvents();
    const deduped = pending.filter((event) => event.id !== item.id);
    deduped.push(item);
    await this.savePendingRelayEvents(deduped.slice(-64));
  }

  private async ackPendingRelayEvent(eventId: string): Promise<void> {
    const normalized = eventId.trim();
    if (!normalized) {
      return;
    }
    const pending = await this.loadPendingRelayEvents();
    const next = pending.filter((event) => event.id !== normalized);
    if (next.length !== pending.length) {
      await this.savePendingRelayEvents(next);
    }
  }

  private async flushPendingRelayEvents(record: AgentRecord): Promise<void> {
    if (!this.relaySocket || this.relaySocket.readyState !== WebSocket.OPEN) {
      return;
    }

    const session = this.ensureRelaySessionState(record);
    const pending = await this.loadPendingRelayEvents();
    if (pending.length === 0) {
      session.pending_event_ids = [];
      return;
    }

    const now = nowMs();
    const next: PendingRelayEvent[] = [];
    let resent = 0;
    let dropped = 0;

    for (const item of pending) {
      if (item.attempts >= RELAY_PENDING_MAX_ATTEMPTS) {
        dropped += 1;
        continue;
      }
      if (now - item.last_attempt_ms < RELAY_PENDING_RETRY_INTERVAL_MS) {
        next.push(item);
        continue;
      }
      try {
        this.relaySocket.send(item.frame_json);
        resent += 1;
        next.push({
          ...item,
          attempts: item.attempts + 1,
          last_attempt_ms: now,
        });
      } catch {
        next.push(item);
        break;
      }
    }

    await this.savePendingRelayEvents(next);
    session.pending_event_ids = next.map((item) => item.id).slice(-32);
    if (dropped > 0) {
      session.publish_failures += dropped;
      session.last_notice = `dropped ${dropped} pending relay events after retries`;
    } else if (resent > 0) {
      session.last_notice = `resent ${resent} pending relay events`;
    }
  }

  private async ensureRelayIdentity(record: AgentRecord): Promise<{
    secretKeyBytes: Uint8Array;
    pubkey: string;
  }> {
    let secretKeyBytes: Uint8Array | null = null;
    let pubkey = "";

    const existingSecretHex = await this.state.storage.get<string>(RELAY_SECRET_STORAGE_KEY);
    if (typeof existingSecretHex === "string" && existingSecretHex.length > 0) {
      try {
        secretKeyBytes = hexToBytes(existingSecretHex);
        pubkey = getPublicKey(secretKeyBytes);
      } catch {
        secretKeyBytes = null;
      }
    }

    if (!secretKeyBytes) {
      for (let attempts = 0; attempts < 8; attempts += 1) {
        const candidate = generateSecretKey();
        try {
          const candidatePubkey = getPublicKey(candidate);
          await this.state.storage.put(RELAY_SECRET_STORAGE_KEY, bytesToHex(candidate));
          secretKeyBytes = candidate;
          pubkey = candidatePubkey;
          break;
        } catch {
          // retry
        }
      }
    }

    if (!secretKeyBytes || !pubkey) {
      throw new Error("failed to initialize relay signing identity");
    }

    if (record.relay_pubkey !== pubkey) {
      record.relay_pubkey = pubkey;
    }
    if (record.bot_pubkey !== pubkey) {
      record.bot_pubkey = pubkey;
    }
    return { secretKeyBytes, pubkey };
  }

  private async ensureRuntimeIdentity(
    record: AgentRecord,
    runtime: RuntimeEngine,
  ): Promise<void> {
    const relayIdentity = await this.ensureRelayIdentity(record);
    runtime.initOrLoadIdentity(bytesToHex(relayIdentity.secretKeyBytes));
  }

  private async waitForRelaySocketOpen(timeoutMs = 1200): Promise<boolean> {
    if (this.relaySocket && this.relaySocket.readyState === WebSocket.OPEN) {
      return true;
    }
    const start = nowMs();
    while (nowMs() - start < timeoutMs) {
      if (this.relaySocket && this.relaySocket.readyState === WebSocket.OPEN) {
        return true;
      }
      if (!this.relaySocket || this.relaySocket.readyState === WebSocket.CLOSED) {
        return false;
      }
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    return !!this.relaySocket && this.relaySocket.readyState === WebSocket.OPEN;
  }

  private async buildMlsRelayPublishPayload(
    record: AgentRecord,
    plaintext: string,
  ): Promise<{ groupTagValue: string; ciphertextB64: string; runtimeEventId: string } | null> {
    const trimmed = plaintext.trim();
    if (!trimmed) {
      return null;
    }

    const preferredGroupId = this.runtimeGroupIds(record)[0];
    if (!preferredGroupId) {
      return null;
    }

    try {
      const runtime = await this.buildRuntimeMachine(record);
      await this.ensureRuntimeIdentity(record, runtime);
      const outbound = runtime.createOutboundGroupMessage(preferredGroupId, trimmed);
      record.runtime_snapshot = runtime.snapshot();

      let groupTagValue = this.runtimeGroupTagValueForGroup(record, outbound.group_id);
      const runtimeEvent = parseRelaySignedEvent(outbound.event_json);
      if (runtimeEvent) {
        const runtimeTags = runtimeEvent.tags;
        if (Array.isArray(runtimeTags)) {
          for (const rawTag of runtimeTags) {
            if (!Array.isArray(rawTag) || rawTag.length < 2) {
              continue;
            }
            if (String(rawTag[0] ?? "").trim().toLowerCase() !== "h") {
              continue;
            }
            const candidate = String(rawTag[1] ?? "").trim().toLowerCase();
            if (candidate) {
              groupTagValue = candidate;
              break;
            }
          }
        }
      }
      if (!groupTagValue) {
        return null;
      }
      return {
        groupTagValue,
        ciphertextB64: outbound.ciphertext_b64,
        runtimeEventId: outbound.event_id,
      };
    } catch (error) {
      console.warn(
        JSON.stringify({
          event: "agent.relay.publish_payload.error",
          agent_id: record.id,
          error: error instanceof Error ? error.message : String(error),
        }),
      );
      return null;
    }
  }

  private async publishRelayKeyPackage(record: AgentRecord): Promise<boolean> {
    const session = this.ensureRelaySessionState(record);
    const now = nowMs();
    if (
      session.rate_limited_until_ms &&
      Number.isFinite(session.rate_limited_until_ms) &&
      session.rate_limited_until_ms > now
    ) {
      const waitMs = session.rate_limited_until_ms - now;
      session.last_notice = `keypackage publish paused (${waitMs}ms remaining after relay rate-limit)`;
      return false;
    }

    const hasSocket = this.ensureRelaySocket(record);
    if (!hasSocket) {
      session.publish_failures += 1;
      session.last_notice = "keypackage publish skipped: relay websocket unavailable";
      return false;
    }

    const open = await this.waitForRelaySocketOpen();
    if (!open || !this.relaySocket || this.relaySocket.readyState !== WebSocket.OPEN) {
      session.publish_failures += 1;
      session.last_notice = "keypackage publish skipped: relay websocket not open";
      return false;
    }

    let keypackagePayload: {
      payload_json: string;
      tags_json: string;
      event_json: string;
    };
    try {
      const runtime = await this.buildRuntimeMachine(record);
      await this.ensureRuntimeIdentity(record, runtime);
      const keypackage = runtime.publishKeypackagePayload();
      keypackagePayload = {
        payload_json: keypackage.payload_json,
        tags_json: keypackage.tags_json,
        event_json: keypackage.event_json,
      };
      record.runtime_snapshot = runtime.snapshot();
    } catch (error) {
      session.publish_failures += 1;
      session.last_notice = `keypackage publish skipped: ${
        error instanceof Error ? error.message : String(error)
      }`.slice(0, 300);
      return false;
    }

    let relayIdentity: { secretKeyBytes: Uint8Array; pubkey: string };
    try {
      relayIdentity = await this.ensureRelayIdentity(record);
    } catch (error) {
      session.publish_failures += 1;
      session.last_notice = `keypackage publish skipped: ${
        error instanceof Error ? error.message : String(error)
      }`.slice(0, 300);
      return false;
    }

    let tags: string[][] = [];
    try {
      const parsedTags = JSON.parse(keypackagePayload.tags_json) as unknown;
      if (Array.isArray(parsedTags)) {
        tags = parsedTags
          .filter((rawTag): rawTag is unknown[] => Array.isArray(rawTag))
          .map((rawTag) => rawTag.map((value) => String(value ?? "")))
          .filter((tag) => {
            if (tag.length < 2) {
              return false;
            }
            const tagKey = tag[0].trim();
            if (!tagKey) {
              return false;
            }
            // NIP-70 protected tags are commonly rejected on publish for kind 443.
            return tagKey !== "-";
          });
      }
    } catch {
      // ignore malformed tags payload and continue with empty tags
    }
    const event: RelaySignedEvent = finalizeEvent(
      {
        kind: NOSTR_KIND_MLS_KEY_PACKAGE,
        created_at: Math.floor(nowMs() / 1000),
        tags,
        content: keypackagePayload.payload_json,
      },
      relayIdentity.secretKeyBytes,
    );
    const eventFrameJson = JSON.stringify(["EVENT", event]);

    try {
      this.relaySocket.send(eventFrameJson);
      await this.queuePendingRelayEvent({
        id: event.id,
        frame_json: eventFrameJson,
        attempts: 1,
        last_attempt_ms: nowMs(),
      });
      session.published_events += 1;
      session.last_published_event_id = event.id;
      session.pending_event_ids = [...session.pending_event_ids, event.id].slice(-32);
      session.last_notice = `published kind ${NOSTR_KIND_MLS_KEY_PACKAGE} keypackage (runtime) ${event.id.slice(0, 12)}`;
      return true;
    } catch (error) {
      session.publish_failures += 1;
      session.last_notice = `keypackage publish failed: ${
        error instanceof Error ? error.message : String(error)
      }`.slice(0, 300);
      return false;
    }
  }

  private async publishSignedRelayEvent(
    record: AgentRecord,
    role: "user" | "assistant",
    content: string,
  ): Promise<void> {
    const session = this.ensureRelaySessionState(record);
    const now = nowMs();
    if (
      session.rate_limited_until_ms &&
      Number.isFinite(session.rate_limited_until_ms) &&
      session.rate_limited_until_ms > now
    ) {
      const waitMs = session.rate_limited_until_ms - now;
      session.last_notice = `publish paused (${waitMs}ms remaining after relay rate-limit)`;
      return;
    }

    const hasSocket = this.ensureRelaySocket(record);
    if (!hasSocket) {
      session.publish_failures += 1;
      session.last_notice = "publish skipped: relay websocket unavailable";
      return;
    }
    const open = await this.waitForRelaySocketOpen();
    if (!open || !this.relaySocket || this.relaySocket.readyState !== WebSocket.OPEN) {
      session.publish_failures += 1;
      session.last_notice = "publish skipped: relay websocket not open";
      return;
    }

    const payload = await this.buildMlsRelayPublishPayload(record, content);
    if (!payload) {
      session.publish_failures += 1;
      session.last_notice = "publish skipped: unable to build mls relay payload";
      return;
    }

    let relayIdentity: { secretKeyBytes: Uint8Array; pubkey: string };
    try {
      relayIdentity = await this.ensureRelayIdentity(record);
    } catch (error) {
      session.publish_failures += 1;
      session.last_notice = `publish skipped: ${
        error instanceof Error ? error.message : String(error)
      }`.slice(0, 300);
      return;
    }

    const event = finalizeEvent(
      {
        kind: NOSTR_KIND_MLS_GROUP_MESSAGE,
        created_at: Math.floor(nowMs() / 1000),
        tags: [
          ["h", payload.groupTagValue],
          ["role", role],
        ],
        content: payload.ciphertextB64,
      },
      relayIdentity.secretKeyBytes,
    );
    const eventFrameJson = JSON.stringify(["EVENT", event]);

    try {
      this.relaySocket.send(eventFrameJson);
      await this.queuePendingRelayEvent({
        id: event.id,
        frame_json: eventFrameJson,
        attempts: 1,
        last_attempt_ms: nowMs(),
      });
      session.published_events += 1;
      session.last_published_event_id = event.id;
      session.pending_event_ids = [...session.pending_event_ids, event.id].slice(-32);
    } catch (error) {
      session.publish_failures += 1;
      session.last_notice = `publish failed: ${
        error instanceof Error ? error.message : String(error)
      }`.slice(0, 300);
      return;
    }

    session.last_notice = `published kind ${NOSTR_KIND_MLS_GROUP_MESSAGE} (${role}) ${event.id.slice(
      0,
      12,
    )} runtime=${payload.runtimeEventId}`;
  }

  private sendRelayObserveRequest(record: AgentRecord): boolean {
    if (!this.relaySocket || this.relaySocket.readyState !== WebSocket.OPEN) {
      return false;
    }

    const session = this.ensureRelaySessionState(record);
    const groupTagValues = this.runtimeGroupTagValues(record);
    if (groupTagValues.length === 0) {
      session.last_notice = "observe skipped: runtime has no joined groups";
      return false;
    }
    const now = nowMs();
    const since =
      session.last_event_at_ms && session.last_event_at_ms > 0
        ? Math.max(0, Math.floor(session.last_event_at_ms / 1000) - 1)
        : Math.floor((now - 2 * 60 * 1000) / 1000);
    const subId = `pika-${record.id}-${Math.floor(now / 1000)}-${Math.floor(
      Math.random() * 10000,
    )}`;

    try {
      if (this.relaySubscriptionId) {
        this.relaySocket.send(JSON.stringify(["CLOSE", this.relaySubscriptionId]));
      }
      const filter: Record<string, unknown> = {
        kinds: [NOSTR_KIND_MLS_GROUP_MESSAGE],
        since,
        limit: 20,
      };
      if (groupTagValues.length > 0) {
        filter["#h"] = groupTagValues;
      }
      this.relaySocket.send(
        JSON.stringify([
          "REQ",
          subId,
          filter,
        ]),
      );
    } catch (error) {
      session.errors += 1;
      session.last_notice =
        error instanceof Error ? `send failed: ${error.message}` : "send failed";
      return false;
    }

    this.relaySubscriptionId = subId;
    session.subscription_id = subId;
    session.last_req_at_ms = now;
    return true;
  }

  private async handleInboundRelayEvent(agentId: string, relayEvent: RelayMlsEvent): Promise<void> {
    const existing = await this.loadRecord();
    if (!existing || existing.id !== agentId) {
      return;
    }
    await this.maybeMarkReady(existing);
    if (existing.status !== "ready") {
      return;
    }

    const ownPubkey = String(existing.relay_pubkey ?? "").trim().toLowerCase();
    if (ownPubkey && relayEvent.pubkey.trim().toLowerCase() === ownPubkey) {
      return;
    }

    const runtimeGroupId = this.resolveRuntimeGroupIdForRelayEvent(existing, relayEvent.tags);
    if (!runtimeGroupId) {
      return;
    }
    let runtimeApplied = false;
    let inboundPlaintext: string | null = null;
    try {
      const runtime = await this.buildRuntimeMachine(existing);
      await this.ensureRuntimeIdentity(existing, runtime);
      const processResult = runtime.processGroupMessageEventJson(
        runtimeGroupId,
        relayEvent.event_json,
      );
      existing.runtime_snapshot = runtime.snapshot();
      runtimeApplied = true;
      if (processResult.message_kind === "application") {
        const candidate = String(processResult.plaintext ?? "").trim();
        inboundPlaintext = candidate || null;
      }
    } catch (error) {
      console.warn(
        JSON.stringify({
          event: "agent.relay.inbound.apply.error",
          agent_id: existing.id,
          relay_event_id: relayEvent.id,
          error: error instanceof Error ? error.message : String(error),
        }),
      );
    }

    const session = this.ensureRelaySessionState(existing);
    if (runtimeApplied) {
      session.inbound_applied_messages += 1;
    }
    if (this.extractRelayRoleTagValue(relayEvent.tags) === "assistant") {
      existing.updated_at_ms = nowMs();
      await this.saveRecord(existing);
      return;
    }

    const decodedContent = inboundPlaintext;
    if (!decodedContent) {
      existing.updated_at_ms = nowMs();
      await this.saveRecord(existing);
      return;
    }

    let replyResult: ReplyGeneration;
    const replyStartedAt = nowMs();
    try {
      replyResult = await this.generateReply(existing, decodedContent);
    } catch (error) {
      existing.last_reply = {
        at_ms: nowMs(),
        latency_ms: nowMs() - replyStartedAt,
        error: error instanceof Error ? error.message : String(error),
      };
      existing.updated_at_ms = nowMs();
      await this.saveRecord(existing);
      return;
    }

    const replyLatencyMs = nowMs() - replyStartedAt;
    existing.history.push({ role: "user", content: decodedContent, at_ms: nowMs() });
    existing.history.push({
      role: "assistant",
      content: replyResult.reply,
      at_ms: nowMs(),
    });
    session.auto_replies += 1;
    this.markSessionActive(existing);
    const hasRelaySocket = this.ensureRelaySocket(existing);
    if (hasRelaySocket) {
      this.sendRelayObserveRequest(existing);
    }
    await this.publishSignedRelayEvent(existing, "assistant", replyResult.reply);
    existing.last_reply = {
      at_ms: nowMs(),
      source: replyResult.source,
      latency_ms: replyLatencyMs,
    };
    existing.updated_at_ms = nowMs();
    await this.saveRecord(existing);
    await this.scheduleRelayAlarm(existing);
  }

  private async handleRelaySocketMessage(agentId: string, rawData: unknown): Promise<void> {
    let text = "";
    if (typeof rawData === "string") {
      text = rawData;
    } else if (rawData instanceof ArrayBuffer) {
      text = new TextDecoder().decode(rawData);
    }
    if (!text.trim()) {
      return;
    }

    let parsed: unknown;
    try {
      parsed = JSON.parse(text);
    } catch {
      return;
    }
    if (!Array.isArray(parsed) || parsed.length === 0) {
      return;
    }

    const kind = parsed[0];
    let ackEventId: string | null = null;
    let rejectEventId: string | null = null;
    let inboundRelayEvent: RelayMlsEvent | null = null;
    await this.patchRelaySession(agentId, (session) => {
      if (kind === "EVENT") {
        const eventObj = parsed[2] as
          | {
              id?: unknown;
              pubkey?: unknown;
              kind?: unknown;
              created_at?: unknown;
              content?: unknown;
              tags?: unknown;
            }
          | undefined;
        const eventKind = Number((eventObj as { kind?: unknown } | undefined)?.kind);
        if (Number.isFinite(eventKind) && eventKind !== NOSTR_KIND_MLS_GROUP_MESSAGE) {
          return;
        }
        session.inbound_events += 1;
        const createdAt = Number(eventObj?.created_at);
        if (Number.isFinite(createdAt) && createdAt > 0) {
          session.last_event_at_ms = Math.floor(createdAt * 1000);
        } else {
          session.last_event_at_ms = nowMs();
        }
        const eventId = String(eventObj?.id ?? "").trim();
        const pubkey = String(eventObj?.pubkey ?? "").trim();
        const content = String(eventObj?.content ?? "").trim();
        const seenIds = new Set(
          (session.seen_event_ids ?? []).map((id) => String(id ?? "").trim()).filter(Boolean),
        );
        if (eventId) {
          if (seenIds.has(eventId)) {
            session.duplicate_events_ignored += 1;
            return;
          }
          session.seen_event_ids = [...seenIds, eventId].slice(-RELAY_SEEN_EVENT_IDS_MAX);
        }
        if (!eventId || !pubkey || !content) {
          return;
        }
        const rawTags = Array.isArray(eventObj?.tags) ? eventObj?.tags : [];
        const tags: string[][] = [];
        for (const rawTag of rawTags) {
          if (!Array.isArray(rawTag) || rawTag.length < 2) {
            continue;
          }
          const key = String(rawTag[0] ?? "").trim();
          const value = String(rawTag[1] ?? "").trim();
          if (!key || !value) {
            continue;
          }
          tags.push([key, value]);
        }
        const relayGroupTag = this.extractRelayGroupTagValue(tags) ?? "";
        const contentFingerprint = `${pubkey.toLowerCase()}:${relayGroupTag}:${fnv1aHashHex(content)}`;
        const seenFingerprints = new Set(
          (session.seen_event_fingerprints ?? [])
            .map((fingerprint) => String(fingerprint ?? "").trim())
            .filter(Boolean),
        );
        if (seenFingerprints.has(contentFingerprint)) {
          session.duplicate_events_ignored += 1;
          return;
        }
        session.seen_event_fingerprints = [...seenFingerprints, contentFingerprint].slice(
          -RELAY_SEEN_EVENT_FINGERPRINTS_MAX,
        );
        inboundRelayEvent = {
          id: eventId,
          pubkey,
          content,
          tags,
          event_json: JSON.stringify(eventObj ?? {}),
        };
        return;
      }
      if (kind === "NOTICE") {
        session.notices += 1;
        session.last_notice = String(parsed[1] ?? "").slice(0, 300);
        return;
      }
      if (kind === "OK") {
        session.oks += 1;
        const eventId = String(parsed[1] ?? "").trim();
        const accepted = parsed[2] === true;
        const message = String(parsed[3] ?? "").slice(0, 300);
        if (eventId) {
          session.last_ok_event_id = eventId;
          session.pending_event_ids = session.pending_event_ids.filter((id) => id !== eventId);
        }
        session.last_ok_message = message || undefined;
        if (eventId) {
          session.acked_events += 1;
        }
        if (accepted) {
          session.rate_limited_until_ms = undefined;
          ackEventId = eventId || null;
        } else {
          session.publish_failures += 1;
          if (message.toLowerCase().includes("rate-limit")) {
            session.rate_limited_until_ms = nowMs() + RELAY_RATE_LIMIT_BACKOFF_MS;
          }
          session.last_notice = `relay rejected ${eventId || "event"}: ${message}`.slice(
            0,
            300,
          );
          rejectEventId = eventId || null;
        }
      }
    });
    if (ackEventId) {
      await this.ackPendingRelayEvent(ackEventId);
    }
    if (rejectEventId) {
      await this.ackPendingRelayEvent(rejectEventId);
    }
    if (inboundRelayEvent) {
      await this.handleInboundRelayEvent(agentId, inboundRelayEvent);
    }
  }

  private closeRelaySocket(reason: string): void {
    if (!this.relaySocket) {
      this.relaySubscriptionId = null;
      return;
    }
    try {
      this.relaySocket.close(1000, reason.slice(0, 123));
    } catch {
      // noop
    }
    this.relaySocket = null;
    this.relaySocketRelay = null;
    this.relaySubscriptionId = null;
  }

  private enqueueRelaySocketMessage(agentId: string, rawData: unknown): void {
    this.relayMessageQueue = this.relayMessageQueue
      .then(() => this.handleRelaySocketMessage(agentId, rawData))
      .catch((error) => {
        console.warn(
          JSON.stringify({
            event: "agent.relay.ws.message_queue.error",
            agent_id: agentId,
            error: error instanceof Error ? error.message : String(error),
          }),
        );
      });
  }

  private ensureRelaySocket(record: AgentRecord): boolean {
    const relay = record.relay_urls[0];
    const session = this.ensureRelaySessionState(record);
    session.relay = relay;
    if (!relay) {
      session.active = false;
      this.closeRelaySocket("no-relay");
      return false;
    }

    if (
      this.relaySocket &&
      this.relaySocketRelay === relay &&
      (this.relaySocket.readyState === WebSocket.OPEN ||
        this.relaySocket.readyState === WebSocket.CONNECTING)
    ) {
      session.active = this.relaySocket.readyState === WebSocket.OPEN;
      return true;
    }

    this.closeRelaySocket("reconnect");
    session.active = false;
    try {
      const socket = new WebSocket(relay);
      this.relaySocket = socket;
      this.relaySocketRelay = relay;
      socket.addEventListener("open", () => {
        void this.patchRelaySession(record.id, (relaySession, persisted) => {
          relaySession.active = true;
          relaySession.relay = relay;
          relaySession.connected_at_ms = nowMs();
          if (this.sendRelayObserveRequest(persisted)) {
            persisted.updated_at_ms = nowMs();
          }
        }).catch((error) => {
          console.warn(
            JSON.stringify({
              event: "agent.relay.ws.session_patch_failed",
              agent_id: record.id,
              relay,
              error: error instanceof Error ? error.message : String(error),
            }),
          );
        });
        console.log(
          JSON.stringify({
            event: "agent.relay.ws.open",
            agent_id: record.id,
            relay,
          }),
        );
      });
      socket.addEventListener("error", () => {
        void this.patchRelaySession(record.id, (relaySession) => {
          relaySession.errors += 1;
          relaySession.active = false;
          relaySession.last_notice = "websocket error";
        }).catch(() => {
          // noop
        });
        console.warn(
          JSON.stringify({
            event: "agent.relay.ws.error",
            agent_id: record.id,
            relay,
          }),
        );
      });
      socket.addEventListener("message", (event) => {
        this.enqueueRelaySocketMessage(record.id, event.data);
      });
      socket.addEventListener("close", (event) => {
        if (this.relaySocket === socket) {
          this.relaySocket = null;
          this.relaySocketRelay = null;
          this.relaySubscriptionId = null;
        }
        void this.patchRelaySession(record.id, (relaySession) => {
          relaySession.active = false;
          relaySession.last_notice = `ws close ${event.code}: ${event.reason || "no reason"}`.slice(
            0,
            300,
          );
        }).catch(() => {
          // noop
        });
        console.warn(
          JSON.stringify({
            event: "agent.relay.ws.close",
            agent_id: record.id,
            relay,
            code: event.code,
            reason: event.reason,
          }),
        );
      });
      return true;
    } catch (error) {
      this.relaySocket = null;
      this.relaySocketRelay = null;
      this.relaySubscriptionId = null;
      session.errors += 1;
      session.last_notice = "websocket connect failed";
      console.warn(
        JSON.stringify({
          event: "agent.relay.ws.connect_failed",
          agent_id: record.id,
          relay,
          error: error instanceof Error ? error.message : String(error),
        }),
      );
      return false;
    }
  }

  private async refreshRelayProbe(record: AgentRecord): Promise<void> {
    const relay = record.relay_urls[0];
    if (!relay) {
      return;
    }
    const session = this.ensureRelaySessionState(record);
    session.relay = relay;
    record.relay_probe = await probeRelay(relay);
    record.updated_at_ms = nowMs();
    await this.saveRecord(record);
    console.log(
      JSON.stringify({
        event: "agent.relay.poll",
        agent_id: record.id,
        relay,
        ok: record.relay_probe.ok,
        status_code: record.relay_probe.status_code,
      }),
    );
  }

  private async maybeMarkReady(record: AgentRecord): Promise<void> {
    const now = nowMs();
    let changed = false;

    if (record.status !== "ready") {
      if (now < record.ready_at_ms) {
        return;
      }
      record.status = "ready";
      record.updated_at_ms = now;
      changed = true;
    }

    if (record.key_package_published_at_ms) {
      if (changed) {
        await this.saveRecord(record);
      }
      return;
    }

    try {
      await this.ensureRelayIdentity(record);
    } catch {
      if (changed) {
        await this.saveRecord(record);
      }
      return;
    }

    const published = await this.publishRelayKeyPackage(record);
    if (!published) {
      if (changed) {
        await this.saveRecord(record);
      }
      return;
    }

    record.key_package_published_at_ms = nowMs();
    record.updated_at_ms = nowMs();
    this.markSessionActive(record);
    const hasRelaySocket = this.ensureRelaySocket(record);
    if (hasRelaySocket) {
      this.sendRelayObserveRequest(record);
    }
    await this.saveRecord(record);
    await this.scheduleRelayAlarm(record);
  }

  private async handleInit(request: Request): Promise<Response> {
    const body = await parseJson<
      CreateAgentRequest & {
        id: string;
        name: string;
        brain: AgentBrain;
        relay_urls: string[];
      }
    >(request);
    let brain: AgentBrain;
    try {
      brain = asBrain(body.brain);
    } catch (error) {
      return json({ error: error instanceof Error ? error.message : String(error) }, 400);
    }

    const existing = await this.loadRecord();
    if (existing) {
      existing.name = body.name;
      existing.brain = brain;
      existing.relay_urls = normalizeRelays(body.relay_urls);
      const session = this.ensureRelaySessionState(existing);
      const nextRelay = existing.relay_urls[0];
      if (session.relay && session.relay !== nextRelay) {
        this.closeRelaySocket("relay-updated");
        session.active = false;
      }
      session.relay = nextRelay;
      existing.updated_at_ms = nowMs();
      if (!existing.relay_probe) {
        existing.relay_probe = await probeRelay(existing.relay_urls[0]);
      }
      const providedSecret = String(body.bot_secret_key_hex ?? "").trim();
      if (providedSecret) {
        const secretKeyBytes = hexToBytes(providedSecret);
        const pubkey = getPublicKey(secretKeyBytes);
        await this.state.storage.put(RELAY_SECRET_STORAGE_KEY, bytesToHex(secretKeyBytes));
        existing.bot_pubkey = pubkey;
        existing.relay_pubkey = pubkey;
      }
      await this.ensureRelayIdentity(existing);
      await this.maybeMarkReady(existing);
      await this.saveRecord(existing);
      return json(existing);
    }

    const ts = nowMs();
    const relayUrls = normalizeRelays(body.relay_urls);
    const relayProbe = await probeRelay(relayUrls[0]);

    const created: AgentRecord = {
      schema_version: AGENT_RECORD_SCHEMA_VERSION,
      id: body.id,
      name: body.name,
      brain,
      status: "booting",
      created_at_ms: ts,
      updated_at_ms: ts,
      ready_at_ms: ts + BOOT_DELAY_MS,
      relay_urls: relayUrls,
      bot_pubkey: toPubkeyHint(body.id),
      relay_probe: relayProbe,
      relay_session: {
        active: false,
        relay: relayUrls[0],
        inbound_events: 0,
        notices: 0,
        oks: 0,
        errors: 0,
        published_events: 0,
        publish_failures: 0,
        acked_events: 0,
        inbound_applied_messages: 0,
        auto_replies: 0,
        duplicate_events_ignored: 0,
        pending_event_ids: [],
        seen_event_ids: [],
        seen_event_fingerprints: [],
      },
      history: [],
      runtime_snapshot: defaultRuntimeSnapshot(),
    };

    const providedSecret = String(body.bot_secret_key_hex ?? "").trim();
    if (providedSecret) {
      const secretKeyBytes = hexToBytes(providedSecret);
      const pubkey = getPublicKey(secretKeyBytes);
      await this.state.storage.put(RELAY_SECRET_STORAGE_KEY, bytesToHex(secretKeyBytes));
      created.bot_pubkey = pubkey;
      created.relay_pubkey = pubkey;
    }
    await this.ensureRelayIdentity(created);
    await this.saveRecord(created);
    return json(created);
  }

  private async handleStatus(): Promise<Response> {
    const existing = await this.loadRecord();
    if (!existing) {
      return json({ error: "agent not found" }, 404);
    }
    await this.maybeMarkReady(existing);
    if (!existing.relay_pubkey || existing.bot_pubkey !== existing.relay_pubkey) {
      await this.ensureRelayIdentity(existing);
      await this.saveRecord(existing);
    }
    if (existing.session_active_until_ms && existing.session_active_until_ms > nowMs()) {
      const hasRelaySocket = this.ensureRelaySocket(existing);
      if (hasRelaySocket) {
        await this.flushPendingRelayEvents(existing);
        if (this.sendRelayObserveRequest(existing)) {
          existing.updated_at_ms = nowMs();
          await this.saveRecord(existing);
        }
      }
      await this.scheduleRelayAlarm(existing);
    }
    return json(existing);
  }

  private collectPiReplyFromEvents(events: unknown[]): string {
    const textParts: string[] = [];
    for (const rawEvent of events) {
      const event = this.unwrapPiRpcEvent(rawEvent);
      if (!event) {
        continue;
      }
      const eventType = String(event.type ?? "").trim();
      if (eventType === "message_update") {
        const assistant = event.assistantMessageEvent;
        if (assistant?.type === "text_delta" && typeof assistant.delta === "string") {
          textParts.push(assistant.delta);
        }
        if (typeof event.delta === "string") {
          textParts.push(event.delta);
        }
      } else if (eventType === "assistant_message" || eventType === "assistant_text") {
        const text = String(event.text ?? event.message ?? "").trim();
        if (text) {
          textParts.push(text);
        }
      } else if (!eventType) {
        const direct = String(event.reply ?? event.result?.reply ?? "").trim();
        if (direct) {
          textParts.push(direct);
        }
      } else if (eventType === "agent_end") {
        break;
      } else if (eventType === "response" && event.success === false) {
        break;
      }
    }
    return textParts.join("").trim();
  }

  private unwrapPiRpcEvent(rawEvent: unknown): PiRpcEvent | null {
    if (!rawEvent || typeof rawEvent !== "object") {
      return null;
    }

    let candidate = rawEvent as PiRpcEvent;
    for (let depth = 0; depth < 3; depth += 1) {
      if (typeof candidate.type === "string" || typeof candidate.reply === "string") {
        return candidate;
      }
      if (candidate.event && typeof candidate.event === "object") {
        candidate = candidate.event as PiRpcEvent;
        continue;
      }
      if (candidate.data && typeof candidate.data === "object") {
        candidate = candidate.data as PiRpcEvent;
        continue;
      }
      break;
    }

    return candidate;
  }

  private parsePiEventLines(bodyText: string): unknown[] {
    const events: unknown[] = [];
    for (const line of bodyText.split(/\r?\n/)) {
      let candidate = line.trim();
      if (!candidate || candidate.startsWith(":") || candidate.startsWith("event:")) {
        continue;
      }
      if (candidate.startsWith("data:")) {
        candidate = candidate.slice("data:".length).trim();
      }
      if (!candidate || candidate === "[DONE]") {
        continue;
      }
      try {
        events.push(JSON.parse(candidate));
      } catch {
        // ignore non-JSON lines in mixed output
      }
    }
    return events;
  }

  private extractPiReplyFromBody(bodyText: string): string {
    const trimmed = bodyText.trim();
    if (!trimmed) {
      return "";
    }

    try {
      const payload = JSON.parse(trimmed) as
        | { reply?: string; events?: unknown[]; result?: { reply?: string } }
        | unknown[];

      if (Array.isArray(payload)) {
        const reply = this.collectPiReplyFromEvents(payload);
        if (reply) {
          return reply;
        }
      } else {
        const directReply = String(payload.reply ?? payload.result?.reply ?? "").trim();
        if (directReply) {
          return directReply;
        }
        if (Array.isArray(payload.events)) {
          const reply = this.collectPiReplyFromEvents(payload.events);
          if (reply) {
            return reply;
          }
        }
        const wrappedReply = this.collectPiReplyFromEvents([payload]);
        if (wrappedReply) {
          return wrappedReply;
        }
      }
    } catch {
      // fall through to event-line/plain-text parsing
    }

    const events = this.parsePiEventLines(trimmed);
    if (events.length > 0) {
      const reply = this.collectPiReplyFromEvents(events);
      if (reply) {
        return reply;
      }
    }

    if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
      return "";
    }
    return trimmed;
  }

  private async tryPiAdapter(record: AgentRecord, content: string): Promise<string> {
    const base = this.env.PI_ADAPTER_BASE_URL?.trim();
    if (!base) {
      throw new Error("brain=pi requires PI_ADAPTER_BASE_URL");
    }

    const normalizedBase = base.replace(/\/+$/, "");
    const reqBody = {
      agent_id: record.id,
      agent_name: record.name,
      type: "prompt",
      message: content,
      rpc: {
        type: "prompt",
        message: content,
      },
      history: record.history,
    };
    const headers: Record<string, string> = {
      "content-type": "application/json",
    };
    const token = this.env.PI_ADAPTER_TOKEN?.trim();
    if (token) {
      headers.authorization = `Bearer ${token}`;
    }

    const fetchAdapter = (path: "/rpc" | "/reply") =>
      this.fetchWithTimeout(
        `${normalizedBase}${path}`,
        {
          method: "POST",
          headers,
          body: JSON.stringify(reqBody),
        },
        parseTimeoutMs(this.env.PI_ADAPTER_TIMEOUT_MS, DEFAULT_PI_ADAPTER_TIMEOUT_MS),
      );

    const startedAt = nowMs();
    let adapterPath: "/rpc" | "/reply" = "/rpc";
    let resp = await fetchAdapter(adapterPath);
    if (resp.status === 404 || resp.status === 405) {
      adapterPath = "/reply";
      resp = await fetchAdapter(adapterPath);
    }
    const latencyMs = nowMs() - startedAt;

    if (!resp.ok) {
      const body = await resp.text();
      throw new Error(`pi-adapter HTTP ${resp.status}: ${body.slice(0, 300)}`);
    }

    const reply = this.extractPiReplyFromBody(await resp.text());
    if (!reply) {
      throw new Error("pi-adapter returned empty reply");
    }

    console.log(
      JSON.stringify({
        event: "agent.reply.success",
        source: "pi-adapter",
        adapter_path: adapterPath,
        agent_id: record.id,
        latency_ms: latencyMs,
      }),
    );
    return reply;
  }

  private async fetchWithTimeout(
    url: string,
    init: RequestInit,
    timeoutMs: number,
  ): Promise<Response> {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort("timeout"), timeoutMs);
    try {
      return await fetch(url, {
        ...init,
        signal: controller.signal,
      });
    } catch (error) {
      if (controller.signal.aborted) {
        throw new Error(`pi-adapter request timed out after ${timeoutMs}ms`);
      }
      throw error;
    } finally {
      clearTimeout(timeoutId);
    }
  }

  private async generateReply(record: AgentRecord, content: string): Promise<ReplyGeneration> {
    try {
      const reply = await this.tryPiAdapter(record, content);
      return { reply, source: "pi-adapter" };
    } catch (error) {
      console.warn(
        JSON.stringify({
          event: "agent.reply.error",
          source: "pi-adapter",
          agent_id: record.id,
          error: error instanceof Error ? error.message : String(error),
        }),
      );
      throw new Error(
        `brain=pi reply generation failed: ${
          error instanceof Error ? error.message : String(error)
        }`,
      );
    }
  }

  private async buildRuntimeMachine(record: AgentRecord): Promise<RuntimeEngine> {
    return await WasmRuntimeContractStateMachine.fromSnapshot(
      normalizeRuntimeSnapshot(record.runtime_snapshot),
    );
  }

  private async handleRuntimeProcessWelcome(request: Request): Promise<Response> {
    const existing = await this.loadRecord();
    if (!existing) {
      return json({ error: "agent not found" }, 404);
    }

    const body = await parseJson<RuntimeProcessWelcomeRequest>(request);
    try {
      const runtime = await this.buildRuntimeMachine(existing);
      await this.ensureRuntimeIdentity(existing, runtime);
      const groupId = body.group_id ?? "";
      const wrapperEventIdHex = body.wrapper_event_id_hex ?? "";
      const welcomeEventJson = body.welcome_event_json ?? "";
      const result =
        wrapperEventIdHex.trim() && welcomeEventJson.trim()
          ? runtime.processWelcomeEventJson(groupId, wrapperEventIdHex, welcomeEventJson)
          : runtime.processWelcome(groupId);
      existing.runtime_snapshot = runtime.snapshot();
      this.markSessionActive(existing);
      const hasRelaySocket = this.ensureRelaySocket(existing);
      if (hasRelaySocket) {
        this.sendRelayObserveRequest(existing);
      }
      existing.updated_at_ms = nowMs();
      await this.saveRecord(existing);
      await this.scheduleRelayAlarm(existing);
      return json(result);
    } catch (error) {
      return json(
        { error: error instanceof Error ? error.message : String(error) },
        400,
      );
    }
  }

}
