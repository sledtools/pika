---
summary: Share one Cargo target dir across all worktrees to skip redundant rebuilds (76s -> 5s)
read_when:
  - setting up a new development machine
  - creating worktrees and noticing slow first builds
  - looking to reclaim disk space from multiple target/ directories
---

# Shared Cargo Target Directory Across Worktrees

## Why

Each git worktree gets its own `target/` directory. Every first build in a new worktree recompiles all dependencies from scratch (~76s). With many worktrees this also eats massive disk space.

A shared target directory lets Cargo reuse compiled artifacts across all worktrees. Measured improvement: **76s -> 5s** for a fresh worktree build.

## Setup (per-machine, takes 30 seconds)

`.cargo/config.toml` is already gitignored, so this is local to your machine.

1. **Create `.cargo/config.toml`** in the repo root with an absolute path to a shared target dir:

   ```toml
   [build]
   target-dir = "/absolute/path/to/pika/.shared-target"
   ```

   Replace the path with wherever your pika repo lives. The path **must be absolute** because Cargo resolves relative paths from the working directory, not the config file.

2. **Done.** All worktrees under `pika/worktrees/` and `pika/.worktrees/` are subdirectories of the repo root, so Cargo's parent-directory walk finds the same config automatically.

3. **Symlink `target` to `.shared-target`** so recipes that hardcode `target/` still work:

   ```bash
   rm -rf target && ln -s .shared-target target
   ```

4. **(Optional)** Delete old per-worktree `target/` dirs to reclaim disk space:

   ```bash
   rm -rf worktrees/*/target .worktrees/*/target
   ```

## How it works

- Cargo walks parent directories to find `.cargo/config.toml`
- All worktrees share one `target/` directory via the config
- Cargo's incremental compilation handles the rest: deps compiled by any worktree are reused

## Caveats

- **No concurrent builds**: Cargo holds a lock on the target dir. If two worktrees build at the same time, one blocks until the other finishes.
- **Absolute path**: Machine-specific, but since `.cargo/config.toml` is gitignored that's fine.
- **Branch switching thrash**: If two worktrees have very different deps, Cargo may recompile more. In practice most worktrees share 99% of deps.
- **Stale `target/` directory**: If you previously built without the shared target config, a `target/` directory with old artifacts may still exist in the repo root. The justfile's `ios-gen-swift` and other recipes reference `target/$PROFILE/`, so a stale `target/` will shadow the correct `.shared-target/` output, causing uniffi to generate bindings from an outdated dylib. **After enabling the shared target, delete or replace the old `target/` directory:**

  ```bash
  rm -rf target && ln -s .shared-target target
  ```

  The symlink ensures recipes that hardcode `target/` still find the correct artifacts.

## Optional: sccache

As a complementary optimization, install `sccache` to cache individual `rustc` invocations by input hash:

```bash
brew install sccache
```

In `~/.cargo/config.toml` (your home dir, not the repo):

```toml
[build]
rustc-wrapper = "/opt/homebrew/bin/sccache"
```

This helps when the shared target dir has a cache miss. On its own it only cuts ~76s to ~45s, so it's a nice-to-have on top of the shared target, not a replacement.
