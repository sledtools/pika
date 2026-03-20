import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { CallToolRequestSchema, ListToolsRequestSchema } from "@modelcontextprotocol/sdk/types.js";

import { resolvePikachatClaudeConfig } from "./config.js";
import { PikachatClaudeChannel } from "./channel-runtime.js";

const config = resolvePikachatClaudeConfig(process.env);

function log(message: string): void {
  process.stderr.write(`${message}\n`);
}

const mcp = new Server(
  { name: config.channelSource, version: "0.1.0" },
  {
    capabilities: {
      experimental: { "claude/channel": {} },
      tools: {},
    },
    instructions:
      `Messages arrive as <channel source="${config.channelSource}" chat_id="..." sender_id="..." event_id="..." chat_type="direct|group">...</channel>. ` +
      `Reply with the reply tool, passing the chat_id from the tag. ` +
      `React with the react tool, passing chat_id and event_id from the tag. ` +
      `Unknown direct-message senders may require the approve_pairing tool before their messages will be delivered.`,
  },
);

const runtime = new PikachatClaudeChannel({
  config,
  logger: {
    debug: (message) => log(message),
    info: (message) => log(message),
    warn: (message) => log(message),
    error: (message) => log(message),
  },
  onNotification: async ({ content, meta }) => {
    try {
      await mcp.notification({
        method: "notifications/claude/channel",
        params: {
          content,
          meta,
        },
      });
    } catch (err) {
      log(
        `[pikachat-claude] failed to forward notification chat_id=${meta.chat_id ?? "unknown"}: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  },
});

function textResult(text: string) {
  return { content: [{ type: "text", text }] };
}

function requireNonEmptyString(args: Record<string, unknown>, key: string): string {
  const value = args[key];
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`missing or invalid '${key}'`);
  }
  return value.trim();
}

mcp.setRequestHandler(ListToolsRequestSchema, async () => ({
  tools: [
    {
      name: "reply",
      description: "Send a text reply and optional files back through pikachat.",
      inputSchema: {
        type: "object",
        properties: {
          chat_id: { type: "string", description: "Target chat/group ID from the channel tag" },
          text: { type: "string", description: "Reply text" },
          reply_to: { type: "string", description: "Optional inbound event/message ID to reply to" },
          files: {
            type: "array",
            items: { type: "string" },
            description: "Absolute file paths to send as attachments",
          },
        },
        required: ["chat_id"],
      },
    },
    {
      name: "react",
      description: "React to a message by event ID.",
      inputSchema: {
        type: "object",
        properties: {
          chat_id: { type: "string" },
          event_id: { type: "string" },
          emoji: { type: "string" },
        },
        required: ["chat_id", "event_id", "emoji"],
      },
    },
    {
      name: "access_status",
      description: "Show current DM policy, sender allowlist, groups, and pending pairings.",
      inputSchema: { type: "object", properties: {} },
    },
    {
      name: "approve_pairing",
      description: "Approve a pending DM pairing code and add the sender to the allowlist.",
      inputSchema: {
        type: "object",
        properties: {
          code: { type: "string" },
        },
        required: ["code"],
      },
    },
    {
      name: "deny_pairing",
      description: "Deny a pending DM pairing code.",
      inputSchema: {
        type: "object",
        properties: {
          code: { type: "string" },
        },
        required: ["code"],
      },
    },
    {
      name: "set_dm_policy",
      description: "Set DM policy to pairing, allowlist, or disabled.",
      inputSchema: {
        type: "object",
        properties: {
          policy: { type: "string", enum: ["pairing", "allowlist", "disabled"] },
        },
        required: ["policy"],
      },
    },
    {
      name: "allow_sender",
      description: "Add a sender pubkey to the DM allowlist.",
      inputSchema: {
        type: "object",
        properties: {
          sender_id: { type: "string" },
        },
        required: ["sender_id"],
      },
    },
    {
      name: "remove_sender",
      description: "Remove a sender pubkey from the DM allowlist.",
      inputSchema: {
        type: "object",
        properties: {
          sender_id: { type: "string" },
        },
        required: ["sender_id"],
      },
    },
    {
      name: "enable_group",
      description: "Enable a group and optionally require mentions and restrict allowed senders.",
      inputSchema: {
        type: "object",
        properties: {
          group_id: { type: "string" },
          require_mention: { type: "boolean" },
          allow_from: {
            type: "array",
            items: { type: "string" },
          },
        },
        required: ["group_id"],
      },
    },
    {
      name: "disable_group",
      description: "Disable a group from delivering inbound messages.",
      inputSchema: {
        type: "object",
        properties: {
          group_id: { type: "string" },
        },
        required: ["group_id"],
      },
    },
  ],
}));

mcp.setRequestHandler(CallToolRequestSchema, async (request) => {
  const args = (request.params.arguments ?? {}) as Record<string, unknown>;
  switch (request.params.name) {
    case "reply": {
      const result = await runtime.reply({
        chatId: requireNonEmptyString(args, "chat_id"),
        text: typeof args.text === "string" ? args.text : undefined,
        replyTo: typeof args.reply_to === "string" ? args.reply_to : undefined,
        files: Array.isArray(args.files) ? args.files.map((entry) => String(entry)) : undefined,
      });
      const notes = result.notes.length > 0 ? ` notes=${JSON.stringify(result.notes)}` : "";
      return textResult(`sent${notes}`);
    }
    case "react": {
      await runtime.react({
        chatId: requireNonEmptyString(args, "chat_id"),
        eventId: requireNonEmptyString(args, "event_id"),
        emoji: requireNonEmptyString(args, "emoji"),
      });
      return textResult("reaction sent");
    }
    case "access_status": {
      return textResult(JSON.stringify(await runtime.accessStatus(), null, 2));
    }
    case "approve_pairing": {
      const result = await runtime.approvePairing(requireNonEmptyString(args, "code"));
      return textResult(result.senderId ? `approved ${result.senderId}` : "pairing code not found");
    }
    case "deny_pairing": {
      const result = await runtime.denyPairing(requireNonEmptyString(args, "code"));
      return textResult(result.senderId ? `denied ${result.senderId}` : "pairing code not found");
    }
    case "set_dm_policy": {
      const policy = String(args.policy ?? "");
      if (policy !== "pairing" && policy !== "allowlist" && policy !== "disabled") {
        throw new Error(`invalid policy: ${policy}`);
      }
      return textResult(JSON.stringify(await runtime.setDmPolicy(policy), null, 2));
    }
    case "allow_sender": {
      return textResult(JSON.stringify(await runtime.allowSender(requireNonEmptyString(args, "sender_id")), null, 2));
    }
    case "remove_sender": {
      return textResult(
        JSON.stringify(await runtime.removeSender(requireNonEmptyString(args, "sender_id")), null, 2),
      );
    }
    case "enable_group": {
      return textResult(
        JSON.stringify(
          await runtime.enableGroup(
            requireNonEmptyString(args, "group_id"),
            args.require_mention !== false,
            Array.isArray(args.allow_from) ? args.allow_from.map((entry) => String(entry)) : [],
          ),
          null,
          2,
        ),
      );
    }
    case "disable_group": {
      return textResult(JSON.stringify(await runtime.disableGroup(requireNonEmptyString(args, "group_id")), null, 2));
    }
    default:
      throw new Error(`unknown tool: ${request.params.name}`);
  }
});

async function main(): Promise<void> {
  await runtime.start();
  await mcp.connect(new StdioServerTransport());
}

void main().catch(async (err) => {
  log(`[pikachat-claude] fatal: ${err instanceof Error ? err.stack ?? err.message : String(err)}`);
  try {
    await runtime.stop();
  } catch (stopErr) {
    log(`[pikachat-claude] stop failed: ${stopErr instanceof Error ? stopErr.message : String(stopErr)}`);
  }
  process.exit(1);
});

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => {
    void runtime.stop().finally(() => process.exit(0));
  });
}
