---
summary: How to add a new integration test scenario using pikahut::testing APIs
read_when:
  - adding a new integration test flow
---

# Adding A New Integration Scenario

This repo's integration contract is library-first: new flows should be authored as Rust selectors using `pikahut::testing` APIs.

## 1. Pick The Right Test Target

- Deterministic/local flow: `crates/pikahut/tests/integration_deterministic.rs`
- Heavy OpenClaw gateway flow: `crates/pikahut/tests/integration_openclaw.rs`
- Public/deployed nondeterministic flow: `crates/pikahut/tests/integration_public.rs`
- Primal nightly smoke: `crates/pikahut/tests/integration_primal.rs`
- Manual/debug-only flow: add a selector under a dedicated manual test target.

## 2. Gate Capabilities Explicitly

Use `Capabilities` + `Requirement` before doing heavy setup.

```rust
use pikahut::testing::{Capabilities, Requirement};

let caps = Capabilities::probe(&workspace_root());
if let Err(skip) = caps.require_all_or_skip(&[Requirement::HostMacOs, Requirement::Xcode]) {
    eprintln!("SKIP: {skip}");
    return Ok(());
}
```

## 3. Use TestContext + CommandRunner

Own state/artifacts with `TestContext`, defaulting to preserve-on-failure.

```rust
use pikahut::testing::{CommandRunner, CommandSpec, TestContext};

let mut context = TestContext::builder("my-scenario").build()?;
let runner = CommandRunner::new(&context);
runner.run(&CommandSpec::cargo().args(["test", "-p", "pikahut", "--help"]))?;
context.mark_success();
```

## 4. Reuse Scenario Modules

If flow logic already exists under `pikahut::testing::scenarios`, call it from the test selector instead of duplicating orchestration.

## 5. Keep Selectors Lane-Ready

- Mark heavy/nondeterministic tests as `#[ignore]`.
- Ensure selector names are stable and descriptive.
- Update `docs/testing/ci-selectors.md` with the new selector-to-lane mapping.

## 6. Keep Wrappers Thin

If a `just` recipe or script is needed for compatibility, make it dispatch to the selector only. Do not add new bespoke shell orchestration.
