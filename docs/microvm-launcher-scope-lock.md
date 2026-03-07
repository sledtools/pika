---
summary: Scope-lock guardrails for the simplified private microVM launcher.
read_when:
  - proposing new vm-spawner endpoints or config knobs
  - changing microvm launcher request/response contracts
  - reviewing launcher architecture expansions
---

# MicroVM Launcher Scope Lock

This doc prevents the simplified launcher from regressing into a generic VM platform.

## Allowed vm-spawner responsibilities

- perform host-local privileged operations for create, recover, delete, and health checks
- derive deterministic host layout from `vm_id` (unit, tap, IP, MAC, gcroots, state paths)
- write/rewrite runtime boot metadata required to start or recreate the guest

## Allowed private API surface

- `GET /healthz`
- `POST /vms`
- `POST /vms/:id/recover`
- `DELETE /vms/:id`

No additional production endpoints are in scope.

## Allowed request/response surface

Create request:

- `guest_autostart` payload only
  - `command`
  - `env`
  - `files`

Create/recover response:

- `id`
- `status`

## Allowed config surface

- host bind/network settings (bridge, IP pool, gateway, DNS)
- deterministic state/runtime artifact paths
- CPU/memory defaults and bounds
- command path overrides (`systemctl`, `ip`, `nix`, `chown`, `chmod`)
- runtime artifact mount/install specs
- prewarm enablement for the retained runner path

## Explicit non-goals

- no generic remote-dev VM launcher behavior
- no authoritative lifecycle database/state machine in `vm-spawner`
- no multiple launcher variants without a separate product decision and scope lock update
