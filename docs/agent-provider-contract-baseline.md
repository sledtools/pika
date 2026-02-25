---
summary: Baseline scope lock for the unified agent provider workstream (fly, workers, microvm)
read_when:
  - changing provider contract constants or relay domain defaults
  - reviewing baseline before adding new provider behavior
---

# Agent Provider Contract Baseline (T0)

This document locks scope for the unified provider workstream before behavior changes land.

## Scope

- Providers in scope: `fly`, `workers`, `microvm`
- Default relay domain rule: only `*.nostr.pikachat.org` defaults

## Baseline Constants

Canonical machine-readable baseline lives in:

- `cli/src/provider_contract_baseline.rs`

Current locked targets:

- Providers: `fly`, `workers`, `microvm`
- Relay defaults:
  - `wss://us-east.nostr.pikachat.org`
  - `wss://eu.nostr.pikachat.org`

## Notes

- This baseline module is intentionally non-runtime for now.
- Follow-up nodes (`T1+`) should implement behavior changes against this contract.
