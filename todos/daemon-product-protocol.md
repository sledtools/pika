# Daemon Product Protocol

This is the living follow-up plan after the `pika_core` / Marmot extraction work.

It is not another core-architecture migration plan. The core runtime/query/command/result
boundaries now exist. The next job is to cash that work out into a more complete, product-worthy
daemon protocol surface.

## Why This Exists

The Marmot refactor removed most of the duplicated business logic between the app and daemon:

- shared session open/recompute/sync state
- shared durable queries
- shared runtime command boundaries
- shared operation-result boundaries
- thinner host paths across media, calls, outbound messaging, group creation, welcomes, and
  membership evolution

The biggest remaining gap is not inside the shared runtime anymore.

The biggest remaining gap is that the daemon protocol surface is still incomplete from a product
perspective. In particular, group/membership evolution is shared in runtime code but not exposed as
real production daemon protocol commands.

So the next project should be:

- do not invent more core abstractions
- do not reopen the Marmot extraction
- make the daemon product surface match the shared runtime capabilities that already exist

## Role Of This Document

This document is a living product/protocol plan.

It should:

- define the intended daemon protocol surface
- clarify what is currently exposed vs missing
- keep future prompts grounded in real runtime capabilities
- keep testing focused on request-level behavior, not only internal seams

It should not:

- prescribe a rigid implementation order if reality changes
- restart the `pika_core` extraction
- turn into a speculative long-term daemon rewrite

## Current Ground Truth

The current native daemon protocol already supports real production commands for:

- relay/keypackage setup
- pending-welcome list + accept
- group list + message history read
- member listing
- group membership add
- group membership remove
- leave group
- group profile read
- group profile metadata update (`name` / `about`)
- group profile image upload
- explicit `group_updated` events after local group/membership/profile mutations
- message send
- media send
- typing send
- call invite / accept / reject / end
- init-group bootstrap

The current native daemon protocol does **not** expose real production commands for:

- other membership/group evolution operations
- broader governance/admin-role operations
- a general inbound event bus beyond the focused group/product event family

Current group-profile mutation scope is intentionally metadata + image only:

- `update_group_profile` can set `name` / `about`
- `upload_group_profile_image` can set `picture`
- both preserve the rest of the current effective profile fields
- callers still cannot explicitly clear `picture`
- it returns the same full profile shape as `get_group_profile`, including the carried-forward
  `picture`

Current group-profile read/write contract is intentionally keyed to the effective admin-authored
group view:

- `get_group_profile` returns the latest admin-authored group profile metadata if present
- when no admin-authored metadata exists yet, it falls back to joined-group summary `name` /
  `about`
- in that fallback case, `picture_url: null` is the current implicit signal that no explicit
  profile metadata has been seen yet
- `upload_group_profile_image` preserves the current `name` / `about` and updates only `picture`
- `upload_group_profile_image` accepts base64-in-JSON today; the 8 MB limit applies to decoded
  image bytes, so wire payloads are larger because of base64 expansion

Current group-update observability is intentionally focused, not generic:

- the daemon emits a typed `group_updated` event after successful local `init_group`,
  `add_members`, `remove_members`, `leave_group`, `update_group_profile`, and
  `upload_group_profile_image` commands
- the daemon also emits the same `group_updated` family for honest inbound remote changes:
  remote membership commits and remote group-profile metadata messages
- each event carries the `nostr_group_id`, an update kind, and current member/profile snapshots
  when they are still cheap to query after the mutation or inbound update
- `leave_group` emits a lifecycle event without member/profile snapshots because the group is no
  longer joined at that point; the same shape is reused if an inbound remote membership commit
  removes the local member
- for the common local and remote membership/profile paths, consumers should not need an immediate
  poll after receiving `group_updated`
- remote welcome/join lifecycle follow-through and broader governance/admin-role observability are
  still future product work rather than part of this MVP
The shared runtime already has most of the underlying membership machinery:

- membership prep in `prepare_add_members(...)`
- finalize/merge in `finalize_published_evolution(...)`
- explicit membership operation results
- welcome-delivery planning after evolution publish

So the missing piece is mostly protocol/product wiring, not core Marmot logic.

## Product Direction

The daemon should become a complete enough remote control surface for serious first-party product
use, not just a bot transport shim.

That means the product daemon should be able to support:

- chat/group bootstrap
- membership changes
- media and calls
- enough query surface to drive a product shell or agent host
- explicit request-level success/failure behavior

This does **not** mean it should become a second UI model or a host-neutral app state container.

## Working Principles

1. Reuse existing shared runtime boundaries.
Do not add new architecture unless the current runtime surface is truly insufficient.

2. Prefer real production daemon commands over test-only seams.
The next value is in request-level product behavior, not more internal helper tests.

3. Keep protocol stable and explicit.
New commands/results/events should be versioned, typed, and intentionally named.

4. Prefer one coherent product seam at a time.
For example, “membership evolution” is a good domain PR. “random protocol odds and ends” is not.

5. Keep request-level tests close to real daemon behavior.
The confidence problem now is more about product surface coverage than shared-runtime coverage.

## Remaining Gap: Broader Group Governance

The core group-management and group-observability MVP is now on the native daemon surface. The
next value is narrower governance/product follow-through, not more basic group mutation wiring.

### What Exists Already

- shared membership prep/finalize logic in `pika-marmot-runtime`
- app production usage of that shared path
- daemon production `add_members`
- daemon production `list_members`
- request-level success/failure coverage for the membership MVP

### What Is Missing

- admin-role changes and any other governance mutations the product actually needs
- any broader daemon mapping for future governance-specific events/results
- product decisions for richer remote lifecycle observability beyond the current snapshot event
  family

### Smallest Honest Product Surface

The smallest honest group-management surface is now shipped:

- `add_members`
- `list_members`
- `remove_members`
- `leave_group`
- `get_group_profile`
- `update_group_profile`
- `upload_group_profile_image`

Current `add_members` shape:

```json
{
  "cmd": "add_members",
  "request_id": "...",
  "nostr_group_id": "...",
  "peer_pubkeys": ["...", "..."]
}
```

Current success payload:

- target `nostr_group_id`
- added pubkeys
- resulting member count
- welcome delivery count

Current failure classes:

- bad group id
- invalid pubkey
- no key package for peer
- publish failed
- merge failed
- mdk/runtime failure

Current `list_members` shape:

```json
{
  "cmd": "list_members",
  "request_id": "...",
  "nostr_group_id": "..."
}
```

Current read payload:

- target `nostr_group_id`
- `members: [{ pubkey, is_admin }]`
- resulting member count

### Honest Follow-Through

What should become shared-through-runtime:

- prep
- finalize
- membership operation result
- welcome-delivery planning/final follow-through

What should remain daemon-local:

- protocol mapping
- error-code choices
- retry/publish scheduling policy if still daemon-specific

## Likely Follow-On Commands

After the membership/group-management MVP, the next most likely protocol candidates are:

1. admin-role changes if product needs them
2. remote welcome/join lifecycle observability if product consumers need push state there too
3. any broader group-profile policy only if product consumers need more than the current
   admin-authored view

I would not start with all of these at once.

## Event Surface

Current daemon events are good for:

- ready
- welcome received
- group joined / group created
- message received
- call events
- `group_updated` for local group mutations and honest inbound remote membership/profile changes

The main remaining group-event gaps are narrower now:

- remote welcome/join lifecycle follow-through in the same event family
- broader governance/admin-role eventing if product needs it

I do **not** think we should invent a big event system first.

The right sequencing is:

1. keep the current snapshot-oriented `group_updated` family stable
2. only add the next missing product event when a concrete consumer actually needs it
3. avoid a generic daemon event bus unless the product surface truly forces it

## Test Strategy

The next confidence work should be request-level and production-facing.

Required for each new daemon protocol command:

1. command parsing/serde test
2. daemon request-level success test
3. daemon request-level failure test
4. at least one shared-runtime boundary test stays green underneath

Highest-value current request-level gaps:

- `init_group` end-to-end daemon request path
- `send_media` / `send_media_batch` request path
- `invite_call` / `reject_call` / `end_call` request paths
- any future governance/admin-role request paths if they land

## Suggested Implementation Order

### Phase A: Expand Beyond Membership MVP

1. Keep `add_members` / `list_members` / `remove_members` / `leave_group` stable
2. Add the next highest-value production mutation
3. Reuse the same shared membership/group runtime seams
4. Keep daemon-local error mapping explicit and stable

Acceptance:

- daemon membership MVP stays stable at request level
- no new core runtime architecture is needed

### Phase B: Fill The Read Side

1. Expand read-side group surfaces only where product shells need them
2. Reuse shared joined-group/member snapshot queries
3. Add request-level tests

Acceptance:

- daemon read surfaces stay product-usable without inventing new host-neutral state

### Phase C: Group Update Surface

1. Decide whether `update_group_profile` is needed now
2. If yes, expose it through daemon protocol
3. Add the minimum event/result surface needed for group-update observability

Acceptance:

- group mutation surface is coherent enough for first-party product use

## Open Questions

These are the main questions I still want answered as we broaden the daemon product surface:

1. Should the daemon aim to be a generally complete first-party product surface for group
   management, or just enough for agent/automation use cases?

2. The membership commands currently accept whatever `PublicKey::parse(...)` accepts, which is
   convenient but not a tightly-specified product contract yet. Do we want to freeze that, or
   eventually narrow the protocol to one canonical input form?

3. Do we want membership changes to remain request/response only at first, or do we want to add a
   daemon event like `group_updated` as part of the first slice?

## Non-Goals

This project should not become:

- a new `pika_core` extraction plan
- a full `pikad` rename/topology project
- a general event-bus rewrite
- ACP/OpenClaw/Pi cleanup
- app UI/state cleanup

## Success Condition

This follow-up is successful if:

- the daemon protocol becomes a materially more complete product surface
- the implementation mostly reuses the shared runtime already built
- request-level confidence improves
- iteration speed goes up because product features stop requiring app-only flows or test-only
  seams
