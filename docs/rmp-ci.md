---
summary: CI model for combined Pika + RMP lanes and nightly platform checks
read_when:
  - changing CI workflows or just pre-merge/nightly recipes
  - debugging RMP init/run checks in CI
---

# RMP CI

RMP checks are integrated into the repo's single CI entrypoint, not a separate workflow.

## Pre-merge (single gate)

- Workflow: `.github/workflows/pre-merge.yml`
- Required status: `pre-merge`
- Internal lanes:
  - `check-pika`: existing app checks via `just pre-merge-pika`
  - `check-rmp`: RMP template/CLI checks via `just pre-merge-rmp`

`just pre-merge-rmp` is Linux-safe and validates:

- `rmp init` scaffolding (default, android-only, ios-only)
- generated project Rust core compilation (`cargo check -p pika_core`)

## Nightly

- Workflow: `.github/workflows/rmp-init-smoke.yml` (workflow name: `nightly`)
- Linux lane (`nightly-linux`): `just rmp-nightly-linux`
  - scaffolds project
  - ensures Android AVD
  - runs `rmp run android` in CI/headless mode
- macOS lane (`nightly-macos-ios`): `just rmp-nightly-macos`
  - scaffolds project
  - runs `rmp run ios` on a simulator

## Notes

- `rmp run android` now allows headless emulators in CI (`CI=1` or `RMP_ANDROID_ALLOW_HEADLESS=1`).
- The generated project intentionally keeps MVP internal names aligned with current `rmp run`/`rmp bindings` assumptions for fast iteration.
