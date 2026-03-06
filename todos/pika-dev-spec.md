---
summary: Spec for pika-dev — a lightweight TypeScript service that watches GitHub issues labeled `pika-dev`, spawns Pi coding agent sessions to solve them, and exposes a web UI for monitoring, steering, and forking sessions locally
read_when:
  - working on pika-dev service
  - setting up automated issue-solving agents
  - deploying to dev.pikachat.org
  - integrating Pi coding agent as a service
---

# pika-dev — Automated Issue Solver

A thin TypeScript service that watches for GitHub issues labeled `pika-dev` on
`sledtools/pika`, spawns Pi coding agent sessions to work on them (max 5
concurrent), and exposes a web UI where whitelisted devs can monitor progress,
steer agents via chat, and fork sessions locally.

Deployed to `dev.pikachat.org` on pika-build. Separate service from pika-news
(which stays at `news.pikachat.org`). Eventually needs to run on Mac too (for
iOS-related features).

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────┐
│  pika-dev (TypeScript / Node.js)                        │
│                                                          │
│  ┌────────────┐  ┌─────────────┐  ┌──────────────────┐  │
│  │ Issue       │  │ Agent       │  │ Web UI           │  │
│  │ Poller      │→ │ Scheduler   │→ │ (Hono + SSE)     │  │
│  │ (GitHub API)│  │ (max 5)     │  │                  │  │
│  └────────────┘  └─────────────┘  └──────────────────┘  │
│                        ↓                   ↑             │
│                ┌───────────────┐           │             │
│                │ Agent Runner  │───────────┘             │
│                │ (Pi SDK)      │        (events/SSE)     │
│                │  × up to 5   │                          │
│                └───────────────┘                          │
│                                                          │
│  Storage: SQLite (better-sqlite3)                        │
│  Auth: NIP-98 (same whitelisted npubs as pika-news)      │
│  Agent: @mariozechner/pi-coding-agent SDK                │
└─────────────────────────────────────────────────────────┘
```

---

## Tech Stack

| Layer | Choice | Rationale |
|-------|--------|-----------|
| Runtime | Node.js (LTS) | Pi SDK is TypeScript, native integration |
| Web framework | Hono | Tiny, fast, runs everywhere (Node, Bun, Deno) |
| Agent SDK | `@mariozechner/pi-coding-agent` | Direct SDK — spawn, steer, follow-up, fork, get_state |
| LLM provider | `@mariozechner/pi-ai` | Multi-provider (Anthropic, OpenAI, Google, etc.) |
| Chat UI components | `@mariozechner/pi-web-ui` | Reusable web components for AI chat (already built) |
| Database | SQLite via `better-sqlite3` | Simple, no external deps, matches pika-news pattern |
| Live streaming | SSE (Server-Sent Events) | Agent events → browser in real-time |
| Auth | Nostr NIP-98 | Same mechanism + whitelist as pika-news |
| Git isolation | `git worktree` | Each issue gets its own worktree + branch |
| PR creation | `gh` CLI | Already available on pika-build |
| Deployment | NixOS + systemd | Same infra as pika-news |

---

## Components

### 1. Issue Poller

Polls GitHub API for issues on `sledtools/pika` labeled `pika-dev`. Mirrors
the polling pattern from pika-news's PR poller.

**Behavior:**
- Poll interval: configurable, default 60 seconds
- Fetches open issues with `pika-dev` label via `GET /repos/{owner}/{repo}/issues?labels=pika-dev&state=open`
- Upserts into SQLite: issue number, title, body, author, labels, created_at, updated_at
- Tracks `last_polled_at` cursor to avoid re-processing
- Detects new issues → sets status to `queued`
- Detects issue edits (body changed) → can re-prompt running agent or re-queue
- Detects issue closed → tears down running agent session if any
- Detects label removed → tears down running agent session if any

**SQLite schema:**

```sql
CREATE TABLE issues (
    id INTEGER PRIMARY KEY,
    issue_number INTEGER UNIQUE NOT NULL,
    title TEXT NOT NULL,
    body TEXT,
    author TEXT,
    labels TEXT,  -- JSON array
    state TEXT NOT NULL DEFAULT 'open',
    github_updated_at TEXT,
    last_polled_at TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE sessions (
    id INTEGER PRIMARY KEY,
    issue_id INTEGER NOT NULL REFERENCES issues(id),
    status TEXT NOT NULL DEFAULT 'queued',
      -- queued | starting | running | completed | failed | cancelled
    branch_name TEXT,
    worktree_path TEXT,
    pi_session_path TEXT,  -- path to Pi JSONL session file
    pr_number INTEGER,
    pr_url TEXT,
    error_message TEXT,
    started_at TEXT,
    completed_at TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE session_events (
    id INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES sessions(id),
    event_type TEXT NOT NULL,
      -- agent_start | turn_start | message_start | message_update |
      -- tool_execution_start | tool_execution_end | turn_end |
      -- agent_end | steer | error
    payload TEXT,  -- JSON
    created_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE chat_messages (
    id INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES sessions(id),
    role TEXT NOT NULL,  -- 'user' | 'assistant' | 'system'
    npub TEXT,           -- who sent it (for user messages)
    content TEXT NOT NULL,
    created_at TEXT DEFAULT (datetime('now'))
);
```

### 2. Agent Scheduler

Manages the pool of active agent sessions. Enforces the concurrency limit.

**Behavior:**
- Runs on a tick (every 5 seconds)
- Counts sessions with status `starting` or `running`
- If count < `max_concurrent` (default 5), picks oldest `queued` session
- Hands off to Agent Runner
- Monitors running sessions for timeouts (configurable, default 30 min)
- On timeout: sends abort to Pi, marks session as `failed`
- On agent completion: triggers PR creation flow

**Concurrency limit rationale:** 5 is enough to keep the pipeline moving without
overwhelming pika-build's resources. Each Pi session is a Node.js process doing
LLM API calls + occasional bash/file ops — not CPU-heavy.

### 3. Agent Runner

Spawns and manages Pi coding agent sessions using the SDK directly.

**Setup per session:**

```bash
# Create isolated worktree for this issue
git worktree add /var/lib/pika-dev/workspaces/issue-{N} \
    -b pika-dev/issue-{N} origin/main
```

**Pi SDK integration:**

```typescript
import { createAgentSession } from "@mariozechner/pi-coding-agent";
import { getModel } from "@mariozechner/pi-ai";

const model = getModel(config.provider, config.model);
// e.g. getModel("anthropic", "claude-sonnet-4-5-20250929")

const { session } = await createAgentSession({
    model,
    thinkingLevel: config.thinkingLevel,  // "high"
    workingDirectory: worktreePath,
    sessionManager: SessionManager.file(sessionDir),
    tools: ["read", "write", "edit", "bash", "grep", "glob", "find", "ls"],
    systemPrompt: buildSystemPrompt(issue),
    extensions: [pikaDevExtension],  // custom extension for git/PR ops
});

// Subscribe to all events for streaming to web UI
session.subscribe((event) => {
    persistEvent(sessionId, event);
    broadcastSSE(sessionId, event);
});

// Initial prompt
await session.prompt(buildIssuePrompt(issue));
```

**System prompt template:**

```
You are pika-dev, an automated coding agent working on the pika project
(an end-to-end encrypted messaging app built on MLS over Nostr).

You are solving GitHub issue #{number}: "{title}"

Issue body:
{body}

Instructions:
- You are working in an isolated git worktree on branch `pika-dev/issue-{N}`
- Read relevant code to understand the codebase before making changes
- Make focused, minimal changes that address the issue
- Run `just test` to verify your changes compile and pass tests
- When you are done, summarize what you changed and why

Repository structure:
- rust/         — Core Rust library (pika_core, MLS, Nostr, app state)
- ios/          — iOS app (SwiftUI)
- android/      — Android app (Kotlin)
- cli/          — pikachat CLI
- crates/       — Workspace crates (~22 crates)
- docs/         — Architecture documentation
```

**On completion:**

```bash
# In the worktree
git add -A
git commit -m "pika-dev: Fix #{N} — {title}"
git push origin pika-dev/issue-{N}

# Create draft PR
gh pr create \
    --title "pika-dev: Fix #{N} — {title}" \
    --body "$(cat <<'EOF'
Automated fix for #{N}.

## Summary
{agent_summary}

## Changes
{file_list}

---
Generated by [pika-dev](https://dev.pikachat.org) using Pi coding agent.
Fork locally: `just dev-fork {N}`
EOF
)" \
    --draft \
    --base main \
    --head pika-dev/issue-{N}
```

**On completion, also comment on the issue:**

```bash
gh issue comment {N} --body "$(cat <<'EOF'
pika-dev created a draft PR: #{pr_number}

**Fork locally:**
```
just dev-fork {N}
```

**View session:** https://dev.pikachat.org/session/{session_id}
EOF
)"
```

### 4. Steering (Re-prompt via Chat)

Whitelisted devs can steer a running agent session through the web UI chat.

**Flow:**
1. Dev authenticates via NIP-98 (same as pika-news)
2. Opens session detail page → "Agent Chat" tab
3. Types a message
4. Server calls `session.steer(message)` on the running Pi session
5. Pi incorporates the steering and continues working
6. All events stream back via SSE

**Pi SDK steering modes:**
- `session.steer(msg)` — interrupt current work, redirect agent
- `session.followUp(msg)` — add context after agent finishes current turn

**Chat is read-only when:**
- Session status is `completed`, `failed`, or `cancelled`
- Shows full history + agent summary + PR link + fork button

### 5. Web UI

Minimal server-rendered HTML + SSE for live updates. No SPA framework — just
Hono serving HTML templates with progressive enhancement via pi-web-ui
components and vanilla JS for SSE.

**Routes:**

| Route | Description |
|-------|-------------|
| `GET /` | Dashboard — active sessions, queue, completed |
| `GET /session/:id` | Session detail — live activity + chat + fork |
| `GET /session/:id/events` | SSE stream of agent events |
| `POST /session/:id/chat` | Send chat message (steer/follow-up) |
| `GET /session/:id/export` | Download Pi session JSONL |
| `POST /auth/challenge` | NIP-98 challenge |
| `POST /auth/verify` | NIP-98 verify → token |
| `GET /health` | Health check |

**Dashboard (`/`):**

```
┌─────────────────────────────────────────────────────┐
│  pika-dev                            Active: 3 / 5  │
├─────────────────────────────────────────────────────┤
│                                                      │
│  ● Running                                          │
│  ┌────────────────────────────────────────────────┐  │
│  │ #142  Fix MLS key rotation bug                 │  │
│  │       Turn 12 · 8 files read · 2 files edited  │  │
│  │       Started 14 min ago                        │  │
│  │       [View] [Chat]                             │  │
│  ├────────────────────────────────────────────────┤  │
│  │ #138  Add relay reconnect backoff              │  │
│  │       Turn 5 · 3 files read                     │  │
│  │       Started 6 min ago                         │  │
│  │       [View] [Chat]                             │  │
│  └────────────────────────────────────────────────┘  │
│                                                      │
│  ◌ Queued                                           │
│  ┌────────────────────────────────────────────────┐  │
│  │ #145  Fix push notification crash on Android   │  │
│  │       Queued 2 min ago (waiting for slot)       │  │
│  └────────────────────────────────────────────────┘  │
│                                                      │
│  ✓ Completed                                        │
│  ┌────────────────────────────────────────────────┐  │
│  │ #135  Handle empty group name gracefully       │  │
│  │       PR #201 (draft) · 3 files changed        │  │
│  │       Completed 1 hour ago                      │  │
│  │       [View] [PR] [Fork]                        │  │
│  └────────────────────────────────────────────────┘  │
│                                                      │
└─────────────────────────────────────────────────────┘
```

**Session detail (`/session/:id`):**

```
┌─────────────────────────────────────────────────────┐
│  Issue #142: Fix MLS key rotation bug               │
│  Status: Running · Turn 12 · Branch: pika-dev/142   │
├────────────────────────┬────────────────────────────┤
│                        │                            │
│  Agent Activity        │  [Chat] [Logs]             │
│                        │                            │
│  ▸ Turn 1              │  ┌──────────────────────┐  │
│    Reading             │  │ justin: Try using    │  │
│    rust/src/mls/mod.rs │  │ the commit_pending   │  │
│                        │  │ API instead of the   │  │
│  ▸ Turn 2              │  │ manual approach      │  │
│    Reading             │  │                      │  │
│    rust/src/mls/       │  │ agent: Good call,    │  │
│      key_rotation.rs   │  │ switching to         │  │
│                        │  │ commit_pending...    │  │
│  ▸ Turn 3              │  │                      │  │
│    Editing             │  │ ┌──────────────────┐ │  │
│    key_rotation.rs     │  │ │ Type a message.. │ │  │
│    +12 -3 lines        │  │ │          [Send]  │ │  │
│                        │  │ └──────────────────┘ │  │
│  ▸ Turn 4              │  └──────────────────────┘  │
│    Running             │                            │
│    `just test`         │  Fork locally:             │
│    ✓ passed            │  ┌──────────────────────┐  │
│                        │  │ just dev-fork 142    │  │
│  ...                   │  │              [Copy]  │  │
│                        │  └──────────────────────┘  │
│                        │                            │
│                        │  [Download Session JSONL]  │
│                        │                            │
└────────────────────────┴────────────────────────────┘
```

### 6. Fork Locally

Dead simple for v1. The agent pushes its WIP branch to GitHub after each
meaningful step (commit after every successful test run or significant edit
batch).

**`just dev-fork` recipe (added to pika justfile):**

```bash
# Fork a pika-dev agent session locally
dev-fork ISSUE:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Fetching pika-dev branch for issue #{{ ISSUE }}..."
    git fetch origin "pika-dev/issue-{{ ISSUE }}"
    git checkout "pika-dev/issue-{{ ISSUE }}"
    echo ""
    echo "You're now on the agent's branch."
    echo "The agent's changes are in your working tree."
    echo "Continue working with your preferred tool (pi, claude, codex, etc.)"
```

**Future enhancement (v2):** Export the Pi session JSONL so devs can resume
the exact conversation:

```bash
dev-fork-with-session ISSUE:
    #!/usr/bin/env bash
    set -euo pipefail
    git fetch origin "pika-dev/issue-{{ ISSUE }}"
    git checkout "pika-dev/issue-{{ ISSUE }}"
    # Download session from pika-dev service
    curl -sL "https://dev.pikachat.org/session/issue-{{ ISSUE }}/export" \
        -o .pi/sessions/pika-dev-{{ ISSUE }}.jsonl
    echo "Session downloaded. Resume with: pi --session .pi/sessions/pika-dev-{{ ISSUE }}.jsonl"
```

### 7. Auth (NIP-98)

Identical to pika-news. Reuse the same Nostr-based auth flow.

**Whitelist:** Same `allowed_npubs` list as pika-news (configurable in
`pika-dev.toml`). Controls who can:
- View the dashboard (public, no auth needed)
- Send chat messages / steer agents (auth required, whitelist enforced)
- Export sessions (auth required, whitelist enforced)

**Flow:**
1. Client requests challenge: `POST /auth/challenge` → `{ nonce }`
2. Client signs with Nostr key (NIP-98 kind 27235)
3. Client verifies: `POST /auth/verify` → `{ token, npub }`
4. Token used in subsequent requests (cookie or header), 24h TTL

Can share the auth implementation with pika-news via a shared npm package or
just copy the ~100 lines of NIP-98 verification code.

---

## Configuration

**`pika-dev.toml`:**

```toml
# GitHub
github_repo = "sledtools/pika"
github_label = "pika-dev"
github_token_env = "GITHUB_TOKEN"
poll_interval_secs = 60

# Agent
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
thinking_level = "high"
max_concurrent_sessions = 5
session_timeout_mins = 30
workspace_base = "/var/lib/pika-dev/workspaces"
session_base = "/var/lib/pika-dev/sessions"

# Web
bind_address = "127.0.0.1"
bind_port = 8789

# Auth
allowed_npubs = [
    "npub1z6ujr8...",  # Gigi
    "npub1derggg...",  # Gladstein
    "npub1qny3tk...",  # Odell
    # ... same list as pika-news
]
```

---

## Project Structure

```
crates/pika-dev/
├── package.json
├── tsconfig.json
├── pika-dev.toml           # default config
├── src/
│   ├── index.ts            # entry point — start poller, scheduler, server
│   ├── config.ts           # load & validate pika-dev.toml
│   ├── db.ts               # SQLite schema, migrations, queries
│   ├── poller.ts           # GitHub issue polling loop
│   ├── scheduler.ts        # session queue management, concurrency control
│   ├── runner.ts           # Pi SDK session lifecycle (spawn, steer, teardown)
│   ├── git.ts              # worktree management, commit, push, PR creation
│   ├── auth.ts             # NIP-98 challenge/verify/token
│   ├── server.ts           # Hono routes + SSE endpoints
│   ├── templates/          # HTML templates (or just template literals)
│   │   ├── dashboard.ts
│   │   ├── session.ts
│   │   └── layout.ts
│   └── extension.ts        # Pi extension for pika-specific tools/context
├── static/
│   └── style.css
└── README.md
```

~15 source files. Target: under 2,000 lines total for v1.

---

## Deployment

### NixOS module (`infra/nix/modules/pika-dev.nix`)

```nix
{ config, pkgs, ... }:

let
  pikaDevPkg = pkgs.buildNpmPackage {
    pname = "pika-dev";
    version = "0.1.0";
    src = ../../crates/pika-dev;
    npmDepsHash = "sha256-...";
  };
in {
  systemd.services.pika-dev = {
    description = "pika-dev automated issue solver";
    wantedBy = [ "multi-user.target" ];
    after = [ "network-online.target" ];

    serviceConfig = {
      Type = "simple";
      User = "pika-dev";
      Group = "pika-dev";
      WorkingDirectory = "/var/lib/pika-dev";
      EnvironmentFile = [ "/run/secrets/pika-dev-env" ];
      ExecStart = "${pikaDevPkg}/bin/pika-dev --config /etc/pika-dev/pika-dev.toml --db /var/lib/pika-dev/pika-dev.db";
      Restart = "always";
      RestartSec = "5s";

      # Security hardening
      NoNewPrivileges = true;
      ProtectSystem = "strict";
      ReadWritePaths = [ "/var/lib/pika-dev" ];
    };
  };

  users.users.pika-dev = {
    isSystemUser = true;
    group = "pika-dev";
    home = "/var/lib/pika-dev";
    createHome = true;
  };
  users.groups.pika-dev = {};
}
```

**Secrets** (via sops-nix):
- `GITHUB_TOKEN` — GitHub API token (for polling issues + creating PRs)
- `ANTHROPIC_API_KEY` — or whichever provider is configured

**Reverse proxy:** nginx on pika-build, `dev.pikachat.org` → `127.0.0.1:8789`

### Justfile recipes

```bash
# Run pika-dev locally
dev MAX_SESSIONS="2":
    #!/usr/bin/env bash
    set -euo pipefail
    set -a; source .env; set +a
    export GITHUB_TOKEN="$(gh auth token)"
    cd crates/pika-dev
    npx tsx src/index.ts \
        --config pika-dev.toml \
        --db ../../.tmp/pika-dev.db \
        --max-sessions {{ MAX_SESSIONS }}

# Fork a pika-dev agent session locally
dev-fork ISSUE:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Fetching pika-dev branch for issue #{{ ISSUE }}..."
    git fetch origin "pika-dev/issue-{{ ISSUE }}"
    git checkout "pika-dev/issue-{{ ISSUE }}"
    echo ""
    echo "You're now on the agent's branch."
    echo "Continue working with your preferred tool."
```

---

## Implementation Plan

### Phase 1: Core loop (MVP)

Get the basic issue → agent → PR pipeline working end-to-end.

1. **Scaffold project** — `package.json`, deps, tsconfig, entry point
2. **Database** — SQLite schema + migration runner + basic queries
3. **Config** — TOML loader with validation
4. **Issue Poller** — GitHub API polling, upsert issues, detect new labeled issues
5. **Git utilities** — worktree create/remove, commit, push, `gh pr create`
6. **Agent Runner** — Pi SDK integration, spawn session in worktree, subscribe
   to events, handle completion/failure
7. **Scheduler** — tick loop, enforce concurrency limit, timeout handling
8. **Wire it all together** — index.ts starts poller + scheduler + handles
   graceful shutdown

Test by manually creating a labeled issue and watching it get picked up.

### Phase 2: Web UI

Add the dashboard and session detail pages.

1. **Hono server** — basic routes, static files, HTML templates
2. **Dashboard page** — list active/queued/completed sessions
3. **Session detail page** — show agent activity log from DB
4. **SSE endpoint** — stream live events for running sessions
5. **Event persistence** — store agent events in `session_events` table
6. **Live activity view** — JS that connects to SSE and updates the page

### Phase 3: Chat + Auth

Add steering and authentication.

1. **NIP-98 auth** — challenge/verify endpoints, token management
2. **Chat UI** — message input + history display on session detail page
3. **Chat backend** — receive message, call `session.steer()`, store in DB
4. **Chat via SSE** — include assistant responses in the event stream
5. **Permission gating** — auth required for chat, public for viewing

### Phase 4: Fork locally + polish

1. **Periodic push** — agent commits + pushes WIP after each successful test run
2. **Justfile recipes** — `dev-fork` command
3. **Session export** — download JSONL endpoint
4. **GitHub comments** — bot comments on issue when session starts/completes
5. **Error handling** — retry logic, graceful degradation, session cleanup
6. **Worktree cleanup** — prune old worktrees after PR is merged or session
   is stale (>7 days)

---

## Open Questions / Future Work

- **Model selection per issue:** Could use issue labels to select different
  models (e.g. `pika-dev:opus` for hard problems, `pika-dev:sonnet` default).
  For now, single model configured globally.

- **Pi extension for pika:** A custom extension that registers pika-specific
  tools (run iOS simulator, check MLS test vectors, etc.) and injects project
  context (AGENTS.md, architecture docs). Not needed for v1 — the system prompt
  + standard tools are sufficient.

- **Session resumption:** If the service restarts, can we reconnect to running
  Pi sessions? Pi sessions persist as JSONL, so we could resume them. For v1,
  just mark interrupted sessions as `failed` and let them be re-queued.

- **Mac deployment:** Service needs to run on Mac for iOS builds eventually.
  TypeScript + Node.js is fully cross-platform, so this is mostly a deployment
  question (launchd instead of systemd, Homebrew instead of Nix). Defer.

- **Integration with pika-news:** Could eventually share the same web UI,
  showing PR tutorials alongside agent sessions. Or link from pika-news PR
  tutorial to the pika-dev session that created it. Defer — keep separate
  for now.

- **Webhook vs polling:** Could switch from polling to GitHub webhooks for
  faster response. Requires exposing a public endpoint or using a webhook
  relay. Polling is simpler and good enough for v1 (60s latency is fine).

- **Cost tracking:** Log token usage per session (Pi supports this via events).
  Show on dashboard. Useful for budgeting.

- **Multiple repos:** Config supports a single repo for now. Could extend to
  watch multiple repos. Low priority.

- **Download button on web UI:** The session detail page has a "Download
  Session JSONL" link and a copyable `just dev-fork N` command. Future: could
  generate a one-click `.sh` script that does both (fetches branch + downloads
  session).
