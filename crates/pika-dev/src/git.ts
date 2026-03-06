import { execFile } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const BOT_GIT_USER_NAME = process.env.PIKA_DEV_GIT_USER_NAME ?? "pika-dev";
const BOT_GIT_USER_EMAIL = process.env.PIKA_DEV_GIT_USER_EMAIL ?? "pika-dev@pikachat.org";

export interface CommandResult {
  stdout: string;
  stderr: string;
}

export interface CommandRunner {
  run(cmd: string, args: string[], options?: { cwd?: string }): Promise<CommandResult>;
}

export class ShellCommandRunner implements CommandRunner {
  async run(cmd: string, args: string[], options?: { cwd?: string }): Promise<CommandResult> {
    try {
      const result = await execFileAsync(cmd, args, {
        cwd: options?.cwd,
        maxBuffer: 10 * 1024 * 1024,
      });
      return {
        stdout: result.stdout,
        stderr: result.stderr,
      };
    } catch (error) {
      const err = error as Error & { stdout?: string; stderr?: string; code?: number };
      const stdout = err.stdout ?? "";
      const stderr = err.stderr ?? "";
      const code = err.code ?? -1;
      throw new Error(
        `${cmd} ${args.join(" ")} failed (exit ${code})\nstdout:\n${stdout}\nstderr:\n${stderr}`,
      );
    }
  }
}

export interface WorktreeInfo {
  branchName: string;
  worktreePath: string;
}

export interface PullRequestInfo {
  number: number | null;
  url: string | null;
}

export interface GitOps {
  createWorktree(issueNumber: number, workspaceBase: string, baseBranch: string): Promise<WorktreeInfo>;
  cleanupWorktree(worktreePath: string): Promise<void>;
  commitAllAndPush(worktreePath: string, branchName: string, message: string): Promise<boolean>;
  hasCommitsAhead(worktreePath: string, baseBranch: string): Promise<boolean>;
  createDraftPr(options: {
    worktreePath: string;
    repo: string;
    title: string;
    body: string;
    base: string;
    head: string;
  }): Promise<PullRequestInfo>;
  commentIssue(repo: string, issueNumber: number, body: string, cwd: string): Promise<void>;
}

export class GitService implements GitOps {
  constructor(
    private readonly repoRoot: string,
    private readonly runner: CommandRunner,
  ) {}

  async createWorktree(issueNumber: number, workspaceBase: string, baseBranch: string): Promise<WorktreeInfo> {
    const branchName = `pika-dev/issue-${issueNumber}`;
    const worktreePath = path.join(workspaceBase, `issue-${issueNumber}`);

    fs.mkdirSync(workspaceBase, { recursive: true });

    if (fs.existsSync(worktreePath)) {
      await this.safeRun("git", ["worktree", "remove", "--force", worktreePath], this.repoRoot);
      fs.rmSync(worktreePath, { recursive: true, force: true });
    }

    await this.runner.run("git", ["fetch", "origin"], { cwd: this.repoRoot });
    await this.safeRun("git", ["worktree", "prune"], this.repoRoot);

    const candidates = uniqueBranches([
      baseBranch,
      "origin/main",
      "origin/master",
      "main",
      "master",
    ]);

    let lastError: Error | null = null;
    for (const candidate of candidates) {
      try {
        await this.runner.run(
          "git",
          ["worktree", "add", "-B", branchName, worktreePath, candidate],
          { cwd: this.repoRoot },
        );
        return { branchName, worktreePath };
      } catch (error) {
        lastError = error instanceof Error ? error : new Error(String(error));
        await this.safeRun("git", ["worktree", "remove", "--force", worktreePath], this.repoRoot);
        fs.rmSync(worktreePath, { recursive: true, force: true });
      }
    }

    throw new Error(
      `failed to create worktree for issue ${issueNumber}; tried ${candidates.join(", ")}${lastError ? `: ${lastError.message}` : ""}`,
    );
  }

  async cleanupWorktree(worktreePath: string): Promise<void> {
    await this.safeRun("git", ["worktree", "remove", "--force", worktreePath], this.repoRoot);
    fs.rmSync(worktreePath, { recursive: true, force: true });
    await this.safeRun("git", ["worktree", "prune"], this.repoRoot);
  }

  async commitAllAndPush(worktreePath: string, branchName: string, message: string): Promise<boolean> {
    const status = await this.runner.run("git", ["-C", worktreePath, "status", "--porcelain"]);
    if (status.stdout.trim().length === 0) {
      return false;
    }

    await this.runner.run("git", ["-C", worktreePath, "add", "-A"]);
    await this.runner.run(
      "git",
      [
        "-C",
        worktreePath,
        "-c",
        `user.name=${BOT_GIT_USER_NAME}`,
        "-c",
        `user.email=${BOT_GIT_USER_EMAIL}`,
        "commit",
        "-m",
        message,
      ],
    );
    await this.runner.run(
      "git",
      ["-C", worktreePath, "push", "--force-with-lease", "-u", "origin", branchName],
    );
    return true;
  }

  async hasCommitsAhead(worktreePath: string, baseBranch: string): Promise<boolean> {
    const baseRef = baseBranch.startsWith("origin/") ? baseBranch : `origin/${baseBranch}`;
    const result = await this.runner.run(
      "git",
      ["-C", worktreePath, "rev-list", "--count", `${baseRef}..HEAD`],
    );
    const aheadCount = Number.parseInt(result.stdout.trim(), 10);
    return Number.isFinite(aheadCount) && aheadCount > 0;
  }

  async createDraftPr(options: {
    worktreePath: string;
    repo: string;
    title: string;
    body: string;
    base: string;
    head: string;
  }): Promise<PullRequestInfo> {
    const createArgs = [
      "pr",
      "create",
      "--repo",
      options.repo,
      "--title",
      options.title,
      "--body",
      options.body,
      "--draft",
      "--base",
      options.base,
      "--head",
      options.head,
    ];

    let createResult: CommandResult | null = null;
    try {
      createResult = await this.runner.run("gh", createArgs, { cwd: options.worktreePath });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (isExistingPrError(message)) {
        const existing = await this.lookupPullRequest(options.worktreePath, options.repo, options.head);
        if (existing) {
          return existing;
        }
      }
      throw error;
    }

    const viewed = await this.lookupPullRequest(options.worktreePath, options.repo, options.head);
    if (viewed) {
      return viewed;
    }

    const urlMatch = createResult.stdout.match(/https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/pull\/(\d+)/);
    if (urlMatch) {
      return {
        number: Number(urlMatch[1]),
        url: urlMatch[0],
      };
    }

    return { number: null, url: null };
  }

  async commentIssue(repo: string, issueNumber: number, body: string, cwd: string): Promise<void> {
    await this.runner.run(
      "gh",
      ["issue", "comment", String(issueNumber), "--repo", repo, "--body", body],
      { cwd },
    );
  }

  private async safeRun(cmd: string, args: string[], cwd: string): Promise<CommandResult | null> {
    try {
      return await this.runner.run(cmd, args, { cwd });
    } catch {
      return null;
    }
  }

  private async lookupPullRequest(
    cwd: string,
    repo: string,
    head: string,
  ): Promise<PullRequestInfo | null> {
    const viewResult = await this.safeRun(
      "gh",
      ["pr", "view", head, "--repo", repo, "--json", "number,url"],
      cwd,
    );
    const viewParsed = parseSinglePrInfo(viewResult?.stdout ?? "");
    if (viewParsed) {
      return viewParsed;
    }

    const listResult = await this.safeRun(
      "gh",
      ["pr", "list", "--repo", repo, "--head", head, "--state", "open", "--json", "number,url"],
      cwd,
    );
    return parseListPrInfo(listResult?.stdout ?? "");
  }
}

function uniqueBranches(branches: string[]): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const branch of branches) {
    if (seen.has(branch)) {
      continue;
    }
    seen.add(branch);
    result.push(branch);
  }
  return result;
}

function parseSinglePrInfo(raw: string): PullRequestInfo | null {
  if (!raw.trim()) {
    return null;
  }
  try {
    const parsed = JSON.parse(raw) as { number?: number; url?: string };
    if (typeof parsed.number === "number" && typeof parsed.url === "string" && parsed.url.length > 0) {
      return { number: parsed.number, url: parsed.url };
    }
  } catch {
    return null;
  }
  return null;
}

function parseListPrInfo(raw: string): PullRequestInfo | null {
  if (!raw.trim()) {
    return null;
  }
  try {
    const parsed = JSON.parse(raw) as Array<{ number?: number; url?: string }>;
    const first = parsed[0];
    if (first && typeof first.number === "number" && typeof first.url === "string" && first.url.length > 0) {
      return { number: first.number, url: first.url };
    }
  } catch {
    return null;
  }
  return null;
}

function isExistingPrError(message: string): boolean {
  const normalized = message.toLowerCase();
  return normalized.includes("pull request already exists")
    || normalized.includes("a pull request for branch")
    || normalized.includes("already has an open pull request");
}
