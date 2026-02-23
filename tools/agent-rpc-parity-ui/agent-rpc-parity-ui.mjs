#!/usr/bin/env node

import { randomUUID } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import process from "node:process";
import readline from "node:readline";
import { pathToFileURL } from "node:url";

async function loadPiCodingAgentModule() {
  const errors = [];
  try {
    return await import("@mariozechner/pi-coding-agent");
  } catch (error) {
    errors.push(`direct import failed: ${error instanceof Error ? error.message : String(error)}`);
  }

  const require = createRequire(import.meta.url);
  const candidatePaths = [];
  const candidateRoots = new Set();

  const explicitModulePath = String(process.env.PIKA_PI_CODING_AGENT_PATH ?? "").trim();
  if (explicitModulePath) {
    candidatePaths.push(explicitModulePath);
  }

  const explicitNodeModules = String(process.env.PIKA_PI_NODE_MODULES ?? "").trim();
  if (explicitNodeModules) {
    candidateRoots.add(explicitNodeModules);
  }

  const npmRoot = spawnSync("npm", ["root", "-g"], { encoding: "utf8" });
  if (npmRoot.status === 0) {
    const root = npmRoot.stdout.trim();
    if (root) {
      candidateRoots.add(root);
    }
  }

  candidateRoots.add(path.join(os.homedir(), ".npm-global", "lib", "node_modules"));
  candidateRoots.add(path.join(os.homedir(), ".bun", "install", "global", "node_modules"));
  candidateRoots.add("/opt/homebrew/lib/node_modules");
  candidateRoots.add("/usr/local/lib/node_modules");

  for (const root of candidateRoots) {
    if (!root) continue;
    candidatePaths.push(path.join(root, "@mariozechner", "pi-coding-agent", "dist", "index.js"));
    candidatePaths.push(path.join(root, "pi-coding-agent", "dist", "index.js"));
    try {
      const resolved = require.resolve("@mariozechner/pi-coding-agent", { paths: [root] });
      candidatePaths.push(resolved);
    } catch {
      // Continue trying other roots.
    }
  }

  const seen = new Set();
  for (const candidate of candidatePaths) {
    const trimmed = String(candidate ?? "").trim();
    if (!trimmed || seen.has(trimmed)) continue;
    seen.add(trimmed);

    try {
      if (trimmed.startsWith("file://")) {
        return await import(trimmed);
      }
      if (trimmed.includes("/") || trimmed.startsWith(".")) {
        const resolvedPath = path.resolve(trimmed);
        if (!fs.existsSync(resolvedPath)) continue;
        return await import(pathToFileURL(resolvedPath).href);
      }
      return await import(trimmed);
    } catch (error) {
      errors.push(`${trimmed}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  const hint = [
    "Unable to load @mariozechner/pi-coding-agent for RPC parity UI.",
    "Install it with: npm install -g @mariozechner/pi-coding-agent",
    "Or set PIKA_PI_CODING_AGENT_PATH to the package dist/index.js path."
  ].join(" ");
  throw new Error(`${hint} Attempts: ${errors.join(" | ")}`);
}

const {
  AuthStorage,
  DefaultResourceLoader,
  InteractiveMode,
  ModelRegistry,
  SessionManager,
  SettingsManager
} = await loadPiCodingAgentModule();

const FRAMED_PROTOCOL_VERSION = 1;
const STREAMS = new Set(["rpc_event", "rpc_response", "rpc_request", "control"]);
const RPC_MESSAGE_PREFIX = "__PI_RPC__";
const DEFAULT_MOQ_URLS = [
  "https://us-east.moq.pikachat.org/anon",
  "https://eu.moq.pikachat.org/anon"
];

const HEARTBEAT_MS = 10_000;
const HEARTBEAT_TIMEOUT_MS = 30_000;
const MAX_REORDER_WINDOW = 4096;
const parsedRequestTimeout = Number.parseInt(process.env.PIKA_AGENT_RPC_REQUEST_TIMEOUT_MS ?? "30000", 10);
const RPC_REQUEST_TIMEOUT_MS = Number.isFinite(parsedRequestTimeout) && parsedRequestTimeout > 0 ? parsedRequestTimeout : 30_000;
const MARMOTD_STDERR_MODE = String(process.env.PIKA_AGENT_MARMOTD_STDERR ?? "quiet").trim().toLowerCase();
const TRANSPORT_MODE_RAW = String(process.env.PIKA_AGENT_RPC_TRANSPORT ?? "moq").trim().toLowerCase();
const TRANSPORT_MODE = TRANSPORT_MODE_RAW === "nostr" ? "nostr" : "moq";
const USE_NOSTR_TRANSPORT = TRANSPORT_MODE === "nostr";

function createAuthStorage() {
  if (AuthStorage && typeof AuthStorage.inMemory === "function") {
    return AuthStorage.inMemory();
  }
  return new AuthStorage();
}

function requiredEnv(name) {
  const value = process.env[name];
  if (!value || !value.trim()) {
    throw new Error(`missing required env var ${name}`);
  }
  return value.trim();
}

function parseJsonList(raw) {
  if (!raw || !raw.trim()) return [];
  try {
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.map((x) => String(x)).map((x) => x.trim()).filter((x) => x.length > 0);
  } catch {
    return [];
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

const MARMOTD_BIN = requiredEnv("PIKA_AGENT_MARMOTD_BIN");
const STATE_DIR = requiredEnv("PIKA_AGENT_STATE_DIR");
const GROUP_ID = requiredEnv("PIKA_AGENT_GROUP_ID");
const BOT_PUBKEY = requiredEnv("PIKA_AGENT_BOT_PUBKEY").toLowerCase();
const MACHINE_ID = (process.env.PIKA_AGENT_MACHINE_ID ?? "").trim();
const FLY_APP_NAME = (process.env.PIKA_AGENT_FLY_APP_NAME ?? "").trim();
const RELAYS = parseJsonList(process.env.PIKA_AGENT_RELAYS_JSON ?? "");
const MOQ_URLS = parseJsonList(process.env.PIKA_AGENT_MOQ_URLS_JSON ?? "");

const EFFECTIVE_MOQ_URLS = MOQ_URLS.length > 0 ? MOQ_URLS : DEFAULT_MOQ_URLS;
const parsedFragSize = Number.parseInt(process.env.PIKA_AGENT_RPC_FRAGMENT_BYTES ?? "6000", 10);
const MAX_FRAGMENT_BYTES = Number.isFinite(parsedFragSize) && parsedFragSize >= 256 ? parsedFragSize : 6000;

let daemon = null;
let daemonOutRl = null;
let daemonErrRl = null;
let callId = null;
let sessionId = null;
let shutdownRequested = false;
let heartbeatTimer = null;
let heartbeatHealthTimer = null;
let lastPongAt = Date.now();
let openAckSeen = false;
let sawLegacyPtyPayload = false;
const openAckWaiters = new Set();
const pendingInviteCallIds = new Set();
const staleInviteCallIds = new Set();

const waiters = new Set();

const nextOutSeq = new Map();
const expectedInSeq = new Map();
const inPending = new Map();
const inFragments = new Map();
const inProcessed = new Set();
const rpcLineBuffers = new Map();

for (const stream of STREAMS) {
  nextOutSeq.set(stream, 0);
  expectedInSeq.set(stream, 0);
  inPending.set(stream, new Map());
  rpcLineBuffers.set(stream, "");
}

function sendCmd(cmd) {
  if (!daemon || !daemon.stdin || daemon.killed) return;
  daemon.stdin.write(`${JSON.stringify(cmd)}\n`);
}

function encodePayloadHex(obj) {
  return Buffer.from(JSON.stringify(obj), "utf8").toString("hex");
}

function sendCallPayload(payloadObj) {
  if (USE_NOSTR_TRANSPORT) {
    sendCmd({
      cmd: "send_message",
      nostr_group_id: GROUP_ID,
      content: `${RPC_MESSAGE_PREFIX}${JSON.stringify(payloadObj)}`
    });
    return;
  }
  if (!callId) return;
  sendCmd({
    cmd: "send_call_data",
    call_id: callId,
    payload_hex: encodePayloadHex(payloadObj)
  });
}

function sendEndCall(callIdToEnd, reason) {
  const id = String(callIdToEnd ?? "").trim();
  if (!id) return;
  sendCmd({ cmd: "end_call", call_id: id, reason });
}

function decodeCallPayload(msg) {
  const payloadHex = String(msg.payload_hex ?? "").trim();
  if (!payloadHex) return null;
  try {
    const raw = Buffer.from(payloadHex, "hex");
    const payload = JSON.parse(raw.toString("utf8"));
    if (!payload || typeof payload !== "object") return null;
    return payload;
  } catch {
    return null;
  }
}

function resetFramingState() {
  inFragments.clear();
  inProcessed.clear();
  openAckSeen = false;
  for (const waiter of [...openAckWaiters]) {
    openAckWaiters.delete(waiter);
    clearTimeout(waiter.timeout);
    waiter.resolve();
  }
  for (const stream of STREAMS) {
    nextOutSeq.set(stream, 0);
    expectedInSeq.set(stream, 0);
    inPending.set(stream, new Map());
    rpcLineBuffers.set(stream, "");
  }
}

function pruneProcessedSet(stream, nextExpectedSeq) {
  if (inProcessed.size < MAX_REORDER_WINDOW * 2) return;
  const cutoff = nextExpectedSeq - MAX_REORDER_WINDOW;
  if (cutoff <= 0) return;
  for (const key of [...inProcessed]) {
    const parts = key.split("|");
    if (parts.length !== 3) continue;
    const [sid, keyStream, keySeqRaw] = parts;
    if (sid !== sessionId || keyStream !== stream) continue;
    const keySeq = Number(keySeqRaw);
    if (Number.isInteger(keySeq) && keySeq < cutoff) {
      inProcessed.delete(key);
    }
  }
}

function sendFramed(stream, payload) {
  if (!sessionId) return;
  if (!USE_NOSTR_TRANSPORT && !callId) return;
  if (!STREAMS.has(stream)) return;
  const seq = nextOutSeq.get(stream) ?? 0;
  nextOutSeq.set(stream, seq + 1);

  const fragCount = Math.max(1, Math.ceil(payload.length / MAX_FRAGMENT_BYTES));
  for (let fragIndex = 0; fragIndex < fragCount; fragIndex += 1) {
    const start = fragIndex * MAX_FRAGMENT_BYTES;
    const end = start + MAX_FRAGMENT_BYTES;
    const fragment = payload.subarray(start, end);
    sendCallPayload({
      v: FRAMED_PROTOCOL_VERSION,
      session_id: sessionId,
      stream,
      seq,
      frag_index: fragIndex,
      frag_count: fragCount,
      payload_b64: fragment.toString("base64")
    });
  }
}

function sendControl(payload) {
  sendFramed("control", Buffer.from(JSON.stringify(payload), "utf8"));
}

function emitEvent(msg) {
  for (const waiter of [...waiters]) {
    if (waiter.done) continue;
    try {
      if (waiter.predicate(msg)) {
        waiter.done = true;
        waiters.delete(waiter);
        clearTimeout(waiter.timeout);
        waiter.resolve(msg);
      }
    } catch {
      // Ignore predicate errors.
    }
  }
}

function waitForEvent(predicate, timeoutMs, label) {
  return new Promise((resolve, reject) => {
    const waiter = {
      predicate,
      resolve,
      reject,
      done: false,
      timeout: setTimeout(() => {
        if (waiter.done) return;
        waiter.done = true;
        waiters.delete(waiter);
        reject(new Error(`timeout waiting for ${label}`));
      }, timeoutMs)
    };
    waiters.add(waiter);
  });
}

function noteOpenAck() {
  openAckSeen = true;
  for (const waiter of [...openAckWaiters]) {
    openAckWaiters.delete(waiter);
    clearTimeout(waiter.timeout);
    waiter.resolve();
  }
}

function waitForOpenAck(timeoutMs) {
  if (openAckSeen) {
    return Promise.resolve();
  }
  return new Promise((resolve, reject) => {
    const waiter = {
      resolve,
      timeout: setTimeout(() => {
        openAckWaiters.delete(waiter);
        if (sawLegacyPtyPayload) {
          reject(
            new Error(
              "rpc open_ack not received: remote appears to be in PTY bridge mode. Redeploy bot image with rpc-capable pi-bridge and ensure PI_BRIDGE_CALL_MODE=rpc is set."
            )
          );
          return;
        }
        reject(new Error("timeout waiting for rpc open_ack"));
      }, timeoutMs)
    };
    openAckWaiters.add(waiter);
  });
}

class RpcBridgeClient {
  constructor() {
    this.requestCounter = 0;
    this.pendingRequests = new Map();
    this.sessionListeners = new Set();
    this.extensionUiListeners = new Set();
  }

  requestCounter;
  pendingRequests;
  sessionListeners;
  extensionUiListeners;

  nextId() {
    this.requestCounter += 1;
    return `rpc_${this.requestCounter}`;
  }

  sendRawRpcRequest(payloadObj) {
    const line = `${JSON.stringify(payloadObj)}\n`;
    sendFramed("rpc_request", Buffer.from(line, "utf8"));
  }

  sendCommand(payloadObj) {
    const id = payloadObj.id ?? this.nextId();
    const request = { ...payloadObj, id };
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingRequests.delete(String(id));
        reject(new Error(`timeout waiting for response to ${request.type}`));
      }, RPC_REQUEST_TIMEOUT_MS);
      this.pendingRequests.set(String(id), {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        }
      });
      this.sendRawRpcRequest(request);
    });
  }

  onSessionEvent(listener) {
    this.sessionListeners.add(listener);
    return () => this.sessionListeners.delete(listener);
  }

  onExtensionUiRequest(listener) {
    this.extensionUiListeners.add(listener);
    return () => this.extensionUiListeners.delete(listener);
  }

  handleResponse(obj) {
    if (!obj || obj.type !== "response") return;
    const id = String(obj.id ?? "").trim();
    if (!id) return;
    const pending = this.pendingRequests.get(id);
    if (!pending) return;
    this.pendingRequests.delete(id);
    if (obj.success) {
      pending.resolve(obj);
    } else {
      pending.reject(new Error(String(obj.error ?? `rpc ${obj.command} failed`)));
    }
  }

  handleEvent(obj) {
    if (!obj || typeof obj !== "object") return;
    if (obj.type === "response") {
      this.handleResponse(obj);
      return;
    }
    if (obj.type === "extension_ui_request") {
      for (const listener of this.extensionUiListeners) {
        listener(obj);
      }
      return;
    }
    for (const listener of this.sessionListeners) {
      listener(obj);
    }
  }

  rejectPending(error) {
    for (const pending of this.pendingRequests.values()) {
      pending.reject(error);
    }
    this.pendingRequests.clear();
  }
}

class RemoteAgentSessionAdapter {
  constructor(rpcClient) {
    this.rpcClient = rpcClient;
    this.authStorage = createAuthStorage();
    this.modelRegistry = new ModelRegistry(this.authStorage);
    this.settingsManager = SettingsManager.inMemory();
    this.sessionManager = SessionManager.inMemory(process.cwd());
    this.resourceLoader = new DefaultResourceLoader({
      cwd: process.cwd(),
      settingsManager: this.settingsManager,
      noExtensions: true,
      noSkills: true,
      noPromptTemplates: true
    });
    this.agent = {
      state: {
        messages: [],
        model: undefined,
        thinkingLevel: "off"
      },
      abort: async () => {
        await this.abort();
      },
      waitForIdle: async () => {
        while (this.isStreaming || this.isCompacting) {
          await sleep(50);
        }
      }
    };

    this.extensionRunner = undefined;
    this._listeners = new Set();
    this._messages = [];
    this._sessionState = {
      model: undefined,
      thinkingLevel: "off",
      isStreaming: false,
      isCompacting: false,
      steeringMode: "all",
      followUpMode: "one-at-a-time",
      sessionId: randomUUID(),
      autoCompactionEnabled: true,
      messageCount: 0,
      pendingMessageCount: 0
    };
    this._steeringQueue = [];
    this._followUpQueue = [];
    this._forkMessages = [];
    this.retryAttempt = 0;
    this._contextUsage = undefined;

    this.rpcClient.onSessionEvent((event) => {
      this._applyEvent(event);
      this._emit(event);
    });
  }

  rpcClient;
  authStorage;
  modelRegistry;
  settingsManager;
  sessionManager;
  resourceLoader;
  extensionRunner;
  agent;
  _listeners;
  _messages;
  _sessionState;
  _steeringQueue;
  _followUpQueue;
  _forkMessages;
  _contextUsage;
  retryAttempt;

  get state() {
    return {
      model: this._sessionState.model,
      thinkingLevel: this._sessionState.thinkingLevel,
      isStreaming: this._sessionState.isStreaming,
      isCompacting: this._sessionState.isCompacting,
      messages: this._messages
    };
  }

  get model() {
    return this._sessionState.model;
  }

  get thinkingLevel() {
    return this._sessionState.thinkingLevel;
  }

  get isStreaming() {
    return Boolean(this._sessionState.isStreaming);
  }

  get isCompacting() {
    return Boolean(this._sessionState.isCompacting);
  }

  get isBashRunning() {
    return this.isStreaming;
  }

  get messages() {
    return this._messages;
  }

  get steeringMode() {
    return this._sessionState.steeringMode ?? "all";
  }

  get followUpMode() {
    return this._sessionState.followUpMode ?? "one-at-a-time";
  }

  get sessionFile() {
    return this._sessionState.sessionFile;
  }

  get sessionId() {
    return this._sessionState.sessionId;
  }

  get sessionName() {
    return this._sessionState.sessionName;
  }

  get scopedModels() {
    return [];
  }

  get promptTemplates() {
    return this.resourceLoader.getPrompts().prompts;
  }

  get autoCompactionEnabled() {
    return Boolean(this._sessionState.autoCompactionEnabled);
  }

  get pendingMessageCount() {
    return Number(this._sessionState.pendingMessageCount ?? 0);
  }

  get systemPrompt() {
    return "";
  }

  subscribe(listener) {
    this._listeners.add(listener);
    return () => this._listeners.delete(listener);
  }

  _emit(event) {
    for (const listener of [...this._listeners]) {
      listener(event);
    }
  }

  async _send(command) {
    const response = await this.rpcClient.sendCommand(command);
    return response.data;
  }

  _appendMessageEntry(message) {
    try {
      this.sessionManager.appendMessage(message);
    } catch {
      // Keep adapter resilient to unknown message shapes.
    }
  }

  _rebuildSessionEntries(messages) {
    const cwd = this.sessionManager.getCwd();
    this.sessionManager = SessionManager.inMemory(cwd);
    for (const message of messages) {
      this._appendMessageEntry(message);
    }
  }

  _updateContextUsageFromMessage(message) {
    const usage = message?.usage;
    const model = this._sessionState.model;
    if (!usage || !model || !model.contextWindow) {
      return;
    }
    const tokens = Number(usage.input ?? 0) + Number(usage.output ?? 0) + Number(usage.cacheRead ?? 0) + Number(usage.cacheWrite ?? 0);
    this._contextUsage = {
      tokens,
      contextWindow: model.contextWindow,
      percent: model.contextWindow > 0 ? (tokens / model.contextWindow) * 100 : 0
    };
  }

  async _refreshForkMessages() {
    try {
      const data = await this._send({ type: "get_fork_messages" });
      if (Array.isArray(data?.messages)) {
        this._forkMessages = data.messages
          .filter((message) => message && typeof message === "object")
          .map((message) => ({
            entryId: String(message.entryId ?? ""),
            text: String(message.text ?? "")
          }))
          .filter((message) => message.entryId.length > 0 && message.text.length > 0);
      }
    } catch {
      // Best-effort cache refresh only.
    }
  }

  _computeSessionStats() {
    const messages = Array.isArray(this._messages) ? this._messages : [];
    const userMessages = messages.filter((message) => message?.role === "user").length;
    const assistantMessages = messages.filter((message) => message?.role === "assistant").length;
    const toolResults = messages.filter((message) => message?.role === "toolResult").length;

    let toolCalls = 0;
    let totalInput = 0;
    let totalOutput = 0;
    let totalCacheRead = 0;
    let totalCacheWrite = 0;
    let totalCost = 0;

    for (const message of messages) {
      if (message?.role !== "assistant") continue;
      const content = Array.isArray(message.content) ? message.content : [];
      toolCalls += content.filter((part) => part?.type === "toolCall").length;

      const usage = message.usage ?? {};
      totalInput += Number(usage.input ?? 0);
      totalOutput += Number(usage.output ?? 0);
      totalCacheRead += Number(usage.cacheRead ?? 0);
      totalCacheWrite += Number(usage.cacheWrite ?? 0);
      totalCost += Number(usage?.cost?.total ?? 0);
    }

    return {
      sessionFile: this._sessionState.sessionFile,
      sessionId: this._sessionState.sessionId,
      userMessages,
      assistantMessages,
      toolCalls,
      toolResults,
      totalMessages: messages.length,
      tokens: {
        input: totalInput,
        output: totalOutput,
        cacheRead: totalCacheRead,
        cacheWrite: totalCacheWrite,
        total: totalInput + totalOutput + totalCacheRead + totalCacheWrite
      },
      cost: totalCost
    };
  }

  _getLastAssistantText() {
    const messages = Array.isArray(this._messages) ? this._messages : [];
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const message = messages[index];
      if (message?.role !== "assistant") continue;
      const content = Array.isArray(message.content) ? message.content : [];
      const text = content
        .filter((part) => part?.type === "text")
        .map((part) => String(part.text ?? ""))
        .join("")
        .trim();
      if (text.length > 0) {
        return text;
      }
    }
    return undefined;
  }

  _applyEvent(event) {
    const type = String(event?.type ?? "");
    if (type === "agent_start") {
      this._sessionState.isStreaming = true;
      return;
    }
    if (type === "agent_end") {
      this._sessionState.isStreaming = false;
      if (Array.isArray(event.messages)) {
        this._messages = event.messages;
        this._rebuildSessionEntries(this._messages);
        if (this._messages.length > 0) {
          this._updateContextUsageFromMessage(this._messages[this._messages.length - 1]);
        }
      }
      this._refreshForkMessages().catch(() => {});
      return;
    }
    if (type === "auto_compaction_start") {
      this._sessionState.isCompacting = true;
      return;
    }
    if (type === "auto_compaction_end") {
      this._sessionState.isCompacting = false;
      return;
    }
    if (type === "auto_retry_start") {
      this.retryAttempt = Number(event.attempt ?? 0);
      return;
    }
    if (type === "auto_retry_end") {
      this.retryAttempt = 0;
      return;
    }
    if (type === "message_end" && event.message) {
      this._messages = [...this._messages, event.message];
      this._appendMessageEntry(event.message);
      this._updateContextUsageFromMessage(event.message);
      this.agent.state = {
        ...(this.agent.state ?? {}),
        model: this._sessionState.model,
        thinkingLevel: this._sessionState.thinkingLevel,
        messages: this._messages
      };
      return;
    }
  }

  async syncRemoteState() {
    const state = await this._send({ type: "get_state" });
    this._sessionState = { ...this._sessionState, ...state };
    this.agent.state = {
      ...(this.agent.state ?? {}),
      model: this._sessionState.model,
      thinkingLevel: this._sessionState.thinkingLevel,
      messages: this._messages
    };

    const messagesData = await this._send({ type: "get_messages" });
    if (Array.isArray(messagesData?.messages)) {
      this._messages = messagesData.messages;
      this._rebuildSessionEntries(this._messages);
      const last = this._messages[this._messages.length - 1];
      if (last) {
        this._updateContextUsageFromMessage(last);
      }
    }

    await this._refreshForkMessages();

    this.agent.state = {
      ...(this.agent.state ?? {}),
      model: this._sessionState.model,
      thinkingLevel: this._sessionState.thinkingLevel,
      messages: this._messages
    };
  }

  async bindExtensions() {
    // Remote session already has its own extension runtime.
  }

  async prompt(text, options = {}) {
    await this._send({
      type: "prompt",
      message: text,
      images: options.images,
      streamingBehavior: options.streamingBehavior
    });
  }

  async steer(text, images) {
    this._steeringQueue.push(text);
    await this._send({ type: "steer", message: text, images });
  }

  async followUp(text, images) {
    this._followUpQueue.push(text);
    await this._send({ type: "follow_up", message: text, images });
  }

  clearQueue() {
    const cleared = {
      steering: [...this._steeringQueue],
      followUp: [...this._followUpQueue]
    };
    this._steeringQueue = [];
    this._followUpQueue = [];
    return cleared;
  }

  getSteeringMessages() {
    return this._steeringQueue;
  }

  getFollowUpMessages() {
    return this._followUpQueue;
  }

  async abort() {
    await this._send({ type: "abort" });
  }

  async newSession(options = {}) {
    const data = await this._send({ type: "new_session", parentSession: options.parentSession });
    await this.syncRemoteState();
    return !Boolean(data?.cancelled);
  }

  async switchSession(sessionPath) {
    const data = await this._send({ type: "switch_session", sessionPath });
    await this.syncRemoteState();
    return !Boolean(data?.cancelled);
  }

  async fork(entryId) {
    const data = await this._send({ type: "fork", entryId });
    await this.syncRemoteState();
    return {
      selectedText: String(data?.text ?? ""),
      cancelled: Boolean(data?.cancelled)
    };
  }

  async navigateTree() {
    return { cancelled: true };
  }

  getUserMessagesForForking() {
    return this._forkMessages;
  }

  getLastAssistantText() {
    return this._getLastAssistantText();
  }

  async setModel(model) {
    await this._send({ type: "set_model", provider: model.provider, modelId: model.id });
    this._sessionState.model = model;
  }

  async cycleModel() {
    const data = await this._send({ type: "cycle_model" });
    if (!data) {
      return undefined;
    }
    if (data.model) {
      this._sessionState.model = data.model;
      this._sessionState.thinkingLevel = data.thinkingLevel ?? this._sessionState.thinkingLevel;
    }
    return data;
  }

  setScopedModels() {
    // Scoped models are local-only and not supported for remote sessions.
  }

  setThinkingLevel(level) {
    this._sessionState.thinkingLevel = level;
    this._send({ type: "set_thinking_level", level }).catch(() => {});
  }

  cycleThinkingLevel() {
    this._send({ type: "cycle_thinking_level" })
      .then((data) => {
        if (data?.level) {
          this._sessionState.thinkingLevel = data.level;
        }
      })
      .catch(() => {});
    return this._sessionState.thinkingLevel;
  }

  getAvailableThinkingLevels() {
    return ["off", "minimal", "low", "medium", "high", "xhigh"];
  }

  setSteeringMode(mode) {
    this._sessionState.steeringMode = mode;
    this._send({ type: "set_steering_mode", mode }).catch(() => {});
  }

  setFollowUpMode(mode) {
    this._sessionState.followUpMode = mode;
    this._send({ type: "set_follow_up_mode", mode }).catch(() => {});
  }

  async compact(customInstructions) {
    return await this._send({ type: "compact", customInstructions });
  }

  abortCompaction() {
    // RPC mode has no dedicated abort compaction command.
  }

  abortBranchSummary() {
    // Branch summarization/navigation is not available over current RPC command set.
  }

  setAutoCompactionEnabled(enabled) {
    this._sessionState.autoCompactionEnabled = enabled;
    this._send({ type: "set_auto_compaction", enabled }).catch(() => {});
  }

  setAutoRetryEnabled(enabled) {
    this._send({ type: "set_auto_retry", enabled }).catch(() => {});
  }

  abortRetry() {
    this._send({ type: "abort_retry" }).catch(() => {});
  }

  async executeBash(command) {
    return await this._send({ type: "bash", command });
  }

  abortBash() {
    this._send({ type: "abort_bash" }).catch(() => {});
  }

  recordBashResult() {
    // No-op for remote session.
  }

  getSessionStats() {
    return this._computeSessionStats();
  }

  exportToHtml(outputPath) {
    return this._send({ type: "export_html", outputPath }).then((data) => data?.path);
  }

  setSessionName(name) {
    this._send({ type: "set_session_name", name }).catch(() => {});
  }

  getContextUsage() {
    return this._contextUsage;
  }

  async reload() {
    await this.syncRemoteState();
  }
}

const rpcClient = new RpcBridgeClient();
let remoteSession = null;
let interactiveMode = null;
let extensionUiChain = Promise.resolve();

function handleRpcTextStream(stream, payload) {
  const existing = rpcLineBuffers.get(stream) ?? "";
  const combined = existing + payload.toString("utf8");
  const lines = combined.split("\n");
  const tail = lines.pop() ?? "";
  rpcLineBuffers.set(stream, tail);

  for (const rawLine of lines) {
    const line = rawLine.trim();
    if (!line) continue;
    let parsed;
    try {
      parsed = JSON.parse(line);
    } catch {
      continue;
    }

    if (stream === "rpc_response") {
      rpcClient.handleResponse(parsed);
    } else {
      rpcClient.handleEvent(parsed);
    }
  }
}

function handleControlPayload(payload) {
  let parsed;
  try {
    parsed = JSON.parse(payload.toString("utf8"));
  } catch {
    return;
  }
  const type = String(parsed?.type ?? "");

  if (type === "pong") {
    lastPongAt = Date.now();
    return;
  }
  if (type === "open_ack") {
    noteOpenAck();
    return;
  }
  if (type === "ping") {
    const pong = { type: "pong" };
    if (Object.prototype.hasOwnProperty.call(parsed, "ts")) {
      pong.ts = parsed.ts;
    }
    sendControl(pong);
    return;
  }
  if (type === "close") {
    const reason = String(parsed?.reason ?? "remote_close");
    const code = parsed?.code;
    process.stderr.write(`\n[agent] remote closed session: reason=${reason} code=${code ?? "n/a"}\n`);
    requestShutdown(0).catch(() => {});
  }
}

function handleCompleteEnvelope(stream, payload) {
  if (stream === "rpc_event" || stream === "rpc_response") {
    handleRpcTextStream(stream, payload);
    return;
  }
  if (stream === "control") {
    handleControlPayload(payload);
  }
}

function ingestEnvelope(envelope) {
  if (!envelope || typeof envelope !== "object") return;
  if (Object.prototype.hasOwnProperty.call(envelope, "t")) {
    const legacyType = String(envelope.t ?? "").trim();
    if (legacyType === "stdout" || legacyType === "resize" || legacyType === "stdin" || legacyType === "exit") {
      sawLegacyPtyPayload = true;
      return;
    }
  }
  if (Number(envelope.v) !== FRAMED_PROTOCOL_VERSION) return;
  if (String(envelope.session_id ?? "").trim() !== sessionId) return;

  const stream = String(envelope.stream ?? "").trim();
  if (!STREAMS.has(stream)) return;

  const seq = Number(envelope.seq);
  const fragIndex = Number(envelope.frag_index);
  const fragCount = Number(envelope.frag_count);
  if (!Number.isInteger(seq) || seq < 0) return;
  if (!Number.isInteger(fragIndex) || fragIndex < 0) return;
  if (!Number.isInteger(fragCount) || fragCount <= 0 || fragIndex >= fragCount) return;

  const dedupeKey = `${sessionId}|${stream}|${seq}`;
  if (inProcessed.has(dedupeKey)) return;

  const expected = expectedInSeq.get(stream) ?? 0;
  if (seq < expected) {
    inProcessed.add(dedupeKey);
    return;
  }
  if (seq - expected > MAX_REORDER_WINDOW) {
    return;
  }

  let fragmentPayload;
  try {
    fragmentPayload = Buffer.from(String(envelope.payload_b64 ?? ""), "base64");
  } catch {
    return;
  }

  const fragKey = `${stream}|${seq}`;
  let bucket = inFragments.get(fragKey);
  if (!bucket) {
    bucket = { fragCount, parts: new Map() };
    inFragments.set(fragKey, bucket);
  } else if (bucket.fragCount !== fragCount) {
    inFragments.delete(fragKey);
    return;
  }

  if (!bucket.parts.has(fragIndex)) {
    bucket.parts.set(fragIndex, fragmentPayload);
  }

  if (bucket.parts.size < fragCount) return;

  const assembled = [];
  for (let index = 0; index < fragCount; index += 1) {
    const part = bucket.parts.get(index);
    if (!part) {
      return;
    }
    assembled.push(part);
  }
  inFragments.delete(fragKey);

  const pending = inPending.get(stream) ?? new Map();
  inPending.set(stream, pending);
  if (!pending.has(seq)) {
    pending.set(seq, Buffer.concat(assembled));
  }

  let nextSeq = expectedInSeq.get(stream) ?? 0;
  while (pending.has(nextSeq)) {
    const payload = pending.get(nextSeq);
    pending.delete(nextSeq);
    inProcessed.add(`${sessionId}|${stream}|${nextSeq}`);
    handleCompleteEnvelope(stream, payload);
    nextSeq += 1;
  }
  expectedInSeq.set(stream, nextSeq);
  pruneProcessedSet(stream, nextSeq);
}

function startHeartbeat() {
  lastPongAt = Date.now();
  heartbeatTimer = setInterval(() => {
    sendControl({ type: "ping", ts: Date.now() });
  }, HEARTBEAT_MS);
  heartbeatHealthTimer = setInterval(() => {
    if (Date.now() - lastPongAt > HEARTBEAT_TIMEOUT_MS) {
      process.stderr.write("\n[agent] heartbeat timeout\n");
      requestShutdown(1).catch(() => {});
    }
  }, 1000);
}

function stopHeartbeat() {
  if (heartbeatTimer) {
    clearInterval(heartbeatTimer);
    heartbeatTimer = null;
  }
  if (heartbeatHealthTimer) {
    clearInterval(heartbeatHealthTimer);
    heartbeatHealthTimer = null;
  }
}

function spawnDaemon() {
  const args = ["daemon", "--state-dir", STATE_DIR, "--allow-pubkey", BOT_PUBKEY];
  for (const relay of RELAYS) {
    args.push("--relay", relay);
  }

  daemon = spawn(MARMOTD_BIN, args, { stdio: ["pipe", "pipe", "pipe"] });
  daemonOutRl = readline.createInterface({ input: daemon.stdout });
  daemonErrRl = readline.createInterface({ input: daemon.stderr });

  daemonOutRl.on("line", (line) => {
    const trimmed = line.trim();
    if (!trimmed) return;
    let msg;
    try {
      msg = JSON.parse(trimmed);
    } catch {
      return;
    }

    emitEvent(msg);

    const type = String(msg?.type ?? "");
    if (type === "call_session_started") {
      const eventCallId = String(msg.call_id ?? "");
      if (eventCallId && staleInviteCallIds.has(eventCallId)) {
        sendEndCall(eventCallId, "stale_invite");
        return;
      }
    }
    if (type === "call_session_ended") {
      const eventCallId = String(msg.call_id ?? "");
      if (eventCallId) {
        pendingInviteCallIds.delete(eventCallId);
        staleInviteCallIds.delete(eventCallId);
      }
    }
    if (type === "error") {
      process.stderr.write(`[daemon] ${String(msg.message ?? "unknown error")}\n`);
      return;
    }
    if (USE_NOSTR_TRANSPORT && type === "message_received") {
      if (String(msg.nostr_group_id ?? "") !== GROUP_ID) return;
      if (String(msg.from_pubkey ?? "").toLowerCase() !== BOT_PUBKEY) return;
      const content = String(msg.content ?? "");
      if (!content.startsWith(RPC_MESSAGE_PREFIX)) return;
      try {
        ingestEnvelope(JSON.parse(content.slice(RPC_MESSAGE_PREFIX.length)));
      } catch {
        // Ignore malformed framed payloads.
      }
      return;
    }
    if (USE_NOSTR_TRANSPORT) {
      return;
    }
    if (type === "call_data" && callId && String(msg.call_id ?? "") === callId) {
      const payload = decodeCallPayload(msg);
      if (payload) {
        ingestEnvelope(payload);
      }
      return;
    }
    if (type === "call_session_ended" && callId && String(msg.call_id ?? "") === callId) {
      process.stderr.write(`\n[agent] call ended: ${String(msg.reason ?? "ended")}\n`);
      requestShutdown(0).catch(() => {});
    }
  });

  daemonErrRl.on("line", (line) => {
    if (MARMOTD_STDERR_MODE !== "show") return;
    const trimmed = line.trim();
    if (!trimmed) return;
    process.stderr.write(`[marmotd] ${trimmed}\n`);
  });

  daemon.on("exit", (code, signalName) => {
    if (!shutdownRequested) {
      process.stderr.write(`[agent] marmotd exited unexpectedly (code=${code ?? "n/a"} signal=${signalName ?? "n/a"})\n`);
      requestShutdown(1).catch(() => {});
    }
  });
}

function inviteCall(moqUrl) {
  const id = randomUUID();
  pendingInviteCallIds.add(id);
  sendCmd({
    cmd: "invite_call",
    call_id: id,
    nostr_group_id: GROUP_ID,
    peer_pubkey: BOT_PUBKEY,
    moq_url: moqUrl,
    broadcast_base: `pika/rpc/${id}`,
    track_name: "rpc0",
    track_codec: "bytes"
  });
  return id;
}

async function waitForReady() {
  await waitForEvent((msg) => String(msg?.type ?? "") === "ready", 15_000, "marmotd ready");
}

async function waitForCallStart(id) {
  try {
    const msg = await waitForEvent(
      (event) => {
        const type = String(event?.type ?? "");
        if (String(event?.call_id ?? "") !== id) return false;
        return type === "call_session_started" || type === "call_session_ended";
      },
      20_000,
      `call start ${id}`
    );
    const type = String(msg?.type ?? "");
    if (type === "call_session_started") {
      return true;
    }
    pendingInviteCallIds.delete(id);
    staleInviteCallIds.delete(id);
    return false;
  } catch {
    return false;
  }
}

async function sendExtensionUiResponse(responseObj) {
  rpcClient.sendRawRpcRequest(responseObj);
}

function setTerminalTitle(title) {
  const safe = String(title ?? "").replace(/[\u0007\u001b]/g, "");
  process.stdout.write(`\u001b]2;${safe}\u0007`);
}

async function processExtensionUiRequest(request) {
  if (!interactiveMode) return;

  const method = String(request?.method ?? "");
  if (!method) return;

  if (method === "notify") {
    interactiveMode.showExtensionNotify(String(request.message ?? ""), request.notifyType);
    return;
  }
  if (method === "setStatus") {
    interactiveMode.setExtensionStatus(String(request.statusKey ?? "status"), request.statusText);
    return;
  }
  if (method === "setWidget") {
    interactiveMode.setExtensionWidget(
      String(request.widgetKey ?? "widget"),
      Array.isArray(request.widgetLines) ? request.widgetLines.map((x) => String(x)) : undefined,
      { placement: request.widgetPlacement }
    );
    return;
  }
  if (method === "setTitle") {
    setTerminalTitle(String(request.title ?? "pi"));
    return;
  }
  if (method === "set_editor_text") {
    interactiveMode.editor?.setText?.(String(request.text ?? ""));
    interactiveMode.ui?.requestRender?.();
    return;
  }

  if (method === "select") {
    const value = await interactiveMode.showExtensionSelector(
      String(request.title ?? "Select"),
      Array.isArray(request.options) ? request.options.map((x) => String(x)) : [],
      { timeout: request.timeout }
    );
    if (value === undefined) {
      await sendExtensionUiResponse({ type: "extension_ui_response", id: request.id, cancelled: true });
    } else {
      await sendExtensionUiResponse({ type: "extension_ui_response", id: request.id, value });
    }
    return;
  }

  if (method === "confirm") {
    const confirmed = await interactiveMode.showExtensionConfirm(
      String(request.title ?? "Confirm"),
      String(request.message ?? ""),
      { timeout: request.timeout }
    );
    await sendExtensionUiResponse({ type: "extension_ui_response", id: request.id, confirmed });
    return;
  }

  if (method === "input") {
    const value = await interactiveMode.showExtensionInput(
      String(request.title ?? "Input"),
      request.placeholder ? String(request.placeholder) : undefined,
      { timeout: request.timeout }
    );
    if (value === undefined) {
      await sendExtensionUiResponse({ type: "extension_ui_response", id: request.id, cancelled: true });
    } else {
      await sendExtensionUiResponse({ type: "extension_ui_response", id: request.id, value });
    }
    return;
  }

  if (method === "editor") {
    const value = await interactiveMode.showExtensionEditor(
      String(request.title ?? "Editor"),
      request.prefill ? String(request.prefill) : undefined
    );
    if (value === undefined) {
      await sendExtensionUiResponse({ type: "extension_ui_response", id: request.id, cancelled: true });
    } else {
      await sendExtensionUiResponse({ type: "extension_ui_response", id: request.id, value });
    }
  }
}

rpcClient.onExtensionUiRequest((request) => {
  extensionUiChain = extensionUiChain
    .then(() => processExtensionUiRequest(request))
    .catch((error) => {
      process.stderr.write(`[agent] extension UI error: ${error.message}\n`);
      if (request?.id) {
        sendExtensionUiResponse({ type: "extension_ui_response", id: request.id, cancelled: true }).catch(() => {});
      }
    });
});

async function requestShutdown(exitCode) {
  if (shutdownRequested) return;
  shutdownRequested = true;

  stopHeartbeat();

  if (sessionId) {
    try {
      sendControl({ type: "close", reason: "user_exit" });
    } catch {
      // Ignore.
    }
  }

  if (callId) {
    sendEndCall(callId, "user_exit");
  }
  for (const staleId of [...pendingInviteCallIds, ...staleInviteCallIds]) {
    sendEndCall(staleId, "user_exit");
  }
  sendCmd({ cmd: "shutdown" });

  if (daemonOutRl) {
    daemonOutRl.close();
    daemonOutRl = null;
  }
  if (daemonErrRl) {
    daemonErrRl.close();
    daemonErrRl = null;
  }

  if (daemon && !daemon.killed) {
    daemon.kill("SIGTERM");
  }

  for (const waiter of [...openAckWaiters]) {
    openAckWaiters.delete(waiter);
    clearTimeout(waiter.timeout);
    waiter.resolve();
  }

  rpcClient.rejectPending(new Error("session shutting down"));
  await sleep(50);
  process.exit(exitCode);
}

async function main() {
  process.stderr.write("Launching RPC parity agent session (Pi InteractiveMode)...\n");
  if (MACHINE_ID && FLY_APP_NAME) {
    process.stderr.write(`machine: ${MACHINE_ID}  app: ${FLY_APP_NAME}\n`);
  }
  process.stderr.write(`transport: ${TRANSPORT_MODE}\n`);
  if (!USE_NOSTR_TRANSPORT) {
    process.stderr.write("MoQ candidates:\n");
    for (const url of EFFECTIVE_MOQ_URLS) {
      process.stderr.write(`  - ${url}\n`);
    }
  } else {
    process.stderr.write("relays:\n");
    for (const relay of RELAYS) {
      process.stderr.write(`  - ${relay}\n`);
    }
  }
  process.stderr.write("\n");

  spawnDaemon();
  await waitForReady();

  if (!USE_NOSTR_TRANSPORT) {
    let started = false;
    for (const moqUrl of EFFECTIVE_MOQ_URLS) {
      const invitedId = inviteCall(moqUrl);
      process.stderr.write(`[agent] inviting rpc call via ${moqUrl}...\n`);
      const ok = await waitForCallStart(invitedId);
      if (ok) {
        started = true;
        callId = invitedId;
        sessionId = invitedId;
        pendingInviteCallIds.delete(invitedId);
        staleInviteCallIds.delete(invitedId);
        resetFramingState();
        break;
      }
      pendingInviteCallIds.delete(invitedId);
      staleInviteCallIds.add(invitedId);
      sendEndCall(invitedId, "relay_fallback");
      process.stderr.write(`[agent] call did not start on ${moqUrl}, trying next relay...\n`);
    }

    if (!started || !callId || !sessionId) {
      throw new Error("failed to start RPC call on available MoQ relays");
    }
  } else {
    sessionId = randomUUID();
    callId = null;
    resetFramingState();
  }

  sendControl({
    type: "open",
    session_id: sessionId,
    term: process.env.TERM ?? "xterm-256color"
  });
  await waitForOpenAck(5_000);

  startHeartbeat();

  remoteSession = new RemoteAgentSessionAdapter(rpcClient);
  await remoteSession.syncRemoteState();

  interactiveMode = new InteractiveMode(remoteSession, {
    verbose: true
  });
  await interactiveMode.run();
}

process.on("SIGINT", () => {
  requestShutdown(0).catch(() => process.exit(0));
});

process.on("SIGTERM", () => {
  requestShutdown(0).catch(() => process.exit(0));
});

process.on("uncaughtException", (error) => {
  process.stderr.write(`[agent] fatal: ${error.message}\n`);
  requestShutdown(1).catch(() => process.exit(1));
});

process.on("unhandledRejection", (error) => {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`[agent] fatal: ${message}\n`);
  requestShutdown(1).catch(() => process.exit(1));
});

main().catch((error) => {
  process.stderr.write(`[agent] failed: ${error.message}\n`);
  requestShutdown(1).catch(() => process.exit(1));
});
