# pikachat-openclaw testing pyramid

This matrix defines the maintained `pikachat-openclaw` test lanes and what each lane is responsible for.

| Lane | Owner scope | Trigger | Pass/fail contract |
| --- | --- | --- | --- |
| `just openclaw-pikachat-deterministic` | Rust sidecar contracts (`crates/pikachat-sidecar`), CLI scenario flows (`pikachat scenario ...`), TypeScript plugin unit tests (`pikachat-openclaw` extension) | Local development + pre-merge | Must pass fully with local dependencies only (local relay + local binaries), no public relay or deployed gateway dependency. |
| `just pre-merge-pikachat` | `pikachat`/`pikachat-sidecar` lint + tests plus deterministic OpenClaw lane | Pre-merge CI (`check-pikachat`) | Blocks merge on any deterministic regression in CLI, sidecar protocol, or plugin channel behavior. |
| `just openclaw-pikachat-e2e` | Full OpenClaw gateway + real `pikachat` sidecar wiring (`pikachat-openclaw` channel id/config) | Nightly and manual debugging | Must prove strict invite/reply integration against a running local gateway; logs/artifacts are required on failure. |
| `just nightly-pikachat` | Narrow integration confidence lane | Nightly CI/manual nightly dispatch | Runs the full gateway E2E lane; failures are integration regressions, not deterministic unit-contract regressions. |

## Lane intent

- Deterministic checks are the primary regression net.
- Full gateway E2E is intentionally narrow and validates wiring, lifecycle, and runtime integration only.
- Any new OpenClaw-facing behavior should add deterministic coverage first, then rely on E2E only as final confirmation.
