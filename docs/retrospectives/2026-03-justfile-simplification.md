---
summary: Retrospective on the 2026 justfile simplification effort
read_when:
  - wondering why the just command surface is now curated
  - revisiting the justfile cleanup decisions later
  - considering another broad command-surface refactor
---

# Justfile Simplification Retrospective

## Outcome

The cleanup succeeded.

- `just` and `just --list` are now a curated human-facing surface instead of a toolbox dump.
- Heavy shell moved out of the root `justfile` and into `scripts/`.
- Major areas now have clearer ownership:
  - mobile build/binding logic,
  - release/version logic,
  - release-secret handling,
  - agent/demo env resolution,
  - infra helper env/bootstrap logic.
- Agents still have an expanded discovery path through `./scripts/agent-brief`.

## What Changed

- Extracted Android and iOS build/binding logic out of the root `justfile`.
- Split the command surface into modules and hid low-signal helpers with `[private]`.
- Consolidated release/version logic behind one owner.
- Consolidated release-secret helper behavior behind one owner.
- Simplified the agent/demo wrapper stack and made env resolution clearer.
- Simplified the macOS release packaging script.
- Moved the remaining shell-heavy infra recipes out of `just/infra.just`.
- Wrote down the long-term command-surface rules in [`docs/just-command-contract.md`](../just-command-contract.md).

## What Worked

- Bounded prompts were the right shape. Small slices kept regressions reviewable.
- Hiding or modularizing commands was usually better than deleting them.
- Moving shell out of `just` improved readability more than renaming commands would have.
- A separate contract doc is a better long-term guardrail than keeping a completed plan around.

## What We Deliberately Did Not Do

- We did not replace `just` with a custom CLI.
- We did not mass-delete compatibility entrypoints.
- We did not rewrite every shell helper into Rust.
- We did not keep iterating once the remaining ideas became mostly cosmetic.

## Maintenance Rule

For future changes:

- root `just` recipes should stay rare and high-signal,
- implementation should default to `scripts/` or a real CLI,
- low-signal/manual/debug helpers should usually start module-local and `[private]`,
- and agents should keep using `./scripts/agent-brief` as the expanded discovery path.

If the command surface drifts again, open a new bounded cleanup slice instead of restarting a broad refactor.
