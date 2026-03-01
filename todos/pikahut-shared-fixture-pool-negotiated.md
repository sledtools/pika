## Spec

This follow-up exists to reduce test-suite cost and improve developer throughput by reusing expensive infrastructure fixtures (relay/Postgres/MoQ) across tests, while still guaranteeing test isolation.

Intent and expected outcome:
1. Keep the existing library-first test UX (`FixtureSpec`, `FixtureHandle`, `TestContext`) stable for callers.
2. Introduce suite-shared infrastructure mode for expensive components.
3. Enforce isolation in code through tenant APIs (not naming conventions in tests).
4. Preserve strict per-test mode for rollback, debugging, and high-isolation scenarios.
5. Deliver this as a follow-up to the ongoing refactor, not a mid-stream scope merge.

Exact build target when done:
1. `pikahut::testing` has an async-safe shared fixture pool with lazy suite-level startup.
2. Shared mode is constrained in phase 1 to a narrow profile: relay + postgres + moq shared; bots/actors remain per-test.
3. Tenant allocation is first-class (`TenantId`/tenant handle) with collision-resistant names and optional deterministic seed override for repro.
4. Shared relay/MoQ usage requires tenant-derived namespace helpers in shared-capable internals.
5. Shared Postgres defaults to schema-per-tenant, with automatic database-per-tenant fallback when needed.
6. Isolation and teardown are hardened and verified under concurrency.
7. Shared mode has minimal rollout controls: enable shared mode, force strict per-test mode, deterministic tenant seed.
8. Diagnostics/artifacts clearly record pooling behavior (reused vs started, chosen isolation mode, fallback triggers).

Exact approach:
1. Define lifecycle/isolation contracts first, then implement shared internals behind existing APIs.
2. Use async-safe singleton semantics (`tokio::sync::OnceCell` as phase-1 reference implementation) for shared startup paths.
3. Keep plan execution granular with many small, reviewable steps and acceptance criteria.
4. Roll out in guarded phases: opt-in first, then promote only after stability and isolation proofs pass.

Non-goals:
1. Sharing bot/server actors by default in phase 1.
2. Forcing shared mode across all heavy end-to-end flows immediately.
3. Removing strict per-test mode.

## Plan

1. Add lifecycle policy types for shared-capable fixtures.
Acceptance criteria: library types model `PerTest` and `SharedPerSuite`; defaults are documented.

2. Add tenant identity types and generation contract.
Acceptance criteria: `TenantId`/tenant handle APIs exist with run-id + test-id + counter + random entropy.

3. Add deterministic tenant seed override.
Acceptance criteria: env/config seed option produces reproducible tenant IDs for debugging.

4. Define tenant namespace helper APIs for relay/MoQ.
Acceptance criteria: helper methods produce canonical channel/topic names; raw naming path is not the default.

5. Define Postgres isolation policy contract.
Acceptance criteria: explicit policy states default (`schema`) and fallback (`database`) with trigger semantics.

6. Introduce shared pool module with async-safe initialization.
Acceptance criteria: suite-shared components are lazily initialized once with concurrency-safe behavior.

7. Wire shared relay startup into fixture internals.
Acceptance criteria: relay startup is reused across tests in shared mode; strict mode unchanged.

8. Wire shared MoQ startup into fixture internals.
Acceptance criteria: MoQ startup is reused in shared mode; tenant namespaces are consumed by scenarios.

9. Wire shared Postgres startup into fixture internals.
Acceptance criteria: one Postgres process per suite in shared mode; strict mode unchanged.

10. Implement schema-per-tenant provisioning.
Acceptance criteria: each test receives isolated schema handle with setup and teardown hooks.

11. Implement automatic fallback to database-per-tenant.
Acceptance criteria: fallback triggers are deterministic and logged; tenant still receives isolated handle.

12. Add per-tenant teardown hardening.
Acceptance criteria: teardown is idempotent with retries/backoff for busy resources.

13. Add shared pool poison/recovery handling.
Acceptance criteria: partial startup failures do not permanently poison subsequent tests.

14. Keep per-test actor lifecycle explicit.
Acceptance criteria: bots/servers remain per-test by default and unaffected by infra sharing.

15. Add diagnostics for fixture reuse/startup behavior.
Acceptance criteria: artifacts/logs include reuse counters, startup durations, and selected isolation mode.

16. Add isolation regression tests for relay/MoQ tenancy.
Acceptance criteria: concurrent tenants cannot observe each other’s traffic in default shared mode paths.

17. Add isolation regression tests for Postgres tenancy.
Acceptance criteria: concurrent tenants cannot read/write each other’s data under schema default or DB fallback.

18. Add guardrails for tenant helper enforcement.
Acceptance criteria: shared-capable internals are tested/linted to avoid ad-hoc namespace construction.

19. Add strict fallback mode verification tests.
Acceptance criteria: strict per-test mode remains functional and bypasses shared pool paths.

20. Add rollout toggle wiring in test entrypoints/lane config.
Acceptance criteria: deterministic suites can enable shared mode and force strict mode without code edits.

21. Run deterministic suites in strict mode and shared mode in CI-like environment.
Acceptance criteria: both modes pass baseline selectors; failures preserve actionable artifacts.

22. Document shared mode usage and boundaries.
Acceptance criteria: docs explain when to use shared vs strict mode, tenant API usage, and troubleshooting.

23. Promote shared mode only after stability gate passes.
Acceptance criteria: promotion decision requires passing isolation regressions + deterministic suite reliability.

24. Manual QA gate (user-run): validate end-to-end behavior and speedup.
Acceptance criteria: user runs representative deterministic flows in both strict and shared modes, confirms no cross-test contamination, acceptable teardown reliability, and measurable startup/runtime improvement.
