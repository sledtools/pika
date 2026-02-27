---
summary: Compatibility testing plan for Pika interop with other Marmot-based apps (White Noise)
read_when:
  - setting up or running cross-app compatibility tests
  - adding a new Marmot-based app to the compatibility program
---

# Pika Compatibility Testing Spec (Draft)

## Status

- Owner: Pika maintainers
- Scope: Pika compatibility program across Marmot-based apps (starting with White Noise)
- Primary repos:
  - Pika: `~/code/pika`
  - White Noise Flutter: `~/code/whitenoise`
  - White Noise Rust core: `~/code/whitenoise-rs`
  - Marmot spec: `~/code/marmot`
- Date: 2026-02-21

## Problem Statement

Pika needs automated compatibility tests that verify real cross-app behavior over Marmot protocol, starting with core messaging and scaling to full protocol coverage.

Immediate goals:

- Group creation both directions (`Pika -> White Noise`, `White Noise -> Pika`)
- Basic messages both directions
- Reply semantics both directions
- Reaction semantics both directions

Long-term goals:

- Cover all behavior carried by Marmot protocol (`MIP-00` to `MIP-05`)
- Detect breaking compatibility changes before release
- Nightly compatibility fuzzing against multiple Pika versions/tags

## Current-State Findings

1. Pika already has strong automation surfaces.
- In-process Rust actor API (`FfiApp` / `AppAction` / `AppState`)
  - `rust/src/lib.rs`
  - `rust/src/actions.rs`
  - `rust/src/state.rs`
- Process-level interop interfaces
  - `marmotd` JSONL daemon: `crates/marmotd/src/daemon.rs`
  - `pika-cli`: `cli/src/main.rs`

2. Pika already has relay-backed E2E patterns.
- `rust/tests/e2e_local_relay.rs` validates local relay behavior and app-to-app message flow.

3. White Noise has reusable scenario design we can mirror.
- `whitenoise-rs/src/integration_tests/scenarios/basic_messaging.rs`
- `whitenoise-rs/src/integration_tests/test_cases/shared/*`

4. Version skew is an explicit risk and must be treated as first-class test input.
- Pika HEAD uses MDK `31335bc...` (0.6.0)
- White Noise uses MDK `749976c...` (0.6.0)
- Pika release tags `v0.2.0`..`v0.2.8` use MDK `d6777f...` (0.5.3)

5. Some old/new mixes should be expected fail, not regressions.
- MDK 0.6 tightened key package/welcome requirements (`i` tag, strict encoding, legacy format removal).
- Compatibility policy must model protocol generation boundaries.

## Pika-First Requirements

### Functional

- Compatibility scenarios must run locally and in CI.
- Failure output must classify cause:
  - protocol/wire mismatch
  - app behavior mismatch
  - relay/environment flake
- Must support testing Pika HEAD and older tags.

### Program-level

- Pika controls the harness and baseline policies.
- Harness design must generalize so additional Marmot apps can plug in later.
- Wire-level assertions are required for protocol correctness.

## Architecture Options

### Option A: In-process adapters only

- Pika via `FfiApp`
- White Noise via `whitenoise-rs` direct calls

Pros:

- Fast and debuggable
- Rich state introspection

Cons:

- Tied to internals
- Harder cross-version testing across historical binaries/tags
- Less reusable for non-Rust app stacks

### Option B: Process adapters only

- Every app exposes stable command/event protocol over stdio JSONL

Pros:

- Best long-term interoperability layer
- Best fit for version matrices and tag testing
- Matches existing `marmotd` direction

Cons:

- Higher initial adapter work
- Lower introspection unless protocol exposes enough state

### Option C: Hybrid (Recommended)

- Canonical compatibility lane uses process adapters.
- Optional in-process adapters exist for local developer speed.

Pika recommendation:

- Use `marmotd` protocol as the base/anchor for adapter standardization.
- Keep process boundary as source-of-truth for compatibility pass/fail.

## Proposed Protocol: Marmot Compat Driver Protocol (MCDP) v1

Define a minimal stable driver protocol implemented by each app adapter.

### Required commands

- `get_identity`
- `publish_keypackage`
- `list_pending_welcomes`
- `accept_welcome`
- `list_groups`
- `create_group`
- `send_message`
- `send_reply`
- `send_reaction`
- `fetch_messages`
- `shutdown`

### Required events

- `ready`
- `welcome_received`
- `group_joined`
- `message_received`
- `error`

### Required capability handshake

Example capability keys:

- `supports_reply_tags_e`
- `supports_reply_tags_p_k`
- `supports_reactions_kind7`
- `supports_media_mip04`
- `supports_push_mip05`

Scenarios should be capability-gated, not app-name-gated.

## Adapter Strategy

### Pika adapter (primary)

- Base: `marmotd` (`crates/marmotd/src/daemon.rs`)
- Extend if needed for explicit reply/reaction commands and richer message fetch metadata

### White Noise adapter (peer)

- Add `whitenoise-compatd` in `whitenoise-rs` implementing MCDP v1
- Optional adapter mode to mirror Flutter tag semantics where needed

## Harness Strategy (Owned by Pika)

Create a Pika-owned compatibility harness that executes scenarios against two drivers.

Suggested location:

- `tools/compat-harness/` in `~/code/pika`

Harness responsibilities:

- Driver lifecycle (spawn, healthcheck, teardown)
- Scenario orchestration with deterministic timeouts/retries
- Neutral relay observation for wire-level assertions
- Artifact collection:
  - driver logs
  - relay event traces
  - scenario transcript
- Outcome classes:
  - `pass`
  - `fail`
  - `xfail`
  - `infra-flake`

## Initial Scenario Set (V0)

1. Bootstrap
- Start drivers
- Publish key packages on both sides
- Assert `kind:443` visibility

2. Group create `Pika -> White Noise`
- Pika creates group and invites White Noise
- White Noise processes welcome and joins
- Both sides list group membership consistently

3. Group create `White Noise -> Pika`
- Mirror of #2

4. Basic messaging both directions
- Pika sends text, White Noise receives/decrypts
- White Noise sends text, Pika receives/decrypts

5. Reply semantics both directions
- Reply references valid target message
- Receiving side resolves thread linkage
- Wire assertions enforce required reply tags

6. Reaction semantics both directions
- `kind:7` reaction references valid target
- Receiving side aggregates as expected
- Wire assertions validate reaction target refs

## Assertion Model

### Layer 1: App-observable behavior

- Group exists and is active
- Message/reply/reaction visible in receiver state

### Layer 2: Wire-level assertions

- Required kinds observed (`443`, `1059`, `445`)
- Required tags valid (`h`, reply/reaction refs)
- Critical sequencing checks (welcome after commit acceptance path)

### Layer 3: Invariants

- No duplicate joins from same welcome
- No malformed key package publish format
- No impossible scenario state transitions

## Version Matrix and Nightly Fuzzing

### Matrix axes

- Pika ref set:
  - `HEAD`
  - latest release tag
  - previous N tags (start with N=3)
- Peer app ref set:
  - White Noise HEAD (initial)
  - optional White Noise release tags (later)
- Relay backend:
  - `pika-relay`
  - optional `strfry` lane
- Mode:
  - deterministic smoke
  - randomized fuzz

### Expected-fail policy

Use checked-in policy file, e.g.:

- `tools/compat-harness/expected_failures.yml`

Fields:

- `pair` (pika_ref, peer_ref)
- `scenario`
- `reason`
- `issue`
- `review_by`

Rules:

- Matching known failures report `xfail` (not red)
- `xfail` that starts passing is `unexpected_pass` and should fail policy check until cleaned up

### Fuzz lane

Nightly randomized run should vary:

- Pika tag selection from configured pool
- Scenario order
- Timing jitter and bounded reconnect perturbations

Always print fixed seed for reproducibility.

## CI Rollout (Pika)

Pika already has nightly lanes in:

- `.github/workflows/pre-merge.yml`

Rollout plan:

### Phase 1

- Add spec and harness skeleton
- Implement V0 scenarios for `Pika HEAD <-> White Noise HEAD`
- Run nightly-only

### Phase 2

- Add version matrix (Pika HEAD + tags)
- Add relay observer assertions
- Gate at least one deterministic lane in pre-merge

### Phase 3

- Add fuzz lane and broader MIP coverage
- Add additional Marmot apps as they appear, reusing same driver protocol

## Coverage Mapping

### V0

- `MIP-00`: key package publish/discovery/consumption basics
- `MIP-01`: group creation/join baseline metadata behavior
- `MIP-02`: welcome delivery/accept flow
- `MIP-03`: application messages, replies, reactions

### Later

- `MIP-03` advanced membership mutations + deletion semantics + race paths
- `MIP-04` media interop
- `MIP-05` push interop

## Risks and Mitigations

1. Relay timing flake
- Use deterministic local relays + explicit retry/timeout policy

2. False green from app-only assertions
- Require wire-level assertions on core scenarios

3. Permanent CI red from legacy incompatibility
- Maintain explicit expected-fail policy with review dates

4. Adapter drift
- Keep MCDP v1 contract versioned and test adapter conformance in CI

## Concrete Next Steps

1. Freeze MCDP v1 schema using `marmotd` as baseline.
2. Implement Pika compatibility harness skeleton under `tools/compat-harness/`.
3. Add `whitenoise-compatd` peer adapter in `whitenoise-rs`.
4. Implement six V0 scenarios.
5. Wire nightly lane in Pika CI for `Pika HEAD <-> White Noise HEAD`.
6. Add version-matrix lane with `expected_failures.yml`.
