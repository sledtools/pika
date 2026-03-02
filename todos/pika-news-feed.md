## Spec
Build `news.pikachat.org` directly into the existing Rust service at `crates/pika-server` as an SSR-first PR news system for `sledtools/pika`, with minimal client JS and no SPA framework.

Intent and expected outcome:
- Improve code review throughput by turning PRs into structured, AI-generated learning artifacts.
- Public users can browse PR news; only authorized NIP-07 users can use AI chat.
- Open PRs and merged PRs are displayed in separate feed sections, with open PR entries showing loading state while summaries are generating.

Exact build target:
- Extend the existing `pika-server` binary (Axum + Diesel/Postgres) rather than creating a new service.
- Add news routes under `/news/*`.
- Persist news data in new Diesel/Postgres tables in the same DB.
- Use Anthropic API for summary/tutorial generation with configurable model via `NEWS_SUMMARY_MODEL` env var (default `claude-sonnet-4-5-20250929`; bump to Opus if quality is insufficient).
- Keep PR media as GitHub links only (no re-hosting).

Exact approach:
- Ingestion is hybrid: polling loop is mandatory/authoritative for launch; webhook path is additive and writes into the same upsert path.
- Summary pipeline stores explicit generation status (`pending`, `generating`, `ready`, `failed`) and requeues when PR `head_sha` changes.
- L1 summary: executive paragraph plus extracted GitHub media links.
- L2 summary: ordered tutorial steps with markdown and small diff snippets, navigable by buttons/arrow keys.
- L3 chat: NIP-07 gated, follow-graph gated (followers of `npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y`), with PR-scoped agent session and PR checkout/worktree context.
- SSR via Askama (compile-time checked templates) in `crates/pika-server/templates/news/`. Render markdown to HTML server-side with `pulldown-cmark`, sanitize with `ammonia`.
- Security and safety: webhook signature verification, markdown sanitization before HTML render, bounded worker/session concurrency, TTL cleanup for stale chat/worktree state.
- Secrets/config: extend existing sops-managed `infra/secrets/pika-server.yaml` and `infra/nix/modules/pika-server.nix` env template; no ad hoc secret files.

## Plan
1. Add persistent data model and migration layer in `pika-server` for PR news and chat metadata.
Acceptance criteria: Diesel migration adds `news_pr` (with `media_urls JSONB` column for GitHub media links), `news_summary` (with status enum `pending/generating/ready/failed`), `news_tutorial_step`, `news_chat_session`, and `news_session` (auth) tables; schema supports summary status transitions, PR head-sha invalidation, and per-PR tutorial steps.

2. Implement GitHub ingestion (polling-first, webhook-additive) and unified PR upsert path.
Acceptance criteria: background poller syncs open PRs + merged lookback for `sledtools/pika`; webhook endpoint verifies HMAC signature and triggers same upsert path; open/merged state transitions are reflected in DB; polling alone fully populates feed/backfill behavior.

3. Implement summary/tutorial generation worker with quality-first Anthropic integration.
Acceptance criteria: queued PRs transition through `pending -> generating -> ready|failed`; worker writes L1 executive markdown, media links, and L2 tutorial steps with diff snippets; model is env-configurable via `NEWS_SUMMARY_MODEL`; concurrent generation bounded by semaphore (max 2); retries/error capture are persisted.

4. Add SSR news UI routes and templates with minimal JS step navigation.
Acceptance criteria: Askama templates in `crates/pika-server/templates/news/`; `/news` renders separate Open PR and Merged PR sections; open PRs with in-progress jobs show generating state; `/news/pr/:number` renders executive summary, media, and step-by-step tutorial; arrow keys/buttons navigate tutorial steps without SPA framework; hand-written CSS (~200 lines, dark mode via `prefers-color-scheme`) and vanilla JS served from `/news/static/`.

5. Implement NIP-07 auth and follow-graph gate for chat access.
Acceptance criteria: challenge/sign/verify flow works with `window.nostr`; server validates signatures and session freshness; follow cache is maintained in memory with periodic refresh from Nostr kind-3 contacts for `npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y`; unauthorized users cannot create chat sessions.

6. Implement PR-scoped AI chat runtime with isolated checkout/worktree context.
Acceptance criteria: authenticated users can create chat sessions for a PR; backend uses PR-specific checkout/worktree context; chat streams responses via SSE + POST message API; session/worktree TTL cleanup is enforced; concurrency limits and failure states are explicit.

7. Wire deployment/config/secrets and admin recovery hooks inside existing pika-server infra.
Acceptance criteria: `infra/secrets/pika-server.yaml` and `infra/nix/modules/pika-server.nix` include required news env vars/secrets (GitHub token/webhook secret, Anthropic key/model, chat runtime config); `/news/admin/resync` (secret-protected) can trigger manual re-sync; runtime dependencies for chat are introduced only when chat step is enabled.

8. Add tests and verification coverage for ingestion, status transitions, rendering safety, auth gating, and chat lifecycle.
Acceptance criteria: automated tests cover PR upsert + status transitions, webhook verification, summary parsing/persistence, follow-gate auth checks, and chat session lifecycle/cleanup; `cargo fmt` and targeted `pika-server` tests pass.

9. Manual QA gate (user-run).
Acceptance criteria: user verifies on a deployed/staging `pika-server` instance that (a) `/news` shows open and merged sections, (b) open PRs display generating states then resolve to summaries, (c) tutorial step navigation works by keyboard and buttons, (d) unauthenticated users cannot access chat, (e) followed NIP-07 users can chat against correct PR context, and (f) merged PR and open PR refresh behavior matches live GitHub changes.
