# pikachat-wasm

Wasm-facing protocol runtime contract for the Cloudflare Workers agent path.

Current status: API/snapshot scaffold.

Implemented exports:

- init/load identity
- publish key-package payload
- process welcome
- process group message
- create outbound group message
- snapshot load/save with schema guard

Notes:

- This crate currently provides deterministic state transitions to lock API shape.
- MLS crypto wiring and real MDK-backed operations are the next phase.
