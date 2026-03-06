import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { describe, it } from "node:test";

import { FakeAgentBackend } from "../src/agent.js";
import { PikaDevDb } from "../src/db.js";
import { SessionEventBus } from "../src/event-bus.js";
import type { GitOps, PullRequestInfo, WorktreeInfo } from "../src/git.js";
import { SessionRunner } from "../src/runner.js";
import { createTempDir, createTestConfig } from "./helpers.js";

class FakeGitOps implements GitOps {
  public commits: string[] = [];
  public comments: string[] = [];

  constructor(private readonly worktreePath: string) {}

  async createWorktree(issueNumber: number, _workspaceBase: string, _baseBranch: string): Promise<WorktreeInfo> {
    fs.mkdirSync(this.worktreePath, { recursive: true });
    return {
      branchName: `pika-dev/issue-${issueNumber}`,
      worktreePath: this.worktreePath,
    };
  }

  async cleanupWorktree(_worktreePath: string): Promise<void> {
    // no-op
  }

  async commitAllAndPush(_worktreePath: string, _branchName: string, message: string): Promise<boolean> {
    this.commits.push(message);
    return true;
  }

  async hasCommitsAhead(_worktreePath: string, _baseBranch: string): Promise<boolean> {
    return false;
  }

  async createDraftPr(_options: {
    worktreePath: string;
    repo: string;
    title: string;
    body: string;
    base: string;
    head: string;
  }): Promise<PullRequestInfo> {
    return {
      number: 999,
      url: "https://github.com/sledtools/pika/pull/999",
    };
  }

  async commentIssue(_repo: string, _issueNumber: number, body: string, _cwd: string): Promise<void> {
    this.comments.push(body);
  }
}

describe("runner", () => {
  it("runs queued session to completion with fake backend", async () => {
    const root = createTempDir("pika-dev-runner");
    const config = createTestConfig({
      dbPath: path.join(root, "pika-dev.db"),
      workspace_base: path.join(root, "workspaces"),
      session_base: path.join(root, "sessions"),
    });
    const db = new PikaDevDb(config.dbPath);

    const issue = db.upsertIssue(
      {
        number: 77,
        title: "Runner issue",
        body: "Implement something",
        state: "open",
        user: "alice",
        labels: ["pika-dev"],
        updatedAt: new Date().toISOString(),
        createdAt: new Date().toISOString(),
      },
      new Date().toISOString(),
    ).issue;

    const session = db.ensureQueuedSession(issue.id);

    const git = new FakeGitOps(path.join(root, "workspaces", "issue-77"));
    const eventBus = new SessionEventBus();
    const events: string[] = [];
    const unsubscribe = eventBus.subscribeSessionEvents(session.id, (event) => {
      events.push(event.event_type);
    });

    const runner = new SessionRunner(config, db, git, new FakeAgentBackend(), eventBus);
    await runner.startSession(session.id);

    const updated = db.getSessionById(session.id);
    assert.ok(updated);
    assert.equal(updated.status, "completed");
    assert.equal(updated.pr_number, 999);
    assert.ok(updated.pr_url);

    const chats = db.listChatMessages(session.id, 20);
    assert.ok(chats.some((chat) => chat.role === "assistant"));

    assert.ok(events.includes("agent_start"));
    assert.ok(events.includes("agent_end"));

    unsubscribe();
    db.close();
  });

  it("rejects steering terminal sessions", async () => {
    const root = createTempDir("pika-dev-runner-terminal");
    const config = createTestConfig({
      dbPath: path.join(root, "pika-dev.db"),
    });
    const db = new PikaDevDb(config.dbPath);

    const issue = db.upsertIssue(
      {
        number: 88,
        title: "Terminal issue",
        body: "",
        state: "open",
        user: "alice",
        labels: ["pika-dev"],
        updatedAt: new Date().toISOString(),
        createdAt: new Date().toISOString(),
      },
      new Date().toISOString(),
    ).issue;

    const session = db.ensureQueuedSession(issue.id);
    db.markSessionCompleted(session.id, null, null);

    const runner = new SessionRunner(
      config,
      db,
      new FakeGitOps(path.join(root, "worktree")),
      new FakeAgentBackend(),
      new SessionEventBus(),
    );

    await assert.rejects(
      runner.steerSession(session.id, null, "hello", "steer"),
      /read-only/,
    );

    db.close();
  });
});
