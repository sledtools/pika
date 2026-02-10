---
summary: High-level architecture — Rust core, iOS/Android apps, MLS over Nostr
read_when:
  - starting work on the project
  - need to understand how components fit together
---

# Architecture

Pika is an MLS-encrypted messaging app for iOS and Android, built on the Marmot protocol over Nostr.

## Components

- **Rust core** (`rust/`) — MLS state machine, Nostr transport, UniFFI bindings
- **iOS app** (`ios/`) — Swift UI, uses PikaCore.xcframework
- **Android app** (`android/`) — Kotlin, uses JNI bindings via cargo-ndk
- **pika-cli** (`cli/`) — Command-line interface for testing and agent automation
- **MDK** (external, `~/code/mdk`) — Marmot Development Kit, the MLS library

## Data flow

1. App calls Rust core via UniFFI (Swift) or JNI (Kotlin)
2. Rust core uses MDK for MLS group operations (create, invite, encrypt, decrypt)
3. Rust core uses nostr-sdk to publish/subscribe Nostr events on relays
4. Key packages (kind 443) enable async peer discovery
