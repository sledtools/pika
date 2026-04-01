# Pika Cloud (Incus-Only) Living Plan

This is the focused living plan for the shared cloud substrate that should be consumed by both:

- the managed agent product path in `pika-server`
- the CI runtime path in `pikaci`

This document narrows the broader Incus migration work into an implementation-oriented plan for a
single shared boundary we can actually build against.

It should be read alongside:

- [`docs/incus-migration-plan.md`](../docs/incus-migration-plan.md)
- [`todos/agents-platform.md`](./agents-platform.md)

Those documents describe the broader product and fleet direction. This one is specifically about
the shared runtime substrate and how to simplify the current split between app and CI consumers.

## Current Decision Snapshot

- Incus is the only target substrate we should actively support for new work.
- `microvm.nix` should be treated as legacy or transitional, not as a co-equal backend.
- We want one shared runtime API, not separate app and CI substrate APIs.
- The guest workload running inside the VM should define what `ready` means.
- The cloud layer should provide lifecycle transport, status collection, timeouts, and teardown.
- The rename to `pika-cloud` should be a hard cut, not a compatibility shim.
- The first guest lifecycle contract should be file-based.
- Managed agents should use one persistent Incus volume per customer VM.
- CI runtimes should be destroy-on-completion by default.
- The first implementation should be library-first, not service-first.
- `pika-server` should remain the app product control plane.
- `pikaci` should remain the CI scheduler and execution product.
- The shared substrate should move toward a `pika-cloud` namespace.

## Problem We Are Solving

Today we have two overlapping but separate VM contracts:

- the managed-agent contract centered around `pika-agent-control-plane`, `pika-server`, and the
  current agent startup plan / ready-marker model
- the CI contract centered around `pikaci`, backend-specific guest request payloads, snapshot
  mounts, staged output mounts, and backend-specific launch logic

This makes the system harder to simplify because:

- Incus migration for app and CI still feels like two different projects
- guest lifecycle semantics are duplicated
- runtime layout assumptions are scattered across `pika-server`, `pikaci`, Nix, and guest code
- it is difficult to imagine extracting a shared cloud project because the shared boundary is still
  fuzzy

## Scope Of This Plan

This plan is in scope for:

- defining the shared Incus runtime contract
- deciding what belongs in `pika-cloud` versus `pika-server` and `pikaci`
- standardizing guest lifecycle reporting
- standardizing runtime mounts, outputs, retention, and teardown semantics
- identifying the first implementation slices

This plan is not in scope for:

- redesigning the app-facing `/v1/agents/*` API
- redesigning forge CI scheduling or lane selection
- solving final multi-host placement policy in detail
- finalizing all dashboard/product semantics for managed agents
- preserving `microvm.nix` as a long-term backend

## Proposed Direction

We should converge on a new shared substrate namespace called `pika-cloud`.

The first step does not need to be a new service. It should start as a crate consumed directly by
both `pika-server` and `pikaci`.

Default recommendation:

- hard rename or replace `pika-agent-control-plane` with `pika-cloud`
- keep it library-first initially
- make it Incus-first rather than pretending to be backend-neutral
- move shared runtime types and guest lifecycle contracts into it
- gradually move CI-specific remote runtime contracts out of `pikaci` and into `pika-cloud`

## One Shared Runtime API

We should have one shared runtime API with policy fields, not two separate substrate abstractions
like "managed agent VM" and "CI VM".

The API should answer:

- what image to boot
- what volumes or directories to attach
- what bootstrap payload to inject
- what lifecycle events the guest may emit
- what counts as terminal completion
- what outputs to collect
- what restart and retention policy to apply

The same API should support:

- long-lived customer runtimes with durable state
- short-lived CI runtimes with snapshot mounts and destroy-on-completion behavior

Those are different policies applied to the same substrate, not fundamentally different APIs.

## Guest-Defined Readiness

The cloud layer should not try to define the semantic meaning of `ready`.

Instead:

- the guest workload defines readiness
- the guest emits lifecycle events
- the cloud layer records and exposes them
- the cloud layer enforces watchdog timeouts if the guest never becomes healthy or terminal

This is important because:

- managed agents genuinely have a product-defined "ready" state
- many CI workloads do not
- some CI workloads may want progress or intermediate lifecycle states

The cloud layer should own the mechanism, not the meaning.

## Recommended Guest Lifecycle Contract

The first version should use a simple file-based guest contract because it is easy to inspect and
debug.

Recommended initial convention:

- guest lifecycle event stream:
  - `/run/pika-cloud/events.jsonl`
- guest current status snapshot:
  - `/run/pika-cloud/status.json`
- guest final result:
  - `/run/pika-cloud/result.json`
- guest logs directory:
  - `/run/pika-cloud/logs/`
- collected artifacts root:
  - `/run/pika-cloud/artifacts/`

Recommended event model:

- infrastructure states remain host-observed:
  - requested
  - provisioning
  - booted
  - unreachable
  - stopped
  - destroyed
- workload states are guest-emitted:
  - starting
  - ready
  - failed
  - completed

The first implementation should prefer simple JSONL and result-file semantics over a more complex
socket or RPC transport.

## Recommended Runtime Shape

The shared runtime contract should be centered around a single `RuntimeSpec`.

The exact Rust type names may evolve, but the shape should cover:

- runtime identity
- Incus project, profile, and image alias
- resource limits
- mounts
- bootstrap payload
- guest lifecycle collection paths
- restart policy
- retention policy
- output collection policy
- metadata for product-specific labeling and debugging

The shared mount model should be able to represent:

- persistent volume mounts for managed agent state
- read-only snapshot mounts for CI source
- artifact output mounts
- cache mounts when we explicitly want them

## Incus-Only Simplifications

By going Incus-only we should deliberately simplify several things:

- stop designing around a generic hypervisor abstraction
- stop designing around backend-specific guest request formats
- stop making `pikaci` own separate runtime substrate semantics from the app path
- stop treating host-specific runtime layout as a long-term contract

In practice this means:

- `ProviderKind` should stop being a central design axis for new work
- `RemoteLinuxVmBackend::{Microvm, Incus}` should stop shaping the future shared interface
- `vm-spawner` should be treated as a legacy adapter rather than the future shared API surface

## What `pika-cloud` Should Own

`pika-cloud` should own:

- shared runtime spec types
- guest lifecycle event schema
- guest final-result schema
- mount and retention policy types
- Incus orchestration helpers
- high-level runtime operations like ensure, inspect, collect outputs, destroy

`pika-cloud` should not own:

- app-specific `/v1/agents/*` routes
- app-specific agent UI states
- branch/CI scheduling policy in the forge
- CI lane selection, path filters, or staged target catalog
- Nostr/MLS application semantics

## Consumer Responsibilities

`pika-server` should remain responsible for:

- user and customer ownership
- agent product semantics
- billing window / tenancy semantics
- translating app intent into a `RuntimeSpec`
- interpreting guest lifecycle into app-facing agent status

`pikaci` should remain responsible for:

- lane and job scheduling
- deciding when a CI runtime is needed
- translating a job into a `RuntimeSpec`
- interpreting terminal results into CI success or failure

## Default Policy Conventions

These defaults should be treated as recommended unless implementation teaches us otherwise.

- Managed agent runtimes:
  - Incus VM with disposable root
  - one persistent state volume
  - long retention
  - guest-defined readiness
  - restart or recreate policy owned by product code

- CI runtimes:
  - Incus VM with disposable root
  - read-only source snapshot mount
  - explicit artifact collection
  - no automatic restart by default
  - destroy on completion unless operator debugging says otherwise

## First Implementation Slice

The first bounded slice should not be "build the whole cloud system".

It should be:

1. Create the focused `pika-cloud` substrate crate.
2. Move or rename the shared contract currently living in `pika-agent-control-plane`.
3. Add the shared guest lifecycle event and final-result schema.
4. Add one Incus-first `RuntimeSpec`.
5. Teach one narrow `pikaci` Incus path to use the shared guest lifecycle/result contract instead
   of its own backend-specific guest request/result conventions.

This first slice should not yet:

- build a separate `pika-cloud-service`
- redesign all managed-agent behavior
- migrate every `pikaci` path
- delete all microvm code

## Suggested Phases

### Phase 0: Lock Direction

- commit to Incus-only for new substrate work
- agree that the shared boundary is library-first
- agree that guest-defined readiness is the model

### Phase 1: Create `pika-cloud`

- rename or replace `pika-agent-control-plane`
- split generic substrate types from agent-specific product semantics
- add shared lifecycle/result schema

### Phase 2: CI Contract Migration

- move shared Incus runtime request concepts out of `pikaci`
- standardize guest event and result handling
- keep lane selection and target definitions inside `pikaci`

### Phase 3: Managed Agent Contract Migration

- make `pika-server` target the same shared runtime substrate
- keep app-facing routes unchanged while internal runtime calls move to `pika-cloud`

### Phase 4: Legacy Cleanup

- remove now-redundant duplicate runtime contract code from `pikaci`
- narrow `vm-spawner` to explicit legacy or delete it when safe
- update docs and Nix composition to reflect the new boundary

## Questions We Still Need To Answer

These are the remaining questions that still matter for implementation sequencing. Several early
directional questions are now considered decided and should not be reopened casually.

## Locked Decisions

### 1. Hard Rename To `pika-cloud`

- do a hard rename
- do not keep compatibility shims or re-export crates
- fix imports and call sites aggressively

Rationale:

- the whole point of this workstream is simplification
- carrying both names would prolong confusion about which boundary is canonical

### 2. File-Based Guest Lifecycle Contract

- v1 guest lifecycle transport should be file-based
- the first contract should use `events.jsonl`, `status.json`, and `result.json`

Rationale:

- easy to inspect manually
- easy to debug during Incus bring-up
- enough structure for both managed agents and CI

### 3. Managed Agent Durable State Model

- v1 should assume one persistent Incus volume per customer VM

Rationale:

- simplest durable-state model
- aligns with the existing product trust boundary
- keeps the first runtime contract smaller

### 4. CI Retention Policy

- CI runtimes should be destroy-on-completion by default
- retention should be an explicit debugging or operator override later, not the default contract

Rationale:

- keeps CI semantics simple
- avoids accidental runtime accumulation and unclear cleanup behavior

### 5. Library-First First Slice

- the first implementation should be library-first
- a separate `pika-cloud-service` is explicitly out of scope for the first slices

Question:

- we still need to decide when, if ever, a separate `pika-cloud-service` becomes justified

### Remaining Open Questions

### 1. Exact Crate Topology

Question:

- should the first hard cut be a direct rename of the existing crate path, or a new `pika-cloud`
  crate that absorbs the old code in the same branch while the old crate is deleted?

### 2. Guest Lifecycle Richness

Question:

- for v1, do we want only a small fixed lifecycle vocabulary (`starting`, `ready`, `failed`,
  `completed`) or do we want extensible typed event payloads from day one?

### 3. Incus Ownership Model

Question:

- will both `pika-server` and `pikaci` call Incus directly using the shared library, or do we
  already know that one of them should be the sole Incus-calling process?

## Working Assumptions Unless Overridden

Unless we explicitly decide otherwise, implementation work should assume:

- Incus-only for new substrate work
- one shared runtime API
- guest-defined readiness
- library-first `pika-cloud`
- managed agent durable state via persistent volume
- CI destroy-on-completion behavior
- hard rename to `pika-cloud` with no shim period

## Immediate Next Step

After this plan is accepted, the next implementation-oriented step should be a smaller design note
or prompt that defines:

- the first `pika-cloud` crate/module tree
- the first `RuntimeSpec` and lifecycle/result Rust types
- exactly which `pikaci` Incus path will be migrated first
