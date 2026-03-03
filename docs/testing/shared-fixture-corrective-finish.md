---
summary: Canonical shared-fixture corrective finish reference
read_when:
  - evaluating strict-vs-shared rollout boundaries
  - deciding whether a lane/profile can run shared fixture mode
  - reviewing promotion evidence and rollback rules for shared fixture defaults
---

# Shared Fixture Corrective Finish Reference

This document is the canonical source for strict-vs-shared capability status, rollout boundaries, and promotion evidence rules for the shared-fixture corrective finish effort.

## Capability Matrix (Step 1)

Status legend:
- `SharedSupported`: shared mode is the documented default for this target.
- `StrictOnly`: shared mode is not allowed for this target in the current cycle.
- `Experimental`: shared mode exists only as a bounded validation path and must not be treated as a default.

| Target | Profile / Selector Scope | Status | Notes |
| --- | --- | --- | --- |
| Local deterministic CLI selectors | `integration_deterministic::{cli_smoke_local,cli_smoke_media_local}` | `StrictOnly` | Canonical deterministic lanes remain strict while corrective gates are incomplete. |
| Deterministic boundary/interop selectors | `integration_deterministic::{post_rebase_invalid_event_rejection_boundary,post_rebase_logout_session_convergence_boundary,interop_rust_baseline}` | `StrictOnly` | Boundary and interop deterministic contracts remain strict-only in this cycle. |
| Local deterministic OpenClaw selectors | `integration_deterministic::openclaw_scenario_*` | `StrictOnly` | Shared default is rolled back pending explicit parity/isolation/reliability evidence. |
| Local deterministic UI selectors | `integration_deterministic::{ui_e2e_local_android,ui_e2e_local_ios,ui_e2e_local_desktop}` | `StrictOnly` | Heavy deterministic fixtures remain strict by default. |
| OpenClaw gateway E2E selector | `integration_openclaw::openclaw_gateway_e2e` | `StrictOnly` | No shared-mode promotion in this corrective cycle. |
| Primal interop selector | `integration_primal::primal_nostrconnect_smoke` | `StrictOnly` | Nightly interop remains strict pending dedicated shared evidence. |
| Manual runbook selectors | `integration_manual::{manual_interop_rust_runbook_contract,manual_primal_lab_runbook_contract}` | `StrictOnly` | Manual selectors remain strict-only contracts. |
| Shared fixture infra validation (candidate) | Relay + MoQ + Postgres shared infra validation in deterministic harness | `Experimental` | Allowed only as explicit validation runs with recorded evidence artifacts. |

Current default policy: no lane/profile is `SharedSupported` yet in this finish cycle.

## Immediate Remove/De-Scope List (Step 2)

The following corrective actions are intentionally scoped as immediate removals/de-scopes while shared evidence is incomplete.

| Action | Scope | File-Level References | Rationale |
| --- | --- | --- | --- |
| De-scope shared defaults from deterministic lane contracts | Keep deterministic lane invocations strict-only | `justfile`, `docs/testing/ci-selectors.md`, `docs/testing/integration-matrix.md` | Shared mode must not be implied as default before parity/isolation/reliability evidence exists. |
| Remove ambiguity that manual/public flows are shared candidates | Treat manual and public-network selectors as strict-only | `docs/testing/ci-selectors.md`, `docs/testing/integration-matrix.md` | Nondeterministic/manual flows are not valid promotion evidence for shared defaults. |
| De-scope legacy negotiated shared-fixture spec from active implementation source of truth | Treat prior spec as historical context only | `todos/pikahut-shared-fixture-pool-negotiated.md` | Active corrective implementation source is `todos/shared-fixture-corrective-finish.md`. |
| Remove overlap between older corrective todo files and active finish todo | Keep old files as superseded pointers only | `todos/pikahut-shared-fixture-corrective.md`, `todos/shared-fixture-closeout-corrective.md` | Prevents dual-scope implementation drift and conflicting completion claims. |
| De-scope unsupported shared-promotion assumptions from current cycle | No lane/profile may claim `SharedSupported` status yet | `docs/testing/shared-fixture-corrective-finish.md` | Promotion remains explicitly evidence-gated in later steps. |

## Tenant Namespace Helper Enforcement (Step 3)

Canonical helper surface:
- `crates/pikahut/src/testing/tenant.rs` exposes `TenantNamespace::{relay_namespace,moq_namespace}`.
- Shared-capable scenario consumers currently wired:
- `crates/pikahut/src/testing/scenarios/openclaw.rs`
- `crates/pikahut/src/testing/scenarios/interop.rs`

Enforcement boundary for this cycle:
- Shared-capable internals must route relay/MoQ tenant naming through `TenantNamespace` helpers.
- No additional shared-capable internals are promoted by default in this cycle; strict-only remains default outside explicit experimental validation.

Current corrective position:
- Ad-hoc tenant namespace construction is treated as out-of-policy for shared-capable internals.
- Future shared-capable module additions must reference `TenantNamespace` as the naming source of truth.
