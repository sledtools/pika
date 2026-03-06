import fs from "node:fs";
import process from "node:process";

import { serve } from "@hono/node-server";

import { FakeAgentBackend, PiAgentBackend } from "./agent.js";
import { AuthState } from "./auth.js";
import { loadConfig, parseCliArgs } from "./config.js";
import { PikaDevDb } from "./db.js";
import { SessionEventBus } from "./event-bus.js";
import { GitService, ShellCommandRunner } from "./git.js";
import { GitHubApiClient } from "./github.js";
import { IssuePoller } from "./poller.js";
import { SessionRunner } from "./runner.js";
import { SessionScheduler } from "./scheduler.js";
import { createApp } from "./server.js";

async function main(): Promise<void> {
  const cli = parseCliArgs(process.argv.slice(2));
  const config = loadConfig(cli);

  fs.mkdirSync(config.workspace_base, { recursive: true });
  fs.mkdirSync(config.session_base, { recursive: true });

  const db = new PikaDevDb(config.dbPath);
  const auth = new AuthState(config.allowed_npubs);
  const eventBus = new SessionEventBus();

  const agentBackend = config.agent_backend === "fake"
    ? new FakeAgentBackend()
    : new PiAgentBackend(config.provider, config.model, config.thinking_level);

  const git = new GitService(process.cwd(), new ShellCommandRunner());
  const runner = new SessionRunner(config, db, git, agentBackend, eventBus);
  const github = new GitHubApiClient(config);

  const poller = new IssuePoller(config, db, github, runner);
  const scheduler = new SessionScheduler(config, db, runner);

  if (cli.once) {
    await poller.runOnce();
    await scheduler.tick();
    db.close();
    return;
  }

  const app = createApp({
    config,
    db,
    runner,
    auth,
    eventBus,
  });

  const server = serve({
    fetch: app.fetch,
    hostname: config.bind_address,
    port: config.bind_port,
  });

  poller.start();
  scheduler.start();

  await poller.runOnce();
  await scheduler.tick();

  console.log(
    `pika-dev listening on http://${config.bind_address}:${config.bind_port} (backend=${config.agent_backend})`,
  );

  const shutdown = (): void => {
    poller.stop();
    scheduler.stop();
    server.close();
    db.close();
  };

  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}

void main().catch((error) => {
  console.error(error);
  process.exit(1);
});
