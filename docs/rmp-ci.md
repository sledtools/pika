---
summary: CI model for combined Pika + RMP lanes and nightly platform checks
read_when:
  - changing CI workflows or just pre-merge/nightly recipes
  - debugging RMP init/run checks in CI
---

# RMP CI

RMP checks are integrated into the repo's single CI entrypoint, not a separate workflow.

## Pre-merge

- Canonical orchestrator: the forge on `git.pikachat.org`
- Authoritative lane catalog and path filters: `ci/forge-lanes.toml`
- GitHub workflow: `.github/workflows/pre-merge.yml` as advisory shadow CI
- Internal lanes:
  - `check-pika`: existing app checks via `just pre-merge-pika`
  - `check-rmp`: RMP template/CLI checks via `just pre-merge-rmp`
- GitHub shadow approval gate:
  - if PR actor is `justinmoon`, pre-merge shadow lanes run immediately
  - otherwise shadow lanes target `ci-approval` and require approval

`just pre-merge-rmp` is Linux-safe and validates:

- `rmp init` scaffolding (default, android-only, ios-only)
- generated project Rust core compilation (`cargo check -p pika_core`)

## Nightly

- Canonical scheduler: the forge service on `git.pikachat.org`
- Authoritative lane catalog: `ci/forge-lanes.toml`
- GitHub workflow: `.github/workflows/pre-merge.yml` in `mode=nightly` as an advisory mirror
- Linux lane (`nightly-linux`): `just rmp-nightly-linux`
  - scaffolds project
  - ensures Android AVD
  - runs `rmp run android` in CI/headless mode
  - runs `rmp run iced` under `xvfb-run` with timeout (headless desktop smoke)
- macOS lane (`nightly-macos-ios`): `just rmp-nightly-macos`
  - scaffolds project
  - runs on WarpBuild macOS (`warp-macos-15-arm64-6x`)
  - restores Nix binaries via `DeterminateSystems/magic-nix-cache-action`
  - restores Cargo/target via `WarpBuilds/cache`
  - runs `rmp run ios` on a simulator
- Optional macOS interop lane (`nightly-primal-ios-interop`): `just nightly-primal-ios-interop`
  - disabled by default (set repo variable `PIKA_NIGHTLY_PRIMAL_INTEROP=1` to enable)
  - builds + installs Primal iOS from source at a pinned ref
  - verifies simulator routing for `nostrconnect://` via `simctl openurl`
  - runs a Pika UI smoke test that emits a `nostrconnect://` marker file from the iOS signer bridge

## Notes

- GitHub pre-merge and nightly runs now defer to `scripts/forge-github-ci-shim.py`, which reads `ci/forge-lanes.toml` instead of hand-copying lane filters into workflow YAML.
- `rmp run android` now allows headless emulators in CI (`CI=1` or `RMP_ANDROID_ALLOW_HEADLESS=1`).
- The generated project intentionally keeps MVP internal names aligned with current `rmp run`/`rmp bindings` assumptions for fast iteration.
