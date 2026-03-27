# pika-git

`pika-git` turns the canonical `pika` bare repo into a browser-first branch forge with tutorial-style review pages.

Hosted forge:

- Web UI: `https://git.pikachat.org/git`
- Canonical Git remote: `git@git.pikachat.org:pika.git`

## Config file (`pika-git.toml`)

```toml
poll_interval_secs = 60
model = "claude-sonnet-4-5-20250929"
api_key_env = "ANTHROPIC_API_KEY"
github_token_env = "GITHUB_TOKEN"
worker_concurrency = 2
retry_backoff_secs = 120
bind_address = "127.0.0.1"
bind_port = 8787
bootstrap_admin_npubs = ["npub1..."]

[forge_repo]
repo = "sledtools/pika"
canonical_git_dir = "/var/lib/pika-git/pika.git"
default_branch = "master"
mirror_remote = "github"
mirror_poll_interval_secs = 300
mirror_timeout_secs = 120
hook_url = "http://127.0.0.1:8787/git/webhook"
```

- `forge_repo`: canonical forge metadata for the single hosted `pika` bare repo.
- `forge_repo.mirror_remote`: outbound mirror remote name in the canonical bare repo, for example `github`.
- `forge_repo.mirror_poll_interval_secs`: hosted background mirror cadence in seconds. Set `0` to disable background mirroring and keep manual sync only.
- `forge_repo.mirror_timeout_secs`: hard timeout for outbound mirror push and inspection git commands. This bounds stale mirror jobs so they do not linger for hours and interfere with unrelated git work.
- `crates/pikaci/src/forge_lanes.rs`: checked-in source of truth for canonical pre-merge lane selection and nightly lane definitions. Branch pushes are evaluated against the branch head's proposed manifest; nightly uses the default branch manifest.
- `forge_repo.hook_url`: internal canonical-bare-repo hook target. This is used by the forge-managed Git hook path, not by GitHub repo webhooks.
- `poll_interval_secs`: interval used by hosted mode repair scans of the canonical bare repo.
- `model`: Anthropic model name for tutorial generation.
- `api_key_env`: environment variable containing the API key.
- `github_token_env`: environment variable containing the GitHub token used for outbound mirror operations.
- `worker_concurrency`: max hosted generation jobs claimed per pass.
- `retry_backoff_secs`: delay before retrying retry-safe generation failures.
- `bind_address`: hosted server bind address.
- `bind_port`: hosted server bind port.
- `bootstrap_admin_npubs`: Nostr pubkeys that can always sign in and manage the runtime chat allowlist from `/git/admin`.

## Canonical CI

- Branch-push pre-merge CI is now orchestrated by the forge from `crates/pikaci/src/forge_lanes.rs`.
- Nightly scheduling is now orchestrated by the forge service from the same manifest.
- GitHub Actions release and TestFlight workflows remain in place, but GitHub is no longer the canonical control plane for day-to-day pre-merge or nightly CI.
- Hosted mirror sync needs the configured Git remote plus whatever credentials that remote requires. For a GitHub HTTPS remote, set the env named by `github_token_env` so the admin page and manual sync controls can report failures cleanly.
- GitHub repo webhooks are not required for normal forge operation. The canonical forge uses the bare repo's installed hooks plus outbound mirroring to GitHub.

## Hosted Deploy And QA

- Use `docs/forge-hosted-manual-qa.md` as the short deploy and manual testing checklist for hosted forge mode.

## Local mode

`pika-git local` runs against the current git repo/worktree.

- Default diff base: `origin/main`, fallback `main`, then compatibility fallback to `origin/master` and `master`.
- Override base: `--base <ref>`.
- Append local staged/unstaged changes: `--include-uncommitted`.
- Output path: `--out <path>` (defaults to `./pika-git-local.html`).
- Auto-open: enabled by default; disable with `--no-open`.

## Local hosted dev

If you want the forge-style web UI locally, use:

```bash
./scripts/pika-git
```

That wrapper:

- creates a local hosted-mode config under `.tmp/`
- points the forge at this repo's shared git dir
- stores sqlite state at `.tmp/pika-git.db`
- exports `GITHUB_TOKEN` from `gh auth token`
- runs `pika-git serve` under `cargo watch`

The default bind is `127.0.0.1:8787`. Override with `PIKA_GIT_PORT=8788 ./scripts/pika-git`.
