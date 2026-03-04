import type { PikaDevConfig } from "./config.js";
import { PikaDevDb } from "./db.js";
import type { RunnerApi } from "./runner.js";

export class SessionScheduler {
  private timer: NodeJS.Timeout | null = null;
  private inFlight = false;
  private readonly launching = new Set<number>();

  constructor(
    private readonly config: PikaDevConfig,
    private readonly db: PikaDevDb,
    private readonly runner: RunnerApi,
  ) {}

  start(): void {
    if (this.timer) {
      return;
    }

    this.timer = setInterval(() => {
      void this.safeTick();
    }, 5_000);
  }

  stop(): void {
    if (!this.timer) {
      return;
    }

    clearInterval(this.timer);
    this.timer = null;
  }

  async tick(): Promise<void> {
    if (this.inFlight) {
      return;
    }
    this.inFlight = true;

    try {
      await this.failTimedOutSessions();
      await this.launchQueuedSessions();
    } finally {
      this.inFlight = false;
    }
  }

  private async safeTick(): Promise<void> {
    try {
      await this.tick();
    } catch (error) {
      console.error("scheduler tick failed", error);
    }
  }

  private async failTimedOutSessions(): Promise<void> {
    const cutoff = new Date(Date.now() - this.config.session_timeout_mins * 60 * 1000).toISOString();
    const timedOut = this.db.listTimedOutSessions(cutoff);

    for (const session of timedOut) {
      await this.runner.abortSession(session.id, "session timed out");
      this.db.markSessionFailed(session.id, "session timed out");
    }
  }

  private async launchQueuedSessions(): Promise<void> {
    while (true) {
      const activeCount = this.db.countActiveSessions() + this.launching.size;
      if (activeCount >= this.config.max_concurrent_sessions) {
        return;
      }

      const queued = this.db.getOldestQueuedSession();
      if (!queued) {
        return;
      }

      if (this.launching.has(queued.session.id) || this.runner.isSessionActive(queued.session.id)) {
        return;
      }

      this.launching.add(queued.session.id);
      void this.runner
        .startSession(queued.session.id)
        .catch((error) => {
          const message = error instanceof Error ? error.message : String(error);
          this.db.markSessionFailed(queued.session.id, message);
        })
        .finally(() => {
          this.launching.delete(queued.session.id);
        });
    }
  }
}
