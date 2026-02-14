---
summary: Plan for integrating zig-mdx into Pika's Rust backend to parse markdown chat messages
read_when:
  - evaluating markdown parsing architecture for chat rendering
  - implementing native zig-mdx support in rust/
  - planning iOS/Android rendering changes for parsed markdown messages
---

# zig-mdx Integration Plan

## Goal

Use `zig-mdx` as a dependency of Pika's Rust core so chat messages can be parsed as markdown/MDX and rendered as rich text in iOS and Android, while preserving plain-text fallback behavior.

## Current State

- Pika Rust core stores and emits message `content` as plain strings in `AppState`.
- iOS and Android currently render `content` directly as plain text bubbles.
- `zig-mdx` currently exposes:
  - Zig library API (`src/lib.zig`)
  - WebAssembly exports (`src/wasm_exports.zig`)
- `zig-mdx` does not currently provide a native C ABI for Rust FFI integration.

## Recommended Architecture

Use native Zig -> C ABI -> Rust FFI.

- Do not parse via WASM inside Rust.
- Add a small stable C ABI in `zig-mdx`.
- Build/link `zig-mdx` as a static library from Pika's Rust build.
- Parse messages in Rust and expose derived markdown data over UniFFI.

Why this approach:
- Lower runtime overhead than embedding WASM in Rust.
- Better control of allocator and error boundaries.
- Works cleanly across host, iOS, and Android builds with predictable linking.

## Phased Implementation Plan

## Phase 1: Add Native C ABI in zig-mdx

1. Add `src/c_api.zig` with exported functions:
   - `zigmdx_parse_json(input_ptr, input_len, out_ptr, out_len, out_status)`
   - `zigmdx_free(ptr, len)`
2. Define explicit status codes (ok, parse error, invalid utf8, internal error).
3. Keep output format as JSON AST initially for faster integration.
4. Add C header emission (for example `zigmdx.h`) as part of build.
5. Add ABI-level smoke test so future refactors do not silently break FFI.

Deliverable:
- `zig-mdx` can be called from plain C/Rust without WASM.

## Phase 2: Package and Pin zig-mdx in Pika

1. Vendor or otherwise pin `zig-mdx` source into Pika for reproducibility.
2. Pin to a specific commit and document update process.
3. Avoid direct dependency on `~/dev/sec/zig-mdx` at build time.

Deliverable:
- CI and local builds use the same deterministic `zig-mdx` revision.

## Phase 3: Toolchain and Build Integration in Pika

1. Ensure Zig 0.15.x is available in Pika dev/CI environment (`flake.nix`).
2. Extend `rust/build.rs` to:
   - build `zig-mdx` static lib for current Cargo `TARGET`
   - emit `cargo:rustc-link-search` and `cargo:rustc-link-lib`
3. Add target mapping for:
   - host targets
   - iOS (`aarch64-apple-ios`, simulator targets)
   - Android (`aarch64-linux-android`, `armv7`, `x86_64`)
4. Preserve existing iOS linker workaround logic.

Deliverable:
- `cargo build -p pika_core` links to native `zig-mdx` automatically.

## Phase 4: Rust FFI Wrapper and Parse API

1. Add module(s):
   - `rust/src/markdown/mod.rs`
   - `rust/src/markdown/zig_mdx.rs`
2. Keep all `unsafe` inside this module.
3. Expose safe API:
   - `parse_markdown(content: &str) -> Result<ParsedMarkdown, MarkdownError>`
4. Add bounds and protections:
   - max input length
   - time/size guardrails where practical
   - strict UTF-8 handling
5. Add unit tests for:
   - valid markdown
   - malformed input
   - large message behavior
   - memory ownership/free correctness

Deliverable:
- Safe, tested Rust API around Zig parser.

## Phase 5: Integrate Parsing into AppState Derivation

1. Parse when deriving `ChatMessage` in storage refresh path.
2. Keep original `content` unchanged.
3. Add derived field for parsed result (or render-friendly spans model).
4. Add caching keyed by message id to avoid reparsing every full snapshot.
5. On parser failure:
   - store parse error metadata if useful
   - render as plain text fallback

Deliverable:
- Markdown parse data appears in Rust-owned state without breaking existing behavior.

## Phase 6: UniFFI Surface and Native Rendering

1. Update Rust state models (`rust/src/state.rs`) for markdown representation.
2. Regenerate bindings:
   - `just ios-gen-swift`
   - `just gen-kotlin`
3. iOS:
   - update `ios/Sources/Views/ChatView.swift`
   - render markdown nodes/spans, fallback to plain `content` on unsupported nodes
4. Android:
   - update `android/app/src/main/java/com/pika/app/ui/screens/ChatScreen.kt`
   - render markdown nodes/spans, fallback to plain `content`
5. Keep copy/share actions based on raw `content`.

Deliverable:
- Both platforms render parsed markdown with graceful fallback.

## Phase 7: Validation and Rollout

1. Rust validation:
   - `just fmt`
   - `just clippy --lib --tests`
   - `just test --lib --tests`
2. Build validation:
   - host build for `pika_core`
   - Android/iOS Rust compilation targets
3. UI verification:
   - manual smoke tests for markdown cases
   - confirm unchanged behavior for plain text messages
4. Optional staged rollout:
   - behind feature flag if risk needs containment

Deliverable:
- End-to-end confidence that markdown parsing is stable across targets.

## Open Design Decisions

1. Data model over FFI:
   - JSON AST direct
   - normalized span model (likely better for native rendering)
2. Parse timing:
   - on state refresh (simple, needs cache)
   - on message ingest + persisted derived data (more complex, cheaper on refresh)
3. MDX handling policy:
   - full support in data model, partial render support in UI
   - markdown-only subset for first release
4. Vendoring strategy for `zig-mdx`:
   - git subtree/submodule/vendor copy + bump script

## Risks and Mitigations

- Cross-compilation/linking complexity (iOS/Android):
  - Mitigation: treat build integration as separate early milestone and validate per target.
- FFI memory safety bugs:
  - Mitigation: single Rust wrapper boundary, explicit ownership rules, tests for alloc/free symmetry.
- Performance regressions from reparsing:
  - Mitigation: cache parsed output by message id and cap parse size.
- Rendering mismatch between platforms:
  - Mitigation: shared normalized render model and fallback-to-plain-text guarantee.

## Acceptance Criteria

1. `pika_core` builds with native `zig-mdx` across host + mobile targets.
2. Sending and receiving plain text still behaves exactly as before.
3. Markdown in messages renders with expected formatting on both platforms.
4. Unsupported syntax and parse errors gracefully fall back to plain text.
5. Pre-merge checks pass and no known memory leaks are introduced.
