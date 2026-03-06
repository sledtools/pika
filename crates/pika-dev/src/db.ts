import fs from "node:fs";
import path from "node:path";

import BetterSqlite3 from "better-sqlite3";

import type {
  ChatMessageRecord,
  DashboardSnapshot,
  GitHubIssue,
  IssueRecord,
  SessionEventRecord,
  SessionEventType,
  SessionRecord,
  SessionStatus,
  SessionWithIssue,
} from "./types.js";

export class PikaDevDb {
  private readonly db: BetterSqlite3.Database;

  constructor(dbPath: string) {
    const dir = path.dirname(dbPath);
    fs.mkdirSync(dir, { recursive: true });
    this.db = new BetterSqlite3(dbPath);
    this.db.pragma("journal_mode = WAL");
    this.db.pragma("foreign_keys = ON");
    this.migrate();
  }

  close(): void {
    this.db.close();
  }

  upsertIssue(
    issue: GitHubIssue,
    lastPolledAt: string,
  ): {
    issue: IssueRecord;
    isNew: boolean;
    bodyChanged: boolean;
    updatedChanged: boolean;
    meaningfulChanged: boolean;
  } {
    const existing = this.getIssueByNumber(issue.number);
    const labelsJson = JSON.stringify(issue.labels);

    if (existing) {
      this.db
        .prepare(
          `
          UPDATE issues
          SET title = ?,
              body = ?,
              author = ?,
              labels = ?,
              state = ?,
              github_updated_at = ?,
              last_polled_at = ?,
              updated_at = datetime('now')
          WHERE issue_number = ?
        `,
        )
        .run(
          issue.title,
          issue.body,
          issue.user,
          labelsJson,
          issue.state,
          issue.updatedAt,
          lastPolledAt,
          issue.number,
        );

      const updated = this.getIssueByNumber(issue.number);
      if (!updated) {
        throw new Error(`failed to read issue after update: #${issue.number}`);
      }

      return {
        issue: updated,
        isNew: false,
        bodyChanged: existing.body !== issue.body,
        updatedChanged: existing.github_updated_at !== issue.updatedAt,
        meaningfulChanged:
          existing.title !== issue.title
          || existing.body !== issue.body
          || existing.labels !== labelsJson
          || existing.state !== issue.state,
      };
    }

    const result = this.db
      .prepare(
        `
        INSERT INTO issues (
          issue_number,
          title,
          body,
          author,
          labels,
          state,
          github_updated_at,
          last_polled_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
      `,
      )
      .run(
        issue.number,
        issue.title,
        issue.body,
        issue.user,
        labelsJson,
        issue.state,
        issue.updatedAt,
        lastPolledAt,
      );

    const created = this.getIssueById(Number(result.lastInsertRowid));
    if (!created) {
      throw new Error(`failed to read issue after insert: #${issue.number}`);
    }

    return {
      issue: created,
      isNew: true,
      bodyChanged: false,
      updatedChanged: true,
      meaningfulChanged: true,
    };
  }

  getIssueByNumber(issueNumber: number): IssueRecord | null {
    const row = this.db
      .prepare("SELECT * FROM issues WHERE issue_number = ?")
      .get(issueNumber) as IssueRecord | undefined;
    return row ?? null;
  }

  getIssueById(issueId: number): IssueRecord | null {
    const row = this.db
      .prepare("SELECT * FROM issues WHERE id = ?")
      .get(issueId) as IssueRecord | undefined;
    return row ?? null;
  }

  listOpenIssueNumbers(): number[] {
    const rows = this.db
      .prepare("SELECT issue_number FROM issues WHERE state = 'open'")
      .all() as Array<{ issue_number: number }>;
    return rows.map((row) => row.issue_number);
  }

  markIssueState(issueNumber: number, state: string, lastPolledAt: string): void {
    this.db
      .prepare(
        `
        UPDATE issues
        SET state = ?, last_polled_at = ?, updated_at = datetime('now')
        WHERE issue_number = ?
      `,
      )
      .run(state, lastPolledAt, issueNumber);
  }

  ensureQueuedSession(issueId: number): SessionRecord {
    const active = this.findActiveSessionForIssue(issueId);
    if (active) {
      return active;
    }

    const existingQueued = this.db
      .prepare(
        `
        SELECT * FROM sessions
        WHERE issue_id = ? AND status = 'queued'
        ORDER BY id DESC
        LIMIT 1
      `,
      )
      .get(issueId) as SessionRecord | undefined;
    if (existingQueued) {
      return existingQueued;
    }

    const insert = this.db
      .prepare("INSERT INTO sessions (issue_id, status) VALUES (?, 'queued')")
      .run(issueId);
    const created = this.getSessionById(Number(insert.lastInsertRowid));
    if (!created) {
      throw new Error(`failed to read session after insert for issue ${issueId}`);
    }
    return created;
  }

  findActiveSessionForIssue(issueId: number): SessionRecord | null {
    const row = this.db
      .prepare(
        `
        SELECT * FROM sessions
        WHERE issue_id = ?
          AND status IN ('queued', 'starting', 'running')
        ORDER BY id DESC
        LIMIT 1
      `,
      )
      .get(issueId) as SessionRecord | undefined;
    return row ?? null;
  }

  getSessionById(sessionId: number): SessionRecord | null {
    const row = this.db
      .prepare("SELECT * FROM sessions WHERE id = ?")
      .get(sessionId) as SessionRecord | undefined;
    return row ?? null;
  }

  getSessionWithIssue(sessionId: number): SessionWithIssue | null {
    const row = this.db
      .prepare(
        `
        SELECT
          s.id AS s_id,
          s.issue_id AS s_issue_id,
          s.status AS s_status,
          s.branch_name AS s_branch_name,
          s.worktree_path AS s_worktree_path,
          s.pi_session_path AS s_pi_session_path,
          s.pr_number AS s_pr_number,
          s.pr_url AS s_pr_url,
          s.error_message AS s_error_message,
          s.started_at AS s_started_at,
          s.completed_at AS s_completed_at,
          s.created_at AS s_created_at,
          s.updated_at AS s_updated_at,

          i.id AS i_id,
          i.issue_number AS i_issue_number,
          i.title AS i_title,
          i.body AS i_body,
          i.author AS i_author,
          i.labels AS i_labels,
          i.state AS i_state,
          i.github_updated_at AS i_github_updated_at,
          i.last_polled_at AS i_last_polled_at,
          i.created_at AS i_created_at,
          i.updated_at AS i_updated_at
        FROM sessions s
        JOIN issues i ON i.id = s.issue_id
        WHERE s.id = ?
      `,
      )
      .get(sessionId) as Record<string, unknown> | undefined;

    if (!row) {
      return null;
    }

    return this.mapJoinedSession(row);
  }

  listSessionWithIssueByStatuses(statuses: SessionStatus[], limit: number): SessionWithIssue[] {
    const placeholders = statuses.map(() => "?").join(", ");
    const rows = this.db
      .prepare(
        `
        SELECT
          s.id AS s_id,
          s.issue_id AS s_issue_id,
          s.status AS s_status,
          s.branch_name AS s_branch_name,
          s.worktree_path AS s_worktree_path,
          s.pi_session_path AS s_pi_session_path,
          s.pr_number AS s_pr_number,
          s.pr_url AS s_pr_url,
          s.error_message AS s_error_message,
          s.started_at AS s_started_at,
          s.completed_at AS s_completed_at,
          s.created_at AS s_created_at,
          s.updated_at AS s_updated_at,

          i.id AS i_id,
          i.issue_number AS i_issue_number,
          i.title AS i_title,
          i.body AS i_body,
          i.author AS i_author,
          i.labels AS i_labels,
          i.state AS i_state,
          i.github_updated_at AS i_github_updated_at,
          i.last_polled_at AS i_last_polled_at,
          i.created_at AS i_created_at,
          i.updated_at AS i_updated_at
        FROM sessions s
        JOIN issues i ON i.id = s.issue_id
        WHERE s.status IN (${placeholders})
        ORDER BY s.created_at DESC
        LIMIT ?
      `,
      )
      .all(...statuses, limit) as Array<Record<string, unknown>>;

    return rows.map((row) => this.mapJoinedSession(row));
  }

  getOldestQueuedSession(): SessionWithIssue | null {
    const row = this.db
      .prepare(
        `
        SELECT
          s.id AS s_id,
          s.issue_id AS s_issue_id,
          s.status AS s_status,
          s.branch_name AS s_branch_name,
          s.worktree_path AS s_worktree_path,
          s.pi_session_path AS s_pi_session_path,
          s.pr_number AS s_pr_number,
          s.pr_url AS s_pr_url,
          s.error_message AS s_error_message,
          s.started_at AS s_started_at,
          s.completed_at AS s_completed_at,
          s.created_at AS s_created_at,
          s.updated_at AS s_updated_at,

          i.id AS i_id,
          i.issue_number AS i_issue_number,
          i.title AS i_title,
          i.body AS i_body,
          i.author AS i_author,
          i.labels AS i_labels,
          i.state AS i_state,
          i.github_updated_at AS i_github_updated_at,
          i.last_polled_at AS i_last_polled_at,
          i.created_at AS i_created_at,
          i.updated_at AS i_updated_at
        FROM sessions s
        JOIN issues i ON i.id = s.issue_id
        WHERE s.status = 'queued'
        ORDER BY s.created_at ASC
        LIMIT 1
      `,
      )
      .get() as Record<string, unknown> | undefined;

    return row ? this.mapJoinedSession(row) : null;
  }

  countActiveSessions(): number {
    const row = this.db
      .prepare(
        "SELECT COUNT(*) AS count FROM sessions WHERE status IN ('starting', 'running')",
      )
      .get() as { count: number };
    return row.count;
  }

  listTimedOutSessions(cutoffIso: string): SessionRecord[] {
    return this.db
      .prepare(
        `
        SELECT * FROM sessions
        WHERE status IN ('starting', 'running')
          AND started_at IS NOT NULL
          AND datetime(started_at) < datetime(?)
      `,
      )
      .all(cutoffIso) as SessionRecord[];
  }

  markSessionStarting(sessionId: number, branch: string, worktreePath: string, piSessionPath: string | null): void {
    this.db
      .prepare(
        `
        UPDATE sessions
        SET status = 'starting',
            branch_name = ?,
            worktree_path = ?,
            pi_session_path = ?,
            started_at = COALESCE(started_at, datetime('now')),
            updated_at = datetime('now')
        WHERE id = ?
      `,
      )
      .run(branch, worktreePath, piSessionPath, sessionId);
  }

  markSessionRunning(sessionId: number): void {
    this.db
      .prepare(
        `
        UPDATE sessions
        SET status = 'running',
            started_at = COALESCE(started_at, datetime('now')),
            updated_at = datetime('now')
        WHERE id = ?
      `,
      )
      .run(sessionId);
  }

  markSessionCompleted(sessionId: number, prNumber: number | null, prUrl: string | null): void {
    this.db
      .prepare(
        `
        UPDATE sessions
        SET status = 'completed',
            pr_number = ?,
            pr_url = ?,
            error_message = NULL,
            completed_at = datetime('now'),
            updated_at = datetime('now')
        WHERE id = ?
      `,
      )
      .run(prNumber, prUrl, sessionId);
  }

  markSessionFailed(sessionId: number, errorMessage: string): void {
    this.db
      .prepare(
        `
        UPDATE sessions
        SET status = 'failed',
            error_message = ?,
            completed_at = datetime('now'),
            updated_at = datetime('now')
        WHERE id = ?
      `,
      )
      .run(errorMessage, sessionId);
  }

  markSessionCancelled(sessionId: number, reason: string): void {
    this.db
      .prepare(
        `
        UPDATE sessions
        SET status = 'cancelled',
            error_message = ?,
            completed_at = datetime('now'),
            updated_at = datetime('now')
        WHERE id = ?
      `,
      )
      .run(reason, sessionId);
  }

  cancelActiveSessionsForIssue(issueId: number, reason: string): number[] {
    const rows = this.db
      .prepare(
        `
        SELECT id
        FROM sessions
        WHERE issue_id = ?
          AND status IN ('queued', 'starting', 'running')
      `,
      )
      .all(issueId) as Array<{ id: number }>;

    for (const row of rows) {
      this.markSessionCancelled(row.id, reason);
    }
    return rows.map((row) => row.id);
  }

  addSessionEvent(sessionId: number, eventType: SessionEventType, payload: Record<string, unknown>): SessionEventRecord {
    const result = this.db
      .prepare(
        `
        INSERT INTO session_events (session_id, event_type, payload)
        VALUES (?, ?, ?)
      `,
      )
      .run(sessionId, eventType, JSON.stringify(payload));

    const row = this.db
      .prepare("SELECT * FROM session_events WHERE id = ?")
      .get(Number(result.lastInsertRowid)) as SessionEventRecord | undefined;

    if (!row) {
      throw new Error(`failed to read session event ${result.lastInsertRowid}`);
    }
    return row;
  }

  listSessionEvents(sessionId: number, limit: number): SessionEventRecord[] {
    return this.db
      .prepare(
        `
        SELECT *
        FROM session_events
        WHERE session_id = ?
        ORDER BY id ASC
        LIMIT ?
      `,
      )
      .all(sessionId, limit) as SessionEventRecord[];
  }

  addChatMessage(sessionId: number, role: "user" | "assistant" | "system", npub: string | null, content: string): ChatMessageRecord {
    const result = this.db
      .prepare(
        `
        INSERT INTO chat_messages (session_id, role, npub, content)
        VALUES (?, ?, ?, ?)
      `,
      )
      .run(sessionId, role, npub, content);

    const row = this.db
      .prepare("SELECT * FROM chat_messages WHERE id = ?")
      .get(Number(result.lastInsertRowid)) as ChatMessageRecord | undefined;
    if (!row) {
      throw new Error(`failed to read chat message ${result.lastInsertRowid}`);
    }
    return row;
  }

  listChatMessages(sessionId: number, limit = 200): ChatMessageRecord[] {
    return this.db
      .prepare(
        `
        SELECT *
        FROM chat_messages
        WHERE session_id = ?
        ORDER BY id ASC
        LIMIT ?
      `,
      )
      .all(sessionId, limit) as ChatMessageRecord[];
  }

  getDashboardSnapshot(maxConcurrent: number): DashboardSnapshot {
    const running = this.listSessionWithIssueByStatuses(["starting", "running"], 50);
    const queued = this.listSessionWithIssueByStatuses(["queued"], 50);
    const completed = this.listSessionWithIssueByStatuses(["completed", "failed", "cancelled"], 100);

    return {
      activeCount: running.length,
      maxConcurrent,
      running,
      queued,
      completed,
    };
  }

  private migrate(): void {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS issues (
        id INTEGER PRIMARY KEY,
        issue_number INTEGER UNIQUE NOT NULL,
        title TEXT NOT NULL,
        body TEXT NOT NULL DEFAULT '',
        author TEXT NOT NULL DEFAULT '',
        labels TEXT NOT NULL DEFAULT '[]',
        state TEXT NOT NULL DEFAULT 'open',
        github_updated_at TEXT NOT NULL DEFAULT '',
        last_polled_at TEXT NOT NULL DEFAULT '',
        created_at TEXT DEFAULT (datetime('now')),
        updated_at TEXT DEFAULT (datetime('now'))
      );

      CREATE TABLE IF NOT EXISTS sessions (
        id INTEGER PRIMARY KEY,
        issue_id INTEGER NOT NULL REFERENCES issues(id),
        status TEXT NOT NULL DEFAULT 'queued',
        branch_name TEXT,
        worktree_path TEXT,
        pi_session_path TEXT,
        pr_number INTEGER,
        pr_url TEXT,
        error_message TEXT,
        started_at TEXT,
        completed_at TEXT,
        created_at TEXT DEFAULT (datetime('now')),
        updated_at TEXT DEFAULT (datetime('now'))
      );

      CREATE TABLE IF NOT EXISTS session_events (
        id INTEGER PRIMARY KEY,
        session_id INTEGER NOT NULL REFERENCES sessions(id),
        event_type TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_at TEXT DEFAULT (datetime('now'))
      );

      CREATE TABLE IF NOT EXISTS chat_messages (
        id INTEGER PRIMARY KEY,
        session_id INTEGER NOT NULL REFERENCES sessions(id),
        role TEXT NOT NULL,
        npub TEXT,
        content TEXT NOT NULL,
        created_at TEXT DEFAULT (datetime('now'))
      );

      CREATE INDEX IF NOT EXISTS idx_issues_issue_number ON issues(issue_number);
      CREATE INDEX IF NOT EXISTS idx_issues_state ON issues(state);
      CREATE INDEX IF NOT EXISTS idx_sessions_issue_id ON sessions(issue_id);
      CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);
      CREATE INDEX IF NOT EXISTS idx_events_session_id ON session_events(session_id);
      CREATE INDEX IF NOT EXISTS idx_chat_session_id ON chat_messages(session_id);
    `);
  }

  private mapJoinedSession(row: Record<string, unknown>): SessionWithIssue {
    const session: SessionRecord = {
      id: Number(row.s_id),
      issue_id: Number(row.s_issue_id),
      status: row.s_status as SessionStatus,
      branch_name: (row.s_branch_name as string | null) ?? null,
      worktree_path: (row.s_worktree_path as string | null) ?? null,
      pi_session_path: (row.s_pi_session_path as string | null) ?? null,
      pr_number: (row.s_pr_number as number | null) ?? null,
      pr_url: (row.s_pr_url as string | null) ?? null,
      error_message: (row.s_error_message as string | null) ?? null,
      started_at: (row.s_started_at as string | null) ?? null,
      completed_at: (row.s_completed_at as string | null) ?? null,
      created_at: String(row.s_created_at),
      updated_at: String(row.s_updated_at),
    };

    const issue: IssueRecord = {
      id: Number(row.i_id),
      issue_number: Number(row.i_issue_number),
      title: String(row.i_title),
      body: String(row.i_body ?? ""),
      author: String(row.i_author ?? ""),
      labels: String(row.i_labels ?? "[]"),
      state: String(row.i_state),
      github_updated_at: String(row.i_github_updated_at ?? ""),
      last_polled_at: String(row.i_last_polled_at ?? ""),
      created_at: String(row.i_created_at),
      updated_at: String(row.i_updated_at),
    };

    return { session, issue };
  }
}
