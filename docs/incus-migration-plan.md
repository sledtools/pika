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

- `microvm.declaredRunner`
- remote `microvm-run` launch semantics
- `virtiofs` host-shared directories for workspace, artifacts, cargo caches, and staged outputs
- host-side runner preparation and remote synchronization

That means `pikaci` migration is a real backend redesign, not a transport change.

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
- a transitional period where old `microvm` names may remain internally but no new interfaces
  should hard-code the old model

The goal is to prevent the rest of the app from knowing whether the backing provider is
`microvm.nix` or Incus.

Current transition status:

- `ProviderKind` now has both `microvm` and `incus`
- managed-agent request/command contracts can carry provider-neutral selection plus provider-specific params
- the server routes managed-VM lifecycle calls through a thin provider seam, with the existing microVM backend as the default implementation
- new managed-environment rows now persist the chosen provider identity and resolved provider config so later status/recover/launch paths do not drift with process env changes
- the first Incus dev lane is now real for create, status, delete, and an image-backed guest boot path
- the Incus dev path currently requires explicit endpoint, project, profile, storage-pool, and image-alias config, and it models each managed environment as one disposable VM root plus one attached persistent custom volume mounted at `/mnt/pika-state`
- the first managed-agent Incus guest image is Nix-built and imported as a VM image artifact rather than assembled from host-local runner directories
- the canonical `pika-build` host now runs both the existing microVM host stack and the Incus dev lane side by side; it still needs operator one-time setup for the Incus bridge, storage pool, project, and profile before request-scoped Incus provisioning can work
- the canonical `pika-build` host now also carries only the narrow Incus bridge firewall/input/forward allowances required for guest DHCP, DNS, and outbound relay access; host-only services remain behind the normal host ingress policy instead of trusting all traffic from `incusbr0`
- the provider now supports trusted TLS client-certificate auth for remote `pika-server -> pika-build:8443` mutations via server-side cert/key path config; the repo-managed `pika-server` Nix module can now inject that canary env and sops-backed cert/key paths for a normal deployed canary
- Incus readiness now comes from inside the guest via the Incus guest file API against `/workspace/pika-agent/service-ready.json`; `guest_ready=true` is only reported when that marker exists and validates
- the first authenticated end-to-end canary now reaches `state=ready` and `startup_phase=ready` for a fresh request-scoped Incus provision against the canonical `pika-build` host
- Incus backup status, recover, and restore now use a first thin Incus-native operational model:
  state durability lives in the attached custom volume, backup status is the freshness of the latest
  state-volume snapshot, recover starts or restarts the current appliance around that volume, and
  restore rolls the state volume back to its latest snapshot before starting the appliance again
- automatic state-volume snapshot creation policy and operator-selected restore points are still deferred
- OpenClaw launch/proxy behavior remains intentionally unsupported in this phase
- server startup should remain on the microVM default provider for now; request-scoped Incus provisioning is the current safe canary lane until the OpenClaw launch/proxy surface is migrated

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

Today some OpenClaw launch and proxy behavior relies on spawner-specific APIs and host-side access
to guest files.

That needs to change.

The Incus design should give us:

- a clean way to expose the managed UI service
- a way to retrieve or inject launch credentials without reading guest config off the host
- an auth boundary that still belongs to Pika, not directly to Incus

This likely means the platform should either:

- generate and store launch metadata in Pika-controlled state, or
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
- `pika-server` should continue to deploy with `microvm` as the default provider and use explicit request-scoped Incus provisioning for internal canary validation
- the concrete operator path for this phase lives in `docs/incus-dev-lane.md`
- public validation currently reaches real create plus ready; delete is still validated through the provider seam and the existing dashboard reset path because there is not yet a public v1 delete endpoint
- we should not flip `PIKA_AGENT_VM_PROVIDER=incus` globally until the customer-facing OpenClaw launch/proxy path is migrated or explicitly replaced

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

#### Phase B: Build An Incus Executor Backend

Implement a new `pikaci` executor that can:

- create or reuse an Incus VM
- transfer the workspace snapshot
- execute the guest workload
- collect artifacts and logs
- tear down or recycle the instance

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
