---
summary: Command-surface contract for curated root just recipes, module/private helpers, and agent discovery
read_when:
  - adding or reorganizing just recipes
  - deciding whether a command belongs at root, in a module, or as a private helper
  - updating scripts/agent-brief or just help/discovery text
---

# Just Command Contract

This repo intentionally keeps a small human-facing `just` surface and a larger module tree for lower-signal tasks.

## Discovery Model

- Humans start with `just`, `just --list`, and `just info`.
- Agents use `./scripts/agent-brief` as the supported expanded discovery path.
- `JUST_UNSTABLE=1 just --list --list-submodules` is the raw expanded tree when the full module view is needed.

## Authoring Rules

1. Root recipes should be rare and high-signal.
   Root is for common developer entrypoints, stable CI-facing commands, and a small set of well-known workflows.
2. Implementation should not live in root `just` recipes by default.
   Put real behavior in `scripts/` or a dedicated CLI, then make the `just` recipe a thin wrapper.
3. New low-signal helpers should start in modules, not root.
   Manual QA helpers, debug plumbing, migration tools, and one-off operational helpers should live in a module and usually be marked `[private]`.
4. Preserve compatibility before reorganizing names.
   If a command is documented, used by CI, or likely to be muscle memory, keep the existing entrypoint working via an alias or thin wrapper unless there is a deliberate migration plan.
5. Keep human and agent discovery aligned.
   Do not create a separate agent-only task file. Update `scripts/agent-brief` when the expanded discovery path or its important context changes.

## Adding A New `just` Command

Use this default decision order:

1. Does this belong in an existing CLI or script instead of `just`?
2. If it needs a `just` entrypoint, can it live in an existing module?
3. Should it be `[private]` because it is manual-only, low-signal, or implementation detail?
4. Is there a strong reason it must be root-visible for humans?

If the answer to step 4 is not clearly yes, do not add it to the visible root surface.

## Review Checklist

Before landing a new or changed recipe, check:

- `just --list` still reads like a curated human surface.
- low-signal helpers are module-local or `[private]`.
- the recipe body is mostly routing/orchestration, not a large inline shell program.
- `scripts/agent-brief` still points agents at the right expanded discovery path.
