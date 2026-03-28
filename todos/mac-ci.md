# Mac CI

This is the living plan for getting macOS CI from "good enough" to "as strong as Linux CI", and then past it.

The recent Apple CI work materially improved the situation:

- pre-merge Apple is now compile-only
- pre-merge Apple is split into desktop and iOS lanes
- Apple compile work now leans much harder on Nix
- Apple host assets are pinned
- host-local iOS build reuse is much better

That work is real and deployed.

Live validation after deploy:

- desktop-only branch changes queue `apple_desktop_compile`
- iOS-only branch changes queue `apple_ios_compile`
- the old "always run one big `apple_host_sanity` smoke" behavior is no longer the intended selector path

## Current State

Today, branch CI can run these Apple pre-merge lanes:

- `check-apple-desktop-compile`
- `check-apple-ios-compile`

Current design:

- desktop lane is compile-only
- iOS lane is compile-only
- heavier Apple runtime work stays in nightly Apple bundle coverage

What is already good:

- unrelated branches should usually avoid Apple entirely
- desktop-only changes can avoid iOS Apple work
- iOS-only changes can avoid desktop Apple work
- shared Rust/build-system changes still correctly hit Apple when they can break Apple builds

What is still not good enough:

- Apple compile throughput is still effectively single-slot
- Apple still depends on one remote host and host-local mutable caches
- Apple does not yet have Linux-grade operational clarity or cache portability

## What Linux Still Has That Mac Does Not

Linux CI is still ahead in four important ways:

1. Better cache model

- Linux is closer to true derivation-first CI
- Linux build products are easier to share and reason about through Nix caches
- Apple still depends on a hybrid of Nix artifacts plus host-local Xcode mutable state

2. Better horizontal scaling

- Linux already has a cleaner story for running multiple independent jobs at once
- Apple compile work still serializes on one compile slot
- Apple runtime work still bottlenecks on one host more than it should

3. Better operational simplicity

- Linux does not depend on Xcode, simulator runtimes, or `DerivedData`
- Apple still has more moving parts outside the Nix store

4. Better failure isolation

- Linux lanes tend to map more cleanly onto hermetic prepare/run boundaries
- Apple still has shared host-local state that can create confusing reuse or contention issues

## Remaining Work For Parity

This is the work needed to make Mac CI as good as Linux CI in practice.

### 1. Increase Apple compile concurrency safely

This is the next real bottleneck.

Right now:

- `apple_desktop_compile` and `apple_ios_compile` share the forge concurrency group `apple-compile`
- the Apple remote wrapper also serializes compile work behind one compile lock

That means:

- the split lane selection is correct
- but only one Apple compile lane can run at a time

What we need:

- separate compile concurrency groups for desktop and iOS
- separate remote lock files for desktop and iOS
- separate prepared roots and cache scopes where needed
- careful validation of shared Rust/Xcode caches under parallel use

Target end state:

- desktop and iOS compile jobs can run concurrently when they are independent
- one Apple compile job should not queue behind another by default

### 2. Formalize Apple cache layers

We have the right hybrid direction, but it should be more explicit.

The cache stack should be:

1. Nix-managed immutable layers

- Rust dependencies
- Rust Apple target outputs
- generated bindings
- xcframework assembly inputs and outputs where practical

2. Apple-managed host assets

- Xcode
- simulator runtimes
- device types / simulator asset contract

3. Host-local mutable caches

- `DerivedData`
- simulator state
- `xcresult`
- Xcode incremental graph state

What is missing:

- clearer cache ownership and docs
- explicit cache invalidation rules
- stronger observability around cold/warm hits
- more confidence that cache keys match real Apple inputs

### 3. Add stronger Rust cache reuse on Apple

We already added `sccache` to the Apple shell path.

What is still missing:

- prove `sccache` is materially helping the expensive Apple Rust rebuild paths
- measure hit rates on real branch traffic
- size and persist the cache appropriately on the Apple host
- decide whether Apple CI should publish or preserve `sccache` state across future worker hosts

Parity target:

- Rust-heavy Apple rebuilds should behave much closer to Linux warm rebuilds than they do today

### 4. Reduce the remaining Xcode-native cost

After the recent work, the slow part is increasingly the Xcode side rather than the Rust side.

We should:

- keep profiling `xcodebuild` with timing summaries
- remove or conditionalize any always-run script phases from pre-merge compile paths
- keep compile-only lanes ruthlessly compile-only
- avoid unnecessary simulator/device lifecycle work in compile-only lanes

Parity target:

- Apple pre-merge work should be dominated by true compile cost, not by avoidable Xcode overhead

### 5. Improve Apple operational visibility

Linux CI is easier to reason about because the runtime model is simpler.

Apple needs better operator clarity around:

- cache hit vs cache miss
- waiting on concurrency group vs waiting on remote lock
- host-local compile reuse
- prepare duration vs bundle duration
- lock contention
- stale prepared state

We already have phase reports and timings, but the next step is to surface them more clearly in forge UI and `ph`.

Parity target:

- when Apple is slow, it should be obvious why

### 6. Harden the Apple worker model

One Mac host is still too fragile as a long-term answer.

To reach Linux-like confidence we need:

- a clean worker model for Apple capacity
- explicit host health
- explicit available slots
- better recovery when the Apple host wedges

This does not mean "jump straight to many VMs first".
It means:

- clean host lifecycle
- clear scheduling model
- then add more parallel capacity on top

## Work That Would Make Mac CI Better Than Linux CI

This is the ambitious tier. It is not required for parity.

### 1. Opportunistic Apple burst capacity

Example:

- always-on rented Apple host as the stable baseline
- volunteer MacBook Pro runner while you are working
- scheduler decides how much extra Apple capacity is available

If this is done well:

- daytime Apple queue time can beat Linux queue time for some workflows
- heavy Apple follow-up work can drain much faster than it does now

### 2. VM-backed Apple isolation with preserved warm caches

This is the right long-term direction if we want much higher Apple throughput.

The key is not just "run Tart VMs".
The key is:

- mount or share the right persistent cache layers
- preserve warm Rust/Xcode state where safe
- keep runtime isolation where it actually helps

If we get this right:

- Apple runtime jobs become much more parallel
- flaky stateful Apple work becomes easier to isolate

### 3. Better selector precision than Linux

Apple branch lane selection is already becoming more precise.

We can push this further:

- tighter desktop-only vs iOS-only scoping
- narrower shared dependency surfaces
- fewer unnecessary Apple triggers on unrelated changes

If we do this aggressively, Apple CI could become more selective than some Linux lanes.

### 4. Apple-specific compile heuristics

Potential examples:

- skip parts of app assembly for compile-only smoke when link coverage is already sufficient
- smarter reuse of prebuilt Apple intermediates
- lane-specific compile shortcuts keyed by exact change surface

Linux does not need as much special handling here.
Apple may justify it.

## Recommended Sequence

This is the order that makes the most sense from here.

1. Keep the deployed forge and checked-in `ph` in sync

- avoid another stale deploy / stale client mismatch
- make `ph` use the live `/git/api` surface everywhere in practice

2. Split Apple compile throughput

- separate concurrency groups
- separate remote locks
- validate concurrent desktop + iOS compile behavior

3. Measure real cache performance

- `sccache` hit rates
- warm vs cold xcframework times
- warm vs cold `DerivedData` times

4. Profile remaining Xcode cost

- use timing summaries on real Apple branches
- cut always-run Xcode work from pre-merge paths

5. Design the next Apple capacity model

- rented bigger always-on Mac host vs volunteer laptop burst
- only after the cache and compile path are solid

6. Add optional Apple parallelism beyond one host

- Tart or similar
- only after host/cache boundaries are explicit

## Concrete Open Questions

These are the main decisions still worth making.

- Should desktop and iOS compile lanes get separate forge concurrency groups immediately?
- Should desktop and iOS compile lanes share one `CARGO_TARGET_DIR`, or should concurrent mode split them?
- Should Apple compile lanes stay host-native forever, with only runtime lanes moving into VMs?
- Should the long-term Apple baseline be a rented `64 GiB` mini, the new laptop as a volunteer runner, or both?
- How much of the Apple Rust layer should be pushed further into dedicated Darwin Nix derivations?

## Bottom Line

Mac CI is no longer the old broken shape.

The big wins are already in:

- compile-only Apple pre-merge
- split Apple desktop/iOS selection
- better Apple caching
- deployed forge understands the split selectors

What is still missing for Linux parity is mostly:

- concurrency
- stronger cache portability
- better operational visibility
- a more scalable worker model

What would make Mac CI better than Linux CI is:

- burst Apple capacity
- VM-backed Apple runtime parallelism with warm caches
- even more selective Apple lane triggering
