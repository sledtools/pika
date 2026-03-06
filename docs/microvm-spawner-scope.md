---
summary: Scope lock for vm-spawner as a private privileged microVM host adapter.
read_when:
  - when changing vm-spawner responsibilities, APIs, or host config
---

# MicroVM Spawner Scope

`pika-server` is the authoritative control plane for agent ownership, `owner -> agent -> vm_id`
mapping, lifecycle phase, and recover or delete decisions.

`vm-spawner` is a private privileged host adapter. Its allowed responsibilities are:

- `POST /vms`: create a microVM using host defaults plus guest autostart payload.
- `POST /vms/:id/recover`: reboot first, then recreate from the same persistent `/root` home if
  reboot fails.
- `DELETE /vms/:id`: stop the unit, remove host-local boot artifacts, and remove the persistent
  home for that `vm_id`.
- `GET /healthz`: report local process health only.

`vm-spawner` explicitly does not own:

- any authoritative lifecycle database or replayable VM registry
- owner lookup, phase tracking, or capacity policy
- public APIs for enumeration, inspection, exec, or informational capacity reporting
- runtime variants other than the durable `prebuilt-cow` path
- guest SSH access, per-VM SSH keys, or session-token registries

Files under `/var/lib/microvms/<vm_id>` are host-local boot inputs only. They may contain guest
metadata, persistent `/root` contents, and runner symlinks, but they are not control-plane truth.

Host naming and network identity are derived from `vm_id` and host defaults so recover and delete
remain correct after spawner restart without loading a spawner-owned registry.
