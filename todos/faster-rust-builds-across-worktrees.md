# Shared Cargo Target Directory Across Worktrees

## Problem

Each git worktree gets its own `target/` directory. Every first build in a new worktree recompiles all dependencies from scratch (~76s). With ~20 worktrees this also eats massive disk space (was 74 GB).

## Solution: Shared `target-dir`

Point all worktrees at a single shared Cargo target directory via `.cargo/config.toml` in the repo root.

### Setup

1. **Add `.shared-target/` to `.gitignore`**

2. **Create `.cargo/config.toml`** (already gitignored):
   ```toml
   [build]
   target-dir = "/Users/justin/code/pika/.shared-target"
   ```

3. **Delete old per-worktree `target/` dirs** to reclaim space:
   ```bash
   rm -rf worktrees/*/target .worktrees/*/target target
   ```

### Why it works

- Cargo walks parent directories to find `.cargo/config.toml`
- All worktrees under `pika/worktrees/` and `pika/.worktrees/` are subdirectories of the repo root, so they all find the same config
- Cargo's normal incremental compilation handles the rest: deps already compiled by any worktree are reused, only changed crates recompile
- Measured: fresh worktree build goes from **76s → 5s**

### Caveats

- **No concurrent builds**: Cargo holds a lock on `target/`. If two worktrees build simultaneously, one blocks until the other finishes. This matters if multiple agents are building in parallel.
- **Absolute path**: The `target-dir` must be absolute because Cargo resolves relative paths from the working directory, not the config file location. This means the config is machine-specific (but it's already gitignored, so that's fine).
- **Branch switching thrash**: If two worktrees have very different dependency trees and you alternate builds between them, Cargo may recompile more than expected. In practice most worktrees share 99% of deps so this is minor.

### Optional: sccache

As a complementary optimization, install `sccache` and set it as the global rustc wrapper:

```bash
brew install sccache
```

In `~/.cargo/config.toml`:
```toml
[build]
rustc-wrapper = "/opt/homebrew/bin/sccache"
```

This caches individual `rustc` invocations by input hash. It helps when the shared target dir has a cache miss (e.g., source actually differs between branches). On its own it only cuts build time from 76s → 45s, so it's a nice-to-have on top of the shared target dir, not a replacement.
