# Pika Cloud Implementation Plan

This is the working implementation plan for the shared cloud substrate used by:

- `pika-server` for managed customer runtimes
- `pikaci` for CI runtimes

This document is intentionally biased toward execution, not debate. The high-level direction is
already decided. The goal here is to keep the next code slices coherent and small.

Related context:

- [`todos/agents-platform.md`](./agents-platform.md)

## Core Direction

- `pika-cloud` is the shared runtime substrate.
- It starts as a library crate, not a service.
- Incus is the only target substrate for new work.
- Guest-defined readiness is the model.
- The first guest lifecycle contract is file-based.
- Managed agents use one persistent Incus volume per customer VM.
- CI runtimes are destroy-on-completion by default.
- Hard cuts beat compatibility shims.

## Hard Cuts

These are no longer open questions.

- Replace the old shared control-plane crate with `pika-cloud` in one branch. Do not keep a shim crate.
- Delete the legacy dedicated microvm crate.
- Delete the old product/backend split from `pikaci` while keeping the upstream local guest module contract.
- Treat the old spawner path as dead code and remove it as soon as the surviving consumers are gone.
- Fix imports, CI targets, Nix references, and docs aggressively instead of carrying both names.

Git history is the compatibility layer for removed code.

## What Problem `pika-cloud` Solves

Today we still have two different runtime contracts:

- the managed-agent path, which still carries spawner-shaped and guest-marker-era assumptions
- the `pikaci` path, which has its own Incus guest request/result contract

`pika-cloud` exists to replace that split with one shared Incus runtime boundary:

- one shared runtime spec
- one shared guest lifecycle/result contract
- one shared mount/retention/output policy model
- one shared set of Incus-facing helpers

The consumers stay separate. The runtime substrate becomes shared.

## Scope

In scope:

- shared runtime types
- shared guest lifecycle and terminal result schema
- shared mount, retention, and output collection types
- shared Incus runtime helpers
- migration of the first real `pikaci` Incus path onto the shared contract
- migration of the managed-agent internals toward the same runtime vocabulary
- deletion of dead legacy runtime code that no longer serves the target system

Out of scope for the first slices:

- a separate `pika-cloud-service`
- redesigning `/v1/agents/*`
- redesigning forge scheduling
- supporting multiple substrates
- preserving a separate legacy local-guest backend abstraction

## Design Rules

### 1. Incus-Only

Do not design new code around a generic hypervisor abstraction.

- no new `ProviderKind`-driven architecture
- no new backend-neutral VM interface for its own sake
- no further investment in legacy local-guest parity work

The shared boundary may become broader later, but the first implementation should be explicit and
honest: this is an Incus substrate.

### 2. Guest-Defined Readiness

The guest workload decides when it is `ready`.

The cloud layer is responsible for:

- collecting lifecycle files
- surfacing status
- enforcing timeouts
- collecting outputs
- tearing the runtime down

The cloud layer should not encode workload-specific meanings like "OpenClaw health is good" or
"this CI job is done enough."

### 3. File-Based Lifecycle Contract

The first shared guest contract is:

- event stream: `/run/pika-cloud/events.jsonl`
- status snapshot: `/run/pika-cloud/status.json`
- final result: `/run/pika-cloud/result.json`
- guest logs: `/run/pika-cloud/logs/`
- guest artifacts: `/run/pika-cloud/artifacts/`

This contract is deliberately simple, inspectable, and easy to debug during bring-up.

### 4. Small Fixed Lifecycle Vocabulary

Use a small fixed vocabulary first, with optional structured `details`.

Infrastructure states are host-observed:

- `requested`
- `provisioning`
- `booted`
- `unreachable`
- `stopped`
- `destroyed`

Workload states are guest-emitted:

- `starting`
- `ready`
- `failed`
- `completed`

Do not build a large typed event taxonomy before the shared path exists.

### 5. Disposable Root, Explicit Durability

Managed agents:

- disposable VM root
- one persistent state volume
- long-lived runtime
- guest-defined readiness

CI:

- disposable VM root
- readonly source snapshot mount
- explicit artifact collection
- no automatic restart by default
- destroy on completion

These are policy differences on top of one substrate model, not two different systems.

## Crate Boundary

`pika-cloud` should own:

- `RuntimeSpec`
- lifecycle event, status, and result types
- mount types
- retention policy types
- output collection policy types
- shared Incus config types
- Incus runtime operations and helpers used by multiple consumers

`pika-cloud` should not own:

- app routes
- dashboard copy and UI states
- forge scheduling
- CI lane selection
- application semantics like Nostr, MLS, or OpenClaw product behavior

## Temporary Practicality Rule

Some currently shared app-facing types are still consumed by multiple crates, especially
`pika-server`, `pikachat`, and `pika_core`.

For the hard cut, it is acceptable for `pika-cloud` to temporarily carry those surviving shared
types if that keeps the rename and cleanup smaller. Do not invent a second new crate unless there
is a clear payoff.

The priority is to collapse duplicate runtime contracts, not to achieve perfect taxonomy on day
one.

## First `pika-cloud` Shape

The first crate should stay small and obvious. A good initial layout is:

- `crates/pika-cloud/src/lib.rs`
- `crates/pika-cloud/src/spec.rs`
- `crates/pika-cloud/src/lifecycle.rs`
- `crates/pika-cloud/src/mount.rs`
- `crates/pika-cloud/src/policy.rs`
- `crates/pika-cloud/src/incus.rs`
- `crates/pika-cloud/src/paths.rs`

Expected contents:

- `spec.rs`
  - `RuntimeSpec`
  - runtime identity
  - image, project, profile, resources
  - bootstrap payload description
- `mount.rs`
  - persistent volume mounts
  - readonly snapshot mounts
  - artifact mounts
  - optional cache mounts
- `policy.rs`
  - restart policy
  - retention policy
  - output collection policy
- `lifecycle.rs`
  - event/status/result schemas
  - fixed event vocabulary
- `paths.rs`
  - canonical `/run/pika-cloud/...` paths
- `incus.rs`
  - shared Incus-facing helper types and operations

## First Shared Runtime Shape

The first `RuntimeSpec` should cover:

- runtime identity
- Incus project/profile/image alias
- instance name
- resource limits
- mount declarations
- bootstrap payload description
- lifecycle collection paths
- restart policy
- retention policy
- output collection policy
- debug metadata and labels

This should be one runtime model with policy fields, not separate "managed agent runtime" and "CI
runtime" root types.

## Consumer Responsibilities

`pika-server` remains responsible for:

- customer ownership
- agent product semantics
- billing and tenancy
- mapping app intent into `RuntimeSpec`
- mapping runtime lifecycle back into app-visible state

`pikaci` remains responsible for:

- job and lane scheduling
- deciding when to launch a runtime
- mapping jobs into `RuntimeSpec`
- mapping terminal result into CI pass/fail

## Current Code Smells To Remove

These are the main simplification targets:

- the old shared control-plane crate mixed substrate, app contract, spawner request types, and
  legacy control envelopes
- `pika-server` still uses spawner-shaped types like `SpawnerVmResponse`
- `pikaci` still has its own Incus guest contract under `/artifacts/*.json`
- the only remaining `microvm` namespace should be the upstream local guest module contract
- CI and docs still named the old shared control-plane crate as a canonical surface

## Implementation Phases

### Phase 1: Hard Cut to `pika-cloud`

- add `crates/pika-cloud`
- move the surviving shared contract types out of the old shared control-plane crate
- delete that crate
- update Cargo manifests, imports, tests, lane filters, Nix references, and docs

This phase is a rename by replacement, not a compatibility bridge.

### Phase 2: Delete Legacy Runtime Dead Weight

- delete the legacy dedicated microvm crate
- delete the old product/backend split from `pikaci` while keeping the upstream local guest module contract
- remove related target definitions, tests, and docs
- remove the old spawner path once nothing active depends on it

The point is not to preserve optionality. The point is to shrink the system.

### Phase 3: Shared CI Guest Contract

Migrate the current `pikaci` Incus path to the shared guest lifecycle contract:

- stop using the ad hoc `/artifacts/guest-request.json` plus `/artifacts/result.json` shape as the
  defining contract
- move the canonical lifecycle/result paths to `/run/pika-cloud/...`
- make the Incus guest image and `pikaci` Rust code agree on the new contract
- keep `pikaci` scheduler and lane logic where it is

The first migrated path is the existing remote Incus executor. No legacy product/backend fallback remains, while the upstream local guest module contract stays in place for local execution.

### Phase 4: Shared Managed-Agent Contract

Move `pika-server` internals toward the same substrate vocabulary:

- replace spawner-shaped internal response types with `pika-cloud` runtime/lifecycle types
- keep the app-facing routes unchanged
- keep the durable one-volume-per-customer model
- preserve OpenClaw-specific product logic in `pika-server`

### Phase 5: Cleanup

- remove duplicate runtime contract code from `pikaci` and `pika-server`
- delete dead Nix and CI references to removed crates/backends
- tighten docs around the surviving Incus-only model

## First Implementation Slice

The first slice should be intentionally small:

1. Create `crates/pika-cloud`.
2. Move the surviving shared contract and path types into it.
3. Add the shared lifecycle event, status, and result schemas.
4. Add the first `RuntimeSpec`, mount, and policy types.
5. Migrate the `pikaci` Incus guest contract to `/run/pika-cloud/...`.
6. Remove the old product/backend split from `pikaci` while keeping the upstream local guest module contract.

This slice should not try to finish the whole migration.

## Execution Notes

- Prefer deletion over adaptation when the old path is no longer strategic.
- Prefer changing call sites aggressively over introducing transitional compatibility helpers.
- When a choice exists between "cleaner later" and "smaller system now," prefer the smaller
  system unless it blocks the shared Incus runtime path.
- Use git history for reference instead of preserving dead code in-tree.

## Immediate Next Step

Start implementation with the crate hard cut and the first shared contract:

- create `crates/pika-cloud`
- move the surviving shared types there
- define canonical `/run/pika-cloud/...` paths
- add the first lifecycle/status/result schema
- switch the `pikaci` Incus guest contract to that layout
- remove the old product/backend split from `pikaci` while keeping the upstream local guest module contract
