# Ship OpenClaw Marmot

## Context

OpenClaw Marmot is an MLS (Messaging Layer Security) end-to-end encrypted group
messaging channel for OpenClaw, built on Nostr. It has two halves:

- **Rust sidecar** (`rust_harness/`) — the MLS/Nostr engine, runs as a
  long-lived JSONL daemon process
- **TypeScript plugin** (`openclaw/extensions/marmot/`) — an OpenClaw channel
  plugin that spawns and manages the Rust sidecar

The two communicate over a JSONL protocol on stdin/stdout (defined in
`rust_harness/src/daemon.rs` and `openclaw/extensions/marmot/src/sidecar.ts`).

The project already works end-to-end. Four phase scripts (`scripts/phase1.sh`
through `scripts/phase4_openclaw_marmot.sh`) prove it with strict token-matched
integration tests over a local Docker relay. Previous successful runs are
visible in `.state/`.

The goal is to get this distributable to other people.

## What needs to happen

### 1. CI: run phase 1–3 on every push

**This is the highest priority.** Without CI, dependency bumps (MDK, nostr-sdk)
can silently break things and the only way to catch it is remembering to run
scripts locally.

Write `.github/workflows/ci.yml` that:

- Triggers on push and pull_request
- Runs on ubuntu-latest
- Installs Rust 1.92.0 (use `rust-toolchain.toml`)
- Starts Docker (GitHub Actions runners have Docker pre-installed)
- Runs `cargo clippy -p rust_harness -- -D warnings`
- Runs `./scripts/phase1.sh`, `./scripts/phase2.sh`, `./scripts/phase3.sh`
  sequentially (each manages its own Docker relay lifecycle)
- Phase 4 is excluded from CI because it requires a full OpenClaw checkout with
  pnpm workspace resolution; it stays manual for now

The phase scripts already handle relay startup, random port assignment, and
cleanup. They exit nonzero on failure. No special CI scaffolding is needed
beyond having Docker and Rust available.

### 2. Fill in the plugin config schema

The `openclaw.plugin.json` currently has an empty `configSchema`:

```json
{
  "configSchema": {
    "type": "object",
    "additionalProperties": false,
    "properties": {}
  }
}
```

This means OpenClaw won't validate anything users put in `channels.marmot`.
Fill it in to match the actual config shape in
`openclaw/extensions/marmot/src/config.ts` (`MarmotChannelConfig`):

- `relays` (array of strings, required for configured status)
- `stateDir` (string, optional)
- `sidecarCmd` (string, optional)
- `sidecarArgs` (array of strings, optional)
- `autoAcceptWelcomes` (boolean, default true)
- `groupPolicy` (enum: "allowlist" | "open", default "allowlist")
- `groupAllowFrom` (array of strings, optional)
- `groups` (object, string keys, optional)

This prevents silent config mistakes and makes `openclaw doctor` useful for
Marmot users.

### 3. Rename the binary

The Rust binary is called `rust_harness`. This is a development name. Rename it
to `marmotd` (or `marmot-sidecar`):

- Rename the Cargo package in `rust_harness/Cargo.toml` (`name = "marmotd"`)
- Rename the directory `rust_harness/` → `marmotd/`
- Update `Cargo.toml` workspace members
- Update all scripts that reference `rust_harness` or `cargo run -p rust_harness`
- Update the TS plugin's default sidecar command resolution in `channel.ts`
  (currently falls back to `"rust_harness"`)

### 4. Fix the plugin dependency on `openclaw`

The plugin's `package.json` has:

```json
"dependencies": { "openclaw": "workspace:*" },
"devDependencies": { "openclaw": "workspace:*" }
```

This only works inside the OpenClaw monorepo. For standalone distribution:

- Move `openclaw` to `peerDependencies` with a version range (e.g.,
  `"openclaw": ">=2026.1.0"`)
- Remove it from `dependencies` and `devDependencies`
- Verify the plugin still loads when installed via `openclaw plugins install`

Check how other published OpenClaw extensions (e.g., `@openclaw/nostr`,
`@openclaw/matrix`, `@openclaw/msteams`) handle this — they all use
`"openclaw": "workspace:*"` in devDependencies but pnpm's `publishConfig` or
the npm pack flow may handle resolution. Follow whatever pattern the other
`@openclaw/*` packages use.

### 5. Cross-compile and publish Rust binary via GitHub Releases

Add a release workflow (`.github/workflows/release.yml`) triggered on git tags
(e.g., `v0.1.0`) that:

- Cross-compiles the Rust binary for:
  - `x86_64-unknown-linux-gnu`
  - `aarch64-unknown-linux-gnu`
  - `x86_64-apple-darwin`
  - `aarch64-apple-darwin`
- Attaches the binaries to a GitHub Release
- Names them predictably (e.g., `marmotd-x86_64-linux`,
  `marmotd-aarch64-darwin`)

Use `cross` or `cargo-zigbuild` for Linux cross-compilation. macOS targets
need a macOS runner or cross-compilation setup.

### 6. Add sidecar auto-download to the plugin (optional but recommended)

Follow the Signal pattern from OpenClaw (`src/commands/signal-install.ts`):

- On `startAccount`, if the sidecar binary isn't found at `channels.marmot.sidecarCmd`
  and isn't on `$PATH`, auto-download it from GitHub Releases
- Use the GitHub Releases API to pick the right platform asset
- Install to `~/.openclaw/tools/marmot/<version>/`
- Cache it so subsequent starts are instant

This is the difference between "install the plugin and it works" vs. "install
the plugin, then separately figure out how to get the Rust binary, then
configure the path." The former is what makes people actually use it.

### 7. Publish the npm package

Once the dependency issue (step 4) is resolved:

```bash
cd openclaw/extensions/marmot
npm publish --access public
```

Users install with:

```bash
openclaw plugins install @openclaw/marmot
```

### 8. Write a user-facing README

The current README is developer-oriented (phase scripts, Docker relay). Write
one for end users covering:

- What Marmot is (one paragraph)
- Install: `openclaw plugins install @openclaw/marmot` (and binary install if
  auto-download isn't implemented)
- Configure: minimal `channels.marmot` config with relays
- How to get invited to a group / how to invite others
- Troubleshooting: `openclaw doctor`, check logs for sidecar stderr

## What NOT to do

- **Don't write unit tests for the Rust code.** The phase scripts already
  exercise all the important paths (key generation, MLS group creation, welcome
  wrapping/unwrapping, message encrypt/decrypt, JSONL protocol). Unit tests
  would duplicate this coverage. If a specific function starts causing bugs,
  add a targeted test then.

- **Don't write vitest tests for the TS plugin.** The config resolution and
  sidecar JSONL handling are straightforward. Phase 3 and 4 test the real
  integration. OpenClaw itself excludes channel surfaces from coverage
  thresholds and relies on E2E validation.

- **Don't try to get phase 4 into CI yet.** It requires a full OpenClaw
  checkout with pnpm workspace resolution. It's a valid local/manual test.

- **Don't add Windows support initially.** Focus on Linux and macOS, which is
  where OpenClaw gateway deployments run.

## Order of operations

1. CI workflow (phases 1–3) — catches regressions, enables confident iteration
2. Config schema — prevents user config mistakes
3. Rename binary — do this before anyone depends on the name
4. Fix plugin deps + npm publish — makes it installable
5. GitHub Releases workflow — makes the binary accessible
6. Auto-download in plugin — makes it seamless
7. User README — makes it usable

Steps 1–3 can be done in a single session. Steps 4–5 are the actual shipping
gate. Steps 6–7 are polish.
