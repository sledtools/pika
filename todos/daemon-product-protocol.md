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
- group profile metadata update (`name` / `about`)
- message send
- media send
- typing send
- call invite / accept / reject / end
- init-group bootstrap

The current native daemon protocol does **not** expose real production commands for:

- other membership/group evolution operations
- group profile image upload
- explicit group-update eventing

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

## Immediate Gap: Broader Group Evolution

The membership MVP is now on the native daemon surface. The next value is the rest of the
group-evolution product surface.

### What Exists Already

- shared membership prep/finalize logic in `pika-marmot-runtime`
- app production usage of that shared path
- daemon production `add_members`
- daemon production `list_members`
- request-level success/failure coverage for the membership MVP

### What Is Missing

- additional group-evolution mutations beyond membership add
- broader daemon mapping for future group-update results/events
- product decisions for profile updates, leaves, and removals

### Smallest Honest Product Surface

The smallest honest membership surface is now shipped:

- `add_members`
- `list_members`

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

1. group profile image upload if a product shell actually needs it
2. admin-role changes if product needs them
3. richer group-update observability if product consumers need push state

I would not start with all of these at once.

## Event Surface

Current daemon events are good for:

- ready
- welcome received
- group joined / group created
- message received
- call events

But for richer group/product behavior, the likely missing event family is:

- group update / membership update

I do **not** think we should invent a big event system first.

The right sequencing is:

1. add real request-level membership mutation
2. see what explicit daemon event/result shape is actually needed
3. add the smallest honest event/result surface after that

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
- broader future group-evolution request paths after membership MVP

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
