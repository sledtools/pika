# Codex Review 1

Date: 2026-02-15

Scope: clean up project detritus and track next review actions for audio calls.

## 0. Cleanup

- [ ] Delete project plan/debug markdown (keep this doc and `todos/codex-review-2.md` only).

## 1. Default Config Consistency

- [ ] Make iOS/Android/Rust agree on defaults:
  - [ ] `call_moq_url` default should match everywhere (recommend: `https://us-east.moq.logos.surf/anon`).
  - [ ] `call_broadcast_prefix` default should match everywhere (`pika/calls`).
- [ ] Add a small guard/test so a mismatch is caught early (unit-ish, not networked).

## 2. Test Suite Shape (Keep Deterministic by Default)

- [ ] Ensure `just test` stays deterministic/offline:
  - [ ] Mark `rust/tests/e2e_real_moq_relay.rs` as `#[ignore]` (or env-gate with “SKIP” when unset).
  - [ ] Mark `rust/tests/e2e_local_marmotd_call.rs` as `#[ignore]` (external binary + real network).
  - [ ] Keep `rust/tests/e2e_deployed_bot_call.rs` as `#[ignore]` (already).
- [ ] Add explicit runners/recipes for nondeterministic lanes:
  - [ ] `just e2e-real-moq` (runs `e2e_real_moq_relay` with `--ignored --nocapture`).
  - [ ] `just e2e-local-marmotd` (runs `e2e_local_marmotd_call` with `--ignored --nocapture`).
  - [ ] Keep `tools/ui-e2e-public` as the UI lane entrypoint.
- [ ] Identify “debug-only” tests that can be deleted once stable (if any exist; review after marking ignores).

## 3. TLS Verification Plan (MOQ)

- [ ] Replace `tls_disable_verify` (mobile) with a safer approach:
  - [ ] Decide between: embedded root store (e.g. webpki roots) vs platform trust integration.
  - [ ] Implement and add a minimal connectivity probe to catch regressions on device (keep it opt-in).
  - [ ] Document the security posture in the spec/runbook.

## 4. Fixture Cleanup

- [ ] Consolidate speech fixtures:
  - [ ] Pick one canonical fixture for “send speech into bot” (either `speech_prompt.wav` or `speech_test.wav`).
  - [ ] Update tests to reference the canonical fixture.
  - [ ] Delete the unused fixture.

## 5. Follow-Up Coverage (Targeted)

- [ ] Add deterministic regression coverage for:
  - [ ] Network subscription keepalive / runtime-lifetime bug class (transport dropped while subscriber task still connecting).
  - [ ] Call teardown cleanup (worker stops; call state ends; no lingering publish loop).
