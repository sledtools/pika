# Justfile Simplification Plan

## Intent

Simplify the root `justfile` without breaking the repo's real workflows.

This plan is intentionally a living document. It is not a rigid sequence of exact steps. After each implementation slice:

1. we review the diff,
2. we update this document,
3. we decide whether the next prompt still makes sense as written,
4. we either continue, revise, or stop.

## Current Snapshot

- Root `justfile`: 1074 lines, 100 recipes.
- Rough inventory:
  - human-facing entrypoints,
  - CI entrypoints,
  - manual/debug helpers,
  - heavy shell implementation mixed directly into recipes.
- Complexity signals:
  - about 30 recipes have no live references outside the root `justfile`,
  - 22 env vars are referenced directly in the root `justfile`,
  - 61 env vars are referenced across `justfile` plus `scripts/`.

## Problem Statement

The root `justfile` currently serves too many roles at once:

- public developer UX,
- CI contract surface,
- manual/debug toolbox,
- and low-level shell implementation.

That makes it noisy for humans, awkward for agents, and expensive to maintain because implementation details are mixed with task routing.

## Outcomes We Want

- Humans should see a small, curated root command surface.
- Agents should still be able to discover and use the full task tree.
- The root `justfile` should mostly route to scripts, tools, or CLIs.
- Shell-heavy implementation should move out of the root `justfile`.
- Duplicate logic should have one owner.
- Existing recipe names should mostly keep working during the transition.
- CI entrypoints should remain stable unless there is a deliberate migration.

## Non-Goals

- Rewriting every helper into Rust immediately.
- Renaming the whole command surface in one shot.
- Deleting large numbers of recipes up front.
- Breaking docs, CI, or muscle-memory workflows just to reduce line count.
- Creating a separate agent-only `justfile`.

## Working Principles

- Prefer hide/private over delete unless something is clearly dead.
- Keep each implementation slice landable on its own.
- Preserve behavior first, then reduce duplication, then reduce surface area.
- Keep the root `justfile` readable even if deeper scripts stay shell for now.
- Use one source of truth for command discovery; do not fork human and agent task definitions.
- Update this document after every slice with what changed, what remains, and whether the next prompt should change.

## Target Shape

### Root surface

Keep a small public root layer, likely in the neighborhood of:

- `info`
- `run-ios`
- `run-android`
- `run-desktop`
- `cli`
- `qa`
- `pre-merge`
- `nightly`
- `release`
- maybe `rmp`
- maybe `pikahut-up`

### Hidden or modular surface

Move everything else into modules and/or private recipes:

- `build`
- `test`
- `agent`
- `rmp`
- `infra`
- `release`
- `_internal`

### Discovery model

- Humans:
  - `just`
  - `just info`
  - `just --list <module>`
- Agents:
  - `scripts/agent-brief`
  - `just --list --list-submodules`
  - `just --groups`

## Workstreams

## Workstream A: Extract Heavy Mobile Build And Binding Logic

Status: active first workstream

Why this is first:

- It is the biggest line-count and readability win.
- It removes the most shell from the root `justfile`.
- It sets the pattern for the rest of the cleanup.

Scope:

- `gen-kotlin`
- `android-rust`
- `android-local-properties`
- `android-release`
- `ios-gen-swift`
- `ios-rust`
- `ios-xcframework`
- `ios-xcodeproj`
- related duplication in `ios-build-sim` / `ios-build-swift-sim`

Requirements:

- Root recipes become thin wrappers.
- Shared logic is factored instead of copied.
- Existing env knobs remain stable for now unless a simplification is clearly behavior-preserving.
- Existing recipe names keep working.
- No namespace or visibility changes are required in the first slice.

Preferred end state:

- mobile implementation owned by scripts or a small dev CLI,
- root `justfile` only orchestrates.

Slice strategy:

- A1: Android extraction first, including shared binding/build helpers where obvious.
- A2: iOS extraction second.
- A3: collapse duplicate simulator build flows.

## Workstream B: Namespace And Visibility Cleanup

Status: planned

Scope:

- introduce `mod` files,
- mark low-signal helpers `[private]`,
- change default output from raw full-list to curated help,
- keep public root surface intentionally small.

Requirements:

- do not break CI entrypoint names in the first pass,
- do not require a second agent-only task file,
- keep module discovery easy.

Expected deletions:

- only clearly dead aliases or wrappers,
- everything else should first be hidden or moved.

Known likely delete candidate:

- `openclaw-pikachat-scenarios`

## Workstream C: Release / Version / Secret Consolidation

Status: planned

Scope:

- release version helpers,
- release secret readers/writers,
- shared age/YubiKey handling,
- tagging/bumping orchestration.

Requirements:

- preserve release behavior,
- reduce duplicate shell,
- reduce the number of places where release rules are encoded.

Possible end state:

- one release helper CLI or one well-factored script family with subcommands.

## Workstream D: Agent / Demo Wrapper Cleanup

Status: planned

Scope:

- `scripts/agent-demo.sh`
- `scripts/demo-agent-microvm.sh`
- `scripts/pikachat-cli.sh`
- `scripts/lib/pika-env.sh`
- related `just` recipes

Requirements:

- one clear owner for env loading,
- one clear owner for agent demo behavior,
- remove ambiguous fallback env behavior,
- reduce wrapper-on-wrapper stacking.

## Workstream E: Miscellaneous Helper Relocation

Status: planned

Scope:

- relay helpers,
- `news`,
- manual QA helpers,
- one-off debug commands,
- RMP scaffold QA helpers that are not meant to be public root commands.

Requirements:

- preserve useful tooling,
- reduce root command noise,
- prefer moving to modules or feature-local homes over immediate deletion.

## Immediate Decisions

- Start with Workstream A.
- The first slice will be A1, not the full mobile rewrite.
- A1 is deliberately Android-first to keep the first implementation prompt bounded while still attacking the highest-leverage area.
- We will not do namespace changes in the same slice as the first extraction.
- We will not do mass deletion before the module/private pass.

## Acceptance Criteria For The Overall Cleanup

- The root `justfile` is materially shorter and easier to scan.
- Most public root recipes are one-liners or short orchestrators.
- Shell-heavy implementation moved to scripts or a dedicated tool.
- The visible root command surface is much smaller than 100 recipes.
- CI still runs through stable entrypoints.
- Agents still have an expanded discovery path through `agent-brief` or submodule listing.

## Prompt Backlog

### Prompt 1

Workstream A1: extract Android mobile build/binding logic out of the root `justfile` while preserving behavior and recipe names.

Status: complete

Prompt text:

```text
You are working on Workstream A1 from `todos/justfile-simplification-plan.md`.

Goal:
Extract the Android mobile build/binding implementation out of the root `justfile` while preserving behavior and existing recipe names.

Scope:
- `gen-kotlin`
- `android-rust`
- `android-local-properties`
- `android-release`

Primary objectives:
- Make the root `justfile` meaningfully shorter and easier to scan in the Android section.
- Turn the recipes above into thin wrappers that delegate to script(s).
- Remove duplicated implementation logic, especially:
  - `cargo ndk --help` flag detection,
  - JNI lib output setup,
  - cargo target-directory / built-library discovery for bindgen.

Constraints:
- Preserve current recipe names and the current env surface for these commands unless a change is clearly behavior-preserving.
- Do not do module / `[private]` / namespace work in this slice.
- Do not change CI entrypoint names.
- Avoid unrelated cleanup outside the Android build/binding area.
- If you create shared helpers that iOS can reuse later, that is good, but do not refactor iOS behavior in this slice.

Implementation guidance:
- Prefer moving logic into `scripts/` and optionally `scripts/lib/` so the root `justfile` becomes routing/orchestration.
- It is fine to introduce one cohesive script with subcommands or a small family of scripts if that keeps ownership clear.
- Keep the resulting implementation readable; do not just move one giant opaque shell blob into a different file without factoring obvious duplication.

Verification:
- Run `just --fmt`.
- Run `bash -n` on any new or changed shell scripts.
- Run the lightest relevant verification you can for the extracted commands without assuming a full Android release environment.
- If heavy verification is not practical, say exactly what you did verify and what remains unverified.

Documentation update:
- Update `todos/justfile-simplification-plan.md` after the change:
  - mark Prompt 1 complete,
  - add a short review note under the Review Log,
  - adjust Prompt 2 if this slice changes the best next step.

Deliverable:
- code changes,
- verification results,
- a short note on remaining risks or follow-up work.
```

### Prompt 2

Workstream A2: extract iOS mobile build/binding logic out of the root `justfile`, reuse `scripts/lib/mobile-build.sh` for bindgen/library discovery where it fits, and collapse duplicated simulator build behavior where safe.

Status: planned

### Prompt 3

Workstream B1: introduce `just` modules and private recipes, keep the existing public root entrypoints, and reduce `just --list` noise.

Status: planned

### Prompt 4

Workstream C1: consolidate release/version helper logic into one coherent implementation surface.

Status: planned

### Prompt 5

Workstream D1: simplify the agent/demo wrapper stack and standardize env ownership.

Status: planned

## Review Log

### 2026-03-09

- Created this living plan.
- Chose Workstream A as the first target because it offers the largest readability win in the root `justfile`.
- Chose Android-first extraction as the first implementation slice to keep the first agent prompt bounded and landable.
- Completed Prompt 1 by moving `gen-kotlin`, `android-rust`, `android-local-properties`, and `android-release` implementation into `scripts/android-build`, with shared host-library discovery in `scripts/lib/mobile-build.sh`.
- Review note: the root Android build/binding section is now wrapper-only, and the new helper boundary should let Prompt 2 reuse the bindgen/library-discovery path instead of copying it again.
- Review follow-up: `gen-kotlin` and `android-release` now force a host `cargo build` freshness check inside the extracted script path, so UniFFI generation no longer reuses stale host artifacts just because the dylibs already exist.
