Read `~/configs/GLOBAL-AGENTS.md` (fallback: https://raw.githubusercontent.com/justinmoon/configs/master/GLOBAL-AGENTS.md). Skip if both unavailable.
Run `./scripts/agent-brief` first thing to get a live context snapshot.

# AGENTS.md

## Overview

Pika is an MLS-encrypted messaging app (iOS + Android) built on the Marmot protocol over Nostr. The Rust core (`rust/`) talks to MDK for MLS and nostr-sdk for relay communication. There is also a `pika-cli` (`cli/`) for agent-driven testing from the command line.

A deployed OpenClaw bot runs on a Hetzner server, reachable over public Nostr relays. It uses the `openclaw-marmot` Rust sidecar plugin to handle Marmot/MLS messaging. The bot only accepts messages from whitelisted pubkeys.

## Whitelisted npubs

Only these two pubkeys can communicate with the deployed bot:

| Who | npub | hex |
|-----|------|-----|
| Justin (real) | `npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y` | `11b9a894813efe60d39f8621ae9dc4c6d26de4732411c1cdf4bb15e88898a19c` |
| Test key | `npub1y2z0c7un9dwmhk4zrpw8df8p0gh0j2x54qhznwqjnp452ju4078srmwp70` | `2284fc7b932b5dbbdaa2185c76a4e17a2ef928d4a82e29b812986b454b957f8f` |

The test nsec is stored in `.env` (gitignored) as `PIKA_TEST_NSEC`. Agents and scripts can source it for automated testing. The iOS app, Android app, and `pika-cli` can all use this nsec to authenticate as the test identity.

## Bot npub

`npub1z6ujr8rad5zp9sr9w22rkxm0truulf2jntrks6rlwskhdmqsawpqmnjlcp`

## Relays

The bot listens on: `wss://relay.primal.net`, `wss://nos.lol`, `wss://relay.damus.io`

## Pre-commit

Before committing, run `cargo fmt` from the repo root to format Rust code.

## Related codebases

| Repo | Local path | Description |
|------|-----------|-------------|
| pika | `~/code/pika` | This repo. iOS + Android app, Rust core, pika-cli. |
| mdk | `~/code/mdk` | Marmot Development Kit. Rust MLS library used by pika and openclaw-marmot. |
| openclaw-marmot | `~/code/pika/openclaw-marmot` | OpenClaw plugin + harness for the Marmot sidecar (`marmotd`). |
| openclaw | `~/code/openclaw` | OpenClaw gateway. The bot framework that hosts the marmot plugin. |
| infra | `~/code/infra` | NixOS deployment config for the Hetzner server (`openclaw-prod`). |
