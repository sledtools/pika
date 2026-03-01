---
summary: Progress checklist for the library-first integration test migration
read_when:
  - checking migration status
  - picking up remaining migration work
---

# Library-First Migration Checklist

Status for `todos/pikahut-library-first-integration-tests.md`.

## Completed Selector-Backed Coverage

- [x] Deterministic selector target exists: `integration_deterministic`.
- [x] Heavy selector target exists: `integration_openclaw`.
- [x] Nondeterministic selector target exists: `integration_public`.
- [x] Primal/nightly selector target exists: `integration_primal`.
- [x] Manual selector target exists: `integration_manual`.
- [x] OpenClaw deterministic scenarios are selector-backed (`openclaw_scenario_*`).
- [x] Public UI and deployed-bot flows are selector-backed (`integration_public::*`).
- [x] `just` recipes invoke selectors as lane contracts (not ad-hoc CLI orchestration).
- [x] Compatibility wrappers in `tools/` and `pikachat-openclaw/scripts/` dispatch into selectors/scenario library.

## Remaining Gaps

- [ ] Manual interop/primal lab selectors currently codify runbook contracts and capability gating; full interactive automation remains intentionally manual.
- [ ] Manual QA gate completed by user sign-off.

## Notes

- This checklist intentionally distinguishes selector-backed coverage from manual-only gaps so CI/docs do not hide unfinished migration work.
- Manual QA sign-off remains the final gate and cannot be auto-completed in unattended mode.
