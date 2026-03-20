---
summary: Validated bootstrap note for reaching the fresh Mac mini from this Mac
read_when:
  - reconnecting to the Mac mini for manual remote experiments
  - checking which bootstrap access path was actually validated
---

## Mac mini remote access

- Machine during validation: `mini's Mac mini`
- Local user during validation: `mini`
- Validated access method: ordinary macOS `Remote Login` (`ssh`) over Tailscale network connectivity

### SSH enablement

`System Settings` -> `General` -> `Sharing` -> enable `Remote Login` for user `mini`.

The validated shell path used the mini user's normal `~/.ssh/authorized_keys`. During bootstrap, the current operator added this Mac's public key there. No separate Tailscale SSH setup was required for the validated path.

### Tailscale

Tailscale was installed on the mini via the macOS download from `tailscale.com`, then joined to the same tailnet as this Mac.

During validation on 2026-03-15, the mini appeared on the tailnet with observed IPv4 `100.92.43.40` and observed MagicDNS name `minis-mac-mini.taild659a1.ts.net`. Treat those as observed values, not a stable provisioning contract.

The first remote shell did not have `tailscale` on `PATH`; the app binary path worked:

```bash
/Applications/Tailscale.app/Contents/MacOS/Tailscale ip -4
```

### Reconnect later

From this Mac, use the mini's current Tailscale IP or current MagicDNS name:

```bash
ssh mini@<current-tailscale-ip>
ssh mini@<current-magicdns-name>
```

Observed working examples during validation:

```bash
ssh mini@100.92.43.40
ssh mini@minis-mac-mini.taild659a1.ts.net
```

### Thin trigger contract

The checked-in thin-trigger entrypoint is:

```bash
./scripts/pikaci-apple-remote.sh prepare --ref <git-ref>
./scripts/pikaci-apple-remote.sh run --ref <git-ref>
./scripts/pikaci-apple-remote.sh run --ref <git-ref> --just-recipe apple-host-sanity
```

The narrow GitHub live-validation entrypoint is the dedicated `apple-mini-validate` workflow. Use one of:

```bash
gh workflow run apple-mini-validate.yml --repo sledtools/pika --ref master -f lane=sanity
gh workflow run apple-mini-validate.yml --repo sledtools/pika --ref master -f lane=bundle
```

Those trigger only the Apple mini validation workflow, not the broader `pre-merge.yml` matrix. `lane=sanity` runs the tiny blocking smoke lane (`just apple-host-sanity`, currently `just desktop-ui-test`) on the mini, and `lane=bundle` runs the heavy nightly bundle (`just apple-host-bundle`).

The Apple CI config/secret model now follows the repo's existing `age` pattern:

- Non-secret Apple CI config is checked into [.github/pikaci-apple.env](/Users/justin/code/pika/worktrees/pikaci-mac/.github/pikaci-apple.env).
- Apple CI secrets are checked into [secrets/pikaci-apple.env.age](/Users/justin/code/pika/worktrees/pikaci-mac/secrets/pikaci-apple.env.age).
- GitHub workflows decrypt that file via the already-existing generic `AGE_SECRET_KEY` backend secret.
- The workflows no longer depend on Apple-specific GitHub vars or Apple-specific GitHub secrets.
- Apple secrets are loaded step-locally via `./scripts/with-pikaci-apple-ci-env`; they are no longer written through `GITHUB_ENV` or `GITHUB_OUTPUT`.

The checked-in public config currently carries:

```bash
PIKACI_APPLE_SSH_HOST=pika-mini.tail029da2.ts.net
PIKACI_APPLE_SSH_USER=mini
PIKACI_APPLE_REMOTE_ROOT=/Volumes/pikaci-data/pikaci-apple
PIKACI_APPLE_KEEP_RUNS=3
PIKACI_APPLE_LOCK_TIMEOUT_SEC=1800
```

Update the encrypted Apple secrets in-repo with:

```bash
PIKACI_APPLE_TAILSCALE_AUTHKEY='tskey-auth-...' \
PIKACI_APPLE_SSH_KEY_FILE="$HOME/.ssh/<apple-mini-private-key>" \
./scripts/encrypt-pikaci-apple-secrets
```

`secrets/pikaci-apple.env.age` must contain:

- `PIKACI_APPLE_TAILSCALE_AUTHKEY`
- `PIKACI_APPLE_SSH_KEY`

The GitHub-side dependency after this change is just the generic repo secret `AGE_SECRET_KEY`.

It uses a git-backed source contract on the mini: the caller sends an exact git bundle for the requested ref, the mini imports it into a bare mirror under `/Volumes/pikaci-data/pikaci-apple`, materializes a stable prepared worktree under `prepared/<commit>`, and reuses that checkout across repeated runs of the same commit. When the wrapper sees a schema-valid prepared checkout for the exact commit, it skips source upload/import entirely and goes straight to the prepared worktree. `prepare` now builds a stronger Apple-shaped staged state there: it prewarms the Apple dev shell, compiles the Rust/Desktop test binaries, and for the nightly bundle it runs `xcodebuild build-for-testing` via `./tools/ios-ui-test prepare`. `run` then reuses that prepared checkout, executes the requested checked-in `just` recipe (default `apple-host-bundle`), and lets `./tools/ios-ui-test run` switch to `xcodebuild test-without-building` when the prepared iOS outputs exist. The wrapper returns phase timing artifacts to the caller, takes a host-local run lock before touching the shared mirror/target state, honors the checked-in `PIKACI_APPLE_LOCK_TIMEOUT_SEC` when callers load `.github/pikaci-apple.env`, schema-checks the prepared marker before reuse, scrubs run-local `.pikaci` state and stale XCTest logs before each operation, prunes old remote run dirs automatically, and keeps only a bounded number of prepared commit dirs.

### Caveats

- The office LAN path was not usable during bootstrap: the mini was on `10.246.40.24` and this Mac was on `10.246.51.11`, with no working route between them.
- Current Tailscale reachability is working, but `tailscale ping` still reports `via DERP(dfw)` rather than a direct peer-to-peer path.
- Same-LAN bootstrap was acceptable for setup, but this is not the intended steady-state CI network posture.
