# Daemon Product Protocol

This document is now a closeout note for the daemon/app parity project, not an active wishlist.

## Decision

Daemon/app parity is done enough for now.

There is no remaining domain-sized parity PR that is clearly worth shipping without broadening the
product or reopening architecture work. The remaining items are future product scope, confidence
work, or intentional host/protocol differences.

## What Shipped

The native daemon protocol now has real production surfaces for the main first-party group
workflows:

- pending welcome list + accept
- group list + message history read
- `init_group`
- `add_members`
- `list_members`
- `remove_members`
- `leave_group`
- `get_group_profile`
- `update_group_profile`
- `upload_group_profile_image`

The daemon also has real production surfaces for the other major app-facing domains:

- `send_message`
- `send_media`
- `send_media_batch`
- `send_typing`
- `invite_call`
- `accept_call`
- `reject_call`
- `end_call`

These surfaces are built on the shared runtime/session/query/command/result seams landed during
the Marmot refactor, rather than daemon-local reimplementation.

## Group Observability

The daemon now exposes a focused, product-usable `group_updated` event family.

It is emitted after successful local mutations:

- `init_group`
- `add_members`
- `remove_members`
- `leave_group`
- `update_group_profile`
- `upload_group_profile_image`

It is also emitted for the honest inbound remote paths that already map cleanly today:

- remote membership commits
- remote group-profile metadata messages

The payload is snapshot-oriented rather than diff-oriented:

- `nostr_group_id`
- `kind`
- current member count and member list when still cheap to query
- current effective group profile when still cheap to query

That is intentionally enough that consumers should not need an immediate poll after the common
local and remote membership/profile updates.

Known semantics:

- `leave_group` reuses the same event family but omits member/profile snapshots because the group
  is no longer joined at that point.
- if a single remote membership commit both adds and removes members, `kind` is reduced to a
  best-effort single direction (`members_added` or `members_removed`); consumers that need the
  full truth should diff the snapshot payload.
- the daemon computes a pre-processing group snapshot for remote commit classification; that extra
  lookup is an accepted MVP cost because it preserves before/after membership inference without a
  broader inbound refactor.

## Group Profile Contract

The daemon’s group-profile surface is intentionally keyed to the effective admin-authored group
view:

- `get_group_profile` returns the latest matching admin-authored profile metadata if present
- when no such metadata exists yet, it falls back to joined-group summary `name` / `about`
- in that fallback case, `picture_url: null` is the current implicit signal that no explicit
  profile metadata has been seen yet
- `update_group_profile` updates only `name` / `about`
- `upload_group_profile_image` updates only `picture`
- both mutation commands preserve the rest of the current effective profile fields
- callers still cannot explicitly clear `picture`

`upload_group_profile_image` currently accepts base64-in-JSON. The 8 MB limit applies to decoded
image bytes; wire payloads are larger because of base64 expansion.

## What This Project Explicitly Did Not Try To Do

These remain intentionally host- or protocol-specific and are not parity bugs:

- protocol JSON shape and error-code choices
- event fanout mechanics
- reconnect/retry policy
- media upload/download execution policy
- call/media runtime ownership
- app UI/state projection
- a general daemon event bus

## Highest-Value Confidence That Now Exists

The most important parity surfaces now have request/event-level coverage rather than only boundary
tests:

- group management requests plus `group_updated`
- group profile read/write/image requests plus `group_updated`
- honest inbound remote membership/profile `group_updated`
- daemon media request/workflow coverage on the shared upload boundary
- daemon call invite publish coverage on the shared call command/result boundary

The remaining confidence work is incremental test depth, not another missing product domain.

## Future Product Work, Not Parity Follow-Up

These are legitimate future product areas, but they are not good candidates for another parity PR:

- broader governance/admin-role operations if product consumers need them
- remote welcome/join lifecycle observability in the same event family
- richer read-side product shell surfaces if a concrete consumer needs them
- any broader daemon event bus or diff-based change model

## Why No Further Parity PR Is Recommended

The daemon now has:

- the main write surfaces humans already use in the app for group management
- a product-usable group profile surface, including image upload
- stable snapshot-style group observability for the common local and remote paths
- real messaging/media/call command surfaces built on the shared runtime

What remains is no longer “the daemon lacks an app-equivalent product domain.” It is either:

- future product expansion
- deeper request-loop confidence work
- or intentionally local host/protocol policy

That is why this project should be treated as closed for now.
