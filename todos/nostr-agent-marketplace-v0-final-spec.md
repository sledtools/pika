# Final Spec: Custom Nostr Agent Marketplace Control Plane v0

## 1. Scope and Product Decisions

This spec defines the CLI <-> server control plane for provisioning and managing coding-agent leases over Nostr.

Locked product decisions:
1. Use custom protocol events (not DVM/NIP-90) for now.
2. Offers are markdown-first; avoid over-structuring server/provider internals.
3. Client selects a server offer; server chooses provisioning internals (Fly/MicroVM/Workers/etc).

## 2. Design Goals

1. Rapid iteration without protocol lock-in.
2. Compatibility-safe versioning and replay handling.
3. Private control traffic encrypted by default.
4. Event model that supports eventual multi-server decentralization.
5. Keep MLS chat data plane unchanged.

## 3. Canonical Contract Identity

Semantic contract identity is payload family + version, not kind numbers alone.

Canonical payload families for v0:
- `agent.offer.v0`
- `agent.order.create.v0`
- `agent.order.status.v0`
- `agent.order.result.v0`
- `agent.order.error.v0`
- `agent.lease.command.v0`
- `agent.lease.status.v0`
- `agent.checkpoint.v0`

Kind numbers are transport mapping and may change in future versions with explicit compatibility notes.

## 4. Provisional Kind Mapping (v0)

| Payload Family | Kind | Direction | Encryption | Replaceable |
|---|---:|---|---|---|
| `agent.offer.v0` | 31890 | server -> public | none (signed) | yes (`d=offer_id`) |
| `agent.order.create.v0` | 24901 | client -> server | required | no |
| `agent.order.status.v0` | 24902 | server -> client | required | no (append) |
| `agent.order.result.v0` | 24903 | server -> client | required | no |
| `agent.order.error.v0` | 24904 | server -> client | required | no |
| `agent.lease.command.v0` | 24910 | client -> server | required | no |
| `agent.lease.status.v0` | 24911 | server -> client | required | yes (`d=lease_id`) |
| `agent.checkpoint.v0` | 24912 | server -> client | required | no |

## 5. Offer Event (`agent.offer.v0`)

### 5.1 Required structured fields/tags
- `offer_id`
- `version` (`v0`)
- `updated_at`
- `supported_protocols` (`pi`, `acp`)
- `order_versions` (accepted order schema versions)
- `status_versions` (emitted status/result schema versions)
- `encryption_required` (`true`)

### 5.2 Optional structured fields/tags
- `payment_mode` (`free|cashu|lightning`)
- `max_ttl_sec`

### 5.3 Markdown-first body
All descriptive and operational details belong in markdown content, including:
- infrastructure and region details
- performance/capacity claims
- model catalog and limits
- policy/abuse constraints
- pricing details (primary source of truth in v0)

Do not introduce structured provider taxonomy in v0.

## 6. Order Create (`agent.order.create.v0`)

Client must send:
- `request_id` (unique event/request identity)
- `idempotency_key` (stable per logical order)
- `offer_id`
- `protocol` (`pi|acp`)
- `requested_ttl_sec`
- `relay_urls`

Optional encrypted fields allowed as best-effort hints:
- model preference
- resource preference
- payment token

Client does not choose provider/runtime substrate.

## 7. Order Status/Result/Error

### 7.1 Status (`agent.order.status.v0`)
Progress stream only (non-terminal).

Required:
- `request_id`
- `status_seq` (monotonic per request)
- `phase` (`queued|provisioning|waiting_keypackage|payment-required`)
- optional `progress_hint`
- optional `lease_id`

### 7.2 Result (`agent.order.result.v0`)
Terminal success event.

Required:
- `request_id`
- `lease_id`
- `bot_pubkey`
- `expires_at`

Privacy rule: `bot_pubkey` is carried in encrypted payload by default (not required as plaintext tag).

### 7.3 Error (`agent.order.error.v0`)
Terminal failure event.

Required:
- `request_id`
- `error_code`
- `hint`
- `retryable` (bool)

## 8. Lease Control and Status

### 8.1 Lease Command (`agent.lease.command.v0`)
Supported commands:
- `terminate`
- `status_query`
- `extend` (optional)
- `checkpoint_now` (optional)

### 8.2 Lease Status (`agent.lease.status.v0`)
Replaceable latest-state event keyed by `lease_id` (`d` tag).

Required fields:
- `phase`
- `expires_at`
- `last_heartbeat_at` (if heartbeat enabled)
- optional `health_summary`

MVP requires event-driven status updates and `status_query`. Periodic heartbeat is recommended post-MVP (e.g., 60s).

## 9. Idempotency, Ordering, and Recovery

Server requirements:
1. Maintain idempotency ledger keyed by (`client_pubkey`, `idempotency_key`) plus normalized order hash.
2. Duplicate logical order must return prior outcome (or deterministic conflict error).
3. Emit monotonic `status_seq` for every status event per request.

Client recovery behavior:
1. On timeout, query relay history by `request_id`.
2. If unresolved, retry with same `idempotency_key`.
3. Accept highest `status_seq` as latest status when out-of-order events occur.

## 10. Security Model

1. All non-offer events require encrypted payloads.
2. Offers are public signed announcements.
3. Server must verify lease ownership (`client_pubkey`) on every lease command.
4. Never expose client secrets in unencrypted tags.

## 11. Version and Compatibility Policy

1. `v0` payload families are immutable once published.
2. Additive optional fields only within a version.
3. Breaking changes require `v1` payload families.
4. Offers must advertise accepted/produced versions.
5. Client pins version in order; server responds with pinned-compatible version.

## 12. MVP Acceptance Slice

Required MVP capabilities:
1. Publish one `agent.offer.v0` event.
2. CLI submits encrypted `agent.order.create.v0`.
3. Server emits progress `agent.order.status.v0` and terminal `agent.order.result.v0` / `agent.order.error.v0`.
4. CLI receives `bot_pubkey` and continues existing MLS flow unchanged.
5. CLI sends `terminate` lease command on exit.
6. Whitelist auth only.
7. Single configured server target (discovery browsing and payments can be deferred).
8. Include version fields and request/idempotency recovery primitives from day one.

## 13. Post-MVP Roadmap

1. Multi-server discovery UX over offers.
2. Optional periodic heartbeat policy and richer health.
3. Payment phases: whitelist -> cashu -> lightning -> streaming top-up.
4. Checkpoint support with versioned format; portability target across servers later.
