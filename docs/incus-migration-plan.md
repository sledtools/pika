---
summary: Holistic plan for migrating the managed agent platform and eventually `pikaci` from `microvm.nix` to Incus.
read_when:
  - evaluating fleet architecture for managed agents
  - planning multi-host VM orchestration
  - replacing `vm-spawner` or `microvm`-specific provider contracts
  - planning the eventual `pikaci` backend migration
---

# Incus Migration Plan

This document describes how Pika should move off `microvm.nix` and onto Incus.

The main reason for the switch is operational, not ideological. We do not want to spend the next
phase of the project building a clustered VM control plane from scratch. We want a fleet manager
that already handles the boring infrastructure problems so we can focus on the agent product.

This is a living plan. It should be updated as implementation work teaches us more about the real
constraints, surprises, and sequencing.

Current `pikaci` OpenClaw Incus note:

- the staged OpenClaw e2e runtime must keep the packaged gateway root free of bundled
  `pikachat-openclaw` extensions
- the test scenario now copies the plugin into a runtime-local state directory before loading it
- the packaged wrapper must source that plugin from a standalone packaged tree instead of
  `/mnt/pikaci-workspace-build/.../lib/openclaw/extensions/...`, or OpenClaw will auto-discover a
  duplicate plugin from the mounted package root and stall before `/health`

The migration has two tracks:

- first, move the managed per-customer agent platform to Incus
- later, migrate `pikaci` to an Incus-backed execution model

The managed agent platform is the higher priority. `pikaci` should be handled separately and later.

## How To Use This Document

This document is meant to guide an agile implementation loop.

The expected workflow is:

- identify the next bounded chunk of work
- write an implementation prompt against this document and current repo state
- have another agent implement that chunk
- review the change carefully, potentially with multiple reviewers
- update this document based on what we learned

This document is not meant to pretend we know every detail up front.

It should do three things well:

- keep the target architecture coherent
- record the migration direction and major constraints
- absorb what we learn from each implementation chunk

If implementation reveals a better sequencing, a missing prerequisite, or a wrong assumption, the
document should be edited rather than defended.

## Decision

We should outsource the infrastructure control-plane layer to Incus while keeping the product
control plane in Pika.

Concretely:

- Incus should own clustered VM lifecycle primitives
- `pika-server` should remain the authoritative product control plane
- guest images and runtime composition should remain Nix-driven where useful
- Pika should stop depending on `microvm.nix`-specific runner and host assumptions over time

This is not a decision to stop using Nix. It is a decision to stop using `microvm.nix` as the
foundation for a multi-host fleet.

## Why We Are Doing This

Our current `microvm.nix` approach works well for a single-host privileged adapter, but the moment
we want a real fleet the work expands into cloud-platform concerns:

- host membership and placement
- clustered state and API semantics
- storage attachment and durable volume management
- snapshots, backups, and restore
- failure handling and evacuation
- network management across hosts
- quotas, isolation, and resource controls
- day-2 operations for a large VM fleet

Those are not differentiating product features for Pika. They are expensive infrastructure chores.

Incus already provides most of that substrate for virtual machines. Using it lets us keep strong
VM isolation for one-customer-per-agent-runtime while avoiding a custom control plane for the lower
layers.

## Product And Trust Boundary

The trust model does not change:

- one customer gets one VM
- that VM is the main isolation, billing, and recovery boundary
- different customers never share a VM
- the managed platform remains authoritative for networking, updates, observability, and recovery

This matters because the reason to prefer VMs here is strong isolation for user-owned agent
workloads. The migration should not weaken that boundary in pursuit of convenience.

## What We Have Today

Today there are two distinct `microvm` consumers:

- the managed agent platform
- `pikaci`

They should not be treated as one migration unit.

### Managed Agent Platform Today

The current managed agent platform consists of:

- `pika-server` as the product control plane and lifecycle authority
- `vm-spawner` as a private privileged single-host VM adapter
- `microvm.nix` host integration and runner generation underneath the spawner

This stack is good at turning one host into a managed VM box. It is not a fleet platform.

### `pikaci` Today

`pikaci` is not merely consuming a generic Linux VM. It is coupled to:

- backend-specific guest launch contracts
- backend-specific mounted runtime layout for workspace, artifacts, cargo caches, and staged outputs
- staged payload contracts that must stay aligned between Nix, executor code, and guest runtime
- host-side runner preparation, synchronization, and artifact collection

That means `pikaci` migration was a real backend redesign, not a transport change.

## What We Need From The Target Platform

The target platform must provide the following for the managed agent fleet:

- strong VM isolation suitable for one-customer-per-runtime
- multi-host clustering with a stable API
- placement across a fleet without writing our own scheduler first
- per-project or per-tenant isolation boundaries and quotas
- durable storage that survives VM restart and host replacement
- snapshot and backup primitives
- structured lifecycle operations for create, start, stop, restart, move, and delete
- a clean way to attach metadata, startup configuration, and guest bootstrap material
- enough observability and eventing to drive product-level status and operations
- a way to restrict public exposure while keeping private control-plane access

The target platform should not require Pika to manage per-host TAP setup, unit files, or host-local
boot directories as control-plane truth.

## Target Architecture

The end state should look like this:

- `pika-server` remains the source of truth for customer ownership, billing period, product state,
  launch tickets, and recover or delete decisions
- Incus becomes the cluster-wide VM substrate
- a Pika provider layer talks to Incus through its API
- guest images are built by Pika using Nix and published in an Incus-consumable form
- durable customer state lives in Incus-managed storage volumes, not host-local
- Pika maps product concepts like `CustomerVm`, `AgentInstance`, backup, recover, and UI launch
  onto Incus lifecycle primitives plus Pika-owned metadata

The key boundary is:

- Incus owns infrastructure state for instances, storage, placement, and cluster membership
- Pika owns product state, tenancy, agent semantics, and customer-facing behavior

## Guiding Simplifications

The migration should actively simplify the system instead of mechanically porting every current
behavior.

The most important simplification is:

- design the managed agent VM as an immutable appliance plus one persistent volume

That means:

- the VM root should be treated as disposable
- the platform should boot from a versioned base image
- durable customer state should live on one attached persistent volume
- recover, upgrade, and host relocation should prefer "create fresh VM from image and reattach the
  volume" over reconstructing mutable host-local state

This model is a better fit for Incus than trying to preserve the current host-local
`microvm.nix` mental model.

Other simplifications we should prefer:

- build a small number of reusable base images instead of per-customer images
- fetch or inject customer-specific startup plans at provision or first boot time
- keep the Incus adapter thin and avoid recreating a thick spawner layer
- stop depending on host-readable guest files for product behavior
- prefer explicit APIs and durable metadata over filesystem convention magic
- treat existing environments pragmatically rather than forcing a perfect migration story

## Missing Decisions We Should Capture Early

The plan should stay living and agile, but there are a few decisions that are important enough to
name early.

### Success Metrics And Kill Criteria

We should define concrete thresholds for:

- cost per customer VM
- VM density per host
- cold and warm start time
- guest-ready time
- recover time
- backup and restore correctness
- operator burden

If the new stack misses these thresholds badly, we should know early rather than drifting.

### Secret And Identity Flow

We need a clear model for:

- how guests receive initial credentials
- how secrets rotate
- which identities are platform-managed versus guest-managed
- what survives snapshot and restore

### Image Versioning And Rollout Policy

We need a versioned image model that defines:

- how images are built and named
- how they are published to Incus
- how a `CustomerVm` records its base generation
- how roll-forward and rollback work

### Break-Glass Operator Access

We should define early:

- who may use console or exec access
- how emergency debugging is audited
- what product guarantees still hold after break-glass intervention

### Disaster Recovery Boundary

We should distinguish:

- recovery on another cluster member
- restore from snapshot or backup within the same fleet
- disaster recovery after losing a whole cluster or storage backend

These are different problems and should not be collapsed into one vague "recover" story.

### Existing Environment Migration Policy

We should decide whether existing managed environments:

- remain on the old backend until deletion
- are recreated on Incus with copied state
- get a one-time migration path

The simplest policy is often "new environments on Incus, old ones retired over time", but the
document should state the decision once made.

## What Incus Should Replace

Incus should replace the parts of the current system that are really infrastructure plumbing:

- single-host spawner lifecycle management
- hand-built fleet placement logic
- host-local VM identity and state directory assumptions
- custom host networking setup as the main abstraction
- bespoke VM recovery mechanics that depend on one host's filesystem layout
- host-specific persistent-home conventions as the primary durability model

Incus should not replace:

- `pika-server` as the authoritative product control plane
- Pika's database records and business logic
- agent startup plans and template semantics
- launch tickets, dashboard policy, and customer-facing API behavior
- product-specific observability, logs, artifacts, and billing logic

## Required New Architecture Pieces

Moving to Incus still requires real work on our side. We are not eliminating the product control
plane. We are eliminating the infrastructure fleet manager we would otherwise have to build.

### 1. Provider Abstraction Cleanup

We should stop baking `microvm` into our domain model where the concept is really "managed VM
provider".

We need:

- provider-neutral naming in contracts and modules where possible
- a clean Incus provider implementation
- clear boundaries between the managed-agent product path and any remaining non-product microVM work

The goal is to prevent the rest of the app from knowing whether the backing provider is
`microvm.nix` or Incus.

Current transition status:

- the managed-agent product contract has now hard-cut to Incus + OpenClaw only
- managed-agent request/command contracts no longer preserve the old `microvm` request shape or Pi/ACP runtime selection
- the server routes managed-agent lifecycle calls only through the Incus provider seam
- `agent_instances` now persists only the resolved Incus config needed for later status, reset, recover, restore, and launch paths
- the first Incus dev lane is now real for create, status, delete, and an image-backed guest boot path
- the Incus dev path currently requires explicit endpoint, project, profile, storage-pool, and image-alias config, and it models each managed environment as one disposable VM root plus one attached persistent custom volume mounted at `/mnt/pika-state`
- the first managed-agent Incus guest image is Nix-built and imported as a VM image artifact rather than assembled from host-local runner directories
- the canonical `pika-build` host now runs both the existing microVM host stack and the Incus dev lane side by side; it still needs operator one-time setup for the Incus bridge, storage pool, project, and profile before request-scoped Incus provisioning can work
- the canonical `pika-build` host now also carries only the narrow Incus bridge firewall/input/forward allowances required for guest DHCP, DNS, and outbound relay access; host-only services remain behind the normal host ingress policy instead of trusting all traffic from `incusbr0`
- the provider now supports trusted TLS client-certificate auth for remote `pika-server -> pika-build:8443` mutations via server-side cert/key path config; the repo-managed `pika-server` Nix module can now inject that canary env and sops-backed cert/key paths for a normal deployed canary
- Incus readiness now comes from inside the guest via the Incus guest file API against `/run/pika-cloud/status.json`; `guest_ready=true` is only reported when that lifecycle snapshot exists and validates
- the first authenticated end-to-end canary now reaches `state=ready` and `startup_phase=ready` for a fresh request-scoped Incus provision against the canonical `pika-build` host
- Incus backup status, recover, and restore now use a first thin Incus-native operational model:
  state durability lives in the attached custom volume, backup status is the freshness of the latest
  state-volume snapshot, recover starts or restarts the current appliance around that volume, and
  restore rolls the state volume back to its latest snapshot before starting the appliance again
- automatic state-volume snapshot creation policy and operator-selected restore points are still deferred
- the internal coworker dashboard path is now Incus-only:
  `/dashboard` provision, recover, reset, launch, and same-origin OpenClaw proxying all route
  through the Incus provider seam rather than the default backend
- the Incus OpenClaw UI path currently uses Incus-managed host proxy ports on `pika-build`
- the Incus OpenClaw dashboard path now requires an explicit proxy-host IPv4 in config rather than
  deriving it from the Incus API endpoint, which keeps tunneled and split-horizon control-plane
  access from corrupting the user-facing proxy target
- the provider now allocates OpenClaw guest IPv4s and host proxy ports by scanning live project
  instances for collisions instead of relying on a hash-derived static assignment
- Incus readiness now rejects stale markers from a previous boot by requiring the ready marker
  `boot_id` to match the guest's current `/proc/sys/kernel/random/boot_id`
- the Incus guest image now explicitly opens the OpenClaw gateway port inside the guest so the
  host-side Incus proxy device can reach the gateway over the guest VM IP
- the internal product path is now intentionally Incus + OpenClaw only; any remaining microVM work
  is outside the managed-agent product path

### Lessons From Real Dogfooding

- reset across provider migration must destroy the old environment via the stored row provider
  first, then reprovision using the new requested policy; otherwise the control plane tries to
  interpret an old row through the wrong substrate
- Incus mutating APIs may return either sync or async responses for the same logical operation, so
  the client must accept both shapes instead of assuming one response mode
- the current internal OpenClaw ingress works by combining:
  - an Incus-managed host proxy port on `pika-build`
  - a guest-side OpenClaw gateway listener
  - guest firewall allowance for that gateway port
- that ingress shape is tactical, not the desired long-term product shape; it works today, but it
  still mutates per-instance proxy devices on demand and should likely be simplified later
- guest secret injection exists today for the internal lane and currently rides the guest bootstrap
  path; it needs a more deliberate long-term model for provenance, rotation, and auditability
- substrate provider selection and guest runtime selection are separate concepts; the old
  `microvm` naming leaked those together and made the internal contract harder to reason about
- after dogfooding the internal lane, we intentionally removed the managed-agent microVM / Pi / ACP
  compatibility layers instead of continuing to pay that tax in product-facing code
- the customer dashboard path is now Incus-only internally, while any remaining microVM work lives
  outside the managed-agent product path
- the final hard cut also removes the live legacy microVM teardown bridge from `pika-server`; old
  managed-agent microVM rows and VMs must now be cleaned up manually before deploy instead of
  staying in the live request path forever

### One-Time Hard-Cut Cleanup

Before deploying the schema cut that removes `agent_instances.provider`, operators should:

1. On the pre-cut schema, query `agent_instances` on `pika-server` for rows where
   `provider='microvm'` and `phase in ('creating', 'ready')`.
2. Record the matching `owner_npub` and `vm_id` values.
3. Delete those matching legacy VMs on `pika-build`:
   - if `vm-spawner` still knows about the VM, use its delete path
   - if `vm-spawner` returns `404`, stop any lingering `microvm@<vm_id>.service` unit and remove
     `/var/lib/microvms/<vm_id>` directly
4. Mark those DB rows `phase='error'` with `vm_id=NULL`, or delete them outright.
5. Deploy the hard cut only after that query returns zero active legacy rows.
6. After the migration lands, verify that `agent_instances.provider` is gone and
   `agent_instances.incus_config` is the surviving config column.

This is intentionally destructive. The surviving managed-agent product path is fresh
reprovision-on-Incus, not perpetual mixed-substrate compatibility.

### Focused Simplification Debt

The next high-signal simplification targets are:

- simplify Incus OpenClaw ingress so less instance mutation happens on demand and the proxy target
  is more static and obvious
- decide on a deliberate guest secret injection model instead of growing more bootstrap-time env
  stamping ad hoc
- automate snapshot creation policy for the Incus state volume so recovery-point protection is not
  purely operator-managed

### 2. Guest Image Pipeline

We need a new image pipeline for managed agents.

It should:

- keep guest composition Nix-driven
- build a bootable VM image suitable for Incus
- include the platform-managed base services
- include agent bootstrap tooling and update hooks
- define a clear way to inject customer-specific or agent-specific startup configuration at create
  time

The important change is that the runtime artifact becomes an image plus attached storage and
metadata, not a `microvm.nix` runner directory.

### 3. Durable Storage Model

The durable home contract must move away from host-local `/var/lib/microvms/<vm_id>/home`.

We need:

- one durable volume per customer VM, or another similarly strong isolated storage unit
- a clear mapping between a Pika `CustomerVm` and its storage object
- snapshot and restore procedures that operate on Incus storage primitives
- backup policy that can be implemented either with Incus-native backups, external snapshots, or a
  hybrid model

The storage identity must survive VM recreation on a different host.

The preferred shape is:

- one persistent customer volume
- one disposable root image

That keeps the recovery model simple and aligns with the immutable appliance approach.

Current first-pass Incus operational model:

- backup unit = the persistent custom volume
- recovery point = the latest snapshot on that volume
- recover = start or restart the current instance around the same volume
- restore = stop the current instance if needed, restore the volume to the latest snapshot, then
  start the appliance again

This is intentionally opinionated and thin. It does not attempt to preserve the old microVM
"durable home on a specific host" mental model, and it does not yet automate snapshot creation.

### 4. Network And Access Model

We need an Incus-native network design for:

- outbound network access for guest agents
- private control-plane reachability
- user-facing UI access where needed
- host-to-guest health checks
- future multi-host and multi-zone growth

We should avoid recreating the current "one host bridge plus one private spawner port" design in a
more complicated form.

### 5. Product-Level Status And Recovery Model

We need to map Pika product actions to Incus operations:

- create customer VM
- start or restart customer VM
- recover after guest failure
- recover after host failure
- restore from snapshot or backup
- delete customer VM and durable state

The product-level meanings of "recover", "restore", "ready", and "launchable" should stay in Pika.
Incus should supply the lower-level primitives, not define our product vocabulary.

### 6. OpenClaw And Built-In UI Integration

The internal dashboard path no longer depends on the spawner-specific customer flow.

The first Incus dashboard implementation now does this:

- OpenClaw launch auth is read from the guest OpenClaw config through the Incus guest file API
- the same-origin proxy targets an Incus-managed host endpoint for the guest gateway instead of a
  spawner URL
- the dashboard only advertises launchability for ready Incus-backed OpenClaw rows

What still remains open:

- hardening the least-privilege network exposure for those Incus-managed host proxy ports
- deciding whether the long-term target should keep host proxy ports or move to a different
  Incus-native ingress shape
- retrieve it through a guest-facing control endpoint owned by the platform

We should not preserve the old host-filesystem coupling.

### 7. Operations And Observability

We need operational support for:

- instance events and failure visibility
- guest readiness signals
- backup visibility
- storage usage monitoring
- fleet capacity monitoring
- debug and emergency operator workflows

Incus gives us infrastructure events and operations, but we still need Pika-specific observability
for agent-level behavior and customer support flows.

## Migration Strategy

The migration should happen in stages, but the stages are planning structure rather than a rigid
contract.

The main rule is: migrate the managed agent platform first, keep `pikaci` on the old stack until
the new agent platform is stable, then migrate `pikaci` deliberately.

### Phase 0: Lock Vocabulary And Scope

Before major implementation work:

- write down the target architecture
- distinguish product control plane from infrastructure control plane
- stop expanding the current `vm-spawner` scope
- start introducing provider-neutral vocabulary where reasonable

This document is part of that phase.

### Phase 1: Stand Up Incus Fleet Foundations

Build the basic infrastructure substrate first.

This phase should produce:

- an Incus cluster bootstrap story in infra
- storage pool definitions
- network definitions
- base project strategy
- cluster-group strategy if we need hardware classes or staged rollout groups
- secure API access for Pika components

This is where we validate that the operational model is acceptable before porting application
logic.

If this phase exposes surprising constraints, we should update the plan before pushing deeper into
application migration.

### Phase 2: Build The Managed Guest Image Model

Create the first Incus-ready managed guest image.

This image should include:

- the platform base
- guest bootstrap logic
- agent runtime prerequisites
- update and observability hooks
- a defined convention for attaching persistent customer storage

This phase should deliberately avoid customer-facing provisioning until we are confident in the base
image lifecycle.

What we learned from the first real dev lane:

- the image must enable both cloud-init and the Incus guest agent or the control plane cannot inject bootstrap or fetch readiness conservatively
- the current managed-agent bootstrap bundle can be reused in Incus, but the image must provide the runtime prerequisites that the old microVM host image used to guarantee implicitly
- the durable-volume contract is simplest when the guest keeps the existing runtime paths and re-homes them onto `/mnt/pika-state` before the managed-agent service starts
- the Incus guest image must install GRUB in EFI-only mode for this qcow2 lane; BIOS-style `/dev/vda` GRUB installation panics the build VM during image creation

### Phase 3: Introduce An Incus Provider Adapter

Build a new provider layer that can:

- create a VM from the managed image
- attach the durable volume
- apply placement or project rules
- start, stop, restart, and delete instances
- observe instance status
- trigger snapshot and restore workflows

This layer can be a new service or a library integration point, but it should be thin.

The important rule is:

- it should adapt Pika semantics to Incus
- it should not become a second custom cloud control plane

### Phase 4: Port The Managed Agent Lifecycle

Switch the managed agent product path over to the Incus provider.

This phase includes:

- provisioning a managed customer VM through Incus
- storing the new VM and volume identity in Pika
- mapping startup and ready states to the new backend
- updating recover, restore, and delete flows
- porting OpenClaw launch and proxy behavior
- preserving the existing product-level API semantics as much as possible

At the end of this phase, newly provisioned managed agents should no longer depend on
`microvm.nix`.

Current validation shape:

- `pika-build` is the first real dev target for the Incus lane via the canonical builder host config plus an operator-run image import step
- the `pika-build` role in this phase is the Incus substrate; the agent API still comes from a `pika-server` process pointed at that Incus endpoint
- `pika-server` now runs the managed-agent product path as Incus + OpenClaw only
- the concrete operator path for this phase lives in `docs/incus-dev-lane.md`
- internal dashboard validation now reaches real create plus ready plus OpenClaw launch/proxy on Incus; delete is still validated through the provider seam and the existing dashboard reset path because there is not yet a public v1 delete endpoint
- remaining microVM infrastructure work belongs to separate surfaces such as `pikaci`, not the managed-agent product path

### Phase 5: Backups, Restore, And Day-2 Operations

After basic lifecycle works, finish the operational story.

This phase should cover:

- backup creation and retention
- restore workflows
- host failure handling
- operator runbooks
- monitoring and alerting
- quota and capacity policies

This phase is required before broad production rollout.

### Phase 6: Canary And Controlled Cutover

Migrate real workloads in a controlled way.

Suggested sequence:

- create only new low-risk managed environments on Incus
- run internal and canary customer environments there first
- validate recovery, backup, and UI workflows
- then shift general provisioning to Incus

During this phase, the old microVM path may remain available only for existing environments.

### Phase 7: Remove The Old Managed-Agent MicroVM Path

Once all managed environments are off the old runtime:

- stop provisioning managed agents through `vm-spawner`
- remove the old managed-agent host assumptions
- remove `microvm.nix` agent-host integration
- archive or delete the old spawner-specific code paths

This phase should not happen until the Incus path has proven backup, recovery, and operational
stability.

## Implementation Cadence

We should execute this migration in small reviewed chunks.

Good chunk boundaries include:

- one provider-contract cleanup
- one guest-image milestone
- one lifecycle operation port
- one dashboard or OpenClaw migration step
- one backup or restore milestone
- one infra bootstrap milestone

Each chunk should end with:

- code landed or explicitly abandoned
- review completed
- follow-up gaps captured
- this document updated if our understanding changed

The point is to stay agile without losing architectural coherence.

## Migration Work By Area

This section groups the code and platform work by responsibility.

### `pika-server`

`pika-server` should stay authoritative, but it needs backend changes.

Required work:

- replace direct assumptions about a private microVM spawner URL
- adopt provider-neutral VM lifecycle contracts
- store Incus-backed instance and storage identity
- update status refresh logic
- update recover, restore, and delete flows
- update UI launch and proxy plumbing

The goal is to keep the user-facing product shape stable while changing the infrastructure backend.

### Current `vm-spawner`

`vm-spawner` should not be expanded into a clustered fleet manager.

Possible end states:

- delete it entirely and talk to Incus directly from Pika components
- keep a very thin internal adapter that only translates Pika requests into Incus API calls

The preferred direction is the thinner option that avoids recreating a cloud control plane.

### Infra

Infra work must shift from single-host microVM setup to cluster bootstrap and operation.

Required work:

- Incus cluster bring-up
- storage pool decisions
- network design
- certificate or token management for Pika-to-Incus access
- node classes and placement policy
- backups and disaster-recovery plans

This is a meaningful infra project, but it is much smaller than inventing our own fleet manager.

### Guest Runtime

Guest runtime work moves from runner directories to image-based provisioning.

Required work:

- managed NixOS guest image definition
- customer volume mount conventions
- guest bootstrap on first start
- health and ready signaling
- update agent and observability hooks

We should define a stable guest contract early so the backend and product layers are not tightly
coupled to ad hoc filesystem assumptions.

## Risks And Open Questions

The migration has real tradeoffs.

### Density And Cost

Incus VMs are not the same data plane as the current microVM-oriented path. We need to benchmark:

- boot time
- steady-state memory overhead
- storage overhead
- density per host
- recovery speed

If the economics only work with extremely dense microVM semantics, we need to know that early.

### Storage Model

We need to decide the exact durable storage model:

- Incus-native backup and snapshot only
- external backup layered on top of Incus storage volumes
- a hybrid model

This decision affects restore semantics and disaster recovery.

### Control Path For Guest Metadata

The old model could inspect guest-adjacent files on the host. The new model should not depend on
that. We need a clean way to handle:

- launch metadata
- guest readiness
- template-specific service credentials
- operator inspection workflows

### Migration Of Existing Managed Environments

We need to decide whether existing environments:

- stay on the old backend until deleted
- get a one-time migration path
- get recreated on Incus with state copied over

This should be decided based on operational simplicity, not aesthetic purity.

## Decision Gates Before Broad Rollout

Before broad production rollout of the managed-agent path, we should be able to answer "yes" to
all of the following:

- can we create a customer VM from a versioned image and attach its persistent volume reliably
- can we rebuild that VM on another host without relying on host-local state
- can we restore customer state from snapshot or backup in a repeatable way
- can we explain the guest secret and identity lifecycle clearly
- do OpenClaw and future templates work without spawner-era filesystem coupling
- are density and cost within acceptable bounds
- do operator workflows for debug, recovery, and incident response feel simpler than the old path

If the answer is "no" for any of these, the plan should be updated before rollout continues.

## Success Criteria For The Managed-Agent Migration

We can consider the managed-agent migration successful when:

- new customer VMs provision through Incus only
- a customer VM can be restarted or recovered on a different host without host-local state tricks
- backups and restore work in the new model
- the Pika product API and dashboard remain coherent
- OpenClaw and future templates launch correctly
- `vm-spawner` is no longer needed for the managed agent product

## `pikaci` Migration

`pikaci` should be migrated after the managed agent platform is stable on Incus.

It deserves its own section because its current architecture is significantly more coupled to
`microvm.nix`.

### Why `pikaci` Is Different

Today `pikaci` depends on:

- `microvm.declaredRunner`
- remote runner materialization
- direct `microvm-run` execution
- `virtiofs` shares for source, artifacts, and caches

The Incus migration for `pikaci` therefore requires a new execution backend rather than a simple
provider swap.

### Target `pikaci` End State

The end state should be:

- an Incus-backed Linux execution backend
- image-based runner provisioning rather than runner-directory provisioning
- explicit artifact and cache transport contracts
- a clean way to attach workspace snapshots and collect outputs

The backend should preserve the product goals of `pikaci` while dropping `microvm.nix`-specific
runtime assumptions.

### `pikaci` Migration Phases

#### Phase A: Define The New Execution Contract

Before writing code, define:

- how a job image is selected
- how the workspace snapshot reaches the guest
- how caches are shared or restored
- how logs and result artifacts come back
- how remote execution timeout and cancellation work

The first implementation chunk should land a thin `remote Linux VM` seam before any real Incus
backend exists:

- keep the current remote `microvm` path as one backend behind that seam
- move backend selection to an explicit `remote Linux VM backend` concept rather than
  `microvm_remote`
- keep backend responsibilities small: prepare dirs, stage snapshot, prepare runtime, launch,
  wait, collect artifacts
- record per-phase timing boundaries so later Incus chunks can benchmark prepare, launch, wait,
  and artifact collection without another schema change

#### Phase B: Build An Incus Executor Backend

Implement a new `pikaci` executor that can:

- create or reuse an Incus VM
- mount the prepared workspace snapshot
- execute the guest workload
- collect artifacts and logs
- tear down or recycle the instance

Current `pika-build` proof status:

- Incus now defaults to the staged pre-merge Linux path on `pika-build` when
  `PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST` points at `pika-build` or `localhost`;
  `PIKACI_REMOTE_LINUX_VM_BACKEND=incus|microvm|auto` is now the only supported
  operator override for backend selection
- the steady-state backend shape is now one shared-mount Incus path rather than a mode split:
  sync the snapshot to `pika-build`, share the snapshot plus staged Linux Rust outputs into the
  guest as readonly `virtiofs` mounts at their final guest paths, keep `/artifacts`,
  `/cargo-home`, and `/cargo-target` guest-local and writable, run the staged wrapper, collect
  artifacts, and delete the VM
- the guest bootstrap contract now lives in the Incus image:
  the image owns the mounted-path layout and
  `/run/current-system/sw/bin/pikaci-incus-run` owns the guest env/log/result contract, so
  `executor.rs` no longer synthesizes those bash launchers per job
- the Incus executor now preserves the staged-job read-only workspace contract and persists remote
  backend phase metadata even when an Incus execution fails during runtime bring-up
- running `pikaci` on `pika-build` itself still needs a localhost fast path instead of SSH for the
  remote work-dir seam, because self-SSH is not guaranteed there
- validated successful Incus runs on `pika-build` now include:
  `pika-actionlint`, `pika-doc-contracts`, `pika-rust-deps-hygiene`, `pre-merge-pika-rust`,
  `pre-merge-notifications`, `pre-merge-fixture-rust`, `pre-merge-rmp`,
  `pre-merge-pikachat-rust`, and the full `pre-merge-pika-followup` target
  (`pika-android-test-compile`, `pikachat-build`, `pika-desktop-check`, `pika-actionlint`,
  `pika-doc-contracts`, `pika-rust-deps-hygiene`)
- `run.json` now records prepare-node timing outside the remote-execution seam, so comparisons can
  separate total wall time from pre-execution prepare work and executor-local phases
- current measurements on `pika-build` are:
  - `pika-actionlint` fast-path Incus: about `32s` wall, about `7.9s` pre-execution prepare, about
    `22.2s` Incus `prepare_runtime`
  - `pika-actionlint` microVM: about `21s` wall, about `14.9s` pre-execution prepare, about `5.5s`
    guest wait time, so the current smallest-lane comparison no longer supports the old claim that
    Incus is simply faster than microVM on this host
  - `pika-doc-contracts` fast-path Incus: about `40s` wall, about `8.9s` pre-execution prepare,
    about `28.6s` Incus `prepare_runtime`
  - `pika-rust-deps-hygiene` fast-path Incus: about `49s` wall, about `14.1s` pre-execution
    prepare, about `31.7s` Incus `prepare_runtime`
  - full `pre-merge-pika-followup` fast-path Incus target: about `95s` wall for six passing jobs,
    with about `17.0s` total shared prepare time before execution and per-job Incus
    `prepare_runtime` mostly in the `22s` to `28s` range
- the old staged workspace-deps blockers are now fixed at the shared reduced-workspace layer:
  the reduced `pika-core` and `rmp` lockfiles were stale, the reduced `pika-core` source omitted
  `tests/support` and `config/channels.json`, notifications needed the same bindgen environment as
  the other desktop-linked lanes, the localhost Incus fast path needed to repoint staged-output
  symlinks, and the follow-up `cargo machete` check needed to ignore CI fixture manifests under
  `nix/ci`
- the staged Linux runtime no longer depends on a mounted host `/nix/store` seam:
  the Incus image now carries the runtime libraries the staged Linux lanes actually need,
  staged wrappers use guest-local interpreters like `/run/current-system/sw/bin/bash` and
  `/run/current-system/sw/bin/node`, and the runtime env now uses guest-local Nix package paths
  directly instead of rewriting `/nix/store` through `PIKACI_STAGED_HOST_NIX_STORE_ROOT`
- that cleanup needed one follow-up fix in validation:
  the initial guest-local `run-rmp-init-smoke-ci` wrapper was materialized with a non-executable
  mode because its shebang was not preserved as a real executable script in the realized store;
  materializing the wrapper via a standalone executable text artifact fixed that, and using the
  guest's real `${pkgs.postgresql}/bin` path restored `initdb` so the notifications lane stopped
  relying on a mounted host-store path for Postgres share data
- the staged `pre-merge-rmp` parity blocker is now fixed end-to-end: the reduced RMP workspace now
  mirrors the generated template dependency surface closely enough for offline Cargo vendor checks,
  and the default Incus path reaches a passing `rmp-init-smoke-ci`
- the remaining Incus parity picture is now closed out:
  - `pre-merge-agent-contracts` is no longer an Incus-default exception:
    the stale host-side deterministic HTTP selectors were removed from the lane
    explicitly and documented as manual-only until they are rewritten against
    the surviving Incus/OpenClaw contract, the local `just pre-merge-agent-contracts`
    recipe now includes `pika-agent-control-plane` just like the staged lane,
    the surviving staged Rust provider-contract surface now passes on Incus on
    `pika-build`, and the lane rides the normal Incus default again
  - `pre-merge-pikachat-openclaw-e2e` now passes on the normal default Incus
    path on `pika-build`:
    the staged scenario no longer loads the plugin through a symlink whose
    canonical manifest path resolves back under `/mnt/pikaci-workspace-build`,
    the packaged OpenClaw E2E tree keeps its bundled `extensions` directory
    empty so the gateway does not auto-discover a duplicate plugin id, and the
    scenario's bounded `/health` wait now covers the measured packaged cold
    start on Incus
    the remaining inner startup cost was not another plugin safety error:
    directly loading the runtime-local copied `pikachat-openclaw` plugin with
    the same Jiti stack the packaged gateway uses took about `66s` in the guest,
    and a matching preserved-state repro showed the gateway then bound `/health`
    and launched the `pikachat` daemon normally
- because of that, the current migration read is:
  staged pre-merge Linux on `pika-build` now defaults to Incus with no
  remaining automatic Incus exclusions in the path discussed in this plan
  and with the guest runtime owned by the Incus image plus staged payloads,
  not by an executor-mounted host `/nix/store`
- staged Linux payloads now also declare their guest mount contract in
  `share/pikaci/payload-manifest.json`, so the Incus executor no longer
  hardcodes the staged workspace payload mount layout beyond the snapshot root
- `pika-git` no longer has to collapse staged `pikaci` runs back into an
  ephemeral temp-worktree `.pikaci` tree:
  structured staged runs can now be pointed at a service-owned persistent
  state root, and the forge API can reload the resulting `RunRecord`,
  per-job log metadata, prepared-output realizations, and host/guest log
  content by `pikaci_run_id` after the temporary source worktree is removed
- staged Linux runs now expose their artifact contract machine-readably:
  `RunRecord` keeps the prepared-output record path, each remote Linux VM job
  keeps explicit Incus image identity (`project`, `alias`, `fingerprint`), and
  both surfaces can be reloaded through `pikaci` and `pika-git` by run id

#### Phase C: Validate Performance And Developer Experience

Benchmark the new backend against the current one for:

- startup time
- throughput
- cache effectiveness
- artifact handling
- failure ergonomics

`pikaci` should not switch permanently until the new backend is operationally acceptable.

#### Phase D: Cut Over And Remove `microvm.nix`

After validation:

- switch staged Linux execution to the Incus backend
- remove `microvm.nix` runner materialization logic from `pikaci`
- delete the remaining `microvm.nix` dependency only after all consumers are gone

## Final End State

The final intended end state is:

- the managed agent platform runs on Incus-backed VMs across a fleet
- `pika-server` remains the product control plane
- `pikaci` also uses an Incus-backed execution model
- `microvm.nix` is no longer a runtime dependency of Pika

This gets us what we actually want:

- strong isolation
- easier fleet operations
- less undifferentiated infrastructure work
- a platform we can scale without quietly becoming a cloud vendor
