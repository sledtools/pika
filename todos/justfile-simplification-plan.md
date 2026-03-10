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

Status: complete

Prompt text:

```text
You are working on Workstream D1 from `todos/justfile-simplification-plan.md`.

Goal:
Simplify the agent/demo helper stack by making env loading and default resolution have one clear owner, while preserving the existing user-facing just commands and script entrypoints.

Scope:
- `scripts/agent-demo.sh`
- `scripts/demo-agent-microvm.sh`
- `scripts/pikachat-cli.sh`
- `scripts/lib/pika-env.sh`
- `just/agent.just`
- any thin wiring in `justfile` / module aliases that is needed to preserve the current command surface

Primary objectives:
- Eliminate duplicated env-loading and env-default logic across the agent/demo helpers.
- Remove wrapper-on-wrapper layering where it is not buying anything.
- Make it obvious which layer owns:
  - `.env` loading,
  - `PIKA_AGENT_API_BASE_URL` defaulting,
  - `PIKA_AGENT_API_NSEC` / `PIKA_TEST_NSEC` fallback behavior,
  - `PIKA_AGENT_MICROVM_BACKEND` propagation.
- Preserve the familiar commands:
  - `just agent-microvm`
  - `just agent-microvm-acp`
  - `just agent-microvm-chat`
  - `just agent-demo`
  - `just agent-demo-acp`
  - `just cli`

Constraints:
- Do not rewrite the Rust CLI itself in this slice unless a very small CLI change is clearly required to remove ambiguity.
- Preserve explicit env overrides that people may already be using:
  - `PIKA_AGENT_API_BASE_URL`
  - `PIKA_SERVER_URL`
  - `PIKA_AGENT_API_NSEC`
  - `PIKA_TEST_NSEC`
- Be conservative about removing older fallback names unless you confirm they are truly unused or you keep compatibility.
- Do not re-open unrelated justfile/module cleanup outside the agent/demo surface.
- Avoid changing release, mobile build, or CI behavior in this slice.

Known problems to target:
- `just/agent.just` manually sources `.env` for `agent-microvm`, while the scripts also have their own env logic.
- `scripts/agent-demo.sh` and `scripts/demo-agent-microvm.sh` resolve base URL and auth env differently.
- `scripts/agent-demo.sh` defaults to `https://api.pikachat.org`, while the CLI and shared env helper default to local `http://127.0.0.1:8080`; if that difference is intentional, make it explicit instead of implicit.
- `scripts/demo-agent-microvm.sh` bypasses the shared env helper entirely.
- `scripts/pikachat-cli.sh` is a thin wrapper but also participates in env loading, so the ownership boundary is muddy.

Implementation guidance:
- Pick one clear owner for agent/demo env resolution. `scripts/lib/pika-env.sh` is the most likely place, but if you see a better boundary, use it and make the ownership obvious.
- Prefer making `just` recipes thin wrappers over script entrypoints, and script entrypoints thin wrappers over one shared env-resolution layer.
- Remove the duplicate `.env` sourcing from `just/agent.just` if the scripts can own it safely.
- Flatten wrapper stacks where possible, but do not sacrifice readability just to remove one process hop.
- If a default behavior is intentionally different for “local dev” versus “remote demo”, encode that as an explicit mode or explicit script behavior rather than hidden drift between helpers.
- Keep the resulting behavior easy for both humans and agents to understand from the code.

Verification:
- Run `JUST_UNSTABLE=1 just --fmt`.
- Run `bash -n` on any new or changed shell scripts.
- Smoke-check the preserved command surface with non-destructive invocations where possible, for example:
  - `just --dry-run agent-microvm`
  - `just --dry-run agent-microvm-acp`
  - `just --dry-run agent-demo`
  - `just --dry-run agent-demo-acp`
  - `just --dry-run agent-microvm-chat`
  - `just cli --help`
- Validate the env-resolution behavior you changed with a lightweight harness or targeted shell checks, especially for:
  - `.env` loading,
  - local-vs-explicit base URL selection,
  - nsec fallback behavior,
  - microVM backend propagation.
- If you cannot safely run a real networked agent demo, say exactly what you verified and what remains unverified.

Documentation update:
- Update `todos/justfile-simplification-plan.md` after the change:
  - mark Prompt 5 complete,
  - add a short review note under the Review Log,
  - adjust Prompt 6 if this slice changes the best next step.

Deliverable:
- code changes,
- verification results,
- a short note on compatibility tradeoffs and any remaining ambiguity you intentionally left in place.
```

### Prompt 6

Workstream E1: relocate the remaining low-signal helper commands into coherent modules or feature-local homes.

Status: complete

Prompt text:

```text
You are working on Workstream E1 from `todos/justfile-simplification-plan.md`.

Goal:
Reduce the remaining “toolbox dump” feeling in the non-root `just` surface by regrouping low-signal helper commands into clearer homes, hiding what should not be discoverable by default, and preserving the few commands that are actually useful entrypoints.

Scope:
- `just/infra.just`
- `just/labs.just`
- `just/rmp_tools.just`
- any related thin aliases or root/module wiring in `justfile`
- any small helper script moves that make ownership clearer, if needed

Primary objectives:
- Make the expanded `just --list --list-submodules` surface easier to scan.
- Separate real entrypoints from niche/manual/debug plumbing.
- Keep useful commands available without making every helper equally prominent.
- Preserve CI and documented commands.

Constraints:
- Do not break existing root entrypoints or documented commands that are still actively used.
- Do not change mobile build, release/version, or agent/demo behavior in this slice.
- Prefer organization and visibility cleanup over behavior changes.
- Be conservative with deletions. Hide, regroup, or move first unless something is clearly dead.
- Do not create another parallel command system; stay within the current `just` module structure unless a small feature-local file is genuinely clearer.

Recommended targets:
- Relay / local backend helpers:
  - `run-relay`
  - `run-relay-dev`
  - `relay-build`
  - `pikahut`
  - `pikahut-up`
  - `news`
- Manual lab and debug helpers:
  - `device`
  - `integration-manual`
  - `interop-rust-manual`
  - `android-manual-qa`
  - `ios-manual-qa`
  - Primal lab helpers
- RMP scaffold helpers:
  - `rmp-init-smoke`
  - `rmp-init-run`
  - `rmp-phase4-qa`
  - `rmp-init-smoke-ci`
  - `rmp-nightly-linux`
  - `rmp-nightly-macos`

What “better” looks like:
- A small number of clearly named modules or sub-areas instead of one grab-bag per topic.
- Manual-only or niche debug commands marked `[private]` or moved behind narrower module names where that improves discovery.
- High-value entrypoints kept visible, for example:
  - likely keep `pikahut-up`
  - likely keep `rmp`
  - keep CI-facing RMP/nightly commands if they are referenced by workflows/docs
- Commands that are basically prompts or notes should be reconsidered:
  - maybe move them closer to the prompts/docs they point at,
  - or keep them but hide them from general listing.

Implementation guidance:
- Start from actual references in docs and workflows; preserve those first.
- Prefer grouping by user intent, not by implementation detail. For example:
  - local infra/runtime
  - manual labs
  - RMP scaffold CI/nightly
  - RMP local experimentation
- If a module currently mixes “real command someone runs” with “rare debug plumbing”, split it.
- Use `[private]` where it improves list hygiene without harming legitimate usage.
- If a recipe is just printing the location of a prompt file, consider whether it should still be a visible command.
- Keep command behavior unchanged unless a tiny cleanup is clearly behavior-preserving.

Verification:
- Run `JUST_UNSTABLE=1 just --fmt`.
- Inspect the new discovery surface with:
  - `just`
  - `just --list`
  - `JUST_UNSTABLE=1 just --list --list-submodules`
- Smoke-check preserved commands with `just --dry-run` where appropriate, including a representative sample from:
  - relay/local infra helpers,
  - manual lab helpers,
  - RMP helpers,
  - any aliases you preserved for compatibility.
- Verify every workflow- or docs-referenced command you touch still exists at the same call site.

Documentation update:
- Update `todos/justfile-simplification-plan.md` after the change:
  - mark Prompt 6 complete,
  - add a short review note under the Review Log,
  - add Prompt 7 only if a clearly bounded next slice remains.

Deliverable:
- code changes,
- verification results,
- a short note on what stayed visible, what was hidden/moved, and what was intentionally left alone.
```

### Prompt 7

Workstream C2: consolidate release-secret helpers and shared age/YubiKey handling.

Status: complete

Prompt text:

```text
You are working on Workstream C2 from `todos/justfile-simplification-plan.md`.

Goal:
Reduce duplication and ambiguity in the release-secret helper scripts by making one implementation surface the clear owner for age/YubiKey decryption, recipient handling, and encrypted release-secret reads/writes.

Scope:
- `scripts/read-keystore-password`
- `scripts/read-zapstore-sign-with`
- `scripts/decrypt-keystore`
- `scripts/encrypt-zapstore-signing`
- `scripts/init-release-secrets`
- `scripts/release-age-recipients`
- any thin just/module wiring only if needed to preserve existing command entrypoints

Primary objectives:
- Remove duplicated age decryption logic and duplicated YubiKey identity-file fallback logic.
- Remove duplicated recipient-loading and encrypted env parsing logic.
- Keep release-secret behavior auditable by making one helper or one small helper family the clear owner.
- Preserve the current script entrypoints and the commands that already call them.

Constraints:
- Do not change the release policy or the encrypted file formats unless required by correctness.
- Do not break existing callers such as:
  - `scripts/android-build`
  - `scripts/zapstore-publish`
  - `scripts/post-release-announcement`
  - `just ship::zapstore-encrypt-signing`
- Preserve current override env vars unless there is a very strong compatibility-safe reason to remove one.
- Keep the diff focused on release-secret handling; do not reopen general release/version flow cleanup.
- Avoid unnecessary crypto or key-management redesign. This slice is about simplification and deduplication, not changing the trust model.

Known duplication to target:
- `AGE_SECRET_KEY` temp-file handling is duplicated.
- YubiKey identity-file default/fallback logic is duplicated.
- encrypted env parsing (`PIKA_KEYSTORE_PASSWORD=...`, `ZAPSTORE_SIGN_WITH=...`) is duplicated.
- recipient loading from `scripts/release-age-recipients` is duplicated.
- command/usage validation boilerplate is duplicated across these scripts.

Implementation guidance:
- Prefer introducing one shared helper, likely under `scripts/lib/`, or one owner script with subcommands plus thin compatibility wrappers.
- Keep external script names working even if they become wrappers.
- Be careful with temp-file safety and cleanup; avoid fixed temp paths.
- If you can simplify `init-release-secrets` by reusing the same shared encrypt/decrypt helpers without making the flow harder to audit, do that.
- Preserve human-readable help/usage for the existing script entrypoints.

Verification:
- Run `bash -n` on any new or changed shell scripts.
- Run `JUST_UNSTABLE=1 just --fmt` if any just wiring changes.
- Smoke-check preserved script entrypoints with `--help` where supported.
- Validate the shared decrypt/read logic with lightweight, non-production fixtures if possible.
- At minimum, verify that:
  - `./scripts/read-keystore-password`
  - `./scripts/read-zapstore-sign-with`
  - `./scripts/decrypt-keystore`
  - `./scripts/encrypt-zapstore-signing --help`
  - `./scripts/init-release-secrets --help`
  still behave consistently at the interface level.
- If you cannot safely run real age/YubiKey/GitHub-secret operations, say exactly what you verified and what remains unverified.

Documentation update:
- Update `todos/justfile-simplification-plan.md` after the change:
  - mark Prompt 7 complete,
  - add a short review note under the Review Log,
  - add Prompt 8 only if another bounded cleanup slice clearly remains.

Deliverable:
- code changes,
- verification results,
- a short note on compatibility tradeoffs and any remaining release-secret risks.
```

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
- Completed Prompt 5 by moving agent/demo env resolution into `scripts/lib/pika-env.sh`, making the local-vs-remote API base URL default explicit by mode, and reducing the `just` agent recipes to script-only wrappers.
- Review note: `.env` loading now happens in the scripts instead of `just`, `PIKA_AGENT_API_BASE_URL` / `PIKA_SERVER_URL` precedence is shared in one helper, `PIKA_AGENT_API_NSEC` now canonicalizes from `PIKA_TEST_NSEC` and legacy `AGENT_API_NSEC`, and `scripts/demo-agent-microvm.sh` now calls `scripts/pikachat-cli.sh` directly instead of routing back through `just`.
- Review follow-up: `scripts/pikachat-cli.sh` now `cd`s to the repo root before `cargo run` so the direct script entrypoints remain location-independent after the wrapper stack was flattened.
- Next step is Prompt 6: move the remaining low-signal relay/news/manual-QA/debug helpers into module or feature-local homes now that the mobile, release, and agent/demo surfaces each have a clearer owner.
- Completed Prompt 6 by marking low-signal infra, labs, and RMP helper recipes `[private]` while keeping the existing root aliases and CI/doc entrypoints intact.
- Review note: `just --list --list-submodules` now keeps `infra`, `labs`, and `rmp_tools` focused on the few real entrypoints, while manual QA prompt pointers, rare debug helpers, remote fulfill plumbing, and local RMP scaffold QA commands remain callable through their established root names.
- Review tradeoff: `pikahut-up`, `rmp`, relay entrypoints, `primal-ios-lab`, and the docs/workflow-facing RMP nightly commands stayed visible; the rest of the touched commands were hidden rather than renamed or deleted.
- Post-rebase Prompt 6 cleanup: re-hid the rebased `pikaci-workspace-deps-*` and `linux-builder-*` operational helpers in `infra`, kept their compatibility aliases intact, and removed them from `just info` so the curated human-facing surface stayed consistent.
- Reviewed the post-rebase Prompt 6 cleanup and found no remaining regressions in the curated command surface; hidden infra helpers still resolve through their preserved root aliases.
- Next step is Prompt 7: consolidate the release-secret helper scripts now that the release/version flow, agent/demo env logic, and command-surface cleanup have all landed.
- Completed Prompt 7 by moving age decryption, `AGE_SECRET_KEY` temp-file handling, YubiKey identity-file selection, encrypted env parsing, and release recipient loading into `scripts/lib/release-secrets.sh`, with the existing public scripts reduced to thin wrappers.
- Review note: `read-keystore-password`, `read-zapstore-sign-with`, `decrypt-keystore`, `encrypt-zapstore-signing`, and the decrypt/test portions of `init-release-secrets` now all share the same helper surface, and `init-release-secrets` no longer uses a fixed temp path for the generated CI private key.
- Review tradeoff: the external script entrypoints and env overrides stayed intact; no Prompt 8 was added because the remaining cleanup work is not yet shaped into a comparably bounded slice.
