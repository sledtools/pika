---
name: orchestrate
description: Orchestrate multi-step cleanup, migration, or refactor work across pika worktrees using one integration branch, bounded subagents, explicit write ownership, forge-native landing, and project-specific safety rules for staged Linux remote runs.
---

# Orchestrate

Use this skill when running a multi-chunk implementation program in `pika` where you need to sequence work, delegate bounded subtasks, and keep checkpoints landable.

## Default Shape

- Keep one integration worktree/branch for the active program.
- Use one implementation worker on the critical path.
- Use one explorer/reviewer in parallel to audit assumptions, stale usage, and regressions.
- Add a second implementation worker only when the write sets are clearly disjoint.

## Worktree Rules

- Run `./scripts/agent-brief` once per new worktree session before substantial work.
- Do not keep doing large cleanup work on a shared scratch branch. Create a dedicated branch per chunk or per program.
- If two chunks may conflict, put them in separate worktrees immediately instead of hoping the merge will be easy.
- Keep a local untracked `JOURNAL.md` in the worktree for simplification opportunities, abstraction ideas, and cleanup candidates noticed in the weeds.
  - Capture the note quickly and keep moving.
  - Do not widen the active chunk just because the journal grew.
  - Triage the journal at the end of the broader project or migration once the primary seams are landed.

## Chunking Rules

- Make each chunk end in a truthful, landable state.
- Prefer deleting migration scaffolding over layering more compatibility code on top.
- Keep prompts aggressive, but scoped to one architectural seam at a time.
- After each chunk:
  - review the result
  - validate locally
  - do targeted live validation when the change affects hosted or remote CI paths
  - upstream or checkpoint before starting the next risky chunk

## Staged Linux Remote Safety

- Do not run multiple staged Linux remote lanes concurrently from the same worktree if they share the default remote prepared-output root.
- The default remote work dir (`/var/tmp/pikaci-prepared-output`) is collision-prone for parallel runs from one worktree.
- If parallel staged Linux remote validation is required, isolate it first:
  - separate worktrees
  - separate remote work dir roots
  - or separate snapshot/request ids with guaranteed non-overlap

## Delegation Pattern

For each chunk:

1. Local orchestrator:
   - identify the architectural seam
   - inspect the current code enough to write a concrete prompt
   - decide what should stay local vs delegated
2. Worker:
   - owns the implementation write set
   - runs required checks
   - reports exact live validation, not just local tests
3. Explorer/reviewer:
   - searches for stale usages, hidden consumers, rollout regressions, and misleading docs/comments
   - should avoid editing files unless explicitly asked

## Landing Pattern

- Prefer forge-native flow for real checkpoints.
- Land or checkpoint before opening the next risky branch.
- Before pushing:
  - ensure the branch is dedicated, not a shared scratch branch
  - inspect divergence from `origin/master` and the tracked remote branch
- After a chunk is locally green, push a dedicated branch immediately and use forge/`ph status`
  as the canonical checkpoint before starting the next risky chunk in a new worktree or branch.
- If a remote scratch branch has moved independently, do not fight it. Push a dedicated branch instead.
- Land early; otherwise collapse to one integration branch.

## Project-Specific Heuristics

- In CI/runtime work, prefer Nix-declared runtime contracts over imperative bash assembly in Rust.
- Treat migration-only env knobs and selector logic as debt. Hard-cut them once the default path is real.
- When a review finds “the real runtime works, but CI reconstructs it differently,” make aligning CI to the real runtime the next chunk.
- When a hosted/forge flow runs tooling from a temporary worktree, check whether the tool’s state root is implicitly anchored to `cwd` before you design API/log/artifact integrations around it.
  If the worktree is ephemeral, persist the tool state outside it first; otherwise every “structured” run contract will collapse back into flattened subprocess output.
- When a producer already emits structured events and has loadable run metadata, inspect the consumer
  seam before inventing a new protocol. In this repo, `pikaci` already had JSONL lifecycle events and
  log metadata; the real gap was that `pika-news` flattened and discarded the durable state.
- For temp-worktree CI in particular, prefer a single service-owned persistent state root over per-worktree `.state` trees.
  If a tool already emits stable run IDs, make `run_id` plus that shared state root enough to reload run records, logs, and artifacts after worktree cleanup.
- When parity is green, remove stale exclusions immediately and prove the normal default path.
- Validate remote-authoritative paths.
- When guest/runtime behavior moves into an Incus image, refresh/import the remote image before live
  validation. Otherwise image drift can masquerade as a runtime regression.
- When replacing `writeShellScript`/store-backed wrappers with guest-local scripts, inspect the realized
  store object on `pika-build`:
  - verify the shebang is at byte 0
  - verify the mode is still executable
  - do not assume a script generated inside a Nix build stayed runnable just because the source text looked right
- After deleting a host-store seam, validate a low-risk control lane first (`pre-merge-rmp` is a good
  default) before burning time on heavier desktop/OpenClaw lanes.
- For hosted `pika-news` CI, prefer one deterministic shared `pikaci` state root near the forge state
  directory over per-run scratch dirs. If `run_id` is the durable key, make the state root stable
  enough that web/API code can load run records and logs later without requiring a DB migration.

## Minimal Output Contract

At the end of each chunk, be able to state:

- what architectural seam changed
- what migration/scaffolding was deleted
- what exact validations passed
- what remains, if anything
- what the next chunk should attack
