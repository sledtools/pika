---
summary: Release process for Pika app artifacts (Android + macOS), iOS TestFlight, and pikachat
read_when:
  - preparing an Android, macOS, iOS, or pikachat release
  - rotating Android signing keys or CI release secrets
  - changing release automation in justfile, Gradle, or GitHub Actions
---

# Release

This repo has two independent release tag families:

| Target | Tag pattern | CI workflow | Artifacts |
|--------|------------|-------------|-----------|
| Pika app release | `pika/v*` (e.g. `pika/v0.2.2`) | `release.yml` | Signed Android APK + macOS universal DMG + SHA256SUMS on GitHub Releases |
| iOS TestFlight | `master` push or manual dispatch | `ios-testflight.yml` | Signed IPA uploaded to TestFlight |
| Zapstore publish | `pika/v*` (e.g. `pika/v0.2.2`) | `release.yml` (`publish-zapstore` job) | NIP-82 app/release/asset events on Zapstore relays |
| Nostr announcement | `pika/v*` (e.g. `pika/v0.2.2`) | `release.yml` (`announce-release` job) | Kind-1 release announcement note |
| pikachat (OpenClaw extension) | `pikachat-v*` (e.g. `pikachat-v0.5.2`) | `pikachat-release.yml` | Linux + macOS binaries on GitHub Releases, npm package |

There is not currently a single cross-repo version file for both release
families. `VERSION` is the source of truth for Pika app artifacts (Android,
macOS, and the iOS marketing version). pikachat uses concrete versions in
`cli/Cargo.toml` and the OpenClaw extension `package.json`, but those can be
synced from `VERSION` with `./scripts/sync-pikachat-version`.

**Important:** All release tags must be created from the `master` branch. Tags on
feature branches break GitHub's auto-generated release notes (it diffs between
tags, and tags on divergent branches include unrelated commits). The `just release`
recipe enforces this with a branch check.

---

## Android APK

### Version source of truth

- Version lives in `VERSION` (format `x.y.z`).
- Android reads it in `android/app/build.gradle.kts`.
- macOS release packaging reads it in `scripts/build-macos-release`.
- iOS TestFlight stamps it into `CFBundleShortVersionString` during the Xcode
  build via `ios/project.yml`.
- `versionCode` is `major*10000 + minor*100 + patch`.
- Helper script:
  - `./scripts/version-read --name`
  - `./scripts/version-read --code`

CI enforces that the pushed tag equals `pika/v$(cat VERSION)`.

For a coordinated cross-channel release, use `just release-commit <version>` to
create one version-only commit that syncs `VERSION`, iOS `MARKETING_VERSION`,
`cli/Cargo.toml`, and the OpenClaw extension `package.json`.

### Runbook

```bash
# 1. Make sure you're on master with a clean tree
git checkout master
git pull origin master

# 2. Create the coordinated release version commit
just release-commit 0.3.0
git push origin master

# 3. Tag and push (this triggers the CI release)
just release 0.3.0

# 4. Monitor the release workflow
gh run list --limit 1
gh run watch <run-id>

# 5. Verify the release
gh release view 'pika/v0.3.0'
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
- Encrypt all encrypted artifacts to all required recipients (source of truth: `scripts/release-age-recipients`):
  - YubiKey primary: `age1yubikey1q0zhu9e7zrj48zmnpx4fg07c0drt9f57e26uymgxa4h3fczwutzjjp5a6y5`
  - YubiKey backup: `age1yubikey1qtdv7spad78v4yhrtrts6tvv5wc80vw6mah6g64m9cr9l3ryxsf2jdx8gs9`
  - CI release key: `age1kywla84vx7ppelcaugeqvzghcggc3dsmskz7fugk6emdcc02zdqs0vzv93`
- Do not use the configs host `server` recipient for Pika release artifacts.
- CI env var required:
  - `AGE_SECRET_KEY` (decrypts all encrypted artifacts in CI)
- Zapstore encrypted env format:
  - `ZAPSTORE_SIGN_WITH=nsec1...` (or NIP-46 bunker URL)
- Helper command:
  - `just zapstore-encrypt-signing`
  - or include in full bootstrap: `PIKA_ZAPSTORE_SIGN_WITH='nsec1...' ./scripts/init-release-secrets`
- Publish helper:
  - `./scripts/zapstore-publish <apk-path> [repo-url]`
  - used by both `just zapstore-publish` and CI to centralize secret handling
- Release announcement helper:
  - `./scripts/post-release-announcement <tag> [repo-url] [zapstore-app-url]`
  - publishes concise kind-1 release announcements to popular relays
- Optional for local hardware-key decrypt:
  - `PIKA_AGE_IDENTITY_FILE` (defaults to `~/configs/yubikeys/keys.txt`)

### Rotate `AGE_SECRET_KEY` safely

1. Generate a fresh CI age keypair and capture:
   - private key (`AGE-SECRET-KEY-...`) for GitHub secret `AGE_SECRET_KEY`
   - public recipient (`age1...`) for `scripts/release-age-recipients`
2. Re-encrypt all release artifacts to the three recipients in `scripts/release-age-recipients`:
   - `android/pika-release.jks.age`
   - `secrets/android-signing.env.age`
   - `secrets/zapstore-signing.env.age`
3. Update GitHub Actions repo secret:
   - `gh secret set AGE_SECRET_KEY --repo sledtools/pika --body '<AGE-SECRET-KEY-...>'`
4. Verify before tagging:
   - local hardware key can decrypt release artifacts
   - `AGE_SECRET_KEY` can decrypt `secrets/android-signing.env.age` and `secrets/zapstore-signing.env.age`

### CI workflow

`/.github/workflows/release.yml` runs on `push.tags: ["pika/v*"]` and `workflow_dispatch`.

Jobs:

1. `approve-release` - requires approval via the protected `release` environment
2. `check` - validates tag/version match and runs `just pre-merge-pika`
3. `build-android` - runs `just android-release`, uploads APK artifact
4. `build-macos` - builds universal (`arm64` + `x86_64`) `Pika.app`, packages DMG via `scripts/build-macos-release`, uploads DMG artifact
5. `publish` - combines artifacts, generates a single `SHA256SUMS`, creates GitHub Release
6. `publish-zapstore` - publishes the built APK artifact to Zapstore relays
7. `announce-release` - publishes a kind-1 release announcement (Zapstore + GitHub links)

The workflow enforces both:
- explicit allowed actors in `approve-release` (`justinmoon`, `futurepaul`, `benthecarman`, `AnthonyRonning`)
- protected `release` environment reviewer approval

Keep both controls enabled.

`publish-zapstore` is gated on `secrets/zapstore-signing.env.age` existing in
git. It decrypts `ZAPSTORE_SIGN_WITH` via `AGE_SECRET_KEY`, uses centralized
`scripts/zapstore-publish` handling (xtrace disabled, masking enabled, temp-file
cleanup), and passes it to `zsp` only for the publish command.

---

## iOS TestFlight

iOS does not use a release tag. The workflow in `/.github/workflows/ios-testflight.yml`
runs on every push to `master` and on manual dispatch.

### Version source of truth

- Marketing version: `VERSION` at the repo root
- Build number in CI: `github.run_number`, written to `ios/.build-number`
- Build number outside CI: timestamp fallback from `ios/project.yml`
  (`date +"%Y%m%d%H%M"`) when `ios/.build-number` is absent

For CI builds on `master`, build-number conflicts should not happen because
`github.run_number` increases monotonically for each workflow run. The timestamp
fallback only matters for local/manual archives and can collide if you archive
twice within the same minute for the same marketing version.

See `docs/ios-testflight-ci.md` for App Store Connect setup and troubleshooting.

---

## pikachat (OpenClaw extension)

pikachat is the sidecar binary used by the OpenClaw bot. It is released as
native binaries (Linux x86_64/aarch64, macOS x86_64/aarch64) on GitHub Releases
and as an npm package.

### Version source of truth

- Rust version: `cli/Cargo.toml`
- npm version: `pikachat-openclaw/openclaw/extensions/pikachat-openclaw/package.json`

Both must match.

Helpers:

- `just release-commit <version>` makes a coordinated version-only commit for
  app + pikachat/OpenClaw releases
- `just release-bump <version>` syncs the version files without committing
- `./scripts/sync-pikachat-version` syncs both files from repo-root `VERSION`
  without committing or tagging
- `./scripts/bump-pikachat.sh` is the pikachat-only release helper: it requires
  a clean `master` checkout and refuses version drift unless
  `PIKACHAT_ALLOW_VERSION_DRIFT=1` is set
- `./scripts/bump-pikachat.sh` with no argument uses repo-root `VERSION`

### Runbook

```bash
# 1. Make sure you're on master with a clean tree
git checkout master
git pull origin master

# 2. Preferred for coordinated releases after the version commit is on master:
gh workflow run pikachat-release.yml -R sledtools/pika -f version=0.5.2

# 3. Monitor the release workflow
gh run list --limit 1
gh run watch <run-id>

# 4. Verify
gh release view pikachat-v0.5.2
npm view pikachat-openclaw version
```

Alternate path for a pikachat-only release from a clean `master` checkout:

```bash
# Reuse repo-root VERSION
./scripts/bump-pikachat.sh

# Or intentionally diverge from VERSION
PIKACHAT_ALLOW_VERSION_DRIFT=1 ./scripts/bump-pikachat.sh 0.5.2

git push origin master pikachat-v0.5.2
```

### CI workflow

`/.github/workflows/pikachat-release.yml` runs on both:

- `push.tags: ["pikachat-v*"]`
- `workflow_dispatch`

Recommended usage:

- coordinated app + pikachat releases: use `workflow_dispatch` after the
  version commit is merged to `master`
- pikachat-only releases: use `./scripts/bump-pikachat.sh` and push the
  `pikachat-vX.Y.Z` tag

Jobs:

1. `build-linux` - builds x86_64 and aarch64 Linux binaries
2. `build-macos` - builds x86_64 and aarch64 macOS binaries
3. `publish-release` - creates GitHub Release with all binaries
4. `publish-npm` - publishes the OpenClaw extension to npm (requires `NPM_TOKEN` secret)
