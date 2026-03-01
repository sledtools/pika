---
summary: Policy for keeping or removing compatibility wrappers during migration
read_when:
  - deciding whether to keep a shell wrapper script
  - deprecating legacy test orchestration
---

# Wrapper Deprecation Policy

Compatibility wrappers are kept only when they improve developer ergonomics and do **not** own integration orchestration logic.

## Policy

1. Wrappers must dispatch to Rust selectors (`cargo test -p pikahut --test ...`).
2. Wrappers may parse user-friendly flags, but cannot start bespoke fixtures/process graphs.
3. Unsupported legacy arguments are accepted only as no-op compatibility inputs with clear warnings.
4. If a wrapper cannot remain thin, remove it and document the replacement selector command.

## Current Wrapper Status

| Wrapper path | Status | Selector contract |
| --- | --- | --- |
| `tools/cli-smoke` | retained (thin) | `integration_deterministic::{cli_smoke_local,cli_smoke_media_local}` |
| `tools/ui-e2e-local` | retained (thin) | `integration_deterministic::ui_e2e_local_*` |
| `tools/ui-e2e-public` | retained (thin) | `integration_public::ui_e2e_public_*` |
| `pikachat-openclaw/scripts/run-scenario.sh` | retained (thin) | `integration_deterministic::openclaw_scenario_*` |
| `pikachat-openclaw/scripts/run-openclaw-e2e.sh` | retained (thin) | `integration_openclaw::openclaw_gateway_e2e` |
| `pikachat-openclaw/scripts/phase*.sh` | retained (thin aliases) | Dispatch to `run-scenario.sh` / `run-openclaw-e2e.sh` |

## Removal Rule

When a wrapper no longer adds DX value, remove it and document:

- replacement selector command
- affected lane/recipe mappings
- migration note in `docs/testing/integration-matrix.md`
