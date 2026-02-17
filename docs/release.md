---
summary: Release process for Android APK and marmotd (OpenClaw extension)
read_when:
  - preparing an Android or marmotd release
  - rotating Android signing keys or CI release secrets
  - changing release automation in justfile, Gradle, or GitHub Actions
---

# Release

This repo has three independent release pipelines, all tag-driven:

| Target | Tag pattern | CI workflow | Artifacts |
|--------|------------|-------------|-----------|
| Android APK | `v*` (e.g. `v0.2.2`) | `release.yml` | Signed APK + SHA256SUMS on GitHub Releases |
| Zapstore publish | `v*` (e.g. `v0.2.2`) | `release.yml` (`publish-zapstore` job) | NIP-82 app/release/asset events on Zapstore relays |
| marmotd (OpenClaw extension) | `marmotd-v*` (e.g. `marmotd-v0.3.2`) | `marmotd-release.yml` | Linux + macOS binaries on GitHub Releases, npm package |

**Important:** All release tags must be created from the `master` branch. Tags on
feature branches break GitHub's auto-generated release notes (it diffs between
tags, and tags on divergent branches include unrelated commits). The `just release`
recipe enforces this with a branch check.

---

## Android APK

### Version source of truth

- Version lives in `VERSION` (format `x.y.z`).
- Android reads it in `android/app/build.gradle.kts`.
- `versionCode` is `major*10000 + minor*100 + patch`.
- Helper script:
  - `./scripts/version-read --name`
  - `./scripts/version-read --code`

CI enforces that the pushed tag equals `v$(cat VERSION)`.

### Runbook

```bash
# 1. Make sure you're on master with a clean tree
git checkout master
git pull origin master

# 2. Bump the version
echo "0.3.0" > VERSION
git add VERSION
git commit -m "release: bump to v0.3.0"
git push origin master

# 3. Tag and push (this triggers the CI release)
just release 0.3.0

# 4. Monitor the release workflow
gh run list --limit 1
gh run watch <run-id>

# 5. Verify the release
gh release view v0.3.0
```

`just release` validates:
- You are on the `master` branch
- `VERSION` file matches the argument
- Git working tree is clean
- Tag does not already exist

### Signing inputs

- Commit only encrypted keystore: `android/pika-release.jks.age`.
- Commit only encrypted signing env: `secrets/android-signing.env.age`.
- Commit only encrypted Zapstore signing env: `secrets/zapstore-signing.env.age`.
- Keep plaintext `android/pika-release.jks` out of git.
- Keep plaintext Zapstore signing env (`secrets/zapstore-signing.env`) out of git.
- Encrypt all encrypted artifacts to all required recipients:
  - YubiKey primary: `age1yubikey1q0zhu9e7zrj48zmnpx4fg07c0drt9f57e26uymgxa4h3fczwutzjjp5a6y5`
  - YubiKey backup: `age1yubikey1qtdv7spad78v4yhrtrts6tvv5wc80vw6mah6g64m9cr9l3ryxsf2jdx8gs9`
  - CI age public key (dedicated release key)
- CI env var required:
  - `AGE_SECRET_KEY` (decrypts all encrypted artifacts in CI)
- Zapstore encrypted env format:
  - `ZAPSTORE_SIGN_WITH=nsec1...` (or NIP-46 bunker URL)
- Helper command:
  - `just zapstore-encrypt-signing`
  - or include in full bootstrap: `PIKA_ZAPSTORE_SIGN_WITH='nsec1...' ./scripts/init-release-secrets`
- Optional for local hardware-key decrypt:
  - `PIKA_AGE_IDENTITY_FILE` (defaults to `~/configs/yubikeys/keys.txt`)

### CI workflow

`/.github/workflows/release.yml` runs on `push.tags: ["v*"]` and `workflow_dispatch`.

Jobs:

1. `check` - validates tag/version match and runs `just pre-merge-pika`
2. `build` - runs `just android-release`, uploads APK + `SHA256SUMS`
3. `publish` - creates GitHub Release with uploaded assets
4. `publish-zapstore` - publishes the built APK artifact to Zapstore relays

`publish-zapstore` is gated on `secrets/zapstore-signing.env.age` existing in
git. It decrypts `ZAPSTORE_SIGN_WITH` via `AGE_SECRET_KEY`, masks it in logs,
and passes it to `zsp` only for the publish command.

---

## marmotd (OpenClaw extension)

marmotd is the Marmot sidecar binary used by the OpenClaw bot. It is released as
native binaries (Linux x86_64/aarch64, macOS x86_64/aarch64) on GitHub Releases
and as an npm package.

### Version source of truth

- Rust version: `crates/marmotd/Cargo.toml`
- npm version: `openclaw-marmot/openclaw/extensions/marmot/package.json`

Both must match. The `bump-marmotd.sh` script keeps them in sync.

### Runbook

```bash
# 1. Make sure you're on master with a clean tree
git checkout master
git pull origin master

# 2. Bump version, commit, and tag (all done by the script)
./scripts/bump-marmotd.sh 0.4.0

# 3. Push commit and tag (this triggers the CI release)
git push origin master marmotd-v0.4.0

# 4. Monitor the release workflow
gh run list --limit 1
gh run watch <run-id>

# 5. Verify
gh release view marmotd-v0.4.0
npm view @openclaw/marmot version
```

### CI workflow

`/.github/workflows/marmotd-release.yml` runs on `push.tags: ["marmotd-v*"]`.

Jobs:

1. `build-linux` - builds x86_64 and aarch64 Linux binaries
2. `build-macos` - builds x86_64 and aarch64 macOS binaries
3. `publish-release` - creates GitHub Release with all binaries
4. `publish-npm` - publishes the OpenClaw extension to npm (requires `NPM_TOKEN` secret)
