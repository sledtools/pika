# Pika Cloud Arbitrary Guest Config Spike

This note sketches how `pika-cloud` could grow from the current hard-coded Incus guest images into
a system that can run arbitrary Nix-defined guest configurations without regressing into the old
fake-backend architecture.

It is a design spike, not an implementation plan for one branch. The goal is to get the shape
right before adding another one-off guest mode.

## Current State

Today the shared substrate is in a good place:

- `pika-cloud` owns the Incus runtime contract
- `pikaci` uses a shared Incus runtime plan and lifecycle contract
- `pika-server` uses the same substrate for managed OpenClaw guests
- the old local `microvm` path is gone

But the guest image layer is still hard-coded in two separate directions:

- [nix/incus/managed-agent-image.nix](/Users/justin/code/pika/worktrees/pika-cloud/nix/incus/managed-agent-image.nix)
  bakes an OpenClaw-oriented managed guest
- [nix/incus/pikaci-image.nix](/Users/justin/code/pika/worktrees/pika-cloud/nix/incus/pikaci-image.nix)
  bakes a CI-oriented guest runner
- [flake.nix](/Users/justin/code/pika/worktrees/pika-cloud/flake.nix) exports those as two
  separate image artifacts with separate import scripts
- [infra/nix/modules/pika-server.nix](/Users/justin/code/pika/worktrees/pika-cloud/infra/nix/modules/pika-server.nix)
  still models host config in product-specific OpenClaw terms

That split is fine for now, but it does not scale to:

- a Pi experimentation VM with SSH access
- an operator-only sandbox image
- multiple managed guest roles
- custom guest packages or modules supplied by the repo

## Goal

Support multiple Incus guest configurations built from Nix without inventing a fake generic VM
provider layer.

More concretely:

- keep `pika-cloud` Incus-first
- let the repo define multiple guest images/configs in a uniform way
- let consumers refer to a guest role/config instead of a hard-coded image alias string
- separate product runtime automation from operator tooling
- make it possible to add a simple SSH-based sandbox guest later without bending the OpenClaw path

## Non-Goals

This spike should not turn into:

- a new backend-neutral hypervisor abstraction
- a new long-running `pika-cloud` service
- a full remote command/control protocol
- immediate Tart support
- immediate user-supplied Nix expressions from outside this repo
- another one-off Pi mode stapled into managed OpenClaw

## Design Principles

### 1. Incus stays the substrate

The generalization is over guest configuration, not over providers.

That means:

- keep `RuntimeSpec` and `IncusRuntimePlan` Incus-specific
- do not reintroduce `ProviderKind`
- express variability in guest image/config inputs, mounts, entry commands, and policies

### 2. Guest role is the unit of reuse

The missing abstraction is not “provider.” It is “guest role.”

A guest role answers:

- what image artifact gets built/imported
- what software is present in the guest
- what entrypoint or bootstrap contract it expects
- what mounts it needs
- what lifecycle/readiness contract it uses
- whether it is product-managed or operator-driven

Examples:

- `managed-openclaw`
- `pikaci-runner`
- future `pi-sandbox`

### 3. Product automation and operator sandboxes should be separate

The current managed OpenClaw path is product runtime. A future Pi sandbox is operator tooling.
Those should not share a startup protocol just because they both boot an Incus VM.

The common layer should be:

- image build/import shape
- runtime plan shape
- lifecycle/artifact conventions where useful
- host-side guest selection/config resolution

The guest-side startup contract should remain role-specific.

### 4. Repo-defined configs first

The first step toward “arbitrary Nix guest configs” should mean:

- arbitrary guest configs defined inside this repo
- built by this repo’s flake
- selected by stable role ids

Not:

- evaluating arbitrary user-provided Nix expressions at runtime
- remote code upload into the hypervisor path

That bigger capability can come later if we actually need it.

## Proposed Model

### Guest Definition

Add a repo-local concept like `GuestDefinition` or `IncusGuestDefinition`.

It should describe:

- `id`: stable internal role id such as `managed-openclaw` or `pikaci-runner`
- `image_alias`: default Incus alias
- `image_package`: flake package that builds the qcow2/metadata artifact
- `lifecycle_contract`: `pika-cloud` lifecycle, custom lifecycle, or none
- `runtime_mode`: managed service, one-shot command runner, or operator sandbox
- `default_mounts`
- `default_resources`
- `default_entry_command` if the role uses one
- optional role-specific bootstrap contract

This is not necessarily a Rust type first. The source of truth may start in Nix plus a small Rust
mirror for the runtime-facing parts.

### Guest Image Family

Restructure the current image layout around shared building blocks:

- common Incus image base module
- shared lifecycle helper module
- role-specific modules layered on top

A likely Nix shape:

- `nix/incus/base-image.nix`
- `nix/incus/modules/lifecycle-helper.nix`
- `nix/incus/modules/ssh-access.nix`
- `nix/incus/roles/managed-openclaw.nix`
- `nix/incus/roles/pikaci-runner.nix`
- future `nix/incus/roles/pi-sandbox.nix`

Then `flake.nix` can build image artifacts from a small table instead of duplicating two separate
image recipes.

This model also needs an explicit rollout story:

- who builds and imports each role image onto the remote Incus host
- whether aliases are mutable pointers or versioned names
- how consumers detect that a remote host is serving a stale image for a role

Without that, a clean role registry in-repo still leaves hidden drift at the hypervisor edge.

### Split the image problem in two

There are really two layers:

1. image composition
2. runtime launch semantics

Image composition decides:

- which packages are installed
- whether SSH is enabled
- whether OpenClaw is present
- which systemd units are baked in

Runtime launch semantics decide:

- which mounts are attached
- which cloud-init files are written
- whether a role is long-lived or disposable
- whether the host expects guest lifecycle files

Those layers should be selectable independently enough that we can reuse an image family without
reusing the same runtime behavior.

### What changes in Rust

`pika-cloud` likely needs very little new code for the first version.

It should keep owning:

- Incus runtime plans
- runtime paths and lifecycle artifacts
- mount/policy/resource vocabulary

It probably should not own:

- the full guest-definition registry
- role-specific bootstrap payloads
- host config naming for OpenClaw, Pi, or CI product semantics

The likely Rust changes belong in consumers:

- `pika-server` should resolve a managed guest role instead of hard-coding one OpenClaw-shaped
  image/config tuple
- `jerichoci` should resolve a CI guest role instead of directly naming `pikaci/dev`

The `pika-cloud` crate might gain only a small shared type if needed, something like:

- `IncusGuestRoleId`
- or `IncusImageReference`

But the role registry itself can stay outside `pika-cloud` initially.

### Host Configuration Shape

The current server module is too product-specific for this future:

- `incusOpenclawGuestIpv4Cidr`
- `incusOpenclawProxyHost`
- product-coupled assertions

The medium-term shape should separate:

- generic Incus host settings
- role-specific settings

For example:

- generic:
  - endpoint
  - project
  - storage pool
  - client/server TLS material
- role-specific:
  - `managed-openclaw` network/proxy settings
  - future sandbox defaults

That lets us add new guest roles without pretending all of them are OpenClaw.

### The future Pi sandbox

This model gives a clean place for the VM you described earlier.

It would be a role like `pi-sandbox` with characteristics:

- image has `pi`, `opencode`, `tmux`, `git`, `jq`, `ripgrep`, `just`, and SSH enabled
- no product daemon
- no custom control protocol
- no OpenClaw coupling
- readiness is either “booted” or “SSH reachable”
- operator interacts over plain SSH

The important point is that this becomes just another guest role, not a new backend and not a
branch inside managed OpenClaw startup logic.

## Recommended Phases

### Phase 0: Design only

Do now:

- align on the guest-role model
- identify which parts belong in Nix, which in `pika-server`, which in `jerichoci`
- do not add a Pi-specific runtime yet

### Phase 1: Refactor image composition

Refactor without product behavior changes:

- extract a shared Incus image base
- extract shared lifecycle helper module
- make managed OpenClaw and `pikaci` images role modules over that base
- keep exported flake packages stable while changing internal structure

This reduces future cost without widening scope yet.

### Phase 2: Add explicit guest role resolution

In Rust/Nix:

- define stable guest role ids
- have `pika-server` and `jerichoci` resolve roles instead of embedding raw image aliases
- move image alias defaults and role metadata into one source of truth
- define how a role maps to an imported remote image and how stale image aliases are detected

This is the key generalization step.

### Phase 3: Add the first non-product operator role

Only after Phase 1 and 2:

- add `pi-sandbox`
- keep it SSH-first and manual
- use it to discover what automation is actually worth adding

### Phase 4: Decide whether truly arbitrary repo-defined guest modules are needed

At that point we can decide whether we need:

- a registry of repo-defined guest roles only
- or a more flexible “compose a guest from Nix modules” mechanism exposed to operators

There is no reason to pay for that complexity before we have real pressure for it.

## Recommended First Implementation Slice

If we start implementing after this spike, the best first slice is not Pi.

It is:

1. factor common Incus image-building logic out of [flake.nix](/Users/justin/code/pika/worktrees/pika-cloud/flake.nix)
   and the two image modules
2. introduce explicit role ids for the two roles that already exist:
   - `managed-openclaw`
   - `pikaci-runner`
3. teach `pika-server` and `jerichoci` to resolve those role ids instead of freehand image alias
   strings

That gives us a real generalization with no speculative extra runtime surface.

## Open Questions

- Should the role registry live primarily in Nix, Rust, or both with one generated from the other?
- Do we want a single import script that takes a role id, or keep per-role helper scripts?
- Who owns remote image rollout for each role, and how do we detect alias drift or stale imports on
  the Incus host?
- Should operator-only sandbox roles use the shared lifecycle contract, or just rely on SSH and
  host-observed boot state?
- How much of role metadata should `pika-server` persist in the database versus resolving at
  runtime?
- When Tart eventually comes back, do we model it as another provider-specific role registry, or as
  a separate runner family outside this first Incus-focused design?

## Recommendation

Do not add a Pi one-off next.

Instead:

- treat this as the design checkpoint
- implement the role/image refactor first
- only then add the first operator sandbox role

That keeps `pika-cloud` honest, avoids another disposable abstraction, and puts the project on a
path where “arbitrary guest configs” means something coherent instead of “more special cases.”
