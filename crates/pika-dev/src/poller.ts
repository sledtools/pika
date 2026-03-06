import type { PikaDevConfig } from "./config.js";
import { PikaDevDb } from "./db.js";
import type { GitHubClient } from "./github.js";

export interface PollerRunnerApi {
  followUpSession(sessionId: number, message: string): Promise<void>;
  abortSession(sessionId: number, reason: string): Promise<void>;
}

export class IssuePoller {
  private timer: NodeJS.Timeout | null = null;
  private inFlight = false;

  constructor(
    private readonly config: PikaDevConfig,
    private readonly db: PikaDevDb,
    private readonly github: GitHubClient,
    private readonly runner: PollerRunnerApi,
  ) {}

  start(): void {
    if (this.timer) {
      return;
    }

    this.timer = setInterval(() => {
      void this.safeRunOnce();
    }, this.config.poll_interval_secs * 1000);
  }

  stop(): void {
    if (!this.timer) {
      return;
    }
    clearInterval(this.timer);
    this.timer = null;
  }

  async runOnce(): Promise<void> {
    if (this.inFlight) {
      return;
    }

    this.inFlight = true;
    try {
      const now = new Date().toISOString();
      const issues = await this.github.listOpenIssuesWithLabel();
      const seen = new Set<number>();

      for (const issue of issues) {
        seen.add(issue.number);
        const result = this.db.upsertIssue(issue, now);
        const activeSession = this.db.findActiveSessionForIssue(result.issue.id);

        if (result.isNew) {
          this.db.ensureQueuedSession(result.issue.id);
          continue;
        }

        if (!result.updatedChanged) {
          continue;
        }
        if (!result.meaningfulChanged) {
          continue;
        }

        if (activeSession && activeSession.status === "running") {
          const message = `Issue #${issue.number} was updated on GitHub. Re-read the issue body and adapt your plan.`;
          this.db.addChatMessage(activeSession.id, "system", null, message);
          await this.runner.followUpSession(activeSession.id, message);
          continue;
        }

        if (!activeSession) {
          this.db.ensureQueuedSession(result.issue.id);
        }
      }

      await this.handleMissingTrackedIssues(now, seen);
    } finally {
      this.inFlight = false;
    }
  }

  private async safeRunOnce(): Promise<void> {
    try {
      await this.runOnce();
    } catch (error) {
      console.error("issue poller run failed", error);
    }
  }

  private async handleMissingTrackedIssues(nowIso: string, seenIssueNumbers: Set<number>): Promise<void> {
    const openIssueNumbers = this.db.listOpenIssueNumbers();

    for (const issueNumber of openIssueNumbers) {
      if (seenIssueNumbers.has(issueNumber)) {
        continue;
      }

      const detail = await this.github.getIssue(issueNumber);
      let state: string | null = null;
      let reason: string | null = null;

      if (!detail || detail.state === "closed") {
        state = "closed";
        reason = "Issue was closed on GitHub";
      } else if (!detail.labels.includes(this.config.github_label)) {
        state = "label_removed";
        reason = `Label ${this.config.github_label} was removed`;
      }

      if (!state || !reason) {
        continue;
      }

      this.db.markIssueState(issueNumber, state, nowIso);
      const issue = this.db.getIssueByNumber(issueNumber);
      if (!issue) {
        continue;
      }

      const cancelledSessionIds = this.db.cancelActiveSessionsForIssue(issue.id, reason);
      for (const sessionId of cancelledSessionIds) {
        await this.runner.abortSession(sessionId, reason);
      }
    }
  }
}
