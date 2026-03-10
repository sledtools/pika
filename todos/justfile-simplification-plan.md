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

Status: complete

Prompt text:

```text
You are working on Workstream A2 from `todos/justfile-simplification-plan.md`.

Goal:
Extract the iOS mobile build/binding implementation out of the root `justfile` while preserving behavior and existing recipe names, and reduce obvious duplication in the simulator build flow.

Scope:
- `ios-gen-swift`
- `ios-rust`
- `ios-xcframework`
- `ios-xcodeproj`
- `ios-build-sim`
- `ios-build-swift-sim`

Primary objectives:
- Make the root `justfile` meaningfully shorter and easier to scan in the iOS section.
- Turn the recipes above into thin wrappers that delegate to script(s).
- Reuse `scripts/lib/mobile-build.sh` for host-library / cargo-target discovery where it actually helps.
- Remove duplicated implementation logic, especially:
  - repeated host-library lookup for UniFFI generation,
  - repeated Swift output normalization,
  - repeated simulator build command construction.

Constraints:
- Preserve current recipe names and the current env surface unless a change is clearly behavior-preserving.
- Do not do module / `[private]` / namespace work in this slice.
- Do not change CI entrypoint names.
- Avoid unrelated cleanup outside the iOS build/binding area.
- Do not rewrite the whole iOS pipeline into a different architecture in one pass.
- Preserve the ability to build only the simulator/Xcode side without forcing unnecessary extra work.

Implementation guidance:
- Prefer moving logic into `scripts/` and optionally `scripts/lib/` so the root `justfile` becomes routing/orchestration.
- It is fine to introduce one cohesive `scripts/ios-build` entrypoint with subcommands if that keeps ownership clear.
- Keep the new script layout readable; factor helpers instead of moving one opaque block.
- Reuse the new Android-side helper patterns where that improves consistency, but do not contort iOS behavior just to match Android mechanically.
- For `ios-build-sim` vs `ios-build-swift-sim`, prefer one underlying implementation with an explicit flag or mode for “skip Rust rebuild” instead of two fully separate copies.

Verification:
- Run `JUST_UNSTABLE=1 just --fmt`.
- Run `bash -n` on any new or changed shell scripts.
- Run the lightest relevant verification you can for the extracted iOS commands without assuming a full signing/device environment.
- At minimum, validate help/dispatch and any pure-shell helper behavior you changed.
- If heavy verification is not practical, say exactly what you did verify and what remains unverified.

Documentation update:
- Update `todos/justfile-simplification-plan.md` after the change:
  - mark Prompt 2 complete,
  - add a short review note under the Review Log,
  - adjust Prompt 3 if this slice changes the best next step.

Deliverable:
- code changes,
- verification results,
- a short note on remaining risks or follow-up work.
```

### Prompt 3

Workstream B1: introduce `just` modules and private recipes, keep the existing public root entrypoints, and reduce `just --list` noise.

Status: complete

Prompt text:

```text
You are working on Workstream B1 from `todos/justfile-simplification-plan.md`.

Goal:
Reduce the visible root `just` surface for humans by introducing `just` modules and `[private]` recipes, while preserving the existing public entrypoints and keeping CI/documented commands working.

Scope:
- root `justfile`
- new `*.just` module files as needed
- any minimal command-discovery updates needed for humans vs agents
- `scripts/agent-brief` only if needed to keep agent discovery strong after module/private changes

Primary objectives:
- Make `just --list` materially less overwhelming for humans.
- Keep the familiar public root entrypoints working.
- Move low-signal, implementation-detail, manual-only, and internal helper recipes out of the root visible list.
- Use `mod` and `[private]` instead of deleting commands aggressively.

Constraints:
- Do not break CI entrypoint names.
- Do not break documented public commands that should remain root-facing.
- Prefer preserving command behavior and implementation as-is; this slice is about organization and visibility, not another major behavior refactor.
- Do not create a second agent-only justfile.
- Avoid broad renaming unless clearly justified and compatibility is preserved.

Recommended approach:
- Keep a small curated root layer for high-signal commands such as:
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
- Move other recipes into a few coherent modules, for example:
  - `build`
  - `test`
  - `agent`
  - `rmp`
  - `infra`
  - `release`
  - `_internal`
- Mark implementation-only helpers `[private]` where that improves list hygiene without losing needed functionality.
- Change the default/root listing behavior so `just` shows curated help rather than dumping the full raw recipe list.
- If needed, update `scripts/agent-brief` so agents still get a rich task snapshot, for example via submodule listing or grouped output.

Recipe selection guidance:
- Clear candidate for deletion if still present only as compatibility cruft:
  - `openclaw-pikachat-scenarios`
- Good candidates to hide or move out of the root visible list:
  - manual QA helpers
  - relay/debug helpers
  - agent demo helpers
  - RMP scaffold QA helpers
  - one-off release/debug plumbing
- Be conservative with anything referenced by CI, README, or active docs. Compatibility matters more than purity.

Verification:
- Run `JUST_UNSTABLE=1 just --fmt`.
- Validate the new root experience with:
  - `just`
  - `just --list`
  - `just --list --list-submodules`
- Smoke-check a representative sample of preserved public commands with `just --dry-run` where appropriate.
- If you touch `scripts/agent-brief`, run it or otherwise verify it still provides useful discovery output.

Documentation update:
- Update `todos/justfile-simplification-plan.md` after the change:
  - mark Prompt 3 complete,
  - add a short review note under the Review Log,
  - adjust Prompt 4 if this slice changes the best next step.

Deliverable:
- code changes,
- verification results,
- a short note on compatibility tradeoffs and any commands intentionally hidden or deleted.
```

### Prompt 4

Workstream C1: consolidate release/version helper logic into one coherent implementation surface.

Status: complete

Prompt text:

```text
You are working on Workstream C1 from `todos/justfile-simplification-plan.md`.

Goal:
Consolidate the release/version helper logic into one coherent implementation surface while preserving current release behavior and existing command entrypoints.

Scope:
- release/version helper scripts under `scripts/`
- any thin just/module wiring needed to keep existing commands working
- release/version command paths such as:
  - `release-bump`
  - `release-commit`
  - `release`
  - `scripts/set-release-version`
  - `scripts/sync-pikachat-version`
  - `scripts/version-read`
  - `scripts/bump-pikachat.sh`

Primary objectives:
- Reduce the number of places where release/version rules are encoded.
- Remove duplicated branch / dirty-tree / version parsing / manifest update logic.
- Keep release workflows auditable by making one implementation surface the clear owner.
- Preserve existing public commands and script entrypoints unless a change is clearly compatibility-safe.

Constraints:
- Do not change the actual release policy unless required by correctness.
- Do not break GitHub Actions workflows, documented release commands, or existing root just entrypoints.
- Prefer consolidating behavior behind one helper or one script family with subcommands, rather than spreading small changes across many scripts again.
- Avoid mixing in the release-secret encryption/decryption consolidation unless it is required for the version-flow cleanup. That can stay for a later slice unless there is a very obvious shared helper worth introducing now.
- Keep the diff focused on release/version behavior; do not reopen justfile/module organization work unless needed for thin wiring.

Recommended approach:
- Pick one clear owner for version parsing and manifest synchronization.
- Make the root/module release recipes thin wrappers over that owner.
- If `scripts/bump-pikachat.sh` can share the same underlying implementation cleanly, do that.
- If a new script with subcommands is the clearest shape, that is acceptable.
- Preserve the existing external UX where practical:
  - `just release-bump <version>`
  - `just release-commit <version>`
  - `just release <version>`
  - `./scripts/bump-pikachat.sh [version]`

Known duplication to target:
- repeated semantic-version validation
- repeated repo cleanliness checks
- repeated branch checks
- repeated version-file / manifest sync logic
- repeated tag existence checks

Verification:
- Run `JUST_UNSTABLE=1 just --fmt`.
- Run `bash -n` on any new or changed shell scripts.
- Smoke-check the preserved command surface with non-destructive invocations where possible, for example help, dry-run-safe cases, or failure-mode validation in a temp repo/worktree.
- Verify that any updated script entrypoints still match what workflows and docs call.
- If you cannot safely run the real release path, say exactly what you verified and what remains unverified.

Documentation update:
- Update `todos/justfile-simplification-plan.md` after the change:
  - mark Prompt 4 complete,
  - add a short review note under the Review Log,
  - adjust Prompt 5 if this slice changes the best next step.

Deliverable:
- code changes,
- verification results,
- a short note on compatibility tradeoffs and any remaining release-risk gaps.
```

### Prompt 5

Workstream D1: simplify the agent/demo wrapper stack and standardize env ownership.

Status: ready

## Review Log

### 2026-03-09

- Created this living plan.
- Chose Workstream A as the first target because it offers the largest readability win in the root `justfile`.
- Chose Android-first extraction as the first implementation slice to keep the first agent prompt bounded and landable.
- Completed Prompt 1 by moving `gen-kotlin`, `android-rust`, `android-local-properties`, and `android-release` implementation into `scripts/android-build`, with shared host-library discovery in `scripts/lib/mobile-build.sh`.
- Review note: the root Android build/binding section is now wrapper-only, and the new helper boundary should let Prompt 2 reuse the bindgen/library-discovery path instead of copying it again.

### 2026-03-10

- Reviewed the corrected Prompt 1 implementation after the initial host-build regression was fixed.
- Review result: no remaining findings in the A1 extraction itself.
- Next step remains Prompt 2: extract the iOS build/binding path and collapse the duplicated simulator build flow onto one underlying implementation.
- Review follow-up: `gen-kotlin` and `android-release` now force a host `cargo build` freshness check inside the extracted script path, so UniFFI generation no longer reuses stale host artifacts just because the dylibs already exist.
- Completed Prompt 2 by moving `ios-gen-swift`, `ios-rust`, `ios-xcframework`, `ios-xcodeproj`, `ios-build-sim`, and `ios-build-swift-sim` implementation into `scripts/ios-build`.
- Review note: the iOS section of the root `justfile` is now wrapper-only for the extracted recipes, `scripts/lib/mobile-build.sh` now owns the shared cargo target lookup used by xcframework assembly, and the simulator build flow now shares one underlying xcodebuild helper with an explicit swift-only mode.
- Reviewed Prompt 2 and found no structural regressions; remaining risk is limited to real Xcode/provisioning verification depth.
- Next step is Prompt 3: namespace and visibility cleanup now that the heavy mobile shell has been moved out of the root `justfile`.
- Completed Prompt 3 by moving low-signal recipes into `just/` modules, shrinking the visible root list to a curated public surface, and preserving documented/CI entrypoints through hidden root aliases.
- Review note: `just` now shows curated root help, `just --list --list-submodules` exposes the expanded module tree for agents, and `scripts/agent-brief` now includes both views so discovery stays strong after the module/private split.
- Review tradeoff: the compatibility alias `openclaw-pikachat-scenarios` was removed because it had no live references outside the root `justfile`; the active root entrypoints and CI/documented commands remain intact.
- Reviewed Prompt 3 and found no structural regressions; remaining risk is mostly limited to obscure external uses of hidden/removed commands outside the repo.
- Next step is Prompt 4: consolidate the release/version helper logic now that the root command surface has been thinned and grouped.
- Completed Prompt 4 by introducing `scripts/release-version` as the single owner for release/version parsing, manifest sync, branch/clean-tree checks, and tag creation, with the existing helper scripts reduced to thin compatibility wrappers.
- Review note: `release-bump`, `release-commit`, `release`, `scripts/set-release-version`, `scripts/sync-pikachat-version`, `scripts/version-read`, and `scripts/bump-pikachat.sh` now all route through the same implementation surface, and the ship module recipes are wrapper-only.
- Review follow-up: while exercising the new release paths, verification surfaced that `just` module recipes were executing from their module directory; all `just/*.just` files now set `working-directory := '..'` so routed commands run from the repo root as intended.
- Next step is Prompt 5: simplify the agent/demo wrapper stack now that the release/version logic has a single owner and the module execution root is fixed.
