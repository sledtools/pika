# Changelog

## 0.5.3

- Fix: align plugin/channel ID with npm package name (`pikachat` → `pikachat-openclaw`).
  - Fixes `openclaw plugins install pikachat-openclaw` failing with validation error.
  - Config path changes: `channels.pikachat` → `channels.pikachat-openclaw`.
  - Extension directory: `extensions/pikachat` → `extensions/pikachat-openclaw`.
- Fix: bump script path updated for renamed extension directory.

## 0.4.0

- Fix: publish `i` (KeyPackageRef) tag on key package events for Pika v0.2.8+ compatibility.
- Feat: typing indicators (eager fire before profile fetch + agent dispatch).
- Feat: per-group users, systemPrompt, and wildcard group support.
- Feat: `SendAudioFile` command for pre-synthesized audio.
- Fix: suppress STT feedback loop during TTS playback.
- Fix: stop spurious auto-restart loop when sidecar is already running.

## 0.3.2

- Fix: use sledtools/pika as default sidecar repo for binary downloads.

## 0.3.1

- Release pipeline: tag `pikachat-v*` is now the single source of truth for versioning across Rust binary and npm package.

## 0.3.0

**Breaking: MLS library upgrade (openmls 0.7.1 -> 0.8.1)**

MDK upgraded from a pinned openmls git fork (0.7.1) to crates.io openmls 0.8.1. MLS wire formats and state are **not interoperable** across this boundary.

- All existing MLS groups will stop working. Users must create new groups after upgrading.
- The bot's MLS state must be wiped on deploy (`/home/openclaw/.openclaw/pikachat/accounts/default/mdk.sqlite`).
- All pika app clients must update to the matching mdk revision before or at the same time as the bot deploy.

## 0.2.0

Initial public release.
