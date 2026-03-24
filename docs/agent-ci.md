---
summary: Deterministic CI lanes for `pikachat agent` providers and how to reproduce them
read_when:
  - changing provider CI gating in `.github/workflows/pre-merge.yml`
  - debugging `check-agent-contracts` failures
---

# Agent Provider CI Lanes

This document defines deterministic CI coverage for `pikachat agent new` providers.

## Blocking Pre-merge Contract Lanes

These lanes are defined canonically in `ci/forge-lanes.toml` and orchestrated by the forge on `git.pikachat.org`. GitHub mirrors them through `.github/workflows/pre-merge.yml` as advisory shadow CI:

- `check-agent-contracts`:
  - Runs the checked-in staged Linux agent provider contract surface on `pikaci`.
  - Covers: `pika-agent-control-plane` unit tests, `pika-agent-microvm` tests, `pika-server` `agent_api::tests`, and the `pika_core` NIP-98 signing contract test.
  - Intentionally does not cover the old host-side `pikahut` deterministic HTTP / CLI selectors anymore. Those selectors still encode legacy vm-spawner-era assumptions, so they were removed from provider-contract CI instead of being silently treated as an Incus parity problem. They are currently manual-only until they are rewritten against the surviving Incus/OpenClaw product contract and given a new truthful lane.
  - Command: `nix develop .#default -c just pre-merge-agent-contracts`

## Advisory Integration Lanes

Real-provider probes stay outside pre-merge gating:

- They run canonically from forge nightly orchestration, with GitHub `mode=nightly` as an advisory mirror.
- A failure in an integration probe should not be used as a pre-merge gate.

## Local Reproduction

Run these commands locally to reproduce provider contract failures:

```bash
# Staged Linux provider contracts
just pre-merge-agent-contracts

# Full pre-merge lane for pikachat crate
just pre-merge-pikachat
```

For the `pikachat-openclaw` pyramid specifically:

```bash
# Deterministic lane (pre-merge)
just openclaw-pikachat-deterministic

# Full OpenClaw gateway E2E lane (nightly/manual)
just openclaw-pikachat-e2e
```

## Trigger Sanity Checks

Use these PR-change patterns to confirm manifest-driven path-filter behavior:

- Touch `just/checks.just`:
  - expected: `check-agent-contracts` runs.
- Touch `just/infra.just`:
  - expected: `check-agent-contracts` runs.
- Touch `crates/pika-agent-control-plane/src/lib.rs`:
  - expected: `check-agent-contracts` runs.
- Touch `crates/pika-agent-microvm/src/lib.rs`:
  - expected: `check-agent-contracts` runs.
- Touch `crates/pika-server/src/agent_api.rs`:
  - expected: `check-agent-contracts` runs.
- Touch `crates/pika-test-utils/src/lib.rs`:
  - expected: `check-agent-contracts` runs.
- Touch `scripts/pikaci-staged-linux-remote.sh`:
  - expected: `check-agent-contracts` runs.
- Touch `cli/src/main.rs` only:
  - expected: `check-pikachat` runs; `check-agent-contracts` no longer owns stale CLI provisioning coverage.
