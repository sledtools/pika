import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { describe, it } from "node:test";

import { AuthState } from "../src/auth.js";
import type { PikaDevConfig } from "../src/config.js";
import { PikaDevDb } from "../src/db.js";
import { SessionEventBus } from "../src/event-bus.js";
import { createApp } from "../src/server.js";
import type { RunnerApi } from "../src/runner.js";
import { createTempDir, createTestConfig } from "./helpers.js";

class FakeRunner implements RunnerApi {
  public steerCalls: Array<{ sessionId: number; message: string; mode: "steer" | "followUp"; npub: string | null }> = [];
  public exportPathBySession = new Map<number, string>();

  async startSession(_sessionId: number): Promise<void> {}

  async followUpSession(_sessionId: number, _message: string): Promise<void> {}

  async steerSession(
    sessionId: number,
    npub: string | null,
    message: string,
    mode: "steer" | "followUp",
  ): Promise<void> {
    this.steerCalls.push({ sessionId, npub, message, mode });
  }

  async abortSession(_sessionId: number, _reason: string): Promise<void> {}

  getSessionExportPath(sessionId: number): string | null {
    return this.exportPathBySession.get(sessionId) ?? null;
  }

  isSessionActive(_sessionId: number): boolean {
    return true;
  }
}

function bootstrap(): {
  app: ReturnType<typeof createApp>;
  db: PikaDevDb;
  config: PikaDevConfig;
  sessionId: number;
  runner: FakeRunner;
  exportPath: string;
  auth: AuthState;
} {
  const root = createTempDir("pika-dev-server");
  const config = createTestConfig({
    dbPath: path.join(root, "pika-dev.db"),
    workspace_base: path.join(root, "workspaces"),
    session_base: path.join(root, "sessions"),
  });

  const db = new PikaDevDb(config.dbPath);
  const issue = db.upsertIssue(
    {
      number: 123,
      title: "Server test issue",
      body: "body",
      state: "open",
      user: "alice",
      labels: ["pika-dev"],
      updatedAt: new Date().toISOString(),
      createdAt: new Date().toISOString(),
    },
    new Date().toISOString(),
  ).issue;

  const session = db.ensureQueuedSession(issue.id);
  db.markSessionRunning(session.id);
  db.addSessionEvent(session.id, "agent_start", { from: "test" });
  db.addChatMessage(session.id, "assistant", null, "hello");

  const runner = new FakeRunner();
  const exportPath = path.join(root, "session-export.jsonl");
  fs.writeFileSync(exportPath, '{"type":"agent_start"}\n');
  runner.exportPathBySession.set(session.id, exportPath);

  const auth = new AuthState([]);
  const app = createApp({
    config,
    db,
    runner,
    auth,
    eventBus: new SessionEventBus(),
  });

  return {
    app,
    db,
    config,
    sessionId: session.id,
    runner,
    exportPath,
    auth,
  };
}

describe("server", () => {
  it("renders dashboard and session pages", async () => {
    const { app, db, sessionId } = bootstrap();

    const dashboard = await app.request("http://localhost/");
    assert.equal(dashboard.status, 200);
    const html = await dashboard.text();
    assert.match(html, /Server test issue/);

    const session = await app.request(`http://localhost/session/${sessionId}`);
    assert.equal(session.status, 200);
    const sessionHtml = await session.text();
    assert.match(sessionHtml, /Agent Events/);

    db.close();
  });

  it("accepts chat and exports session when auth disabled", async () => {
    const { app, db, sessionId, runner } = bootstrap();

    const chat = await app.request(`http://localhost/session/${sessionId}/chat`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        message: "please run tests",
        mode: "steer",
      }),
    });

    assert.equal(chat.status, 200);
    assert.equal(runner.steerCalls.length, 1);
    assert.equal(runner.steerCalls[0].message, "please run tests");

    const exportResp = await app.request(`http://localhost/session/${sessionId}/export`);
    assert.equal(exportResp.status, 200);
    const payload = await exportResp.text();
    assert.match(payload, /agent_start/);

    db.close();
  });

  it("requires auth for chat/export when allowlist is configured", async () => {
    const { db, config, sessionId, runner } = bootstrap();
    const auth = new AuthState([
      "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y",
    ]);
    const app = createApp({
      config,
      db,
      runner,
      auth,
      eventBus: new SessionEventBus(),
    });

    const chat = await app.request(`http://localhost/session/${sessionId}/chat`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ message: "unauthorized chat" }),
    });
    assert.equal(chat.status, 401);

    const exportResp = await app.request(`http://localhost/session/${sessionId}/export`);
    assert.equal(exportResp.status, 401);

    db.close();
  });
});
