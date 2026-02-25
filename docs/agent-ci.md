---
summary: Deterministic CI lanes for `pikachat agent` providers and how to reproduce them
read_when:
  - changing provider CI gating in `.github/workflows/pre-merge.yml`
  - debugging `check-agent-contracts` or `check-workers` failures
---

# Agent Provider CI Lanes

This document defines deterministic CI coverage for `pikachat agent new` providers.

## Blocking Pre-merge Contract Lanes

These lanes are required in `.github/workflows/pre-merge.yml`:

- `check-agent-contracts`:
  - Runs mocked control-plane contracts for Fly + MicroVM (no real cloud credentials/hosts).
  - Command: `nix develop .#default -c just pre-merge-agent-contracts`
- `check-workers` (paused during Workers freeze):
  - Currently runs a no-op skip lane so Workers is not an active execution target.
  - Command: `nix develop .#worker-wasm -c just pre-merge-workers`

The pre-merge summary treats both as required jobs; while frozen, `check-workers` should remain skipped/no-op.

## Advisory Integration Lanes

Real-provider probes stay outside pre-merge gating:

- They run in nightly/manual workflow mode (`mode=nightly`) and are advisory for merge safety.
- A failure in an integration probe should not be used as a pre-merge gate.

## Local Reproduction

Run these commands locally to reproduce provider contract failures:

```bash
# Fly + MicroVM mocked contracts
just pre-merge-agent-contracts

# Workers lane (currently paused/no-op)
just pre-merge-workers

# Full pre-merge lane for pikachat crate
just pre-merge-pikachat
```

## Trigger Sanity Checks

Use these PR-change patterns to confirm path-filter behavior in GitHub Actions:

- Touch `cli/src/fly_machines.rs`:
  - expected: `check-agent-contracts` and `check-pikachat` run.
- Touch `cli/src/main.rs` only:
  - expected: `check-pikachat` runs; `check-agent-contracts` is skipped.
- Touch `workers/**` only:
  - expected: `check-workers` runs (plus any shared lanes selected by other touched paths).
