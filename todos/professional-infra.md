# Multi-Repo Infra Plan

This is the lightweight plan for getting `Jericho`, `shadow`, `pika`, and later
other repos onto one professional deployment and host-management path.

Keep this note short and current.

## Goal

Build one professional infra repo that can serve shared hosts and deploy flows
for multiple repos.

Moving deployment to `~/code/infra` is in service of that goal. It is not the
goal by itself.

The first concrete target is:

- `Jericho` serves `pika`
- `Jericho` then serves `shadow`
- the surrounding infra story stops being Pika-specific and starts being
  multi-repo by default

## Current State

- active professional infra currently lives in `pika/infra`
- personal machine config lives in `~/configs`
- `shadow` still depends on the personal `hetzner` host in `~/configs` for
  Cuttlefish-based runs
- this is good enough for now, but it is not the long-term professional shape

## Requirements

- keep personal machine config in `~/configs`
- move professional host composition out of `pika`
- support multiple repos, not just Pika
- keep final shared-host composition in one place
- let product repos keep ownership of their own product modules, packages, and
  repo-local CI frontends
- make it easy to add new professional consumers after `shadow` without
  redoing the structure again

## Working Model

- `Jericho` owns shared git/CI behavior
- each product repo owns its own frontend into `Jericho`
  - `pikaci` in `pika`
  - future `shadowci` in `shadow`
- the professional infra repo owns:
  - shared host inventory
  - final NixOS host composition
  - deploy flows
  - secrets wiring
  - cross-repo environment composition

## Planned Steps

1. Move the current active infra out of `pika/infra` into `~/code/infra`.
2. Keep it working with minimal redesign.
3. Make that repo consume `Jericho`, `pika`, `shadow`, and later `pika-cloud`
   as inputs instead of treating one product repo as the deployment root.
4. Land `Jericho` as the shared forge/CI system for `pika`.
5. Add the repo-local `shadow` integration needed for `Jericho` to serve it.
6. Move shared professional hosts onto the new infra path before worrying about
   deeper cleanup.
7. Only after that, revisit whether `shadow` still needs the personal `hetzner`
   host or should move to a professional replacement.

## Current Execution

- Step 4 has started: `pikaci` now exposes a generic CI catalog seam, and the
  forge now treats `crates/pikaci/src/ci_catalog.rs` as the checked-in CI
  source of truth instead of `forge_lanes.rs`.
- The forge web surface is being neutralized next so shared `Jericho` APIs and
  UI stop advertising `pikaci` as the product-facing CI name.
- The forge/runtime boundary is now being hard-cut too: new state goes under
  `jerichoci-state` / `.jerichoci`, and migration of existing hosted data will
  happen operationally at deploy time rather than through compatibility code.
- Next up is more forge-neutralization in `pika-git`, then the repo extraction
  and shared-infra move.

## Not The Focus Right Now

- redesigning `shadow` runtime strategy
- deciding Incus vs non-Incus for `shadow`
- perfect repo/module boundaries before the infra move
- moving personal hosts out of `~/configs`
