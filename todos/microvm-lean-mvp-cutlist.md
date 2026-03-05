## Spec

Why this is being done:
The current microvm control path still carries inherited protocol/runtime surfaces that are not needed for the v1 personal-agent MVP and make future changes slower and riskier.

Intent and expected outcome:
Keep only the smallest end-to-end path needed for app-driven ensure/me/recover against private vm-spawner, with admin-managed allowlist and durable state, while removing dead protocol and operator surfaces.

Exact build target (what will exist when done):
1. Single runtime path:
`pika-server` calls vm-spawner directly for create/recover/delete with one spawn mode (`prebuilt-cow`) and no legacy variants.
2. Minimal crate graph:
no runtime dependency on `pika-agent-control-plane`; microvm request/response types live in one place.
3. Minimal vm-spawner API:
only `GET /healthz`, `POST /vms`, `POST /vms/:id/recover`, `DELETE /vms/:id`.
4. Minimal spawner contract:
create/recover request/response fields are reduced to MVP-required data only.
5. Minimal operator/config surface:
unused env vars, list/capacity plumbing, and legacy naming are removed.
6. Clean docs/tests:
all docs and deterministic tests reflect NIP-98 + DB allowlist flow with no bearer-token remnants.

Exact approach (how it will be accomplished technically):
1. Remove unused runtime abstraction layers first (crate and types), then collapse vm-spawner variants and endpoints.
2. Tighten request/response/config surfaces only after call paths are simplified.
3. Keep deterministic coverage around the exact production path (server API + rust core + CLI + spawner create/recover).

## Plan

1. Remove `pika-agent-control-plane` from runtime.
Acceptance criteria: `MicrovmProvisionParams` moves into `pika-agent-microvm` (or `pika-server`), `pika-server` and `pika-agent-microvm` no longer depend on `pika-agent-control-plane`, and workspace membership/deps are removed if crate is fully deleted.

2. Decide whether to fold `pika-agent-microvm` into `pika-server`.
Acceptance criteria: either:
`pika-agent-microvm` is removed and helper/client code lives under `pika-server`, or
crate is kept but explicitly scoped to only shared spawner contract helpers with no extra abstraction.

3. Hard-fix spawn mode to `prebuilt-cow`.
Acceptance criteria: `SpawnVariant` enum/parsing/branches are removed from vm-spawner manager; create/recover paths run one code path only; persisted vm records no longer default to `legacy`.

4. Remove variant fields from contracts.
Acceptance criteria: `spawn_variant` is removed from:
`vm-spawner` create request model, persisted vm model, response model, and `pika-agent-microvm` create payload builder.

5. Trim vm-spawner HTTP surface to 4 endpoints.
Acceptance criteria: remove handlers/routes for:
`GET /vms`, `GET /vms/:id`, `POST /vms/:id/exec`, `GET /capacity`.
No callers remain in repo.

6. Delete manager plumbing used only by removed endpoints.
Acceptance criteria: `VmManager::list`, `VmManager::get`, and `VmManager::capacity` are removed, plus related response models and helper code.

7. Remove periodic health ticker noise.
Acceptance criteria: `vm-spawner` background ticker that logs `vm_count` every 30s is removed; `healthz` remains.

8. Shrink create request to host-owned defaults.
Acceptance criteria: evaluate and remove public request knobs not needed by MVP:
`flake_ref`, `dev_shell`, and possibly `cpu/memory_mb/ttl_seconds`.
If retained, each retained field must have an active production caller and a clear operator reason.

9. Shrink vm response to server-required fields.
Acceptance criteria: response returned to `pika-server` contains only fields needed for continuation (at minimum `id`, optionally `ip/status`).
Fields like ssh key/session token/timing are removed unless there is an active consumer.

10. Remove dead session registry complexity if unused.
Acceptance criteria: if no active runtime consumer for `sessions.json`/`llm_session_token`, remove session file persistence and related config/env.
If retained, document the active consumer and failure mode.

11. Minimize vm-spawner config/env surface.
Acceptance criteria: remove env/config entries tied to deleted endpoints or deleted request fields (including `VM_SPAWN_VARIANT_DEFAULT` and capacity-only settings where possible).

12. Rename legacy secret slot in infra.
Acceptance criteria: replace `agent_owner_token_map` naming with admin-session naming (or dedicated secret key) in Nix+sops without semantic mismatch.

13. Align docs with NIP-98 + allowlist DB and lean control plane.
Acceptance criteria: no docs mention bearer owner token map or `pikachat agent new --token`; all docs describe current `--nsec` flow and admin allowlist ownership.

14. Keep only MVP-relevant deterministic tests.
Acceptance criteria: retained CI tests cover:
server NIP-98 verification, rust-core signed flow, CLI-over-HTTP flow, and spawner create/recover behavior.
Tests for deleted APIs/variants are removed.

15. Add “MVP scope lock” guardrail doc.
Acceptance criteria: one short doc enumerates allowed control-plane endpoints, allowed config knobs, and explicit non-goals to prevent reintroducing removed surfaces.
