---
summary: Final human sign-off gate for the library-first integration migration
read_when:
  - performing manual QA before merging migration work
---

# Manual QA Gate (User Sign-Off)

This is the final human gate for the library-first integration migration.

## Required Matrix

Run the following representative matrix and confirm behavior + artifacts:

1. Deterministic local suite
- `just cli-smoke`
- `just openclaw-pikachat-deterministic`

2. Full OpenClaw E2E
- `just openclaw-pikachat-e2e`

3. Primal nightly smoke path
- `just nightly-primal-ios-interop`
- Treat this as retained manual-only compatibility coverage; it is intentionally not part of the core Apple nightly CI bundle.

4. One manual debug lab flow
- `just primal-ios-lab`
- Optional helpers: `just primal-ios-lab-dump-debug`, `just primal-ios-lab-patch-primal`, `just primal-ios-lab-seed-capture`, `just primal-ios-lab-seed-reset`

## Sign-Off Checklist

- [ ] Integration scenarios run through Rust selectors/library APIs as expected.
- [ ] Failure artifacts are present and useful (logs/config/URL markers).
- [ ] Capability-dependent paths skip with clear reasons on unsupported hosts.
- [ ] Existing compatibility wrappers still behave for expected developer entrypoints.
- [ ] Authoring a new scenario via `docs/testing/add-integration-scenario.md` is sufficient.

## Sign-Off Record

- Reviewer/User:
- Date:
- Notes:
- Outcome: PASS / FAIL
