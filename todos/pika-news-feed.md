## Spec
Build a standalone Rust tool tentatively named `pika-news` with a browser-first UX, developed in this monorepo first (`crates/pika-news`) but intentionally structured for later extraction into `~/code` as an independent repo.

Intent and expected outcome:
- Turn PRs into structured, AI-generated tutorials that improve review throughput and onboarding.
- Make deployment generic so other teams can self-host it for their own GitHub orgs/repos, not just `sledtools/pika`.
- Dogfood in the monorepo first while keeping boundaries clean so extraction is low-friction after validation.
- Support a local developer flow where a CLI command runs against a branch/worktree and opens a browser tutorial output immediately.

Exact build target:
- Add a new Rust crate and binary in this repo (`crates/pika-news`) that can run as:
  - a long-running web service for remote GitHub repos and PR feeds
  - a local one-shot mode that analyzes a local git worktree/branch and launches browser output
- Keep UI browser-only for v1 (SSR + minimal JS); do not build a TUI in this phase.
- Support repository targeting in hosted mode via explicit repo list (`owner/repo`) in config.
- Keep PR media as original GitHub links (no re-hosting).

Exact approach:
- Build local mode first, then hosted mode; do not block early value on server infrastructure.
- Keep shared code simple for v1: plain modules/functions for `diff -> tutorial`, not trait-heavy provider abstractions.
- Use polling-only ingestion for launch; no webhook path in v1.
- Summary pipeline stores explicit generation status (`pending`, `generating`, `ready`, `failed`) and requeues when `head_sha` changes.
- Tutorial output remains layered:
  - L1 executive summary + extracted media links
  - L2 ordered tutorial steps with markdown and compact diff snippets
- Prompting is contract-first and structured:
  - input includes PR metadata, file list, and bounded unified diff content
  - output is validated structured JSON with executive summary + ordered steps
  - each step must include intent, affected files, and evidence snippets anchored to diff hunks
- Local mode semantics are explicit:
  - default comparison is `git diff <base>...HEAD` where `<base>` defaults to `origin/main` (fallback `main`)
  - `--base <ref>` overrides comparison base
  - `--include-uncommitted` optionally appends staged/unstaged changes
  - output writes a self-contained HTML file (`--out` optional) and opens with platform opener (`open` on macOS, `xdg-open` on Linux); `--no-open` disables auto-open
- SSR with Askama templates and server-rendered markdown sanitized via `ammonia`; minimal client JS for step navigation and progressive updates.
- Storage defaults to SQLite for easy self-host deployment.
- Packaging and config are tool-centric (single binary + env/config file), not coupled to `pika-server` infra.

## Plan
1. Scaffold `crates/pika-news` with concrete CLI and config contracts.
Acceptance criteria: workspace builds `pika-news`; CLI includes `serve` and `local`; config file format is defined (repo list, poll interval, model, API key env var, bind address/port); `serve` bind default is explicit and documented in `--help`.

2. Implement local mode end-to-end first (`diff -> generate -> html -> open`).
Acceptance criteria: `pika-news local` runs in current repo/worktree by default; default diff is `<base>...HEAD` with `<base>` defaulting to `origin/main` (fallback `main`); `--base` override and `--include-uncommitted` work; tool writes self-contained HTML and auto-opens browser unless `--no-open` is set.

3. Implement prompt + response contract for tutorial quality.
Acceptance criteria: Anthropic request/response path is deterministic and schema-validated; output includes one executive summary and ordered tutorial steps with titles, intent, affected files, and diff evidence; malformed model output fails clearly with retry-safe errors.

4. Add SQLite storage and migrations for hosted mode state.
Acceptance criteria: schema includes repos, PR records, generation status (`pending/generating/ready/failed`), generated summary/tutorial artifacts, and last-polled markers per repo; migrations run on a fresh DB and preserve existing data.

5. Implement polling ingestion for configured repos (no webhook).
Acceptance criteria: poller syncs open PRs plus merged lookback for explicit configured repos; unified upsert path writes PR metadata and updates `head_sha`; `head_sha` changes mark existing generated artifacts stale and queue regeneration.

6. Implement hosted generation worker.
Acceptance criteria: queued items transition `pending -> generating -> ready|failed`; concurrency is bounded; retries/backoff and failure metadata are persisted; generated artifact format matches local mode output shape.

7. Build browser-first hosted UI (SSR + minimal JS).
Acceptance criteria: feed shows open and merged PRs across configured repos; PR detail page shows summary and tutorial steps with button/arrow navigation; rendering pipeline sanitizes markdown before HTML output; no TUI is introduced.

8. Add tests and sanity checks for local and hosted critical paths.
Acceptance criteria: tests cover diff-base resolution, repo config parsing, poll/upsert transitions, stale-on-`head_sha` change behavior, markdown sanitization, and local-mode artifact generation; `cargo fmt` passes when implementation begins.

9. Manual QA gate (user-run).
Acceptance criteria: user verifies that (a) local mode on a worktree opens a useful tutorial in browser, (b) `--base` and `--include-uncommitted` behave as expected, (c) hosted mode ingests configured repos and renders combined feed, (d) open PRs show generating states then resolve, and (e) merged/open refresh behavior matches live GitHub changes.
