---
summary: Operator runbook for the first real Incus-backed managed-agent validation lane.
read_when:
  - deploying the Incus dev lane on pika-build
  - importing the managed-agent Incus guest image
  - canarying request-scoped Incus provisioning on pika-server
  - validating managed-agent guest readiness on Incus
---

# Incus Dev Lane Runbook

This runbook describes the first real Incus-backed managed-agent validation lane.

It is intentionally narrow:

- `pika-build` is the first Incus host target
- the guest image is Nix-built and imported into Incus manually
- `pika-build` is the Incus substrate for this lane, not the agent API host
- `pika-server` stays on `microvm` as the default provider
- Incus is exercised through explicit request-scoped provisioning

## What This Lane Proves

This lane is meant to prove:

- the Incus guest image boots as a VM
- the guest accepts the attached persistent volume at `/mnt/pika-state`
- cloud-init bootstrap starts the managed-agent service
- the guest publishes readiness at `/workspace/pika-agent/service-ready.json`
- `pika-server` reads that signal through Incus and reports `guest_ready=true`
- Incus-backed rows keep routing to Incus later through the persisted provider config

It does not yet prove:

- customer-facing OpenClaw launch or proxy flows
- a public API delete flow

This lane now also proves the first Incus operational lifecycle model:

- backup status is derived from snapshots on the persistent custom volume
- recover restarts or starts the current appliance around that same volume
- restore rolls the persistent custom volume back to its latest snapshot, then restarts the appliance

What it still does not yet prove:

- automated snapshot creation policy
- customer-facing OpenClaw launch or proxy flows
- a public API delete flow

## Build The Guest Image

Build the Incus dev image artifact:

```bash
scripts/incus-dev-image.sh build
```

This builds `.#pika-agent-incus-dev-image`, which emits:

- `metadata.tar.xz`
- `disk.qcow2`

The image is a NixOS VM image for the Incus dev lane. It includes:

- `cloud-init`
- the Incus guest agent
- `pikachat`
- the OpenClaw gateway runtime dependency currently used by the managed-agent bootstrap bundle
- the base directories expected by the managed-agent service

## Deploy `pika-build` With Incus Enabled

Deploy the canonical `pika-build` host shape:

```bash
nix develop .#infra -c just -f infra/justfile build-deploy
```

That entrypoint now uploads a clean repo snapshot and runs `nixos-rebuild` on `pika-build`
itself, so operators do not need local `x86_64-linux` build support to deploy the canonical host.

`pika-build` now runs both host roles side by side:

- the existing microVM host stack and `vm-spawner`
- the Incus dev lane with `incusd` listening on `:8443`

The canonical host config now also carries the Incus bridge firewall allowances needed for:

- guest DHCP and DNS on `incusbr0`
- guest egress through the host uplink
- without broadly trusting `incusbr0` for host ingress

Expected host-side prerequisites:

- Incus API reachable on `https://pika-build:8443`
- an Incus project for managed-agent dev work
- an Incus profile for the guest
- an Incus storage pool for instance disks and custom volumes

Current one-time setup is still operator-managed:

```bash
ssh root@pika-build
incus network create incusbr0 ipv4.address=auto ipv4.nat=true ipv6.address=none
incus storage create default dir
incus project create pika-managed-agents
incus --project pika-managed-agents profile create pika-agent-dev
incus --project pika-managed-agents profile device add pika-agent-dev eth0 nic network=incusbr0 name=eth0
```

Notes:

- import the managed-agent image into the same project that `pika-server` will target
- the dev Incus host shape does not create `incusbr0` or the `default` storage pool for you; do
  that in the one-time setup before using the provider
- the provider already injects the root disk and the persistent state disk, so this profile must at
  minimum provide a NIC
- if your Incus host does not use `incusbr0`, replace it with the correct network from `incus network list`
- off-host `pika-server` canaries now require a trusted Incus TLS client certificate

## Trust A `pika-server` Incus Client Certificate

The Incus provider now authenticates to remote `https://pika-build:8443` using a trusted TLS
client certificate.

Generate a client keypair for `pika-server`:

```bash
openssl req -x509 -newkey rsa:4096 -nodes -days 365 \
  -subj '/CN=pika-server-incus-client' \
  -keyout pika-server-incus-client.key \
  -out pika-server-incus-client.crt
```

Trust it on `pika-build`, restricted to the managed-agent project:

```bash
scp pika-server-incus-client.crt root@pika-build:/root/
ssh root@pika-build \
  incus config trust add-certificate /root/pika-server-incus-client.crt \
    --projects pika-managed-agents \
    --restricted
```

For an ad hoc local `pika-server` canary process, set:

- `PIKA_AGENT_INCUS_CLIENT_CERT_PATH`
- `PIKA_AGENT_INCUS_CLIENT_KEY_PATH`
- either `PIKA_AGENT_INCUS_SERVER_CERT_PATH` or `PIKA_AGENT_INCUS_INSECURE_TLS=true`

## Import The Image Into Incus

Import the image artifact onto the remote Incus host:

```bash
scripts/incus-dev-image.sh build-import \
  --remote-host pika-build \
  --project pika-managed-agents \
  --alias pika-agent/dev
```

This copies `metadata.tar.xz` and `disk.qcow2` to the remote host and imports them as the chosen
Incus image alias in the target project.

## `pika-build` Smoke Flow

`pika-build` hosts Incus for this lane, but it does not run `pika-server`.

The smoke API base URL must point at a `pika-server` process that is configured to use
`https://pika-build:8443` as its Incus endpoint. That can be:

- a local branch build of `pika-server`
- a dedicated canary deployment of `pika-server`
- the real `pika-server` host with `microvm` still left as the default provider

The Incus provider expects these settings for request-scoped provisioning:

- `provider=incus`
- Incus endpoint
- Incus project
- Incus profile
- Incus storage pool
- Incus image alias

Use the smoke helper:

```bash
scripts/incus-managed-agent-smoke.sh \
  --api-base-url https://pika-server \
  --nsec '<test nsec>' \
  --incus-endpoint https://pika-build:8443 \
  --incus-project pika-managed-agents \
  --incus-profile pika-agent-dev \
  --incus-storage-pool default \
  --incus-image-alias pika-agent/dev
```

Expected results:

1. The chosen `pika-server` process starts healthy with `microvm` still as the default provider.
2. The explicit Incus provision request succeeds.
3. An Incus VM appears with a deterministic `pika-agent-*` instance name.
4. A matching custom volume named `<vm_id>-state` appears.
5. Inside the guest, bootstrap re-homes managed-agent state onto `/mnt/pika-state`.
6. The guest writes `/workspace/pika-agent/service-ready.json`.
7. `GET /v1/agents/me` transitions to `state=ready` and `startup_phase=ready`.

The first authenticated canary for this flow now succeeds on the canonical `pika-build` host shape.

Operator checks:

```bash
ssh pika-build incus list --project pika-managed-agents
ssh pika-build incus storage volume list default --project pika-managed-agents
ssh pika-build incus file pull --project pika-managed-agents <vm_id>/workspace/pika-agent/service-ready.json -
```

## Incus Operational Lifecycle Model

The first Incus operational model is intentionally narrow and volume-centric.

- backup unit: the persistent custom storage volume attached at `/mnt/pika-state`
- recovery point: an Incus snapshot of that custom volume
- recover: bring the current instance back around the existing state volume by starting or restarting it
- restore: roll the state volume back to its latest snapshot, then start the appliance again

This differs from the old microVM model:

- there is no host-local mutable root to preserve
- there is no host-specific durable-home path as the primary contract
- the appliance root stays disposable
- only the attached state volume is treated as durable product state

Current support:

- `backup-status` reports the freshness of the latest state-volume snapshot
- `recover` starts or restarts the current Incus instance in place
- `restore` restores the latest state-volume snapshot and then restarts the current Incus instance

Current limitations:

- this lane does not yet automate snapshot creation
- restore only uses the latest available snapshot, not an operator-selected one
- if there are no state-volume snapshots yet, `backup-status` reports `missing` and restore is rejected

## `pika-server` Canary Mode

Deploy the code with Incus configuration present, but keep the global default provider unchanged:

- do not set `PIKA_AGENT_VM_PROVIDER=incus`
- keep the existing microVM environment working as the default path
- use request-scoped Incus provisioning only for internal validation

Recommended server env for canarying:

- `PIKA_AGENT_INCUS_ENDPOINT`
- `PIKA_AGENT_INCUS_PROJECT`
- `PIKA_AGENT_INCUS_PROFILE`
- `PIKA_AGENT_INCUS_STORAGE_POOL`
- `PIKA_AGENT_INCUS_IMAGE_ALIAS`
- `PIKA_AGENT_INCUS_CLIENT_CERT_PATH`
- `PIKA_AGENT_INCUS_CLIENT_KEY_PATH`
- `PIKA_AGENT_INCUS_SERVER_CERT_PATH` for an explicit trusted server cert
- `PIKA_AGENT_INCUS_INSECURE_TLS=true` only if the dev endpoint uses self-signed TLS

The normal repo-managed `pika-server` Nix module now supports the same canary env through host
config plus either direct file paths or sops-managed file secrets:

- `incusEndpoint`
- `incusProject`
- `incusProfile`
- `incusStoragePool`
- `incusImageAlias`
- `incusInsecureTls`
- `incusClientCertPath`
- `incusClientKeyPath`
- `incusServerCertPath`
- `incusClientCertSecret`
- `incusClientKeySecret`
- `incusServerCertSecret`

Use that path for a real deployed canary instead of only running a local process.

For the current `pika-server -> pika-build` canary, the deployed server must use an Incus endpoint
that is reachable from `pika-server` itself. In practice that means the private tailnet address on
`pika-build` rather than `https://pika-build:8443`, unless the server host can resolve that name.

This lets operators verify:

- startup health can authenticate and probe the Incus client path honestly when Incus canary env is present
- Incus requests can create and observe real managed guests
- existing microVM-backed rows still route to the microVM backend
- the smoke helper is provisioning a fresh Incus environment, not just re-reading an existing owner state

## Deletion Validation

There is still no public v1 delete endpoint.

Today there are two practical ways to validate Incus deletion behavior:

- provider seam test coverage in `pika-server`, which asserts that Incus delete removes both the
  instance and the matching custom volume
- manual canary through the existing dashboard reset path, which exercises provider delete for the
  previous environment before provisioning a replacement

Until a public delete surface exists, the public CLI smoke flow proves create plus readiness, not a
standalone delete action.
