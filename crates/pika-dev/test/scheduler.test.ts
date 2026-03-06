import assert from "node:assert/strict";
import path from "node:path";
import { describe, it } from "node:test";

import type { Database as SqliteDb } from "better-sqlite3";

import { PikaDevDb } from "../src/db.js";
import { SessionScheduler } from "../src/scheduler.js";
import type { RunnerApi } from "../src/runner.js";
import { createTestConfig, createTempDir } from "./helpers.js";

class FakeRunner implements RunnerApi {
  public starts: number[] = [];
  public aborts: Array<{ sessionId: number; reason: string }> = [];

  async startSession(sessionId: number): Promise<void> {
    this.starts.push(sessionId);
  }

  async followUpSession(_sessionId: number, _message: string): Promise<void> {
    // no-op
  }

  async steerSession(
    _sessionId: number,
    _npub: string | null,
    _message: string,
    _mode: "steer" | "followUp",
  ): Promise<void> {
    // no-op
  }

  async abortSession(sessionId: number, reason: string): Promise<void> {
    this.aborts.push({ sessionId, reason });
  }

  getSessionExportPath(_sessionId: number): string | null {
    return null;
  }

  isSessionActive(_sessionId: number): boolean {
    return false;
  }
}

describe("scheduler", () => {
  it("starts only up to max concurrent sessions", async () => {
    const dir = createTempDir("pika-dev-scheduler");
    const config = createTestConfig({
      max_concurrent_sessions: 1,
      dbPath: path.join(dir, "db.sqlite"),
    });
    const db = new PikaDevDb(config.dbPath);

    const issueA = db.upsertIssue(
      {
        number: 1,
        title: "a",
        body: "",
        state: "open",
        user: "x",
        labels: ["pika-dev"],
        updatedAt: new Date().toISOString(),
        createdAt: new Date().toISOString(),
      },
      new Date().toISOString(),
    ).issue;
    const issueB = db.upsertIssue(
      {
        number: 2,
        title: "b",
        body: "",
        state: "open",
        user: "x",
        labels: ["pika-dev"],
        updatedAt: new Date().toISOString(),
        createdAt: new Date().toISOString(),
      },
      new Date().toISOString(),
    ).issue;

    db.ensureQueuedSession(issueA.id);
    db.ensureQueuedSession(issueB.id);

    const runner = new FakeRunner();
    const scheduler = new SessionScheduler(config, db, runner);
    await scheduler.tick();

    assert.equal(runner.starts.length, 1);

    db.close();
  });

  it("fails timed out running sessions", async () => {
    const dir = createTempDir("pika-dev-scheduler-timeout");
    const config = createTestConfig({
      session_timeout_mins: 1,
      dbPath: path.join(dir, "db.sqlite"),
    });
    const db = new PikaDevDb(config.dbPath);

    const issue = db.upsertIssue(
      {
        number: 30,
        title: "old",
        body: "",
        state: "open",
        user: "x",
        labels: ["pika-dev"],
        updatedAt: new Date().toISOString(),
        createdAt: new Date().toISOString(),
      },
      new Date().toISOString(),
    ).issue;

    const session = db.ensureQueuedSession(issue.id);
    db.markSessionRunning(session.id);
    const rawDb = (db as unknown as { db: SqliteDb }).db;
    rawDb
      .prepare("UPDATE sessions SET started_at = datetime('now', '-2 minutes') WHERE id = ?")
      .run(session.id);

    const runner = new FakeRunner();
    const scheduler = new SessionScheduler(config, db, runner);
    await scheduler.tick();

    const updated = db.getSessionById(session.id);
    assert.ok(updated);
    assert.equal(updated.status, "failed");
    assert.equal(runner.aborts.length, 1);

    db.close();
  });

  it("does not fail fresh running sessions before timeout window", async () => {
    const dir = createTempDir("pika-dev-scheduler-fresh");
    const config = createTestConfig({
      session_timeout_mins: 30,
      dbPath: path.join(dir, "db.sqlite"),
    });
    const db = new PikaDevDb(config.dbPath);

    const issue = db.upsertIssue(
      {
        number: 31,
        title: "fresh",
        body: "",
        state: "open",
        user: "x",
        labels: ["pika-dev"],
        updatedAt: new Date().toISOString(),
        createdAt: new Date().toISOString(),
      },
      new Date().toISOString(),
    ).issue;

    const session = db.ensureQueuedSession(issue.id);
    db.markSessionRunning(session.id);

    const runner = new FakeRunner();
    const scheduler = new SessionScheduler(config, db, runner);
    await scheduler.tick();

    const updated = db.getSessionById(session.id);
    assert.ok(updated);
    assert.equal(updated.status, "running");
    assert.equal(runner.aborts.length, 0);

    db.close();
  });
});
