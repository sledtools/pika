## Spec

Why this is being done:
`pikachat` currently works, but the daemon/runtime surface is carrying too many unrelated jobs in one
place: MLS/Nostr transport, message/media/call state, JSONL sidecar hosting, unix-socket remote
mode, OpenClaw integration assumptions, and old Pi-specific bridge patterns. That makes every new
integration feel like a special case and pushes policy into the transport layer.

Intent and expected outcome:
Refactor `pikachat` into a transport-first runtime that is easy to embed from both OpenClaw and ACP
agents, while preserving existing features such as calling, media, welcomes, hypernotes, and
group-chat behavior. Pi should move onto a generic ACP path so Pika no longer carries custom
Pi-specific bridge code.

Exact build target (what will exist when done):
1. One transport/runtime product:
`pikachatd` is the long-running host for Nostr/MLS messaging, welcomes, groups, media, and calls.
It is transport- and agent-agnostic.
2. One reusable domain core:
the runtime logic behind `pikachatd` lives in a library crate with clear submodules for identity,
groups/messages, media, and calls.
3. One versioned embedding contract:
all daemon commands/events are defined in one protocol crate and consumed by both Rust and
TypeScript code, rather than duplicated ad hoc.
4. One explicit host boundary:
hosting a child process, a unix-socket client, OpenClaw, or ACP all happens through explicit
frontend/adapter layers rather than inside the domain core.
5. Preserved product features:
the refactor does not delete core capabilities already present in `pikachat`: welcomes, group
creation/join, message send/receive, encrypted media, hypernotes, reactions, calls, call media
transport, STT/TTS-related event emission hooks, and remote control support.
6. One generic agent boundary for serious harnesses:
ACP becomes the preferred integration boundary for agent runtimes such as Pi, while the existing
native `pikachat` protocol remains for simple bots and internal tooling.
7. Zero custom Pi bridges in Pika:
the Python microVM bridge and the older `pi-bridge.py` path are retired after ACP migration is
complete. Pika no longer owns Pi-specific prompt/reply glue.
8. Two first-class consumers:
OpenClaw continues to work well as a native `pikachat` protocol consumer; Pi works well through
`pi-acp`, with no bespoke runtime logic in this repo.

Exact approach (how it will be accomplished technically):
1. Split `pikachat` by responsibility before changing behavior:
first separate protocol, core runtime, and frontends; only then change Pi/OpenClaw integration
paths.
2. Preserve behavior by extracting existing code, not redesigning features in place:
calls/media/hypernotes stay feature-complete while moving into clearer modules.
3. Keep the current JSONL daemon protocol alive during migration:
OpenClaw and test harnesses continue to function while the new structure lands.
4. Add ACP as a new first-class frontend/boundary:
do not force ACP through the existing `--exec` JSONL child contract.
5. Move Pi to ACP by replacing Pika-owned glue with generic ACP session mapping:
`pikachatd` should talk to an ACP agent process such as `pi-acp`, while `pi-acp` continues to own
Pi RPC translation.
6. Delete special-case glue only after parity tests exist for both OpenClaw and Pi paths.

## Target Architecture

1. `crates/pikachat-core`
Purpose:
own all Nostr/MLS runtime behavior and persistent state.

Contents:
- identity/state loading
- relay management
- welcome ingestion/acceptance
- group creation/listing/message history
- message send/receive
- encrypted media upload/download/decrypt
- call signaling and media transport
- hypernote encode/decode helpers

Non-goals:
- child-process hosting
- ACP
- OpenClaw policy
- Pi-specific behavior
- CLI parsing

2. `crates/pikachat-protocol`
Purpose:
define the versioned wire contract used by `pikachatd` frontends and clients.

Contents:
- `InCmd` / `OutMsg`-style protocol types
- protocol versioning
- serde/json schema fixtures
- compatibility tests

Non-goals:
- runtime logic
- direct relay access

3. `crates/pikachatd`
Purpose:
run the long-lived host process around `pikachat-core`.

Contents:
- stdio JSONL frontend
- unix-socket frontend
- child-process bridge frontend
- ACP bridge frontend
- lifecycle/shutdown/error mapping

Non-goals:
- app/product policy
- OpenClaw route logic
- Pi session logic

4. `cli/`
Purpose:
remain the human/developer CLI, but call into the same service/core used by `pikachatd`.

Contents:
- one-shot commands
- listen/send/invite/etc
- daemon launcher
- test harnesses

5. OpenClaw plugin
Purpose:
adapter from OpenClaw channel semantics to the native `pikachat` daemon protocol.

Contents:
- routing policy
- mention gating
- group-vs-DM session policy
- OpenClaw tool exposure

6. ACP bridge
Purpose:
adapter from `pikachat` conversations to ACP sessions.

Contents:
- group/session mapping
- prompt submission
- assistant chunk streaming
- cancellation
- structured mapping of message/media/reaction/call-related events where ACP supports them

Pi-specific logic should not live here beyond launch/config selection; `pi-acp` remains the ACP
adapter for Pi itself.

## Plan

1. Freeze the target `pikachatd` architecture in docs before moving code.
Acceptance criteria: one short design note states that `pikachatd` is the long-running runtime host,
`pikachat-core` owns transport/domain behavior, `pikachat-protocol` owns wire contracts, and ACP is
the preferred serious-agent boundary.

2. Create explicit crate/module boundaries for core, protocol, and host.
Acceptance criteria: code is reorganized into distinct locations for:
transport/domain logic
wire protocol definitions
host frontends
No new feature work is added directly to the monolithic daemon file during the refactor.

3. Move protocol types out of the daemon implementation.
Acceptance criteria: command/event types currently defined inside the daemon move into a shared
protocol crate/module; Rust host code imports them instead of declaring them locally.

4. Eliminate handwritten protocol duplication between Rust and TypeScript.
Acceptance criteria: the OpenClaw plugin no longer manually re-declares the sidecar protocol shape
from scratch; protocol compatibility is tested against one source of truth.

5. Split `daemon.rs` into focused host modules without changing behavior.
Acceptance criteria: no single host file continues to own all of:
stdio/exec hosting
unix-socket hosting
message/group logic
media logic
call logic
Tests prove behavior parity after extraction.

6. Extract group/welcome/message behavior into dedicated runtime services.
Acceptance criteria: welcome handling, group subscription, list-groups, get-messages, send-message,
send-hypernote, reactions, and typing are implemented through a `groups/messages` service layer,
not as one long daemon match block.

7. Extract media behavior into a dedicated runtime service.
Acceptance criteria: encrypted media upload, media receive/decrypt temp-file handling, and media
batch send logic are moved behind a media service with dedicated tests. OpenClaw and CLI behavior
remain unchanged.

8. Extract call behavior into a dedicated runtime service.
Acceptance criteria: call invite/accept/reject/end, media crypto derivation, transport setup,
audio/data workers, and call debug/event emission are moved behind a call service. Existing calling
ability remains intact.

9. Keep call/STT/TTS support, but push policy out of the runtime host.
Acceptance criteria: `pikachatd` still emits call audio and call lifecycle events needed for STT/TTS
flows, but model-provider or assistant policy does not live inside the transport core. Runtime-level
audio encode/decode and transport remain preserved.

10. Make `pikachatd` the single long-running authority for automation APIs.
Acceptance criteria: unix socket remote mode, stdio mode, and embed/exec mode all call the same
runtime services. There is one authoritative command surface instead of partial `--remote` support
drifting from daemon behavior.

11. Route CLI one-shot commands through the same service layer used by the daemon.
Acceptance criteria: send/listen/groups/messages/invite/welcome operations share implementation
with `pikachatd`; CLI-only behavior is thin orchestration, not duplicated transport logic.

12. Keep the native stdio JSONL protocol as a supported “simple bot” contract.
Acceptance criteria: existing harnesses and OpenClaw can continue using the native protocol during
and after the refactor; it remains versioned and documented.

13. Add ACP as a first-class host/frontend mode in `pikachatd`.
Acceptance criteria: `pikachatd` can operate as an ACP-aware conversation bridge, not by tunneling
ACP through the existing JSONL child contract, but via a dedicated frontend with its own lifecycle.

14. Define `pikachat` conversation-to-ACP mapping explicitly.
Acceptance criteria: the bridge spec states:
- how a DM or group maps to an ACP session
- how inbound messages map to `session/prompt`
- how assistant chunks map back to outbound `send_message`
- how cancellation/timeouts map
- how startup/session metadata maps
- how unsupported ACP surfaces are handled

15. Preserve non-text `pikachat` features while introducing ACP.
Acceptance criteria: the ACP bridge does not require deleting calls, hypernotes, media, or reactions
from `pikachatd`. Unsupported ACP capabilities are surfaced as explicit no-ops, system messages, or
future work, not by deleting runtime behavior.

16. Move the Pi integration to ACP as the only supported serious-agent path.
Acceptance criteria: the microVM agent path launches an ACP-capable agent process such as
`pi-acp`, and `pikachatd` communicates with it through ACP. There is no remaining Pika-owned logic
for local Pi prompt execution, adapter fallback, or direct model HTTP fallback.

17. Delete the embedded Python microVM bridge after ACP parity lands.
Acceptance criteria: `crates/pika-agent-microvm` no longer injects a Python prompt/reply bridge into
guest VMs. Guest startup uses `pikachatd` + ACP agent launch configuration instead.

18. Delete the older `bots/pi-bridge.py` path after ACP parity lands.
Acceptance criteria: the old containerized Pi bridge path is removed from docs, Dockerfiles,
entrypoints, and tests unless an explicit separate product use case is documented.

19. Remove backend ambiguity from Pi integration.
Acceptance criteria: Pi path uses one backend boundary only:
ACP to `pi-acp`, and then Pi RPC beneath it.
There is no `pi-adapter` fallback, no direct Anthropic fallback, and no subprocess scraping logic
owned by this repo for normal Pi chat behavior.

20. Keep OpenClaw as a native `pikachat` protocol consumer and simplify its responsibilities.
Acceptance criteria: OpenClaw plugin continues to own app/product policy such as routing, mention
gating, and tool exposure, but does not need to compensate for protocol drift or duplicated type
definitions.

21. Move feature-neutral metadata into runtime events so adapters guess less.
Acceptance criteria: runtime events expose enough structured information about group identity,
sender metadata, media attachments, call state, and message IDs that OpenClaw and ACP frontends do
not need ad hoc parsing for basic transport facts.

22. Add an ACP session policy for group conversations.
Acceptance criteria: the ACP bridge clearly defines whether:
each Nostr group is its own ACP session
DMs collapse onto one session per peer
owner/group naming is preserved across reloads
session reload/recovery behavior is deterministic

23. Add parity tests for the two supported integration styles.
Acceptance criteria: there are automated tests for:
native `pikachat` protocol consumer behavior
ACP bridge behavior with a fake ACP agent
OpenClaw integration on the native protocol
Pi integration through ACP

24. Add failure-path tests before deleting old bridges.
Acceptance criteria: tests cover:
agent process not found
agent process exits mid-stream
ACP cancel semantics
session reload after host restart
outbound send failures
welcome processing while agent is unavailable
call events while no agent session is bound

25. Add migration docs and operator runbooks for the new topology.
Acceptance criteria: docs explain:
what `pikachatd` is
when to use native protocol vs ACP
how OpenClaw integrates
how Pi integrates through `pi-acp`
what old Pi bridge paths were removed
how to debug host/runtime vs agent-side failures

26. Keep backward compatibility during the migration window, then remove legacy paths deliberately.
Acceptance criteria: the current daemon CLI entrypoints and OpenClaw plugin continue to work while
the refactor lands; removal of legacy bridge code happens only after documented parity checks pass.

27. Add a guardrail doc to keep `pikachatd` transport-first.
Acceptance criteria: one short scope-lock doc states:
- `pikachat-core` owns transport and state, not agent policy
- host frontends may translate protocols, but not implement model-specific chat behavior
- ACP is the preferred serious-agent boundary
- native JSONL is kept for simple bots and internal tooling
- adding a new agent harness must not require new transport logic in the core runtime

## Non-Goals

1. Do not remove calling, media, hypernotes, reactions, welcomes, or remote control support from
the `pikachat` runtime.
2. Do not make OpenClaw depend on ACP for its primary happy path.
3. Do not embed Pi-specific prompt logic into `pikachatd`.
4. Do not redesign Pi or `pi-acp` internals in this repo; treat them as external maintained
components.
5. Do not force every toy bot through ACP; keep the native protocol for simple integrations.
