# pika-news

`pika-news` turns pull request diffs into browser-first tutorial artifacts.

## Config file (`pika-news.toml`)

```toml
repos = ["sledtools/pika"]
poll_interval_secs = 60
model = "claude-sonnet-4-5-20250929"
api_key_env = "ANTHROPIC_API_KEY"
github_token_env = "GITHUB_TOKEN"
merged_lookback_hours = 72
worker_concurrency = 2
retry_backoff_secs = 120
bind_address = "127.0.0.1"
bind_port = 8787
bootstrap_admin_npubs = ["npub1..."]
```

- `repos`: explicit `owner/repo` list for hosted polling mode.
- `poll_interval_secs`: interval used by hosted mode ingestion loop.
- `model`: Anthropic model name for tutorial generation.
- `api_key_env`: environment variable containing the API key.
- `github_token_env`: environment variable containing GitHub API token.
- `merged_lookback_hours`: merged PR lookback window used by poller.
- `worker_concurrency`: max hosted generation jobs claimed per pass.
- `retry_backoff_secs`: delay before retrying retry-safe generation failures.
- `bind_address`: hosted server bind address.
- `bind_port`: hosted server bind port.
- `bootstrap_admin_npubs`: Nostr pubkeys that can always sign in and manage the runtime chat allowlist from `/news/admin`.

`allowed_npubs` remains as a legacy chat-access list for existing deployments, but it no longer grants admin rights. Set `bootstrap_admin_npubs` explicitly for anyone who should manage the in-app admin page.

## Local mode

`pika-news local` runs against the current git repo/worktree.

- Default diff base: `origin/main`, fallback `main`, then compatibility fallback to `origin/master` and `master`.
- Override base: `--base <ref>`.
- Append local staged/unstaged changes: `--include-uncommitted`.
- Output path: `--out <path>` (defaults to `./pika-news-local.html`).
- Auto-open: enabled by default; disable with `--no-open`.
