# Codex Review 2: Mergeability TODOs (audio-2)

Scope: make `just pre-merge` green, reduce flake, align spec/impl enough to land.

## P0: `just pre-merge` must pass

- Fix rustfmt failure in `crates/pika-media/examples/relay_benchmark.rs` (run `cargo fmt --all` and re-check `just fmt`).
- Fix clippy failures (run `just clippy --lib --tests`):
  - `crates/pika-media/src/network.rs:411` replace `seq % 50 == 0` with `seq.is_multiple_of(50)`.
  - `rust/src/core/call_runtime.rs:129` remove unused `MediaTransport::disconnect()` or use it; must not trip `-D dead-code`.
  - `rust/src/core/mod.rs:42` replace manual lowercasing with `!t.eq_ignore_ascii_case("false")`.
  - `rust/tests/e2e_real_moq_relay.rs:95` address `clippy::type_complexity` (type alias or allow at site).
  - `rust/tests/e2e_local_marmotd_call.rs:110` address `clippy::type_complexity` (type alias or allow at site).
  - `rust/tests/e2e_local_marmotd_call.rs:338` and `rust/tests/e2e_local_marmotd_call.rs:349` use `.lines().flatten()` to avoid `clippy::manual_flatten`.
- Fix failing test `rust/tests/e2e_local_marmotd_call.rs` (timeout waiting for daemon `call_debug` `rx_frames>0`).
  - Root-cause: ensure local marmotd emits `call_debug` consistently in fixture mode; consider loosening predicate, increasing timeout, or moving this check behind an env gate.
- Make network-dependent tests opt-in so CI is deterministic:
  - `rust/tests/e2e_real_moq_relay.rs:275` currently runs by default and requires QUIC egress to `https://us-east.moq.logos.surf/anon`.
  - `rust/tests/e2e_local_marmotd_call.rs:866` also depends on real MOQ (`REAL_MOQ_URL` constant) even though signaling is local.
  - Decide: `#[ignore]` by default, or require explicit env like `PIKA_E2E_REAL_MOQ=1`.

## P1: Security/operational sharp edges

- Stop logging secrets: `rust/src/core/call_control.rs:629` logs full invite payload; it includes `relay_auth`.
- TLS verification: `rust/src/core/call_runtime.rs:160` disables TLS verification for mobile on `https://` moq URLs.
  - Decide acceptable short-term policy (lab-only gate, pinned roots, or configurable override).

## P2: Spec drift cleanup (docs vs reality)

- Protocol reality notes (ensure code + deployment agree):
- Signaling includes `relay_auth` in `CallSessionParams` (`rust/src/core/call_control.rs:36`).
- v0 plaintext docs are obsolete: current impl enforces MLS-derived frame crypto + opaque labels + relay-auth token shape.
- Defaults: iOS/Android default `call_moq_url` to `https://us-east.moq.logos.surf/anon` (`ios/Sources/AppManager.swift:33`, `android/app/src/main/java/com/pika/app/AppManager.kt:69`).

## P3: “One-command” manual E2E

- Make `todos/audio-calls-e2e-plan.md` Step 3/4 runnable and current (interop binary, env vars, relay URLs, bot relays).
- Ensure deployed bot relays match client’s publish set (avoid split-brain).
