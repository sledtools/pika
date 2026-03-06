import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { describe, it } from "node:test";

import { GitService, ShellCommandRunner, type CommandResult, type CommandRunner } from "../src/git.js";
import { createTempDir } from "./helpers.js";

function git(cwd: string, ...args: string[]): string {
  return execFileSync("git", args, { cwd, encoding: "utf8" });
}

describe("git service", () => {
  it("creates worktree and pushes commits", async () => {
    const root = createTempDir("pika-dev-git");
    const remote = path.join(root, "remote.git");
    const repo = path.join(root, "repo");

    fs.mkdirSync(repo, { recursive: true });
    git(root, "init", "--bare", remote);

    git(repo, "init");
    git(repo, "config", "user.name", "pika-dev-test");
    git(repo, "config", "user.email", "pika-dev-test@example.com");
    fs.writeFileSync(path.join(repo, "README.md"), "hello\n");
    git(repo, "add", "README.md");
    git(repo, "commit", "-m", "initial");
    git(repo, "branch", "-M", "main");
    git(repo, "remote", "add", "origin", remote);
    git(repo, "push", "-u", "origin", "main");

    const service = new GitService(repo, new ShellCommandRunner());
    const workspaceBase = path.join(root, "workspaces");
    const worktree = await service.createWorktree(42, workspaceBase, "origin/main");

    fs.writeFileSync(path.join(worktree.worktreePath, "feature.txt"), "new change\n");
    const committed = await service.commitAllAndPush(
      worktree.worktreePath,
      worktree.branchName,
      "test commit",
    );

    assert.equal(committed, true);

    const remoteHeads = git(repo, "ls-remote", "--heads", "origin", "pika-dev/issue-42");
    assert.match(remoteHeads, /pika-dev\/issue-42/);
  });

  it("parses PR info from gh output", async () => {
    class FakeRunner implements CommandRunner {
      async run(cmd: string, args: string[]): Promise<CommandResult> {
        if (cmd !== "gh") {
          throw new Error("unexpected command");
        }

        if (args[0] === "pr" && args[1] === "create") {
          return {
            stdout: "https://github.com/sledtools/pika/pull/321\n",
            stderr: "",
          };
        }

        if (args[0] === "pr" && args[1] === "view") {
          return {
            stdout: JSON.stringify({ number: 321, url: "https://github.com/sledtools/pika/pull/321" }),
            stderr: "",
          };
        }

        throw new Error(`unexpected gh args: ${args.join(" ")}`);
      }
    }

    const root = createTempDir("pika-dev-git-gh");
    const service = new GitService(root, new FakeRunner());

    const pr = await service.createDraftPr({
      worktreePath: root,
      repo: "sledtools/pika",
      title: "title",
      body: "body",
      base: "main",
      head: "pika-dev/issue-1",
    });

    assert.equal(pr.number, 321);
    assert.equal(pr.url, "https://github.com/sledtools/pika/pull/321");
  });

  it("returns existing PR when gh pr create reports one already exists", async () => {
    class FakeRunner implements CommandRunner {
      async run(cmd: string, args: string[]): Promise<CommandResult> {
        if (cmd !== "gh") {
          throw new Error("unexpected command");
        }

        if (args[0] === "pr" && args[1] === "create") {
          throw new Error(
            "gh pr create failed (exit 1)\nstdout:\n\nstderr:\na pull request already exists for pika-dev/issue-1",
          );
        }

        if (args[0] === "pr" && args[1] === "view") {
          return {
            stdout: JSON.stringify({ number: 444, url: "https://github.com/sledtools/pika/pull/444" }),
            stderr: "",
          };
        }

        throw new Error(`unexpected gh args: ${args.join(" ")}`);
      }
    }

    const root = createTempDir("pika-dev-git-gh-existing");
    const service = new GitService(root, new FakeRunner());

    const pr = await service.createDraftPr({
      worktreePath: root,
      repo: "sledtools/pika",
      title: "title",
      body: "body",
      base: "master",
      head: "pika-dev/issue-1",
    });

    assert.equal(pr.number, 444);
    assert.equal(pr.url, "https://github.com/sledtools/pika/pull/444");
  });

  it("sets bot git identity when committing", async () => {
    class FakeRunner implements CommandRunner {
      public calls: Array<{ cmd: string; args: string[] }> = [];

      async run(cmd: string, args: string[]): Promise<CommandResult> {
        this.calls.push({ cmd, args });

        if (cmd !== "git") {
          throw new Error("unexpected command");
        }

        if (args.includes("status")) {
          return { stdout: "M changed.txt\n", stderr: "" };
        }

        return { stdout: "", stderr: "" };
      }
    }

    const runner = new FakeRunner();
    const service = new GitService("/tmp/repo", runner);

    const committed = await service.commitAllAndPush(
      "/tmp/repo/worktree",
      "pika-dev/issue-1",
      "commit message",
    );

    assert.equal(committed, true);

    const commitCall = runner.calls.find(
      (call) => call.cmd === "git" && call.args.includes("commit"),
    );
    assert.ok(commitCall);
    assert.ok(commitCall.args.includes("-c"));
    assert.ok(commitCall.args.includes("user.name=pika-dev"));
    assert.ok(commitCall.args.includes("user.email=pika-dev@pikachat.org"));
  });

});
