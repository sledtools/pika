---
summary: How to run and interpret the iOS typing performance audit commands and outputs.
read_when:
  - You need avg/best/worst keypress latency from the composer.
  - You are profiling UI-thread work during chat composer typing.
  - You are troubleshooting samply vs xctrace profiling on iOS simulator.
---

# iOS Typing Perf Testing

This document describes the current typing performance audit flow for the iOS app, focused on one question:

- Does typing into the chat composer block or stall the UI thread?

## Current scope

The current automated flow measures typing in a note-to-self chat and produces:

- Keypress latency summary (`avg`, `best`, `worst`) from UI tests.
- Symbolized stack sampling for work happening during keypress windows.
- Flamegraph output for per-keypress averaged stacks.

This is a first pass and does not yet preload a synthetic 10k-message chat fixture.

## One-liner summary

Run:

```bash
just ios-typing-perf
```

Output includes a single parseable summary line:

```text
typing-keypress-ms avg=<ms> best=<ms> worst=<ms> samples=<n> [bulk_total=<ms> bulk_per_char=<ms>]
```

## Symbolized keypress profile + flamegraph

Run:

```bash
just ios-typing-perf-keypress-profile-symbolized
```

This runs the typing test, attaches Time Profiler to the app process, and emits:

- `typing-keypress-ms ...` summary.
- `typing-keypress-top ...` ranked sampled frames during keypress intervals.
- `typing-keypress-folded path=...` folded stacks file.
- `typing-keypress-flamegraph path=...` SVG flamegraph.
- `time-profiler-trace path=...` trace bundle.

Add `--open` to auto-open the trace/flamegraph viewer path where applicable.

## Samply route

Run:

```bash
just ios-typing-perf-samply
```

Notes:

- `samply` is available in the dev shell via `flake.nix`.
- On macOS simulator attach, samply may panic with an upstream `InvalidAddress` issue.
- The script auto-codesigns a local `samply` copy for attach profiling.
- If samply attach remains unstable, the script falls back to `ios-typing-perf-keypress-profile-symbolized` by default so the command still returns useful output.

Disable fallback if you want strict samply-only behavior:

```bash
PIKA_UI_TYPING_PERF_SAMPLY_FALLBACK_XCTRACE=0 just ios-typing-perf-samply
```

## Key instrumentation points

- UI tests emit keypress signposts (`composer_keypress`) around measured keystrokes.
- App-side signposts can also be enabled to bracket composer text-change handling.
- Time Profiler analysis intersects sampled stacks with keypress intervals and reports per-keypress averages.

## Environment knobs

Common knobs:

- `PIKA_UI_TYPING_PERF_CHARS` (default `120`)
- `PIKA_UI_TYPING_PERF_WARMUP` (default `10`)
- `PIKA_UI_TYPING_PERF_START_DELAY_MS` (default varies by script)
- `PIKA_UI_TYPING_PERF_TRACE_DIR` (default `artifacts/perf`)

Samply-specific knobs:

- `PIKA_UI_TYPING_PERF_SAMPLY_ATTACH_ATTEMPTS`
- `PIKA_UI_TYPING_PERF_SAMPLY_ATTACH_DELAY_S`
- `PIKA_UI_TYPING_PERF_SAMPLY_ATTACH_TIMEOUT_S`
- `PIKA_UI_TYPING_PERF_SAMPLY_FALLBACK_XCTRACE`

## iOS runtime / destination troubleshooting

If builds fail with destination/runtime mismatch, run:

```bash
just doctor-ios
```

The updated iOS tooling scripts prefer an Xcode + simulator runtime pairing compatible with the active `iphonesimulator` SDK and print guidance when runtime components are missing.
