import assert from "node:assert/strict";
import path from "node:path";
import { describe, it } from "node:test";

import { PikaDevDb } from "../src/db.js";
import type { GitHubIssue } from "../src/types.js";
import { createTempDir } from "./helpers.js";

function sampleIssue(number: number): GitHubIssue {
  return {
    number,
    title: `Issue ${number}`,
    body: "body",
    state: "open",
    user: "alice",
    labels: ["pika-dev"],
    updatedAt: new Date().toISOString(),
    createdAt: new Date().toISOString(),
  };
}

describe("db", () => {
  it("upserts issues and queues sessions", () => {
    const dir = createTempDir("pika-dev-db-test");
    const db = new PikaDevDb(path.join(dir, "pika-dev.db"));

    const first = db.upsertIssue(sampleIssue(101), new Date().toISOString());
    assert.equal(first.isNew, true);

    const queued = db.ensureQueuedSession(first.issue.id);
    assert.equal(queued.status, "queued");

    const same = db.ensureQueuedSession(first.issue.id);
    assert.equal(same.id, queued.id);

    const oldest = db.getOldestQueuedSession();
    assert.ok(oldest);
    assert.equal(oldest.issue.issue_number, 101);

    db.close();
  });

  it("stores events and chat messages", () => {
    const dir = createTempDir("pika-dev-db-events");
    const db = new PikaDevDb(path.join(dir, "pika-dev.db"));

    const issue = db.upsertIssue(sampleIssue(202), new Date().toISOString()).issue;
    const session = db.ensureQueuedSession(issue.id);

    db.addSessionEvent(session.id, "agent_start", { model: "fake" });
    db.addChatMessage(session.id, "user", "npub1x", "hello");

    const events = db.listSessionEvents(session.id, 10);
    const chats = db.listChatMessages(session.id, 10);

    assert.equal(events.length, 1);
    assert.equal(chats.length, 1);
    assert.equal(chats[0].content, "hello");

    db.close();
  });
});
