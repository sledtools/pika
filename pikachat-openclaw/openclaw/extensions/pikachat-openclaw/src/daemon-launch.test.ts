import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { resolvePikachatChannelConfig } from "./config.ts";
import { buildPikachatDaemonLaunchSpec, resolveAccountStateDir } from "./daemon-launch.ts";

describe("resolvePikachatChannelConfig", () => {
  it("maps legacy sidecar fields onto daemon launch config", () => {
    const config = resolvePikachatChannelConfig({
      relays: ["wss://relay.example.com"],
      sidecarCmd: "/usr/local/bin/pikachat",
      sidecarArgs: ["daemon", "--relay", "wss://relay.example.com"],
      sidecarVersion: "pikachat-v1.2.3",
      daemonBackend: "acp",
      daemonAcpExec: "npx -y pi-acp",
      daemonAcpCwd: "/srv/pikachat/acp",
    });

    assert.equal(config.daemonCmd, "/usr/local/bin/pikachat");
    assert.deepStrictEqual(config.daemonArgs, ["daemon", "--relay", "wss://relay.example.com"]);
    assert.equal(config.daemonVersion, "pikachat-v1.2.3");
    assert.equal(config.daemonBackend, "acp");
    assert.equal(config.daemonAcpExec, "npx -y pi-acp");
    assert.equal(config.daemonAcpCwd, "/srv/pikachat/acp");
  });
});

describe("buildPikachatDaemonLaunchSpec", () => {
  it("builds native daemon launch by default", async () => {
    const config = resolvePikachatChannelConfig({
      relays: ["wss://relay-a.example.com"],
      autoAcceptWelcomes: true,
    });

    const launch = await buildPikachatDaemonLaunchSpec(
      {
        accountId: "acct-a",
        config,
      },
      {
        resolveCommand: async ({ requestedCmd }) => `/resolved/${requestedCmd}`,
      },
    );

    assert.equal(launch.cmd, "/resolved/pikachat");
    assert.equal(launch.backend, "native");
    assert.equal(launch.autoAcceptWelcomes, true);
    assert.deepStrictEqual(launch.args, [
      "daemon",
      "--relay",
      "wss://relay-a.example.com",
      "--state-dir",
      resolveAccountStateDir({ accountId: "acct-a" }),
      "--auto-accept-welcomes",
    ]);
  });

  it("builds acp-backed daemon launch explicitly", async () => {
    const config = resolvePikachatChannelConfig({
      relays: ["wss://relay-b.example.com"],
      daemonBackend: "acp",
      daemonAcpExec: "npx -y pi-acp",
      daemonAcpCwd: "/root/pika-agent/acp",
      autoAcceptWelcomes: false,
    });

    const launch = await buildPikachatDaemonLaunchSpec(
      {
        accountId: "acct-b",
        config,
      },
      {
        resolveCommand: async ({ requestedCmd }) => `/resolved/${requestedCmd}`,
      },
    );

    assert.equal(launch.cmd, "/resolved/pikachat");
    assert.equal(launch.backend, "acp");
    assert.equal(launch.autoAcceptWelcomes, false);
    assert.deepStrictEqual(launch.args, [
      "daemon",
      "--relay",
      "wss://relay-b.example.com",
      "--state-dir",
      resolveAccountStateDir({ accountId: "acct-b" }),
      "--acp-exec",
      "npx -y pi-acp",
      "--acp-cwd",
      "/root/pika-agent/acp",
    ]);
  });

  it("honors explicit daemon env overrides while keeping legacy sidecar env compatibility", async () => {
    const config = resolvePikachatChannelConfig({
      relays: ["wss://relay-c.example.com"],
      daemonCmd: "/config/pikachat",
      daemonArgs: ["daemon", "--state-dir", "/config/state"],
    });

    const launch = await buildPikachatDaemonLaunchSpec(
      {
        accountId: "acct-c",
        config,
        env: {
          PIKACHAT_DAEMON_CMD: "/env/pikachat",
          PIKACHAT_SIDECAR_ARGS: JSON.stringify(["daemon", "--relay", "wss://env.example.com"]),
        },
      },
      {
        resolveCommand: async ({ requestedCmd }) => requestedCmd,
      },
    );

    assert.equal(launch.cmd, "/env/pikachat");
    assert.deepStrictEqual(launch.args, [
      "daemon",
      "--relay",
      "wss://env.example.com",
      "--auto-accept-welcomes",
    ]);
  });
});
