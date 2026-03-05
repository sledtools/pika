## Spec

Why this is being done:
We need a fast internal checkpoint for the 1-click personal agent flow before opening to external users.

Decision update (2026-03-04):
Restic backup setup is explicitly deferred and is non-blocking for current dogfood/prod validation.

Intent and expected outcome:
Justin, Ben, and Paul can each create exactly one long-lived personal agent VM from the app, chat with it over Marmot, and keep state across restart/recovery.

Exact build target (what will exist when done):
1. App-facing control endpoints:
`POST /v1/agents/ensure`, `GET /v1/agents/me`, `POST /v1/agents/me/recover`.
2. One-agent-per-user invariant:
exactly one active agent per `npub`; duplicate create while `creating` or `ready` is rejected.
3. Managed allowlist operations:
allowlist managed by admin data path (not hardcoded 3-user config).
4. Durable home:
per-agent host-backed path mounted as guest `/root`, with `/workspace` resolving to `/root`.
5. Marmot/MLS durability:
`pikachat` state under `/root` so restart/recover preserves chat context.
6. Backups (deferred; non-blocking for v1):
host-managed restic backup to Cloudflare R2 is tracked for post-v1 hardening and does not gate rollout/testing now.
7. Recovery:
reboot first, then recreate VM while reusing the same persistent `/root` backing path.
8. Private control transport:
`pika-server` reaches `vm-spawner` over private WireGuard path; `vm-spawner` remains non-public.

Exact approach (how it will be accomplished technically):
1. Keep `vm-spawner` as lifecycle backend and `pika-server` as ownership/auth/control layer.
2. Use NIP-98 auth plus DB-backed allowlist gating managed through admin tools.
3. Keep user-facing lifecycle state minimal (`creating`, `ready`, `error`).
4. Validate success through real app usage and Marmot messaging by all three dev users.

## Plan

1. Freeze v1 API contract and state model.
Acceptance criteria: endpoints and error codes are fixed; app-visible states are only `creating`, `ready`, `error`.

2. Add private transport from `pika-server` to `vm-spawner`.
Acceptance criteria: `pika-server` can call `vm-spawner` over WireGuard/private network; `vm-spawner` is not publicly reachable.

3. Add control-plane schema and one-agent-per-user DB constraint.
Acceptance criteria: DB stores `owner_npub`, `agent_id`, `vm_id`, phase, timestamps; only one active agent per `owner_npub` is allowed.

4. Implement admin-managed allowlist gate.
Acceptance criteria: admins can add/remove allowlisted npubs without redeploy; only active allowlist npubs can call ensure/get/recover; others receive `403 not_whitelisted`.

5. Implement `POST /v1/agents/ensure` idempotent flow.
Acceptance criteria: first call provisions; subsequent calls while active do not create another VM and return stable `agent_exists` behavior.

6. Add durable `/root` backing in prebuilt/prebuilt-cow path.
Acceptance criteria: each VM mounts dedicated host persistent home at `/root`; `/workspace` maps to `/root`; files survive VM restart.

7. Enforce Marmot/MLS state path under `/root`.
Acceptance criteria: `pikachat` state directory is under `/root`; encrypted chat context remains valid after restart.

8. Defer host-managed 10-minute restic backups to Cloudflare R2.
Acceptance criteria: explicitly out of v1 rollout gate; absence of restic does not block deploy, whitelist validation, or app flow testing.

9. Implement `POST /v1/agents/me/recover` (reboot, then recreate-with-same-home fallback).
Acceptance criteria: recover reboots unhealthy VM; if still unhealthy, VM is recreated and attached to same persistent home path.

10. Wire app flow for dogfood.
Acceptance criteria: app shows button only for whitelisted npubs, calls `ensure`, polls `GET /v1/agents/me`, and opens chat when `ready`.

11. Execute 3-dev checkpoint gate.
Acceptance criteria: Justin, Ben, and Paul each complete create -> ready -> Marmot exchange; at least one restart/recover preserves state/files.
