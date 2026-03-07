---
summary: Frozen target model for the simplified microVM launcher.
read_when:
  - simplifying launcher runtime variants or request fields
  - reviewing pika-server versus vm-spawner lifecycle ownership
---

# MicroVM Launcher Target

This document freezes the supported launcher product before simplification work changes code.

## Retained runtime path

The retained runtime path is a durable personal-agent appliance with exactly these inputs and recovery rules:

- host-built runner
- host-backed persistent `/root`
- guest autostart payload provided by `pika-server`
- recover by reboot first, then recreate with the same durable home if reboot fails

No request-time launcher mode switching is part of the supported runtime path.

## Rejected product paths

The launcher is not a generic remote dev VM or SSH-first shell host.

- no parallel launcher modes
- no request-time host/guest runtime-shape selection

## Lifecycle authority

`pika-server` is the lifecycle authority for owner-to-agent-to-VM mapping, desired phase, and policy.
`vm-spawner` is only a private privileged adapter for create, recover, delete, and health checks.

- app-visible phase truth comes from `pika-server` records, not from querying `vm-spawner`
- owner lookup and agent-to-VM mapping stay in `pika-server`, not in spawner-managed state
- `vm-spawner` is not consulted for authoritative enumeration or admission policy decisions

## Migration compatibility

During migration, `vm-spawner` keeps a temporary compatibility reader for already-created VMs so `recover` and `delete` remain safe across upgrades.

- deterministic IDs in the current pool use vm-id-derived host layout only when runtime metadata explicitly marks the deterministic layout, or when no legacy network metadata remains
- older IDs (including out-of-pool `vm-xxxxxxxx` and non-production legacy IDs) are loaded from persisted runtime metadata (`metadata/runtime.env`, `metadata/env`, and legacy `vm.json` fallback)
- compatibility metadata is used only to recover host boot inputs (tap, IP, CPU, memory), never as control-plane authority

Compatibility removal point:

- remove this fallback after one full release cycle where production telemetry and host scans show no remaining legacy IDs or legacy metadata-dependent VMs
- when removed, keep deterministic vm-id-only loading and delete the legacy metadata parser/tests in the same change

## Operations

Durable state is the VM home directory:

- `/var/lib/microvms/<vm_id>/home`

Operational guidance:

- backup/restore this durable home path as the primary asset
- treat tap/gcroot wiring and runtime network metadata as reconstructible launcher state
- treat guest autostart payload under `metadata/` as required boot input for recreate; `recover` does not rebuild it from `vm_id` alone

## Workstream scope

This branch is reserved for single launcher path simplification.

- start from a clean branch rather than extending cleanup work
- cherry-pick prior fixes only when they are required for the simplification
- keep bug-fix cleanup and architecture simplification as separate review tracks
