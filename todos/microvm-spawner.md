## Spec

Why this is being done:
The current `vm-spawner` mixes two responsibilities: privileged host execution and partial lifecycle state management. That duplicates control-plane state already owned by `pika-server`, increases drift risk, and makes the microVM path harder to reason about.

Intent and expected outcome:
Keep `vm-spawner` as a tiny private privileged adapter for host-local microVM operations, while moving authoritative lifecycle and ownership state into `pika-server`. The result should be a smaller, more deterministic spawner that can be restarted freely without losing control-plane correctness.

Exact build target (what will exist when done):
1. Authoritative lifecycle state in `pika-server`:
`pika-server` is the source of truth for owner -> agent -> vm mapping, desired lifecycle phase, and recover/delete decisions.
2. Narrow privileged adapter:
`vm-spawner` only performs privileged host actions: create, recover, delete, and health.
3. No spawner-owned durable VM registry:
`vm-spawner` no longer treats `vm.json`, in-memory VM maps, or `sessions.json` as authoritative lifecycle state.
4. Deterministic host layout:
host paths, unit names, tap names, and any required runtime metadata are derived from `vm_id` and host defaults instead of loaded from a separate spawner database.
5. Single supported runtime path:
only the `prebuilt-cow` durable-home path remains for production behavior.
6. Recovery semantics preserved:
recover still means reboot first, then recreate using the same persistent home when reboot fails.
7. Clear host-only metadata boundary:
any remaining files under `/var/lib/microvms/<vm_id>` exist only to boot the VM, not to represent control-plane truth.

Exact approach (how it will be accomplished technically):
1. First remove unused spawner APIs and non-MVP runtime variants.
2. Then move authoritative lifecycle assumptions out of `vm-spawner` and into `pika-server`.
3. Replace spawner record-loading logic with deterministic host derivation from `vm_id`.
4. Keep only the minimum host-local metadata needed to boot, reboot, and recreate a VM with the same persistent home.

## Plan

1. Freeze target architecture and ownership boundaries.
Acceptance criteria: one short design note in code/docs states that `pika-server` owns authoritative lifecycle state and `vm-spawner` owns only privileged host execution; there is no ambiguity about which component is source of truth for `vm_id`, ownership, and app-visible phase.

2. Hard-lock the microVM runtime path to `prebuilt-cow`.
Acceptance criteria: `legacy` and `prebuilt` paths are removed from `vm-spawner`; `SpawnVariant` parsing/branching is deleted; all production callers use one create/recover path with durable `/root`.

3. Trim `vm-spawner` HTTP surface to the minimum private API.
Acceptance criteria: retained endpoints are only `GET /healthz`, `POST /vms`, `POST /vms/:id/recover`, and `DELETE /vms/:id`; `GET /vms`, `GET /vms/:id`, `GET /capacity`, and `POST /vms/:id/exec` are removed along with all callers and tests for deleted routes.

4. Move all lifecycle authority to `pika-server`.
Acceptance criteria: `pika-server` remains the only durable owner of agent lifecycle and owner-to-vm mapping; no production code depends on `vm-spawner` for authoritative enumeration, ownership lookup, phase tracking, or capacity truth.

5. Remove spawner-managed durable VM registry state.
Acceptance criteria: `vm-spawner` no longer maintains an authoritative in-memory `vms` map loaded from disk, does not persist `vm.json` as lifecycle truth, and does not require `load_from_disk()` to reconstruct control-plane state after restart.

6. Remove dead session registry complexity.
Acceptance criteria: `sessions.json`, `SessionRegistry`, `SessionRecord`, `llm_session_token`, and related config/env are deleted unless an active production consumer is identified and documented; if a consumer exists, the file is explicitly reclassified as derived runtime data and not control-plane truth.

7. Replace random per-VM values with deterministic derivation where possible.
Acceptance criteria: values required for host operations, especially tap names and MAC addresses, are derived deterministically from `vm_id` or another stable host-local function; spawner restart does not require persisted records to rediscover them.

8. Reduce the create contract to host-owned defaults plus true caller inputs.
Acceptance criteria: create requests contain only fields that have an active production caller and a clear reason to remain caller-controlled; optional knobs such as `flake_ref`, `dev_shell`, `cpu`, `memory_mb`, and `ttl_seconds` are removed or explicitly justified one by one.

9. Reduce the response contract to continuation data only.
Acceptance criteria: create/recover responses contain only the fields `pika-server` actually needs to continue its flow, at minimum `id` and any strictly required status field; debug-only fields such as timings, ssh details, and variant metadata are removed unless there is an active production consumer.

10. Rework recover/delete to derive host state from `vm_id`.
Acceptance criteria: recover and delete do not depend on loading a prior spawner VM record; they derive unit name, state path, persistent home path, tap name, and other required host artifacts from `vm_id` and host defaults, then perform reboot/recreate/delete deterministically.

11. Keep only host-boot metadata that is strictly required.
Acceptance criteria: files under `/var/lib/microvms/<vm_id>` are limited to the runtime inputs needed to boot and recreate the guest, such as guest metadata and persistent home contents; none of those files are treated as authoritative lifecycle records.

12. Decide the fate of guest SSH access and key material.
Acceptance criteria: either per-VM SSH keys are removed entirely from the spawner contract and host metadata, or they are retained with a documented operator use case and a minimal deterministic lifecycle; there is no leftover SSH machinery without an active consumer.

13. Enforce host-side admission control in the right layer.
Acceptance criteria: if capacity limits remain part of MVP or v1.5 behavior, they are enforced in the authoritative control plane with a clear contract; `vm-spawner` does not expose informational-only capacity APIs that can drift from real policy.

14. Strengthen reconciliation at the control-plane boundary, not inside spawner state.
Acceptance criteria: after `vm-spawner` restart or host reboot, `pika-server` can still make correct recover/delete decisions using its own durable records and deterministic `vm_id`-based host behavior; there is no requirement for spawner-side record replay to restore correctness.

15. Keep backup and restore semantics aligned with the stateless-spawner model.
Acceptance criteria: backup/restore docs and scripts describe persistent guest home as the durable recovery asset, not spawner metadata files; a restore drill for one VM can be completed using `vm_id` plus restic-backed home recovery.

16. Remove obsolete config, docs, and tests.
Acceptance criteria: deleted spawner features have no remaining env vars, Nix options, docs, or deterministic tests; retained tests cover only the production path of `pika-server` auth/ownership plus spawner create/recover/delete behavior.

17. Add a scope-lock doc to prevent state creep back into `vm-spawner`.
Acceptance criteria: one short doc enumerates the allowed spawner responsibilities, allowed private endpoints, allowed config surface, and explicit non-goals, including “no authoritative lifecycle DB/state in vm-spawner”.
