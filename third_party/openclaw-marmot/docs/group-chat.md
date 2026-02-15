# Group Chat Guide

How to set up and manage group chats with the Marmot OpenClaw plugin.

## Overview

The Marmot plugin supports multi-participant MLS group chats over Nostr. Your bot can:

- Join encrypted group chats via MLS welcomes
- Respond when @mentioned (mention gating)
- Buffer unmentioned messages as context for when it IS mentioned
- Resolve sender identities via Nostr profile names
- Distinguish between owner and friend permissions
- Maintain per-group memory

## Configuration

### Basic Group Chat Config

```json
{
  "channels": {
    "marmot": {
      "relays": ["wss://relay.damus.io", "wss://nos.lol", "wss://relay.primal.net"],
      "sidecarCmd": "marmotd",
      "stateDir": "~/.openclaw/.marmot-state",
      "autoAcceptWelcomes": true,
      "groupPolicy": "open",
      "groupAllowFrom": [
        "<owner-hex-pubkey>",
        "<friend1-hex-pubkey>",
        "<friend2-hex-pubkey>"
      ],
      "owner": "<owner-hex-pubkey>",
      "memberNames": {
        "<owner-hex-pubkey>": "Alice",
        "<friend1-hex-pubkey>": "Bob",
        "<friend2-hex-pubkey>": "Charlie"
      }
    }
  }
}
```

### Config Fields

#### `groupAllowFrom` (array of hex pubkeys)
Controls who can send messages to the bot in group chats. Messages from pubkeys not in this list are silently dropped.

#### `owner` (hex pubkey string)
The bot owner's pubkey. The owner gets `CommandAuthorized: true` which allows slash commands and elevated access. If not set, falls back to the first entry in `groupAllowFrom`.

**Important:** Separate `owner` from `groupAllowFrom`. Being in `groupAllowFrom` means "allowed to talk in groups." Being the `owner` means "has full administrative control."

#### `memberNames` (object: pubkey → display name)
Manual display name overrides. Supports both hex pubkeys and npub keys. These take priority over Nostr profile names fetched from relays.

#### `groupPolicy` ("open" | "allowlist")
- `"open"` — Accept messages from any group (filtered by `groupAllowFrom` for senders)
- `"allowlist"` — Only accept messages from groups explicitly listed in `groups` config

#### `groups` (object: group_id → config)
Per-group settings:

```json
{
  "channels": {
    "marmot": {
      "groups": {
        "*": { "requireMention": true },
        "<specific-group-id>": { "requireMention": false }
      }
    }
  }
}
```

#### `dmGroups` (array of group IDs)
Group IDs that should be treated as 1:1 DMs and routed to the bot's main session instead of an isolated group session. Useful for Pika-style DMs that are technically MLS groups.

## How It Works

### Mention Gating

In group chats, the bot only responds when mentioned. Mentions are detected by:

1. `nostr:npub1...` format (Pika/Nostr standard)
2. Raw npub string
3. Raw hex pubkey or `@pubkey`
4. Custom mention patterns from `agents.list[].groupChat.mentionPatterns`

When NOT mentioned, messages are buffered (up to 50) and injected as context when the bot IS eventually mentioned. This gives the bot conversational awareness without responding to every message.

### Sender Identity Resolution

The plugin resolves sender display names in this priority order:

1. **`memberNames` config** — manual overrides (checked first)
2. **Nostr profile (kind:0)** — fetched from relays, cached for 1 hour
3. **npub** — bech32-encoded pubkey (fallback)

Profile names are fetched asynchronously from configured relays using the native Node.js WebSocket API (Node 22+). Results are cached in-memory for 1 hour to avoid repeated relay queries.

### Sender Metadata

In group sessions, the bot receives sender metadata including:

- **SenderName** — resolved display name
- **SenderUsername** — sender's npub (for verifiable identity)
- **SenderTag** — `"owner"` or `"friend"` based on the `owner` config

This allows the bot to verify who is speaking even if display names could be spoofed.

### Owner vs Friend Permissions

| Capability | Owner | Friends |
|---|---|---|
| Chat in group | ✅ | ✅ (when allowed) |
| Trigger bot via @mention | ✅ | ✅ |
| Slash commands (/new, /reset, etc.) | ✅ | ❌ |
| CommandAuthorized | true | false |
| Mention gating | Same as friends | Mention required |

Owner messages in groups go through the same mention-gating flow as friends — the owner must tag the bot to get a response. This prevents the bot from replying to every owner message in a group.

### Session Routing

- **Group chats** → Isolated session per group (`marmot:<accountId>:<groupId>`)
- **DM groups** (in `dmGroups` config) → Main session (shared with other DM channels)
- **Owner DMs** → Main session

Each group session has its own conversation history, independent of the main session and other groups.

## Memory Management

### Recommended Workspace Layout

```
workspace/
  MEMORY.md              ← Long-term memory (loaded in all sessions)
  memory/YYYY-MM-DD.md   ← Daily logs
  groups/
    <nostr_group_id>/
      GROUP_MEMORY.md    ← Per-group context and notes
```

### Best Practices

- **MEMORY.md** — Load in all sessions (main + group). Group members are trusted friends in `groupAllowFrom`. Contains curated long-term knowledge.
- **GROUP_MEMORY.md** — Per-group scratchpad. Update freely with group-specific context: what's been discussed, decisions made, running jokes, active members.
- **Daily memory files** — Log important interactions from any session. These are the raw source; MEMORY.md is the curated distillation.

### AGENTS.md Example

Add instructions to your `AGENTS.md` for group memory handling:

```markdown
## Every Session
1. Read SOUL.md
2. Read memory/YYYY-MM-DD.md (today + yesterday)
3. If in MAIN SESSION: Read MEMORY.md
4. If in GROUP SESSION: Read MEMORY.md AND groups/<group_id>/GROUP_MEMORY.md
```

### Trust Model

Define trust tiers in your `AGENTS.md`:

```markdown
## Group Trust Model

### Owner
- Full access to everything
- Can run commands, access files, change config

### Friends (in groupAllowFrom)
- Can chat, ask questions, get help
- Cannot run commands, modify files, or access secrets
- Be helpful and engaging — they're trusted, not strangers

### What to protect
- Passwords, API keys, secret locations
- Private DM conversations
- Anything the owner shared in confidence
```

## Troubleshooting

### Bot doesn't respond in group
1. Check `groupAllowFrom` includes the sender's pubkey
2. Check `groupPolicy` is `"open"` or the group is in the allowlist
3. Make sure you're @mentioning the bot (use `nostr:npub1...` format)
4. Check logs: `journalctl -u openclaw.service | grep marmot`

### Profile names not resolving
- Profile fetch requires Node.js 22+ (native WebSocket)
- Check relay connectivity — the plugin tries the first 3 relays in your config
- Names are cached for 1 hour; restart gateway to clear cache
- Use `memberNames` as a reliable fallback

### "failed to accept welcome" errors
These are usually harmless — old welcome messages being replayed. The bot will still work with current groups.

### Owner messages creating new sessions instead of going to group
Make sure the owner's pubkey is set in the `owner` config field. Without it, owner messages may route differently.
