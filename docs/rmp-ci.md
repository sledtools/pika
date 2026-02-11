# RMP CI Roadmap

This repo now has a baseline workflow: `.github/workflows/rmp-init-smoke.yml`.

It validates that a fresh `rmp init` project can be generated and its Rust core compiles.

## Current Coverage

- `just rmp-init-smoke`
  - builds `rmp-cli`
  - scaffolds a new project with `rmp init`
  - runs `rmp doctor`
  - runs `cargo check -p pika_core` in the generated project

## Next Coverage Steps

1. Add Android launch smoke in CI
- New Linux job that provisions an emulator and runs `just rmp-init-run PLATFORM=android`.
- Keep this as `workflow_dispatch` first, then promote to scheduled/PR as stability improves.

2. Add iOS simulator launch smoke in CI
- New macOS job that runs `just rmp-init-run PLATFORM=ios`.
- Pin Xcode version and simulator runtime to keep runs deterministic.

3. Promote to merge gates
- Once both launch jobs are stable, split into:
  - fast pre-merge checks (existing pre-merge + scaffold check)
  - slower launch smoke (required or nightly)

## Notes

- The generated project intentionally keeps MVP internal names aligned with current `rmp run`/`rmp bindings` assumptions to keep iteration fast.
- As `rmp` becomes fully generic, tighten the smoke checks to assert custom names and IDs across all generated files.
