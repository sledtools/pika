# Faster CI

Ideas for making pikaci runs significantly faster, mostly through finer-grained parallelism.

## Core insight

microvm spawns are nearly free (virtiofs = zero-copy mounts, cloud-hypervisor boots in sub-second).
The current bottleneck is running multiple test binaries *sequentially* inside one VM.
If we run each binary in its own VM in parallel, wall clock approaches the slowest single binary.

## 1. Instrument per-binary timing

Add timing to the manifest runners (`run-pika-core-test-manifest` etc.) — log start/end/duration
and peak RSS per binary to a JSON file in artifacts. This gives us the dataset to know where the
wins are and what the theoretical minimum wall clock is.

## 2. Consolidate runner flakes

Today each job gets its own runner flake Nix build. The only things that vary per-job are
`guestCommand`, `artifactsDir`, `timeoutSecs`, `socketPath`, `runAsRoot`, and `workspaceReadOnly`.
Everything else (mounts, toolchains, system, host uid/gid) is identical within a target.

Move to one runner image per target (or one universal runner). Inject the command at launch time.
This eliminates per-job Nix build cost and is a prerequisite for cheap many-VM parallelism.

## 3. One binary per VM for pure tests

Split lane-level job specs into binary-level job specs. Non-fixture tests (pika-core unit tests,
agent control plane unit tests, etc.) become one VM per test binary, all running in parallel.
The DAG planner already supports N execute nodes sharing one prepare node — no architectural change.

## 4. Pikahut-dependent tests: defer for now

Tests that need pikahut (postgres, pika-server) are harder to parallelize.
Three options to evaluate later:

- **Pay per VM**: each VM boots its own pikahut. Simple, ~150-200MB per VM, fine on 128GB host.
- **Shared host pikahut**: one pikahut on pika-build, VMs connect over network. Needs test isolation (per-test DB schemas or similar).
- **Hybrid**: parallelize pure tests now, keep fixture lanes coarser until they're the bottleneck.

Lean toward hybrid — get the easy wins first.

## 5. Data-driven scheduling

Once we have per-binary timing data across runs:

- Identify bottleneck binaries (split them or optimize the tests)
- Bin-pack fast tests into shared VMs instead of one-per-VM (reduces VM count without hurting wall clock)
- Detect timing regressions automatically
- Let the scheduler read historical data and pack optimally each run
