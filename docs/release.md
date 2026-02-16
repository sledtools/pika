---
summary: Android release process for signed APK publishing to GitHub Releases
read_when:
  - preparing an Android release tag
  - rotating Android signing keys or CI release secrets
  - changing release automation in justfile, Gradle, or GitHub Actions
---

# Release (Android APK)

This repo uses a tag-driven Android release pipeline.

## Version source of truth

- Version lives in `VERSION` (format `x.y.z`).
- Android reads it in `android/app/build.gradle.kts`.
- `versionCode` is `major*10000 + minor*100 + patch`.
- Helper script:
  - `./scripts/version-read --name`
  - `./scripts/version-read --code`

CI enforces that the pushed tag equals `v$(cat VERSION)`.

## Signing inputs

- Commit only encrypted keystore: `android/pika-release.jks.age`.
- Commit only encrypted signing env: `secrets/android-signing.env.age`.
- Keep plaintext `android/pika-release.jks` out of git.
- Encrypt both artifacts to all required recipients:
  - YubiKey primary: `age1yubikey1q0zhu9e7zrj48zmnpx4fg07c0drt9f57e26uymgxa4h3fczwutzjjp5a6y5`
  - YubiKey backup: `age1yubikey1qtdv7spad78v4yhrtrts6tvv5wc80vw6mah6g64m9cr9l3ryxsf2jdx8gs9`
  - CI age public key (dedicated release key)
- CI env var required:
  - `AGE_SECRET_KEY` (decrypts both encrypted artifacts in CI)
- Optional for local hardware-key decrypt:
  - `PIKA_AGE_IDENTITY_FILE` (defaults to `~/configs/yubikeys/keys.txt`)

`just android-release` decrypts the keystore, builds, and writes:

- `dist/pika-<version>-arm64-v8a.apk`

## Release recipes

- `just android-release`
  - Builds signed release APK (`arm64-v8a`) into `dist/`
- `just release VERSION=x.y.z`
  - Verifies clean git tree
  - Verifies `VERSION` matches argument
  - Creates and pushes tag `vx.y.z`

## CI workflow

`/.github/workflows/release.yml` runs on `push.tags: ["v*"]` and `workflow_dispatch`.

Jobs:

1. `check` - validates tag/version match and runs `just pre-merge-pika`
2. `build` - runs `just android-release`, uploads APK + `SHA256SUMS`
3. `publish` - creates GitHub Release with uploaded assets
