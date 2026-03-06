---
summary: Frozen v1 app-facing MicroVM API contract (paths, lifecycle states, and error codes).
read_when:
  - implementing or reviewing /v1/agents/* endpoints
  - changing app-visible microvm state or error code behavior
---

# MicroVM v1 App API Contract

This document freezes the dogfood v1 app-facing contract for agents.

## Endpoints

- `POST /v1/agents/ensure`
- `GET /v1/agents/me`
- `POST /v1/agents/me/recover`

## App-visible lifecycle states

Only these values are allowed in app-facing responses:

- `creating`
- `ready`
- `error`

## Stable error codes

- `unauthorized` (`401`)
- `not_whitelisted` (`403`)
- `invalid_request` (`400`)
- `agent_exists` (`409`)
- `agent_not_found` (`404`)
- `recover_failed` (`503`)
- `internal` (`500`)

## Source of truth

Canonical constants and tests live in:

- `crates/pika-server/src/agent_api_v1_contract.rs`
