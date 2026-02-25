# Final Runtime Abstractions Spec (Single Canonical Version)

## 1. Scope

This spec defines the post-refactor runtime abstraction model for provider orchestration in `pika-server` and how it interfaces with the Nostr marketplace control plane.

This is the merged final decision from negotiation.

## 2. Core Boundary (Wire vs Internal)

### 2.1 Wire-facing (shared crate)
The shared crate (`crates/pika-provider-types/`) contains only Nostr-facing types:
- `OrderRequest` (and optional `OrderPreferences`)
- `ProtocolKind` (`pi|acp`)
- status/result/error payload types used by CLI and server

It does **not** expose provider internals.

### 2.2 Server-internal (`pika-server`)
`pika-server` owns runtime/provider internals:
- `ProvisionCommand`
- `ProviderKind`
- `RuntimeSubstrate`
- `WorkloadConfig`
- `ResourceRequest`
- `ProvisionedRuntime`
- `ProviderAdapter` trait and implementations

`ProvisionCommand` is server-internal only. Server maps `OrderRequest + server config/policy` into `ProvisionCommand`.

## 3. Internal Model Axes

Use three explicit internal axes:
1. `ProviderKind`: `Fly | Microvm | Workers`
2. `RuntimeSubstrate`: `OciProcess | NixGuestAutostart | RemoteEndpoint`
3. `ProtocolKind`: `Pi | Acp`

`RuntimeSubstrate` remains explicit in internal records/commands for observability and policy checks.

Consistency rule:
- `RuntimeSubstrate` must match `WorkloadConfig` discriminant at construction time.

## 4. WorkloadConfig and Compatibility

Internal workload enum:
- `Docker { image_ref, entrypoint_override, env }`
- `Nix { flake_ref, dev_shell, autostart }`
- `WorkersBroker { runtime_endpoint }`

Promote existing microVM autostart schema as canonical:
- `GuestAutostartSpec { command, env, files }`

Compatibility matrix (server-enforced, deterministic):
- `Fly` ↔ `OciProcess`
- `Microvm` ↔ `NixGuestAutostart`
- `Workers` ↔ `RemoteEndpoint`
- all other pairs rejected with stable compatibility errors

## 5. ProviderAdapter Contract

Each provider implements:
- `provision(cmd)`
- `readiness(handle/runtime)`
- `process_welcome(handle/runtime, welcome)`
- `teardown(handle/runtime)`

Workers remains a first-class provider adapter, but with broker semantics only.

Workers rule:
- never assume local subprocess runtime in workers
- runtime endpoint is external

## 6. Artifact Lifecycle Rule

Docker/OCI artifacts are resolved **before** provider provisioning:
1. optional build/push/lookup
2. resolve immutable image ref (`image@sha256:...`)
3. pass resolved image ref into `provision()`

`provision()` must not embed build orchestration logic.

## 7. Runtime State and Error Modeling

Use a runtime phase enum with payload-less failure state:
- `Queued | Provisioning | WaitingForKeyPackage | Ready | Failed | TearingDown | Terminated`

Store structured error details separately (`last_error`) for stable mapping to wire-level error payloads (`error_code`, `hint`, `retryable`).

## 8. Teardown Policy Naming

Canonical internal naming:
- `DeleteOnExpiry`
- `KeepAlive { max_ttl_seconds }`

This aligns with lease expiry semantics rather than client connection lifecycle.

## 9. Shared Crate Scope (Final)

`crates/pika-provider-types/` includes only:
- wire request types
- wire status/result/error types
- protocol enums needed on both CLI/server sides

No provider/workload/provision internals in shared crate.

## 10. Workers Backing Runtime (MVP)

MVP behavior:
- workers adapter uses pre-configured backing runtime endpoint
- no automatic provisioning chain of backing runtime in MVP

Post-MVP:
- optional higher-level orchestration may auto-provision backing runtime before workers setup
- this orchestration sits above provider adapter layer

## 11. Deterministic Mapping Requirement

Server must have explicit tested mapping:
- input: `OrderRequest + server policy/config`
- output: validated `ProvisionCommand`

This mapping is required to prevent implicit behavior drift.

## 12. Migration Plan

1. Freeze wire-facing schemas in shared crate.
2. Implement server-internal runtime/provider module with compatibility checks.
3. Implement provider adapters (`fly`, `microvm`, `workers`) and state transitions.
4. Switch CLI to remote-first order submission + status subscription.
5. Remove direct provider orchestration from CLI.

## 13. Required Acceptance Checks

1. Wire schema does not require `ProviderKind` or `WorkloadConfig` fields.
2. Compatibility matrix tests are deterministic and CI-blocking.
3. `OrderRequest -> ProvisionCommand` mapping has deterministic tests.
4. Workers `process_welcome` override path is tested separately from default relay-publish path.
5. One end-to-end server test verifies:
   - order ingestion
   - internal command mapping
   - runtime provision/readiness
   - status/result emission
