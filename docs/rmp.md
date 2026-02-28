---
summary: Rust Multiplatform model — Rust-owned state/logic plus native capability bridges
read_when:
  - designing cross-platform features
  - deciding whether code belongs in Rust vs iOS/Android
  - implementing platform-native capabilities (audio, push, external signers, etc)
---

# Rust Multiplatform (RMP) Model

This repo follows a Rust Multiplatform architecture:

- Rust is the source of truth for business logic and state.
- Native apps (iOS/Android) are primarily functional renderers of Rust state.
- We prefer Rust-first implementations by default.
- Native code may host narrowly scoped platform adapters when they are required to deliver true first-class platform UX.

See also:

- `docs/architecture.md` for topology
- `docs/state.md` for `AppState`/`AppUpdate` flow
- `docs/rmp-ci.md` for CI lanes

## UX Invariant

RMP must never produce a second-class experience versus a true native app.

- If a Rust-only approach can achieve native-quality behavior, prefer Rust.
- If platform-native APIs are required to match native UX/quality, use a native capability bridge.
- "Cross-platform purity" does not override user experience quality.

## Core Principle

Rust owns:

- state machines and policy decisions
- protocol/transport/crypto behavior
- long-lived application state (`AppState` + actor-internal derivation state)
- cross-platform invariants and error semantics

Native owns:

- rendering and UX affordances
- platform capability execution where native APIs are required to achieve first-class native behavior (system audio routing, push surfaces, URL handoff, etc)
- short-lived handles to OS resources

Native must not own app business logic.

## Native Capability Bridge Pattern

(`adapter window` is an acceptable alias during transition.)

When we need platform APIs, we use a native capability bridge.

A native capability bridge is a bounded lifecycle where Rust leases a single responsibility to native runtime code while keeping policy/state ownership in Rust.

### Contract shape

1. Rust opens window by command:
- Example: "start call audio IO", "open external signer URL", "arm push decrypt context".

2. Native executes platform side effect:
- OS callbacks, device/session setup, route handling, permission-coupled operations.

3. Native reports events/data back to Rust:
- typed callbacks/events only (no native-owned policy transitions).

4. Rust decides:
- state updates, retries, fallbacks, user-visible outcomes, telemetry semantics.

5. Rust closes window:
- deterministic teardown command and native resource release.

### Guardrails

- Rust-defined API: callbacks and events are versioned and owned by Rust contracts.
- No policy forks: native cannot introduce alternate call/login/message state machines.
- Bounded native state: keep only transient buffers/handles required by OS callbacks.
- Idempotent lifecycle: start/stop/restart paths must be safe during interruptions.
- Observable boundary: emit counters/errors so Rust can compare behavior and decide rollouts.

## What Belongs Where

Put in Rust when it is:

- policy, branching behavior, retries, and fallback decisions
- protocol semantics and validation
- state that affects routing, UX flow, or correctness
- logic shared across iOS/Android/desktop

Put in native adapter when it is:

- hard OS integration point (real-time audio callback graphs, push extension execution, app-switch URL intents)
- route/session/device APIs unavailable or unreliable via cross-platform abstraction
- strict platform UX behavior (system call UI expectations, interruption handling)

Preference order:
1. Rust implementation that preserves native-quality UX.
2. Native capability bridge if and only if required for native-quality UX.

## Current Examples

- External signer bridge:
  - Rust trait boundary in `rust/src/external_signer.rs`
  - native implementations in app layers
  - Rust still owns login flow state machine and error policy
- Push/NSE split:
  - Rust crypto/decrypt logic in `crates/pika-nse`
  - iOS Notification Service Extension hosts Apple notification lifecycle
- Call audio migration (in progress):
  - Rust contract now includes platform capture ingress, playout receiver callbacks, and route/interruption signals.
  - Runtime backend selection supports `platform|cpal|synthetic` (`PIKA_CALL_AUDIO_BACKEND`), with mobile defaulting to `platform` and non-mobile defaulting to `cpal`.
  - Native iOS/Android audio adapters are intentionally scoped as operational bridges (callbacks + OS session hooks), while Rust remains the source of truth for call policy, retries, fallback, and user-visible state transitions.

## RMP Checklist for New Features

Before adding native logic, answer:

1. Can a Rust-first implementation match true native UX/quality on target platforms?
2. Can we isolate it behind a narrow Rust-owned contract?
3. Is native state purely transient/operational?
4. Does Rust still decide policy and user-visible outcomes?
5. Do we have telemetry to validate and rollback?

If any answer is "no", keep moving logic back into Rust until it is.
