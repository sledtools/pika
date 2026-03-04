## Spec

Why this is being done:
After v1 dogfood passes, we need enough hardening to safely open the first-100 external pilot while keeping architecture simple.

Intent and expected outcome:
The same 1-click personal agent flow scales from 3 internal users to a managed first-100 cohort with explicit capacity policy, stronger isolation, and operator-grade support controls.

Exact build target (what will exist when done):
1. Managed whitelist operations:
allowlist managed by admin data path (not hardcoded 3-user config).
2. Pilot capacity controls:
global cap of 100 active agents with stable `capacity_exhausted` responses.
3. Network abuse guardrails:
guest egress blocks for internal/private ranges, inter-guest isolation, and per-VM connection-rate limits.
4. Hardened auth path:
existing token verification formally hardened, or NIP-98 introduced if required.
5. AI provider key hardening:
provider secrets remain server-controlled; guest exposure minimized via controlled token/proxy model when needed.
6. Operator debug + runbook:
admin surfaces show ownership/lifecycle/failure context and document deterministic recovery workflow.

Exact approach (how it will be accomplished technically):
1. Build on top of v1 contracts and infrastructure; do not introduce a new control plane.
2. Roll out incrementally: allowlist management first, then capacity/guardrails, then auth/key hardening.
3. Gate pilot opening on explicit acceptance checks.

## Plan

1. Replace static 3-user gate with admin-managed allowlist.
Acceptance criteria: admins can add/remove whitelisted npubs without code redeploy; app/API gating behavior remains consistent.

2. Add global cap enforcement for first-100.
Acceptance criteria: active-agent cap of 100 is enforced in create path; over-capacity returns stable `capacity_exhausted` error.

3. Add host bridge network/abuse guardrails.
Acceptance criteria: guests cannot reach private/internal ranges (RFC1918, CGNAT/Tailscale, host-local control ports); inter-guest traffic is blocked by default; per-VM new-connection rate limit is enforced.

4. Harden external-user auth path.
Acceptance criteria: auth verification is formally hardened (or replaced with NIP-98); invalid/expired/unauthorized requests are rejected consistently.

5. Harden AI key handling.
Acceptance criteria: raw provider keys are not exposed to app clients; guest secret handling follows server-controlled policy or proxy/token model.

6. Add operator debug surfaces and incident runbook.
Acceptance criteria: admin tools show owner, vm id, lifecycle phase, and last failure context; runbook covers locate, inspect, recover, and verify.

7. Execute pilot opening gate for first-100.
Acceptance criteria: team validates allowlist flow, capacity behavior, isolation checks, and recovery workflow before enabling broader onboarding.
