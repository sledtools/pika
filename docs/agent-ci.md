---
summary: Deterministic CI lanes for `pikachat agent` providers and how to reproduce them
read_when:
  - changing provider CI gating in `.github/workflows/pre-merge.yml`
  - debugging `check-agent-contracts` failures
---

# Agent Provider CI Lanes

This document defines deterministic CI coverage for `pikachat agent new` providers.

## Blocking Pre-merge Contract Lanes

These lanes are required in `.github/workflows/pre-merge.yml`:

- `check-agent-contracts`:
  - Runs mocked HTTP control-plane contracts for MicroVM (no real cloud credentials/hosts).
  - Covers: `pika-agent-microvm` tests, `pika-server` agent API tests, the lower-level `pikahut` deterministic HTTP integration probes (`agent_http_ensure_local`, `agent_http_cli_new_local`, `agent_http_cli_new_idempotent_local`, `agent_http_cli_new_me_recover_local`), and the app-facing `pikahut` provisioning selectors (`agent_launch_provisioning_boundary`, `agent_launch_provisioning_failure_boundary`).
  - Command: `nix develop .#default -c just pre-merge-agent-contracts`

## Advisory Integration Lanes

Real-provider probes stay outside pre-merge gating:

- They run in nightly/manual workflow mode (`mode=nightly`) and are advisory for merge safety.
- A failure in an integration probe should not be used as a pre-merge gate.

## Local Reproduction

Run these commands locally to reproduce provider contract failures:

```bash
# MicroVM mocked contracts
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

Use these PR-change patterns to confirm path-filter behavior in GitHub Actions:

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
- Touch `crates/pika-desktop/src/lib.rs`:
  - expected: `check-agent-contracts` runs.
- Touch `scripts/pikaci-staged-linux-remote.sh`:
  - expected: `check-agent-contracts` runs.
- Touch `crates/pikahut/Cargo.toml`:
  - expected: `check-agent-contracts` runs.
- Touch `crates/pikahut/tests/support.rs`:
  - expected: `check-agent-contracts` runs.
- Touch `cli/src/main.rs` only:
  - expected: `check-pikachat` and `check-agent-contracts` run.
