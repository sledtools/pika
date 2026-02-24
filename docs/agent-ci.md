---
summary: Deterministic CI lane policy for pikachat agent providers
read_when:
  - changing provider CI/workflow path filters
  - updating pre-merge or nightly agent contract coverage
  - debugging provider lane trigger behavior
---

# Agent Provider CI Lanes

This document defines deterministic CI coverage for `pikachat agent new` providers.

## Blocking Pre-merge Contract Lanes

These lanes are required in `.github/workflows/pre-merge.yml`:

- `check-agent-contracts`:
  - Runs mocked control-plane contracts for Fly + MicroVM (no real cloud credentials/hosts).
  - Command: `nix develop .#default -c just pre-merge-agent-contracts`
- `check-workers`:
  - Runs deterministic local Workers contract smokes.
  - Command: `nix develop .#worker-wasm -c just pre-merge-workers`

The pre-merge summary treats both of these as blocking checks.

## Advisory Integration Lanes

Real-provider probes stay outside pre-merge gating:

- They run in nightly/manual workflow mode (`mode=nightly`) and are advisory for merge safety.
- A failure in an integration probe should not be used as a pre-merge gate.

## Local Reproduction

Run these commands locally to reproduce provider contract failures:

```bash
# Fly + MicroVM mocked contracts
just pre-merge-agent-contracts

# Workers deterministic contracts
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
