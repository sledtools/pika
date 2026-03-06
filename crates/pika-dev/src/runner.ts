import fs from "node:fs";
import path from "node:path";

import type { PikaDevConfig } from "./config.js";
import { PikaDevDb } from "./db.js";
import { SessionEventBus } from "./event-bus.js";
import type { GitOps, PullRequestInfo } from "./git.js";
import { buildIssuePrompt, buildSystemPrompt } from "./prompt.js";
import type { AgentBackend, AgentSession, SessionStatus } from "./types.js";

interface ActiveSessionContext {
  agentSession: AgentSession;
  unsubscribe: () => void;
  issueNumber: number;
  issueTitle: string;
  branchName: string;
  worktreePath: string;
  lastWipPushAtMs: number;
  wipPushPromise: Promise<void> | null;
}

export interface RunnerApi {
  startSession(sessionId: number): Promise<void>;
  followUpSession(sessionId: number, message: string): Promise<void>;
  steerSession(sessionId: number, npub: string | null, message: string, mode: "steer" | "followUp"): Promise<void>;
  abortSession(sessionId: number, reason: string): Promise<void>;
  getSessionExportPath(sessionId: number): string | null;
  isSessionActive(sessionId: number): boolean;
}

export class SessionRunner implements RunnerApi {
  private readonly activeSessions = new Map<number, ActiveSessionContext>();

  constructor(
    private readonly config: PikaDevConfig,
    private readonly db: PikaDevDb,
    private readonly git: GitOps,
    private readonly agentBackend: AgentBackend,
    private readonly eventBus: SessionEventBus,
  ) {}

  isSessionActive(sessionId: number): boolean {
    return this.activeSessions.has(sessionId);
  }

  getSessionExportPath(sessionId: number): string | null {
    const active = this.activeSessions.get(sessionId);
    if (active?.agentSession.sessionPath) {
      return active.agentSession.sessionPath;
    }

    const session = this.db.getSessionById(sessionId);
    return session?.pi_session_path ?? null;
  }

  async startSession(sessionId: number): Promise<void> {
    if (this.activeSessions.has(sessionId)) {
      return;
    }

    const joined = this.db.getSessionWithIssue(sessionId);
    if (!joined) {
      throw new Error(`session ${sessionId} not found`);
    }
    if (joined.session.status !== "queued") {
      return;
    }

    const { issue, session } = joined;
    const issueNumber = issue.issue_number;

    const worktree = await this.git.createWorktree(
      issueNumber,
      this.config.workspace_base,
      this.config.base_branch,
    );

    const sessionDirectory = path.join(this.config.session_base, `issue-${issueNumber}`);
    fs.mkdirSync(sessionDirectory, { recursive: true });

    const agentSession = await this.agentBackend.createSession({
      issueNumber,
      issueTitle: issue.title,
      issueBody: issue.body,
      workingDirectory: worktree.worktreePath,
      sessionDirectory,
      systemPrompt: buildSystemPrompt(joined),
    });

    this.db.markSessionStarting(
      session.id,
      worktree.branchName,
      worktree.worktreePath,
      agentSession.sessionPath,
    );

    const activeContext: ActiveSessionContext = {
      agentSession,
      unsubscribe: () => {},
      issueNumber,
      issueTitle: issue.title,
      branchName: worktree.branchName,
      worktreePath: worktree.worktreePath,
      lastWipPushAtMs: 0,
      wipPushPromise: null,
    };

    const unsubscribe = agentSession.subscribe((event) => {
      this.emitSessionEvent(session.id, event.type, event.payload);

      if (event.type === "turn_end") {
        void this.pushWipSnapshot(session.id, "turn_end");
      }
    });

    activeContext.unsubscribe = unsubscribe;
    this.activeSessions.set(session.id, activeContext);

    this.db.markSessionRunning(session.id);

    await this.commentIssue(
      issueNumber,
      [
        `pika-dev started working on issue #${issueNumber}.`,
        "",
        `Session: ${session.id}`,
        `Branch: ${worktree.branchName}`,
      ].join("\n"),
      activeContext.worktreePath,
    );

    try {
      const result = await agentSession.prompt(
        [buildSystemPrompt(joined), "", buildIssuePrompt(joined)].join("\n\n"),
      );
      const assistantMessage = this.db.addChatMessage(session.id, "assistant", null, result.summary);
      this.eventBus.publishChatMessage(session.id, assistantMessage);

      await this.flushWipSnapshot(session.id);

      const finalCommitMessage = `pika-dev: Fix #${issueNumber} - ${trimCommitTitle(issue.title)}`;
      const committed = await this.git.commitAllAndPush(
        worktree.worktreePath,
        worktree.branchName,
        finalCommitMessage,
      );
      const branchHasCommits = committed
        || await this.git.hasCommitsAhead(worktree.worktreePath, this.config.base_branch);

      let prInfo: PullRequestInfo = { number: null, url: null };
      if (branchHasCommits) {
        prInfo = await this.git.createDraftPr({
          worktreePath: worktree.worktreePath,
          repo: this.config.github_repo,
          title: `pika-dev: Fix #${issueNumber} - ${issue.title}`,
          body: buildPrBody(issueNumber, result.summary),
          base: baseBranchName(this.config.base_branch),
          head: worktree.branchName,
        });
      }

      this.db.markSessionCompleted(session.id, prInfo.number, prInfo.url);
      await this.commentIssue(
        issueNumber,
        buildIssueCompletionComment(session.id, issueNumber, prInfo),
        worktree.worktreePath,
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      this.emitSessionEvent(session.id, "error", { message });
      this.db.markSessionFailed(session.id, message);
      throw error;
    } finally {
      activeContext.unsubscribe();
      this.activeSessions.delete(session.id);
    }
  }

  async followUpSession(sessionId: number, message: string): Promise<void> {
    const active = this.activeSessions.get(sessionId);
    if (!active) {
      return;
    }

    await active.agentSession.followUp(message);
    this.emitSessionEvent(sessionId, "steer", {
      mode: "followUp",
      message,
    });
  }

  async steerSession(
    sessionId: number,
    npub: string | null,
    message: string,
    mode: "steer" | "followUp",
  ): Promise<void> {
    const session = this.db.getSessionById(sessionId);
    if (!session) {
      throw new Error(`session ${sessionId} not found`);
    }

    if (isTerminal(session.status)) {
      throw new Error(`session ${sessionId} is ${session.status}; chat is read-only`);
    }

    const userMessage = this.db.addChatMessage(sessionId, "user", npub, message);
    this.eventBus.publishChatMessage(sessionId, userMessage);

    const active = this.activeSessions.get(sessionId);
    if (!active) {
      throw new Error(`session ${sessionId} is not active`);
    }

    if (mode === "followUp") {
      await active.agentSession.followUp(message);
    } else {
      await active.agentSession.steer(message);
    }

    this.emitSessionEvent(sessionId, "steer", {
      npub,
      mode,
      message,
    });
  }

  async abortSession(sessionId: number, reason: string): Promise<void> {
    const active = this.activeSessions.get(sessionId);
    if (!active) {
      return;
    }

    await active.agentSession.abort(reason);
    this.emitSessionEvent(sessionId, "error", { message: reason });
  }

  private async pushWipSnapshot(sessionId: number, reason: string): Promise<void> {
    const active = this.activeSessions.get(sessionId);
    if (!active) {
      return;
    }

    const now = Date.now();
    if (now - active.lastWipPushAtMs < 30_000) {
      return;
    }
    if (active.wipPushPromise) {
      return;
    }

    active.lastWipPushAtMs = now;
    active.wipPushPromise = this.git
      .commitAllAndPush(
        active.worktreePath,
        active.branchName,
        `pika-dev: WIP #${active.issueNumber} (${reason})`,
      )
      .then(() => {
        // no-op
      })
      .catch((error) => {
        const message = error instanceof Error ? error.message : String(error);
        this.emitSessionEvent(sessionId, "error", {
          message: `WIP push failed: ${message}`,
        });
      })
      .finally(() => {
        active.wipPushPromise = null;
      });

    await active.wipPushPromise;
  }

  private async flushWipSnapshot(sessionId: number): Promise<void> {
    const active = this.activeSessions.get(sessionId);
    if (!active || !active.wipPushPromise) {
      return;
    }
    await active.wipPushPromise;
  }

  private async commentIssue(issueNumber: number, body: string, cwd: string): Promise<void> {
    try {
      await this.git.commentIssue(this.config.github_repo, issueNumber, body, cwd);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      console.warn(`warning: failed to comment issue #${issueNumber}: ${message}`);
    }
  }

  private emitSessionEvent(
    sessionId: number,
    eventType: Parameters<PikaDevDb["addSessionEvent"]>[1],
    payload: Record<string, unknown>,
  ): void {
    const persisted = this.db.addSessionEvent(sessionId, eventType, payload);
    this.eventBus.publishSessionEvent(sessionId, persisted);
  }
}

function trimCommitTitle(title: string): string {
  const normalized = title.trim().replace(/\s+/g, " ");
  return normalized.length <= 72 ? normalized : `${normalized.slice(0, 69)}...`;
}

function buildPrBody(issueNumber: number, summary: string): string {
  return [
    `Automated fix for #${issueNumber}.`,
    "",
    "## Summary",
    summary,
    "",
    "---",
    "Generated by pika-dev using the Pi coding agent.",
    `Fork locally: \`just dev-fork ${issueNumber}\``,
  ].join("\n");
}

function buildIssueCompletionComment(
  sessionId: number,
  issueNumber: number,
  prInfo: PullRequestInfo,
): string {
  const lines = [
    `pika-dev finished issue #${issueNumber}.`,
    "",
  ];

  if (prInfo.number && prInfo.url) {
    lines.push(`Draft PR: #${prInfo.number} (${prInfo.url})`);
  } else {
    lines.push("No PR was created (no diff to commit).");
  }

  lines.push("", "Fork locally:", `just dev-fork ${issueNumber}`, "", `Session: ${sessionId}`);
  return lines.join("\n");
}

function baseBranchName(baseBranch: string): string {
  return baseBranch.startsWith("origin/") ? baseBranch.slice("origin/".length) : baseBranch;
}

function isTerminal(status: SessionStatus): boolean {
  return status === "completed" || status === "failed" || status === "cancelled";
}
