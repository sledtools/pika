# JerichoCI Extraction Plan

This is the remaining plan for turning the reusable CI engine into `JerichoCI`
and leaving `pikaci` as the Pika-specific usage layer.

It reflects the code after the recent cleanup:

- `crates/jerichoci/**` now holds the reusable execution engine
- `crates/pikaci/**` now holds the Pika CLI and CI catalog
- `crates/pikaci/src/targets.rs` is the authored source of truth for Pika CI targets
- `crates/pikaci/src/forge_lanes.rs` is now a derived projection for `pika-git`
- `ci/forge-lanes.toml` is gone
- all forge-managed CI lanes now go through `pikaci`

## What Is Done

The major architectural cuts are already in place:

- reusable execution logic lives in `jerichoci`, not in the Pika frontend
- Pika-specific target catalog and CI policy live in `pikaci`
- forge no longer owns an independent authored lane manifest
- placement and runtime backend are modeled separately
- backend-specific setup moved under explicit runtime config

This means the project is no longer blocked on the old `pikaci`/library boundary.
The rest of the work is about making extraction and naming clean rather than
fixing the original design confusion.

## Remaining Goal

The desired end state is:

- `JerichoCI` is a reusable CI execution library/repo
- `pikaci` is the Pika catalog + CLI that uses `JerichoCI`
- `pika-git` consumes a small typed CI catalog API from `pikaci`
- `pika-git` continues to own scheduling, leases, queue truth, and merge gating

## What Still Feels Wrong

### 1. The `pikaci` -> `pika-git` seam is still forge-shaped

`pika-git` currently consumes a derived `ForgeLane` projection.
That is better than TOML, but it is still one layer too forge-specific.

What we want instead:

- `pikaci` exports a typed CI catalog API
- `pika-git` derives its scheduling view from that API
- the shared boundary is "CI entries" rather than "forge lane records"

### 2. `jerichoci` still has historical naming and stringly seams

The engine split exists, but extraction is not yet mechanical because:

- some runtime-visible names still say `pikaci`
- some behavior is still driven by shell strings rather than richer typed config
- some helper binaries and env var names still reflect historical packaging

### 3. Apple concurrency recovery exposed scheduler rough edges

The branch that unified the catalog also exposed a real operational problem:

- stale Apple compile work can leave later lanes queued until manual recovery

That is not a reason to undo the catalog work, but it should become explicit
follow-up work in `pika-git`.

## Recommended Next Steps

### Phase 1: Replace `ForgeLane` as the shared boundary

Goal:

- make `pikaci` export a typed CI catalog API that is not named after forge

Concrete work:

1. introduce a shared exported type in `pikaci` for a CI-exposed target
2. move `pika-git` off `ForgeLane` and onto that typed API
3. keep any forge-only display/transport details inside `pika-git`
4. reduce `forge_lanes.rs` until it is either trivial or gone

Done when:

- `pika-git` no longer depends on a forge-named catalog struct from `pikaci`
- the shared contract reads like "Pika CI catalog" instead of "forge lane manifest"

### Phase 2: Tighten the `JerichoCI` surface

Goal:

- make extraction into its own repo mostly mechanical

Concrete work:

1. audit `crates/jerichoci/**` for remaining `pikaci`-era names
2. rename runtime-visible engine contracts that should belong to `JerichoCI`
3. isolate helper binaries/contracts that are truly engine-owned vs frontend-owned
4. replace ad hoc stringly runtime setup with more explicit typed config where it helps

Done when:

- `jerichoci` can plausibly move to another repo without dragging Pika naming along
- the public engine surface no longer suggests that it is Pika-specific

### Phase 3: Shrink `pikaci` into a clear frontend

Goal:

- make `pikaci` obviously "our CI catalog and CLI"

Concrete work:

1. keep splitting large frontend files into target catalog, command handlers, and rendering
2. keep local/manual targets and forge-exposed targets in one coherent catalog model
3. document the intended ownership boundary in a short stable note

Done when:

- a new contributor can look at `pikaci` and immediately see that it is the Pika layer
- engine concerns are not leaking back upward into the catalog/CLI layer

### Phase 4: Extract `JerichoCI`

Goal:

- move the reusable crate into its own repo without redesigning it again

Concrete work:

1. decide the final crate/package/binary naming
2. move `crates/jerichoci/**` and the engine-owned helpers to a new repo
3. make `pikaci` depend on the extracted engine
4. keep `pika-git` depending only on `pikaci` and engine data/types it truly needs

Done when:

- the repo split is mostly file motion and dependency rewiring

## Suggested Invariants To Add

These are the next invariants worth enforcing once the code catches up:

- `JERICHO-001`: `jerichoci` does not reference Pika repo paths, package names, or target ids
- `JERICHO-002`: `pika-git` consumes a typed Pika CI catalog API, not a separate authored forge manifest
- `JERICHO-003`: all Pika CI lanes are defined exactly once in `pikaci` code
- `JERICHO-004`: stale concurrency-group holders are recoverable without manual DB inspection

## Immediate Next Move

If we keep pushing aggressively, the next cut should be:

1. replace `ForgeLane` with a smaller typed shared CI entry API
2. then clean up the remaining `jerichoci` naming and runtime contract surface

That order keeps the architecture getting simpler rather than moving complexity
around before extraction.
