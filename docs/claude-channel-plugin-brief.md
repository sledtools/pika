---
summary: Implementation brief for a Claude Code channel plugin backed by pikachat daemon
read_when:
  - building or extending the pikachat Claude plugin
  - reviewing channel/plugin transport and access design
---

# Pikachat Claude Channel Plugin Brief

## Goal

Build a Claude Code channel plugin backed by `pikachat daemon`.

The plugin should expose Pika MLS chats to Claude through the Claude channel contract:

- inbound chat messages arrive as `notifications/claude/channel`
- Claude replies via ordinary MCP tools
- sender gating prevents prompt injection

This plugin is a host wrapper around `pikachat daemon`, not a replacement for the daemon protocol.

## Acceptance

- DM routing works
  - approved 1:1 senders reach Claude as channel events
  - Claude can reply into the same DM
- Group routing works
  - groups are explicitly enabled
  - sender allowlists apply to senders, not rooms
  - `requireMention: true` is the default for groups
- Pairing and allowlist exist
  - unknown DM senders get a pairing code
  - approval adds the sender to the allowlist
- Reply, react, and file send work through MCP tools
- Inbound attachments are surfaced with local paths when available
- Local relay e2e proves:
  - remote Pika message
  - Claude channel notification
  - Claude reply tool call
  - remote side receives the reply

## Constraints

- Reuse the TypeScript launcher/client patterns from `pikachat-openclaw`
- Avoid direct SQLite reads unless necessary
- Ask before changing the daemon protocol
- Treat `edit_message` as non-MVP unless a native model emerges

## Existing Reusable Surfaces

The daemon already exposes the main surfaces needed for an MVP:

- `send_message`
- `send_media`
- `send_media_batch`
- `react`
- `send_typing`
- `list_groups`
- `list_members`
- `get_messages`
- `message_received`
- `group_joined`
- `group_created`
- `group_updated`

Relevant references:

- `crates/pikachat-sidecar/src/protocol.rs`
- `crates/pikachat-sidecar/src/daemon.rs`
- `pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/sidecar.ts`
- `pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/daemon-launch.ts`
- `pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/sidecar-install.ts`

## Architecture

- `pikachat daemon` remains the transport/backend child process
- the Claude plugin process is the MCP stdio server
- the plugin:
  - launches the daemon
  - consumes daemon JSONL events
  - applies DM/group access policy
  - emits Claude channel notifications
  - exposes reply/react/file-send/admin tools

## Notification Contract

Each inbound message becomes a Claude channel event with:

- `content`
  - message text
  - attachment summary lines with absolute local paths when present
- `meta`
  - `chat_id`
  - `sender_id`
  - `sender_name`
  - `message_id`
  - `event_id`
  - `chat_type`
  - `group_name`
  - `mentioned`

The server instructions should tell Claude:

- inbound messages arrive as `<channel source="pikachat" ...>`
- use `reply` with the `chat_id` from the tag
- use `react` with the `event_id` from the tag

## Access Model

Store state in `~/.claude/channels/pikachat/access.json`.

Schema:

```json
{
  "dmPolicy": "pairing",
  "allowFrom": [],
  "groups": {},
  "mentionPatterns": [],
  "pendingPairings": {}
}
```

Rules:

- DM:
  - `pairing`: unknown sender gets a code, message is dropped
  - `allowlist`: unknown sender is dropped
  - `disabled`: all DM traffic is dropped
- Group:
  - group must be explicitly enabled
  - per-group `allowFrom` is optional
  - `requireMention` defaults to `true`

## Phases

### 1. MCP wrapper

- create plugin directory with `.claude-plugin/plugin.json` and `.mcp.json`
- bundle a stdio MCP server for runtime use
- reuse daemon launch/install/client logic from `pikachat-openclaw`
- align the TypeScript protocol mirror with the current Rust protocol

### 2. Access model

- implement `access.json`
- pairing lifecycle
- DM and group gating
- mention detection

### 3. Parity gaps

Close host-side gaps first:

- add TypeScript wrappers for `list_members`, `get_messages`, `send_media_batch`, and `group_updated`
- classify DM vs group via daemon metadata and cached member counts
- avoid SQLite reads

Escalate before daemon changes for:

- native `reply_to`
- richer history pagination/search
- explicit historical attachment fetch/download

### 4. Packaging and tests

- deterministic Node tests for access, routing, and formatting
- local relay e2e using real `pikachat daemon`
- plugin README with local dev instructions

## Evaluation Design

Deterministic tests should cover:

- pairing code lifecycle
- DM policy decisions
- group allowlist and mention gating
- notification/meta shaping
- attachment text augmentation
- tool-to-daemon command mapping

Local relay e2e should prove:

1. a remote Pika user sends a DM through a local relay
2. the Claude plugin emits a channel notification
3. the test calls the plugin reply path
4. the remote user receives the reply

## Open Questions / Explicit Non-Goals

- `edit_message` is out of scope for MVP
- reply threading is a parity gap until the daemon exposes reply-tag support
- historical attachment download is a future enhancement
