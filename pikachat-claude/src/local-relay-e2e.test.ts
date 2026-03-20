import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import { PikachatClaudeChannel, type ChannelNotification } from "./channel-runtime.js";
import type { PikachatClaudeConfig } from "./config.js";

const ROOT = path.resolve(import.meta.dirname, "..", "..");

async function waitFor<T>(fn: () => Promise<T | null>, timeoutMs: number, label: string): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = await fn();
    if (value !== null) return value;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`timeout waiting for ${label}`);
}

async function waitForHealth(port: number): Promise<void> {
  await waitFor(async () => {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/health`);
      return response.ok ? true : null;
    } catch {
      return null;
    }
  }, 15_000, "relay health");
}

async function runCommandJson(
  cmd: string,
  args: string[],
  options: { cwd?: string; env?: NodeJS.ProcessEnv } = {},
): Promise<any> {
  const result = await runCommand(cmd, args, options);
  const trimmed = result.stdout.trim();
  return trimmed ? JSON.parse(trimmed) : null;
}

async function runCommand(
  cmd: string,
  args: string[],
  options: { cwd?: string; env?: NodeJS.ProcessEnv } = {},
): Promise<{ stdout: string; stderr: string }> {
  return await new Promise((resolve, reject) => {
    const child = spawn(cmd, args, {
      cwd: options.cwd ?? ROOT,
      env: { ...process.env, ...(options.env ?? {}) },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (buf: Buffer) => {
      stdout += String(buf);
    });
    child.stderr.on("data", (buf: Buffer) => {
      stderr += String(buf);
    });
    child.on("error", reject);
    child.on("exit", (code) => {
      if (code === 0) {
        resolve({ stdout, stderr });
      } else {
        reject(new Error(`${cmd} ${args.join(" ")} failed code=${code}\nstdout:\n${stdout}\nstderr:\n${stderr}`));
      }
    });
  });
}

async function startRelay(tempDir: string): Promise<{ child: ChildProcess; relayUrl: string }> {
  const relayBin = path.join(tempDir, "pika-relay");
  await runCommand("go", ["build", "-o", relayBin, "."], {
    cwd: path.join(ROOT, "cmd", "pika-relay"),
  });
  return await new Promise((resolve, reject) => {
    const child = spawn(relayBin, [], {
      cwd: ROOT,
      env: {
        ...process.env,
        PORT: "0",
        DATA_DIR: path.join(tempDir, "relay-data"),
        MEDIA_DIR: path.join(tempDir, "relay-media"),
      },
      stdio: ["ignore", "ignore", "pipe"],
    });
    let stderr = "";
    child.stderr.on("data", (buf: Buffer) => {
      stderr += String(buf);
      const match = stderr.match(/PIKA_RELAY_PORT=(\d+)/);
      if (match) {
        resolve({ child, relayUrl: `ws://127.0.0.1:${match[1]}` });
      }
    });
    child.on("error", reject);
    child.on("exit", (code) => {
      reject(new Error(`relay exited early code=${code}\nstderr:\n${stderr}`));
    });
  });
}

async function stopChild(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null) return;
  await new Promise<void>((resolve) => {
    child.once("exit", () => resolve());
    child.kill("SIGTERM");
  });
}

test(
  "local relay e2e: inbound message produces notification and reply reaches remote sender",
  { skip: !process.env.RUN_PIKACHAT_CLAUDE_E2E, timeout: 120_000 },
  async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "pikachat-claude-e2e-"));
    let relayChild: ChildProcess | null = null;
    let runtime: PikachatClaudeChannel | null = null;

    try {
      const relay = await startRelay(tempDir);
      relayChild = relay.child;
      const relayPort = Number(new URL(relay.relayUrl).port);
      await waitForHealth(relayPort);

      const senderStateDir = path.join(tempDir, "sender");
      const botStateDir = path.join(tempDir, "bot-state");
      const channelHome = path.join(tempDir, "channel-home");

      const notifications: ChannelNotification[] = [];
      let resolveNotification: ((value: ChannelNotification) => void) | null = null;
      const firstNotification = new Promise<ChannelNotification>((resolve) => {
        resolveNotification = resolve;
      });

      const config: PikachatClaudeConfig = {
        relays: [relay.relayUrl],
        stateDir: botStateDir,
        daemonCmd: "cargo",
        daemonArgs: [
          "run",
          "-q",
          "-p",
          "pikachat",
          "--",
          "--state-dir",
          botStateDir,
          "--relay",
          relay.relayUrl,
          "daemon",
          "--auto-accept-welcomes",
        ],
        daemonBackend: "native",
        autoAcceptWelcomes: true,
        channelSource: "pikachat",
        channelHome,
        accessFile: path.join(channelHome, "access.json"),
        inboxDir: path.join(channelHome, "inbox"),
      };

      runtime = new PikachatClaudeChannel({
        config,
        logger: {
          debug: () => {},
          info: () => {},
          warn: (message) => process.stderr.write(`[e2e warn] ${message}\n`),
          error: (message) => process.stderr.write(`[e2e err] ${message}\n`),
        },
        onNotification: async (notification) => {
          notifications.push(notification);
          resolveNotification?.(notification);
          resolveNotification = null;
        },
      });
      await runtime.start();
      const status = runtime.status();
      assert.ok(status.botPubkey, "runtime should expose bot pubkey");

      await runCommand("cargo", ["run", "-q", "-p", "pikachat", "--", "--state-dir", senderStateDir, "init"], {
        cwd: ROOT,
      });
      const identity = await runCommandJson(
        "cargo",
        ["run", "-q", "-p", "pikachat", "--", "--state-dir", senderStateDir, "identity"],
        { cwd: ROOT },
      );
      await runCommand("cargo", ["run", "-q", "-p", "pikachat", "--", "--state-dir", senderStateDir, "--relay", relay.relayUrl, "publish-kp"], {
        cwd: ROOT,
      });

      await runtime.allowSender(String(identity.pubkey));

      await runCommand(
        "cargo",
        [
          "run",
          "-q",
          "-p",
          "pikachat",
          "--",
          "--state-dir",
          senderStateDir,
          "--relay",
          relay.relayUrl,
          "send",
          "--to",
          String(status.botPubkey),
          "--content",
          "hello from claude e2e",
        ],
        { cwd: ROOT },
      );

      const inbound = await firstNotification;
      assert.equal(inbound.meta.chat_type, "direct");
      assert.equal(inbound.meta.sender_id, String(identity.pubkey).toLowerCase());

      const replyText = "E2E_OK_claude_channel";
      await runtime.reply({ chatId: inbound.meta.chat_id, text: replyText });

      await runCommand(
        "cargo",
        [
          "run",
          "-q",
          "-p",
          "pikachat",
          "--",
          "--state-dir",
          senderStateDir,
          "--relay",
          relay.relayUrl,
          "listen",
          "--timeout",
          "8",
          "--lookback",
          "120",
        ],
        { cwd: ROOT },
      );

      const messages = await waitFor(async () => {
        const result = await runCommandJson(
          "cargo",
          [
            "run",
            "-q",
            "-p",
            "pikachat",
            "--",
            "--state-dir",
            senderStateDir,
            "messages",
            "--group",
            inbound.meta.chat_id,
            "--limit",
            "10",
          ],
          { cwd: ROOT },
        );
        return Array.isArray(result?.messages) &&
          result.messages.some((message: { content?: string }) => String(message.content ?? "").includes(replyText))
          ? result
          : null;
      }, 30_000, "remote reply ingestion");

      assert.ok(
        messages.messages.some((message: { content?: string }) => String(message.content ?? "").includes(replyText)),
      );
      assert.equal(notifications.length >= 1, true);
    } finally {
      if (runtime) {
        await runtime.stop().catch(() => {});
      }
      if (relayChild) {
        await stopChild(relayChild).catch(() => {});
      }
      await rm(tempDir, { recursive: true, force: true });
    }
  },
);
