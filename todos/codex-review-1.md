# Codex Review 1

Date: 2026-02-15

Scope: clean up project detritus and track next review actions for audio calls.

## 0. Cleanup

- [x] Delete project plan/debug markdown (kept: `todos/codex-review-1.md`, `todos/codex-review-2.md`, plus a couple useful build notes).

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

- [x] Replace `tls_disable_verify` (mobile) with a safer approach:
  - [x] Use a shared rustls policy (`crates/pika-tls`) that standardizes on `webpki-roots` (Mozilla bundle) everywhere.
  - [x] Switch MOQ/QUIC transport to `quinn` + `web-transport-quinn` + `moq-lite` (no `moq-native` root-loading).
  - [x] Keep an opt-in device probe: `crates/pika-media/examples/quic_connect_test.rs`.

## 4. Fixture Cleanup

- [ ] Consolidate speech fixtures:
  - [x] Canonical fixture: `speech_prompt.wav`.
  - [x] Update tests to reference the canonical fixture.
  - [x] Delete the unused fixture.

## 5. Follow-Up Coverage (Targeted)

- [ ] Add deterministic regression coverage for:
  - [ ] Network subscription keepalive / runtime-lifetime bug class (transport dropped while subscriber task still connecting).
  - [ ] Call teardown cleanup (worker stops; call state ends; no lingering publish loop).
