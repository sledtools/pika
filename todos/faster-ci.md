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

## 5. Persistent runner pool

Don't boot VMs during CI runs at all. Pre-provision a pool of persistent runners on pika-build.
All mounts (snapshot, nix store, cargo caches, staged deps/build) stay active via virtiofs.
Per-job overhead drops to ~0 (SSH exec + reset artifacts dir).

Between jobs: just `rm -rf /artifacts/*; mkdir -p /artifacts`. No restart. If a runner gets
corrupted, restart that one runner; others keep working.

Sizing: 128GB RAM, 32 cores → 16 runners at 2 cores + 4GB each. More than enough.

Works for either microvm or Incus single-host-shared backend. Incus has nicer lifecycle
management; microvm has slightly faster boot (irrelevant once pool is persistent).

## 6. Longest-first scheduling with bin-packing

With historical per-binary timing data:

1. Sort all binaries longest-first
2. Priority queue of runners ordered by earliest-available time
3. Assign each binary to the runner that becomes free soonest
4. Wall clock = duration of the slowest runner

Example with realistic numbers: 9 binaries totaling 71s sequential, with 4 runners and
longest-first scheduling → 25s wall clock (limited by the single slowest binary at 25s).
Splitting that slow binary into 3 parts → 15s. That's a 4.7x speedup.

Full pre-merge across all targets: maybe 40-60 binaries, 300s sequential. With 16 runners
and longest-first: 30-40s execute time. Add snapshot sync + workspace build → under 60s total.
Versus ~5-10 minutes today.

## 7. Identify and split slow binaries

The scheduling bottleneck is always the single slowest binary. Once instrumented (#1), identify
the outliers and split them (more granular test modules, or separate compilation units).
Each split gives a directly proportional wall-clock improvement until the next binary becomes
the bottleneck.

## Incus considerations

Transfer mode (copying data into guest) adds minutes per job — not viable for sub-minute CI.
Single-host-shared mode (virtiofs disk devices) matches microvm performance and works for the
persistent pool model. This is the only viable Incus path for fast CI.

## Apple path (pika-mini) considerations

Completely separate execution model from the Linux path. Uses Tart macOS VMs on a Mac mini,
orchestrated by `pikaci-apple-remote.sh` (bash, not the Rust pikaci). Source delivered via git
bundles with commit-keyed prepared worktrees. Build caching via shared cargo target dir + Xcode
DerivedData, not Nix.

Apple-side bottlenecks are different:
- macOS VM clone + boot + agent wait: ~10-30s per job (vs sub-second microvm)
- Xcode build-for-testing: expensive, not Nix-cacheable
- Only 2-4 concurrent macOS VMs realistic on a Mac mini (vs 16+ microvms on pika-build)
- Simulator state isolation between concurrent VMs is harder than cargo test isolation

Apple pool model: 2-3 pre-booted Tart VMs with separate simulator + DerivedData paths, tests
dispatched across them. Smaller gains than Linux side but still meaningful for parallel iOS
test suites.

## Snapshot system

Not a cache. Just a filtered copy of the working directory synced to pika-build so runners can
read it via virtiofs. Content-addressed for dedup (skip sync if hash matches). Nix handles all
build artifact caching (WorkspaceDeps, WorkspaceBuild).
