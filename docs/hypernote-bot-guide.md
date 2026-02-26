---
summary: Canonical protocol + implementation guide for hypernotes in pikachat/openclaw
read_when:
  - building bots that send interactive UI
  - integrating hypernote with openclaw channels
  - writing prompts for hypernote generation
---

# Hypernote Bot Guide

This document is the canonical spec for hypernote v1 in this repo.

## Core Model

- `9467`: hypernote message (MDX + optional metadata tags)
- `9468`: hypernote action response (`{"action":"...","form":{...}}`)
- `9468` messages are hidden from timeline UI and used to drive response tallies
- Signed-action publishing is removed in v1

## Daemon Commands

### `send_hypernote`

Send a hypernote to a group:

```json
{
  "cmd": "send_hypernote",
  "request_id": "optional-correlation-id",
  "nostr_group_id": "<hex group id>",
  "content": "<MDX source>",
  "title": "<optional display title>",
  "state": "<optional JSON string>"
}
```

Fields:

| Field | Required | Description |
|---|---|---|
| `nostr_group_id` | yes | Hex group id |
| `content` | yes | MDX source |
| `title` | no | Stored as `title` tag |
| `state` | no | Stored as `state` tag; default interactive state |

Response:

```json
{"type":"ok","request_id":"...","result":{"event_id":"<hex>"}}
```

### `hypernote_catalog`

Query canonical component/action registry from daemon:

```json
{"cmd":"hypernote_catalog","request_id":"r1"}
```

Response:

```json
{
  "type": "ok",
  "request_id": "r1",
  "result": {
    "catalog": {
      "protocol_version": 1,
      "kinds": { "note": 9467, "action_response": 9468 },
      "design_principles": [...],
      "components": [...],
      "actions": [...]
    }
  }
}
```

## CLI Commands

- Send hypernote:
  - `pikachat send-hypernote --group <hex> --content '# Hello'`
  - `pikachat send-hypernote --to <npub> --file note.hnmd`
- Print catalog:
  - `pikachat hypernote-catalog`
  - `pikachat hypernote-catalog --compact`

`.hnmd` frontmatter supports `title` and `state`:

```hnmd
{"title":"Checkout","state":{"amount":"1000"}}
# Confirm
<SubmitButton action="confirm">Confirm</SubmitButton>
```

## Component Registry

Source of truth lives in Rust (`crates/hypernote-protocol`), not Swift/docs.

Current catalog:

- Layout: `Card`, `VStack`, `HStack`
- Typography: `Heading`, `Body`, `Caption`
- Interactive: `TextInput`, `ChecklistItem`, `SubmitButton`
- Catalog entries include terse `design_principles` (both top-level and per-component).

Current key layout principles:

- `HStack`: only for short fixed-width items (for example buttons); never place `ChecklistItem` or variable-length text in `HStack`.
- `ChecklistItem`: labels wrap on narrow widths; keep labels short (around three words), or stack items in `VStack`.
- `Card`: no scrolling; keep content brief.
- `TextInput`: full-width by default; do not nest inside `HStack`.
- General: when in doubt, use `VStack`; use `HStack` only for short pill-shaped items side by side.

Unknown components degrade gracefully and render children.

## Action Registry

Source of truth also lives in `crates/hypernote-protocol`.

Current action model:

- Action family: `submit`
- Trigger: `SubmitButton`
- Response kind: `9468`
- Required link tag: `e` (target hypernote event id)
- Payload: `{"action":"string","form":"object<string,string>"}`
- Visibility: hidden from timeline
- Dedupe: latest response per `(sender, target_hypernote)`

## Response Semantics (`9468`)

When user taps a `SubmitButton`, client publishes:

```json
{"action":"confirm","form":{"amount":"1000","memo":"thanks"}}
```

with an `e` tag pointing to the hypernote message id.

Rust state processing:

- parses only explicit `9468` messages as responses
- ignores undeclared actions (not present in hypernote `SubmitButton action=...`)
- computes:
  - per-action counts (`response_tallies`)
  - current user response (`my_response`)
  - responder list (`responders`)
- excludes `9468` from unread count increments and visible timeline rows

## Polls

Polls are just hypernotes with multiple `SubmitButton`s.

iOS poll composer now dispatches a typed action to Rust (`SendHypernotePoll`), and Rust builds the MDX payload via shared protocol logic (`build_poll_hypernote`). Poll semantics piggyback entirely on the same 9467/9468 message/response model.

## OpenClaw Notes

OpenClaw channel now supports outbound hypernotes:

- Plain text -> `send_message`
- Fenced block -> `send_hypernote`

Supported fenced block labels:

- ```` ```pika-hypernote ```` 
- ```` ```hypernote ```` 
- ```` ```hnmd ```` 

Optional first line metadata JSON inside the block:

```text
{"title":"...", "state": {...}}
<MDX body...>
```

The sidecar `message_received` event now includes `kind`, enabling explicit handling for `9468` action responses.

## Recommended Agent Prompting

Tell agents to:

- prefer standard markdown + registered components
- keep one form per hypernote
- always set `SubmitButton action` explicitly
- for polls, emit a hypernote block instead of ad-hoc text protocols
- avoid relying on removed signed-action behavior
