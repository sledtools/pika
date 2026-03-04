import assert from "node:assert/strict";
import path from "node:path";
import { describe, it } from "node:test";

import { PikaDevDb } from "../src/db.js";
import type { PikaDevConfig } from "../src/config.js";
import type { GitHubClient } from "../src/github.js";
import { IssuePoller, type PollerRunnerApi } from "../src/poller.js";
import type { GitHubIssue } from "../src/types.js";
import { createTestConfig, createTempDir } from "./helpers.js";

class FakeGitHub implements GitHubClient {
  constructor(
    public listResult: GitHubIssue[],
    private readonly detailByNumber: Map<number, GitHubIssue | null> = new Map(),
  ) {}

  async listOpenIssuesWithLabel(): Promise<GitHubIssue[]> {
    return this.listResult;
  }

  async getIssue(issueNumber: number): Promise<GitHubIssue | null> {
    return this.detailByNumber.get(issueNumber) ?? null;
  }
}

class FakeRunner implements PollerRunnerApi {
  public followUps: Array<{ sessionId: number; message: string }> = [];
  public aborts: Array<{ sessionId: number; reason: string }> = [];

  async followUpSession(sessionId: number, message: string): Promise<void> {
    this.followUps.push({ sessionId, message });
  }

  async abortSession(sessionId: number, reason: string): Promise<void> {
    this.aborts.push({ sessionId, reason });
  }
}

function buildIssue(number: number, body = "body"): GitHubIssue {
  const now = new Date().toISOString();
  return {
    number,
    title: `Issue ${number}`,
    body,
    state: "open",
    user: "alice",
    labels: ["pika-dev"],
    updatedAt: now,
    createdAt: now,
  };
}

function makeDbAndConfig(): { db: PikaDevDb; config: PikaDevConfig } {
  const dir = createTempDir("pika-dev-poller");
  const config = createTestConfig({
    dbPath: path.join(dir, "pika-dev.db"),
    workspace_base: path.join(dir, "workspaces"),
    session_base: path.join(dir, "sessions"),
  });
  return {
    db: new PikaDevDb(config.dbPath),
    config,
  };
}

describe("poller", () => {
  it("queues newly discovered issues", async () => {
    const { db, config } = makeDbAndConfig();
    const github = new FakeGitHub([buildIssue(11)]);
    const runner = new FakeRunner();

    const poller = new IssuePoller(config, db, github, runner);
    await poller.runOnce();

    const issue = db.getIssueByNumber(11);
    assert.ok(issue);

    const active = db.findActiveSessionForIssue(issue.id);
    assert.ok(active);
    assert.equal(active.status, "queued");

    db.close();
  });

  it("cancels active sessions when issue closes", async () => {
    const { db, config } = makeDbAndConfig();
    const issue = db.upsertIssue(buildIssue(22), new Date().toISOString()).issue;
    const session = db.ensureQueuedSession(issue.id);
    db.markSessionRunning(session.id);

    const detail = {
      ...buildIssue(22),
      state: "closed" as const,
      labels: ["pika-dev"],
    };
    const github = new FakeGitHub([], new Map([[22, detail]]));
    const runner = new FakeRunner();

    const poller = new IssuePoller(config, db, github, runner);
    await poller.runOnce();

    const updated = db.getIssueByNumber(22);
    assert.ok(updated);
    assert.equal(updated.state, "closed");
    assert.equal(runner.aborts.length, 1);

    db.close();
  });

  it("sends follow-up steering when issue body changes while running", async () => {
    const { db, config } = makeDbAndConfig();
    const initial = buildIssue(33, "first");
    const row = db.upsertIssue(initial, new Date().toISOString()).issue;
    const session = db.ensureQueuedSession(row.id);
    db.markSessionRunning(session.id);

    const updated = {
      ...initial,
      body: "second",
      updatedAt: new Date(Date.now() + 5000).toISOString(),
    };

    const github = new FakeGitHub([updated]);
    const runner = new FakeRunner();
    const poller = new IssuePoller(config, db, github, runner);

    await poller.runOnce();

    assert.equal(runner.followUps.length, 1);
    assert.equal(runner.followUps[0].sessionId, session.id);

    db.close();
  });

  it("ignores comment-only issue updates while running", async () => {
    const { db, config } = makeDbAndConfig();
    const initial = buildIssue(34, "first");
    const row = db.upsertIssue(initial, new Date().toISOString()).issue;
    const session = db.ensureQueuedSession(row.id);
    db.markSessionRunning(session.id);

    const updated = {
      ...initial,
      updatedAt: new Date(Date.now() + 5000).toISOString(),
    };

    const github = new FakeGitHub([updated]);
    const runner = new FakeRunner();
    const poller = new IssuePoller(config, db, github, runner);

    await poller.runOnce();

    assert.equal(runner.followUps.length, 0);

    db.close();
  });

  it("does not queue a new session for comment-only issue updates", async () => {
    const { db, config } = makeDbAndConfig();
    const initial = buildIssue(35, "first");
    const row = db.upsertIssue(initial, new Date().toISOString()).issue;
    const session = db.ensureQueuedSession(row.id);
    db.markSessionCompleted(session.id, 123, "https://github.com/sledtools/pika/pull/123");

    const updated = {
      ...initial,
      updatedAt: new Date(Date.now() + 5000).toISOString(),
    };

    const github = new FakeGitHub([updated]);
    const runner = new FakeRunner();
    const poller = new IssuePoller(config, db, github, runner);

    await poller.runOnce();

    const active = db.findActiveSessionForIssue(row.id);
    assert.equal(active, null);
    assert.equal(runner.followUps.length, 0);

    db.close();
  });
});
