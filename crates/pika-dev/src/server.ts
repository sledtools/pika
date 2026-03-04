import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { getCookie, setCookie } from "hono/cookie";
import { Hono } from "hono";

import type { PikaDevConfig } from "./config.js";
import { AuthState } from "./auth.js";
import { PikaDevDb } from "./db.js";
import { SessionEventBus } from "./event-bus.js";
import type { RunnerApi } from "./runner.js";
import { renderDashboard } from "./templates/dashboard.js";
import { renderSessionPage } from "./templates/session.js";

interface AppContext {
  config: PikaDevConfig;
  db: PikaDevDb;
  runner: RunnerApi;
  auth: AuthState;
  eventBus: SessionEventBus;
}

export function createApp(context: AppContext): Hono {
  const app = new Hono();

  app.get("/health", (c) => {
    return c.json({
      ok: true,
      service: "pika-dev",
      now: new Date().toISOString(),
    });
  });

  app.get("/static/style.css", (c) => {
    const cssPath = resolveStylePath();
    const css = fs.readFileSync(cssPath, "utf8");
    c.header("Content-Type", "text/css; charset=utf-8");
    return c.body(css);
  });

  app.post("/auth/challenge", (c) => {
    const nonce = context.auth.createChallenge();
    return c.json({ nonce });
  });

  app.post("/auth/verify", async (c) => {
    const body = await c.req.json();
    const eventJson = resolveEventJson(body);
    if (!eventJson) {
      return c.text("Missing event JSON", 400);
    }

    try {
      const verified = context.auth.verifyEvent(eventJson);
      setCookie(c, "pika_dev_token", verified.token, {
        httpOnly: true,
        secure: false,
        maxAge: 24 * 60 * 60,
        sameSite: "Lax",
        path: "/",
      });
      return c.json({
        token: verified.token,
        npub: verified.npub,
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      return c.text(message, 401);
    }
  });

  app.get("/", (c) => {
    const snapshot = context.db.getDashboardSnapshot(context.config.max_concurrent_sessions);
    return c.html(renderDashboard(snapshot));
  });

  app.get("/session/:id", (c) => {
    const sessionId = parseSessionId(c.req.param("id"));
    if (!sessionId) {
      return c.text("invalid session id", 400);
    }

    const entry = context.db.getSessionWithIssue(sessionId);
    if (!entry) {
      return c.text("session not found", 404);
    }

    const viewerNpub = resolveViewerNpub(c.req.header("Authorization"), getCookie(c, "pika_dev_token"), context.auth);

    return c.html(
      renderSessionPage({
        entry,
        events: context.db.listSessionEvents(sessionId, 500),
        chatMessages: context.db.listChatMessages(sessionId, 500),
        authEnabled: context.auth.isEnabled(),
        viewerNpub,
      }),
    );
  });

  app.get("/session/:id/events", (c) => {
    const sessionId = parseSessionId(c.req.param("id"));
    if (!sessionId) {
      return c.text("invalid session id", 400);
    }

    const stream = new TransformStream();
    const writer = stream.writable.getWriter();
    const encoder = new TextEncoder();

    let closed = false;

    const send = (event: string, data: unknown): void => {
      if (closed) {
        return;
      }
      const payload = `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
      void writer.write(encoder.encode(payload)).catch(() => {
        closed = true;
      });
    };

    for (const event of context.db.listSessionEvents(sessionId, 500)) {
      send("session_event", event);
    }
    for (const message of context.db.listChatMessages(sessionId, 500)) {
      send("chat_message", message);
    }

    const unsubscribeEvents = context.eventBus.subscribeSessionEvents(sessionId, (event) => {
      send("session_event", event);
    });
    const unsubscribeChat = context.eventBus.subscribeChatMessages(sessionId, (message) => {
      send("chat_message", message);
    });

    const keepAlive = setInterval(() => {
      send("ping", { now: new Date().toISOString() });
    }, 15_000);

    const close = (): void => {
      if (closed) {
        return;
      }
      closed = true;
      clearInterval(keepAlive);
      unsubscribeEvents();
      unsubscribeChat();
      void writer.close();
    };

    c.req.raw.signal.addEventListener("abort", close, { once: true });

    return new Response(stream.readable, {
      headers: {
        "Content-Type": "text/event-stream",
        "Cache-Control": "no-cache, no-transform",
        Connection: "keep-alive",
      },
    });
  });

  app.post("/session/:id/chat", async (c) => {
    const sessionId = parseSessionId(c.req.param("id"));
    if (!sessionId) {
      return c.text("invalid session id", 400);
    }

    const viewer = requireViewer(c.req.header("Authorization"), getCookie(c, "pika_dev_token"), context.auth);
    if (!viewer.ok) {
      return c.text(viewer.message, 401);
    }

    const body = await c.req.json();
    const message = typeof body.message === "string" ? body.message.trim() : "";
    if (!message) {
      return c.text("message is required", 400);
    }

    const mode = body.mode === "followUp" ? "followUp" : "steer";

    try {
      await context.runner.steerSession(sessionId, viewer.npub, message, mode);
      return c.json({ ok: true });
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      return c.text(msg, 400);
    }
  });

  app.get("/session/:id/export", (c) => {
    const sessionId = parseSessionId(c.req.param("id"));
    if (!sessionId) {
      return c.text("invalid session id", 400);
    }

    const viewer = requireViewer(c.req.header("Authorization"), getCookie(c, "pika_dev_token"), context.auth);
    if (!viewer.ok) {
      return c.text(viewer.message, 401);
    }

    const exportPath = context.runner.getSessionExportPath(sessionId);
    if (!exportPath || !fs.existsSync(exportPath)) {
      return c.text("session export not found", 404);
    }

    const content = fs.readFileSync(exportPath, "utf8");
    c.header("Content-Type", "application/x-ndjson; charset=utf-8");
    c.header("Content-Disposition", `attachment; filename="session-${sessionId}.jsonl"`);
    return c.body(content);
  });

  return app;
}

function parseSessionId(raw: string): number | null {
  const parsed = Number.parseInt(raw, 10);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    return null;
  }
  return parsed;
}

function requireViewer(
  authorizationHeader: string | undefined,
  cookieToken: string | undefined,
  auth: AuthState,
):
  | { ok: true; npub: string | null; message: string }
  | { ok: false; npub: null; message: string } {
  if (!auth.isEnabled()) {
    return { ok: true, npub: null, message: "auth disabled" };
  }

  const npub = resolveViewerNpub(authorizationHeader, cookieToken, auth);
  if (!npub) {
    return { ok: false, npub: null, message: "auth required" };
  }

  return { ok: true, npub, message: "ok" };
}

function resolveViewerNpub(
  authorizationHeader: string | undefined,
  cookieToken: string | undefined,
  auth: AuthState,
): string | null {
  const token = extractToken(authorizationHeader) ?? cookieToken;
  if (!token) {
    return null;
  }
  return auth.validateToken(token);
}

function extractToken(header: string | undefined): string | null {
  if (!header) {
    return null;
  }
  const trimmed = header.trim();
  const prefix = "Bearer ";
  if (!trimmed.startsWith(prefix)) {
    return null;
  }
  return trimmed.slice(prefix.length).trim();
}

function resolveStylePath(): string {
  const localFile = fileURLToPath(new URL("../static/style.css", import.meta.url));
  const candidates = [
    path.resolve(process.cwd(), "crates/pika-dev/static/style.css"),
    path.resolve(process.cwd(), "static/style.css"),
    localFile,
  ];

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }

  throw new Error("style.css not found");
}

function resolveEventJson(body: unknown): string | null {
  if (!body || typeof body !== "object") {
    return null;
  }

  const record = body as Record<string, unknown>;
  if (typeof record.eventJson === "string") {
    return record.eventJson;
  }
  if (typeof record.event === "string") {
    return record.event;
  }
  if (record.event && typeof record.event === "object") {
    return JSON.stringify(record.event);
  }
  return null;
}
