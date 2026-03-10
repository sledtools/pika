# Agents Platform

This is a living product and architecture document for a managed multi-agent platform built on
PikaChat and microVMs.

It is intentionally not an implementation checklist. The goal is to capture the current shape of
the product, the boundaries we want to preserve, and the requirements that future implementation
prompts should satisfy. This document should evolve as we learn.

## Role Of This Document

This document is a north-star requirements and architecture document.

It is meant to:

- preserve the intended product shape
- clarify trust boundaries and platform boundaries
- give future implementation prompts a consistent frame of reference

It is not meant to:

- prescribe the exact order of implementation
- override current prerequisite work already underway elsewhere in the repo
- pretend we already know every operational detail

In particular, this document should be treated as a directional companion to ongoing runtime and
`pika_core` cleanup work, not as a signal that the entire platform should immediately become the
top implementation priority.

## Product Direction

We want to build a managed platform at `agents.pikachat.org` where a user can purchase an agent
runtime for a limited period (for example, one month), have it provisioned automatically, and
interact with it through PikaChat and a web dashboard.

The initial product should optimize for:

- simple operations
- clear user mental model
- strong platform control
- room to support multiple agent harnesses
- low long-term maintenance burden

The first version should prioritize the managed experience, not maximum power-user flexibility.

## Current MVP Bias

The current bias for the first shipped version is:

- managed VM first
- one customer per VM
- design for multiple agents per VM from the beginning
- preserve the ability to support multiple templates cleanly
- keep the first shipped surface narrower than the full long-term vision

This means the architecture should be ready for multiple agents per VM, but the first shipped
product does not need to expose every possible capability on day one.

It is acceptable for the MVP to begin with:

- one primary user-visible agent template first
- a limited subset of dashboard actions
- a constrained customization surface

as long as those decisions do not paint us into the wrong long-term shape.

## Core Model

The intended tenancy model is:

- one customer gets one VM
- a customer may run multiple agents inside that VM
- agents within the same customer VM are allowed to collaborate and share selected files
- different customers must never share a VM

The VM is the main trust and billing boundary.

Inside the VM:

- each agent should have its own service identity
- each agent should have its own Unix user and private state directory
- agents should collaborate through explicit shared directories or read-only export directories
- agents should not get blanket write access to each other’s private homes

This supports collaboration without turning the guest into an unstructured shared filesystem.

## Control Plane Vocabulary

The implementation should converge on a small, consistent vocabulary even before the exact schema
is finalized.

Useful terms for the platform are:

- `CustomerVm`
  The managed VM assigned to one customer. This is the main trust, billing, and recovery boundary.

- `AgentInstance`
  One runnable agent inside a customer VM. It has a template, identity, service unit, state, and
  sharing policy.

- `AgentTemplate`
  A runtime family or harness template, such as OpenClaw, NanoClaw, IronClaw, or Pi.

- `Generation`
  A deployable composed configuration for a managed VM, including platform base plus customer and
  agent-layer configuration.

- `UiLaunchTicket`
  A short-lived platform-issued credential used to open a built-in agent UI on its own origin.

- `ManagedMode`
  The current control posture of a VM, such as fully managed by the platform versus a future
  ejected or self-directed mode.

These names are directional and may evolve, but the underlying concepts should remain stable.

## Agent Identity

Each agent should have separate concepts for:

- `template`
- `display_name`
- `slug`
- stable internal `agent_id`

Users should be able to run multiple agents of the same template, including multiple OpenClaw
instances in the same VM.

The display name is user-facing and may contain spaces or friendly labels. The slug and service
identity should be normalized and safe for Unix usernames, filesystem paths, and systemd units.

## Template Model

The platform should not be tied to OpenClaw alone.

We should be able to support multiple harness templates, such as:

- OpenClaw
- NanoClaw
- IronClaw
- Pi

The platform should treat these as templates or runtime families, not as separate products.

`pikachatd` or the equivalent shared PikaChat runtime should remain the stable messaging and
control boundary where possible, while the harness inside the guest is template-specific.

## Managed Platform Boundary

The default offering should be a managed VM, not a fully user-owned server.

We should explicitly separate:

- platform-managed base layer
- customer-managed layer
- agent-managed layer

### Platform-Managed Base

This layer should remain under our control.

It should own things like:

- networking
- observability
- backups and snapshots
- update agent
- recovery path
- secrets plumbing
- `pikachatd`
- root-owned supervisor units
- reverse proxy / UI gateway

Users and agents should not be able to break these pieces in the default managed mode.

### Customer / Agent Customization

The first shipped customization model should be constrained.

The likely default should be:

- per-agent Home Manager config
- per-agent packages and tools
- user services and timers
- shared workspace configuration
- selected typed extension points exposed by the platform

The default managed mode should not allow arbitrary root-owned NixOS mutation.

## Updates and Drift Control

The platform must remain updateable.

That means we cannot treat a VM as an opaque mutable snowflake where agents can arbitrarily rewrite
the whole operating system.

The intended composition is:

- pinned platform base
- customer-level config
- agent-level config

Platform updates should be rolled out by rebuilding a composed configuration on a newer base
revision, with rollback support.

This implies:

- the platform base must stay under our control
- user customization must be layered rather than replacing the base
- observability and recovery must survive user customization in managed mode

## Future Eject Mode

A future `eject` feature is desirable.

This would allow a user to opt out of managed constraints and take full control of their VM,
including arbitrary NixOS changes, with the understanding that:

- the VM may become unsupported
- platform updates may stop or become best-effort only
- observability and recovery guarantees may be reduced
- the user can break the machine

The default product should not start here.

If `eject` exists later, it should be explicit, one-way enough to be meaningful, and ideally tied
to snapshot/restore behavior so the user can return to a managed state by restoring a known-good
generation or migrating state into a fresh managed VM.

## Web Product

We need a new web interface for this platform.

This UI should serve two roles:

- control-plane dashboard for the managed VM and its agents
- launch surface for built-in agent UIs

The dashboard should feel live and operational, not like a static admin page.

## Authentication

The initial authentication method should be Nostr login.

For MVP:

- use Nostr challenge/verify login
- prefer a proper server-issued session cookie after verification
- avoid browser-only bearer token patterns as the primary auth model for this product

Later we may want to support:

- Nostr Connect / bunker flows
- better mobile login UX

## Routing and Origins

The control-plane dashboard and built-in agent UIs should not share the same origin.

Intended shape:

- `agents.pikachat.org` for the main dashboard
- separate subdomains for proxied built-in agent UIs

We should avoid serving third-party or template-provided UIs under the same origin as the main
dashboard. Those UIs may contain arbitrary JavaScript and should not share session scope with the
platform app.

A likely model is:

- dashboard issues a short-lived launch ticket
- user is redirected to an agent-specific UI subdomain
- that subdomain exchanges the ticket for a scoped session and proxies to the guest-local UI

## MVP Dashboard Requirements

The MVP should cover the core managed experience, not every future feature.

The first useful dashboard likely needs:

- landing dashboard for the customer VM
- list of agents in the VM
- per-agent status and lifecycle controls
- “add agent” flow
- template selection
- display name / slug preview
- logs and recent activity
- basic update status
- access to built-in agent UIs

It should also expose enough state to make the system operable:

- VM status
- service health
- last deployment/update result
- recent errors

## UI Technology Direction

For MVP, we should favor a simple server-rendered web app with a small amount of live behavior.

Current preferred direction:

- Rust backend
- SSR templates
- small JavaScript layer
- SSE for live dashboard updates where needed

This is favored over starting with a SPA because:

- the initial product is mostly dashboards, forms, logs, and launch points
- we already have a precedent in `pika-news`
- it reduces frontend build and deployment complexity
- it keeps the first version easy to operate and evolve

A SPA may become justified later if the app grows into a richer control surface with more complex
client-side state, but that should be earned rather than assumed.

## Realtime Requirements

The dashboard should not require page reloads to feel current.

We need some live update mechanism for:

- VM status
- agent service status
- recent activity
- deployment/update progress
- log tails or health events

The initial bias should be toward server-driven updates rather than a heavy frontend state stack.

## Built-In UI Exposure

Many agent templates will have their own built-in web UIs.

The platform should provide a clean way to expose those UIs through the dashboard without forcing
every harness into one shared frontend architecture.

Requirements:

- platform can reverse-proxy to guest-local services
- guest UIs can be opened safely from the dashboard
- guest UIs do not share origin with the control plane
- access should be mediated by platform-issued auth, not open guest ports

## Operational Requirements

The platform should support:

- one VM per customer
- dynamic VM resizing
- multiple agents per VM
- per-agent service lifecycle management
- recoverable deployments
- observability by default
- snapshots/backups
- host-controlled updates

We should bias toward boring operability over maximum flexibility in the first version.

## Security Requirements

Default managed mode should preserve:

- platform-owned observability
- platform-owned update path
- platform-owned recovery path
- per-agent secret isolation
- per-customer VM isolation

Within a customer VM:

- agents may collaborate
- agents should have explicit sharing policy
- shared writable spaces should be deliberate
- cross-agent read access should ideally happen through exported/shared paths, not unrestricted home
  directory access

## PikaChat’s Role

PikaChat should remain central to the platform rather than becoming incidental glue.

The platform should treat PikaChat as the stable messaging and transport layer, while harnesses are
replaceable runtime templates on top of that foundation.

The long-term platform should feel like “managed PikaChat-native agent hosting”, not just generic
VM hosting.

## What We Are Not Deciding Yet

This document does not yet lock in:

- exact control-plane schema
- exact billing implementation
- exact Nix module topology
- exact auth token format
- exact guest reverse-proxy implementation
- exact update rollout machinery
- exact harness integration model for every template

Those should emerge incrementally from implementation and can be added here as they solidify.

## Current Biases

At this moment, the main directional bets are:

- managed VMs first
- one customer per VM
- multiple agents per VM
- one Unix user and service identity per agent
- private agent homes plus explicit shared/export paths
- platform-managed base plus constrained customization
- future eject mode, but not default
- SSR plus lightweight realtime, not SPA-first
- Nostr login first
- separate origins for dashboard vs built-in agent UIs

## Open Questions

Important open questions include:

- how broad the first safe customization surface should be
- whether Home Manager alone is enough for v1 or whether we need typed NixOS extension points
- how agent-to-agent file sharing should be modeled in detail
- how much of the built-in UI proxy/auth model should exist in v1
- what the first supported template set should be
- how updates should interact with user customizations in managed mode
- how mobile-friendly the first Nostr login flow needs to be
- what the right observability surface is for end users versus operators

This document should be updated whenever one of those questions becomes meaningfully more settled.
