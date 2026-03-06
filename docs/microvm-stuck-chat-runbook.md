---
summary: Operator runbook for diagnosing stuck `pikachat agent chat` flows on the HTTP microvm path.
read_when:
  - `pikachat agent chat` hangs, times out, or reports `no_reply_within_timeout`
  - checking `pika-server` -> `vm-spawner` -> guest-agent behavior end to end
---

# MicroVM Stuck-Chat Runbook

This runbook assumes the v1 HTTP path:

`pikachat agent new|me|recover|chat` -> `pika-server /v1/agents/*` -> `vm-spawner /vms*` -> guest agent log at `/workspace/pika-agent/agent.log`

## Fast commands

Local backend (`pikahut-up`):

```bash
just agent-microvm-server-logs
```

Remote control plane and guest logs:

```bash
just agent-microvm-tunnel
just agent-microvm-vmspawner-logs
just agent-microvm-guest-logs vm-1234abcd
```

Infra equivalents:

```bash
nix develop .#infra -c just -f infra/justfile server-logs
nix develop .#infra -c just -f infra/justfile build-vmspawner-logs
nix develop .#infra -c just -f infra/justfile build-guest-logs vm-1234abcd
```

## Find the VM ID

```bash
just cli agent me --nsec <owner-nsec>
```

The response includes `vm_id`. Keep that value for the guest-log and direct spawner checks below.

## Direct checks

Check the app-facing state:

```bash
just cli agent me --nsec <owner-nsec>
just cli agent recover --nsec <owner-nsec>
```

Check the spawner directly over the SSH tunnel:

```bash
curl http://127.0.0.1:8080/healthz | jq
curl http://127.0.0.1:8080/vms/<vm-id> | jq
```

Tail the guest log on the host:

```bash
just agent-microvm-guest-logs <vm-id>
```

Guest log path on the host:

```text
/var/lib/microvms/<vm-id>/workspace/pika-agent/agent.log
```

## Decision points

If `agent me` stays `creating`:

```text
Check `just agent-microvm-server-logs` first.
If you see repeated readiness probes with no `running` transition, check `just agent-microvm-vmspawner-logs`.
If `curl /vms/<vm-id>` shows `status: "starting"`, the VM is still booting; keep watching vm-spawner + guest logs.
If `curl /vms/<vm-id>` fails or the VM is missing, run `just cli agent recover --nsec <owner-nsec>` and re-check the logs with the same request ID.
```

If `agent chat` exits `no_reply_within_timeout`:

```text
Check `just agent-microvm-guest-logs <vm-id>`.
If the guest log never shows the inbound prompt, check `just agent-microvm-server-logs` for the request ID and confirm the send path completed.
If the guest log shows the prompt but no model output, check required env in `/etc/microvm-agent.env` on `pika-build` and restart `vm-spawner` if those secrets were just changed.
If the guest log shows a reply was produced, check relay connectivity and then re-run `just cli agent chat "..." --nsec <owner-nsec>`.
```

If `vm-spawner` looks healthy but the guest log path is missing:

```text
Check `curl http://127.0.0.1:8080/vms/<vm-id> | jq`.
If the VM is gone, the control-plane row is stale; run recover.
If the VM exists but `/var/lib/microvms/<vm-id>/workspace/pika-agent/agent.log` is missing, inspect `journalctl -u vm-spawner -n 200` for autostart or workspace setup failures.
```

If you need to correlate one request across hops:

```text
Use the `request_id` from the `pika-server` response or log line.
Search for that same `request_id` in `pika-server` logs first, then in `vm-spawner` logs.
The spawner logs and upstream error text now preserve that ID across the hop.
```
