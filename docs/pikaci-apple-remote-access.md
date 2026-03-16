---
summary: Validated bootstrap note for reaching the fresh Mac mini from this Mac
read_when:
  - reconnecting to the Mac mini for manual remote experiments
  - checking which bootstrap access path was actually validated
---

## Mac mini remote access

- Machine during validation: `mini's Mac mini`
- Local user during validation: `mini`
- Validated access method: ordinary macOS `Remote Login` (`ssh`) over Tailscale network connectivity

### SSH enablement

`System Settings` -> `General` -> `Sharing` -> enable `Remote Login` for user `mini`.

The validated shell path used the mini user's normal `~/.ssh/authorized_keys`. During bootstrap, the current operator added this Mac's public key there. No separate Tailscale SSH setup was required for the validated path.

### Tailscale

Tailscale was installed on the mini via the macOS download from `tailscale.com`, then joined to the same tailnet as this Mac.

During validation on 2026-03-15, the mini appeared on the tailnet with observed IPv4 `100.92.43.40` and observed MagicDNS name `minis-mac-mini.taild659a1.ts.net`. Treat those as observed values, not a stable provisioning contract.

The first remote shell did not have `tailscale` on `PATH`; the app binary path worked:

```bash
/Applications/Tailscale.app/Contents/MacOS/Tailscale ip -4
```

### Reconnect later

From this Mac, use the mini's current Tailscale IP or current MagicDNS name:

```bash
ssh mini@<current-tailscale-ip>
ssh mini@<current-magicdns-name>
```

Observed working examples during validation:

```bash
ssh mini@100.92.43.40
ssh mini@minis-mac-mini.taild659a1.ts.net
```

### Thin trigger contract

The checked-in thin-trigger entrypoint is:

```bash
./scripts/pikaci-apple-remote.sh prepare --ref <git-ref>
./scripts/pikaci-apple-remote.sh run --ref <git-ref>
./scripts/pikaci-apple-remote.sh run --ref <git-ref> --just-recipe apple-host-sanity
```

It uses a git-backed source contract on the mini: the caller sends an exact git bundle for the requested ref, the mini imports it into a bare mirror under `~/.cache/pikaci-apple`, materializes a stable prepared worktree under `prepared/<commit>`, and reuses that checkout across repeated runs of the same commit. When the wrapper sees a schema-valid prepared checkout for the exact commit, it skips source upload/import entirely and goes straight to the prepared worktree. `prepare` realizes the Apple dev shell and prewarms the generated iOS artifacts there; `run` reuses the prepared checkout, executes the requested checked-in `just` recipe (default `checks::apple-host-bundle`), then returns a debug artifact bundle to the caller. The wrapper takes a host-local run lock before touching the shared mirror/target state, schema-checks the prepared marker before reuse, scrubs run-local `.pikaci` state and stale XCTest logs before each operation, prunes old remote run dirs automatically, and keeps only a bounded number of prepared commit dirs.

### Caveats

- The office LAN path was not usable during bootstrap: the mini was on `10.246.40.24` and this Mac was on `10.246.51.11`, with no working route between them.
- Current Tailscale reachability is working, but `tailscale ping` still reports `via DERP(dfw)` rather than a direct peer-to-peer path.
- Same-LAN bootstrap was acceptable for setup, but this is not the intended steady-state CI network posture.
