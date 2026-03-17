---
summary: Implementation prompt for the first bounded Incus migration chunk: provider-neutral VM abstractions with no behavior change yet.
read_when:
  - starting the first code change for the Incus migration
  - delegating the provider-abstraction cleanup chunk to another agent
---

# Incus Phase 1A Prompt: Provider Abstraction Cleanup

Use [incus-migration-plan.md](/Users/justin/code/pika/worktrees/incus/docs/incus-migration-plan.md)
as the architectural source of truth for this chunk.

## Goal

Introduce provider-neutral vocabulary and interfaces for the managed VM backend without changing
runtime behavior yet.

This is a preparatory refactor. The system should continue to use the existing microVM-backed path
after this change, but the rest of the application should be less hard-coded to `microvm` naming.

## Why This Chunk

This is the first implementation chunk because it is:

- small enough to review carefully
- a prerequisite for adding an Incus provider cleanly
- useful even before infra bootstrap work is complete
- low risk compared to starting with fleet or storage changes

The migration plan explicitly says we should avoid baking `microvm` into the domain model where the
real concept is "managed VM provider".

## Scope

In scope:

- introduce provider-neutral type names and module-facing interfaces where practical
- keep the current microVM implementation as the only runtime backend
- add compatibility aliases or transitional names if needed to avoid a giant rename
- update tests affected by the interface cleanup
- update the living plan if implementation reveals a better naming direction

Out of scope:

- Incus API integration
- changing provisioning behavior
- changing storage behavior
- changing guest startup semantics
- `pikaci` changes
- deleting the current microVM backend

## Primary Areas To Inspect

- [crates/pika-agent-control-plane/src/lib.rs](/Users/justin/code/pika/worktrees/incus/crates/pika-agent-control-plane/src/lib.rs)
- [crates/pika-agent-microvm/src/lib.rs](/Users/justin/code/pika/worktrees/incus/crates/pika-agent-microvm/src/lib.rs)
- [crates/pika-server/src/agent_api.rs](/Users/justin/code/pika/worktrees/incus/crates/pika-server/src/agent_api.rs)
- [crates/pika-server/src/customer/openclaw.rs](/Users/justin/code/pika/worktrees/incus/crates/pika-server/src/customer/openclaw.rs)
- [docs/agent-provider-contract-baseline.md](/Users/justin/code/pika/worktrees/incus/docs/agent-provider-contract-baseline.md)
- [docs/incus-migration-plan.md](/Users/justin/code/pika/worktrees/incus/docs/incus-migration-plan.md)

## Concrete Tasks

1. Audit the current public and semi-public `microvm`-named types that are really provider-neutral
   concepts.

2. Introduce a provider-neutral vocabulary for the managed VM backend. Examples might include:

- `VmProvider` instead of a spawner-specific concept where appropriate
- provider-neutral provision params or resolved params
- a neutral provider kind enum that can later represent `microvm` and `incus`

3. Keep compatibility where needed. It is acceptable to:

- keep old serialized names if changing them would be risky right now
- add aliases or wrapper types during the transition
- keep module paths stable if a rename would create too much churn for this chunk

4. Refactor call sites so core product code depends on the new neutral interfaces rather than
   directly on `MicrovmSpawnerClient` semantics wherever practical in this slice.

5. Add or update tests to prove:

- current microVM behavior still works
- serialization and wire contracts are unchanged unless explicitly intended
- the new naming layer does not break existing callers

6. If the refactor reveals a better migration naming scheme than the current plan suggests, update
   [docs/incus-migration-plan.md](/Users/justin/code/pika/worktrees/incus/docs/incus-migration-plan.md)
   with that learning.

## Constraints

- Do not add Incus-specific behavior yet.
- Do not create a thick new abstraction layer with speculative methods we do not need yet.
- Prefer a minimal interface that matches the current managed-agent lifecycle surface.
- Preserve current runtime behavior.
- Avoid changing external API payloads unless there is a very strong reason and the change is
  clearly documented.

## Acceptance Criteria

- the managed agent product still uses the current microVM backend successfully
- code outside the backend layer is less coupled to `microvm` naming
- the refactor is small enough to review in one pass
- tests pass for the touched area
- the living migration plan remains accurate after the refactor

## Suggested Output

The implementation should include:

- code changes
- tests
- a short change summary
- any migration-plan updates required by what was learned
- a short note calling out residual naming debt left for later chunks
