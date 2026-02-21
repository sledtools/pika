---
summary: Local Primal iOS interop lab for debugging Pika NIP-46 login callbacks and relay traffic
read_when:
  - debugging Pika to Primal to Pika signer login behavior on iOS
  - running repeatable simulator interop sessions with seeded state
  - inspecting nostr-connect callback and relay evidence for flaky login flows
---

# Primal iOS Interop Lab

This lab is for debugging real `Pika -> Primal -> Pika` NIP-46 login interop with evidence:
- local relay traffic logs
- local tap for kind-`24133` (`NostrConnect`) events
- optional Primal debug instrumentation
- reusable seeded simulator state

## Commands

Known working command on Justin's machine (UDID is machine-specific):

```bash
cd /Users/justin/code/pika/worktrees/signer
PIKA_IOS_SIM_UDID=D238FBF0-CB05-4363-B872-D524A60B4F48 \
PIKA_IOS_BUNDLE_ID=com.justinmoon.pika.dev \
./tools/run-ios --sim
```

```bash
just primal-ios-lab-patch-primal
just primal-ios-lab
just primal-ios-lab-dump-debug
just primal-ios-lab-seed-capture
just primal-ios-lab-seed-reset
```

Equivalent direct tool:

```bash
./tools/primal-ios-interop-lab run
```

## What `run` does

1. Ensures a dedicated simulator exists (`Pika Primal Lab` by default).
2. Starts a local `nostr-rs-relay` on a random `ws://127.0.0.1:<port>`.
3. Starts `nostr_connect_tap` to log all kind-24133 events from that relay.
4. Builds/installs Primal from `~/code/primal-ios-app` (configurable).
5. Enables Pika external signer + nostr-connect debug dump via simulator launchctl env.
6. Builds/installs/launches Pika with relay config pinned to the local relay.
7. Streams simulator logs (`Pika` + `Primal` + `nostr_connect` markers) into a run folder.

The tool prints:
- `relay_url`
- `run_dir`
- paths for `relay.log`, `nostr_connect_tap.log`, `sim.log`
- Pika debug snapshot path (`nostr_connect_debug.json`)

## Seeded simulator workflow

Use this to avoid redoing Primal onboarding/login every run.

1. Prepare simulator once manually (Primal logged-in, permissions set).
2. Capture seed:
```bash
just primal-ios-lab-seed-capture
```
3. Before a new debug session, reset lab simulator from seed:
```bash
just primal-ios-lab-seed-reset
```

## Primal instrumentation

`just primal-ios-lab-patch-primal` applies a local debug patch to:

`~/code/primal-ios-app/Primal/Scenes/RemoteSigner/RemoteSignerSignInController.swift`

It logs:
- initializeConnection begin
- initializeConnection success
- initializeConnection nil return
- initializeConnection thrown error
- callback open attempts

## Decode connect responses

After Pika starts Nostr Connect login, dump its snapshot:

```bash
just primal-ios-lab-dump-debug
```

That command prints a helper to run:
- `nostr_connect_tap` with the client `nsec` from snapshot
- decrypted `nip44`/`nip04` payloads (when decryptable)

## Notes

- Debug snapshot contains secrets (`client_nsec`, connect `secret`); treat as sensitive.
- Current scope is manual interop debugging; not intended for per-PR CI.
