# Pika Release Pipeline — Agreed Design

## 1. Version Source of Truth

**Plain `VERSION` file** at repo root (e.g. `0.1.0`).

- Gradle reads it for both debug and release builds via `file("../VERSION").readText().trim()`.
- `versionCode` computed as `major*10000 + minor*100 + patch`.
- `just release VERSION=x.y.z` validates the `VERSION` file matches the argument, then creates the tag.
- CI cross-checks: `GITHUB_REF_NAME` must equal `v$(cat VERSION)` — mismatched tags fail the build.
- A helper script `scripts/version-read` with `--name` / `--code` flags serves non-Gradle consumers.
- iOS will read the same file when added later.

## 2. Keystore Management

**age-encrypted keystore committed to repo** with env-var-based passwords (not hardcoded).

- Commit `android/pika-release.jks.age` to repo. Gitignore `*.jks` (plaintext).
- `age` added to nix dev shell (already in nixpkgs).
- Decrypt at build time: `age -d` using `AGE_SECRET_KEY` from environment.
- Keystore password stored in `.env` as `PIKA_KEYSTORE_PASSWORD` (not hardcoded in Gradle).
- Gradle `signingConfigs.release` reads password from `System.getenv("PIKA_KEYSTORE_PASSWORD")`.
- CI gets `AGE_SECRET_KEY` and `PIKA_KEYSTORE_PASSWORD` as GitHub Actions secrets.

**One-time setup (developer runs):**
```bash
keytool -genkeypair -v \
  -keystore android/pika-release.jks \
  -alias pika -keyalg RSA -keysize 2048 -validity 10000 \
  -dname "CN=Pika, O=Pika, L=Unknown, ST=Unknown, C=US"
age-keygen -o /tmp/pika-age-key.txt
age -e -r <age-public-key> -o android/pika-release.jks.age android/pika-release.jks
rm android/pika-release.jks
```

## 3. Release APK ABI Targets

**arm64-v8a only.** Zapstore only supports this platform. Keep ABI target list as a variable in the justfile recipe so armv7 can be added later if needed.

## 4. Zapstore CLI Packaging

**Nix derivation fetching pinned pre-built binary**, added to the default dev shell.

- `fetchurl` per platform (macos-arm64, linux-amd64) with pinned sha256 hashes.
- Exposes `zapstore` on PATH inside `nix develop`.
- Version 0.2.4 (current latest).

## 5. CI Workflow Design

**New `release.yml`** triggered by `push.tags: ["v*"]` + `workflow_dispatch`.

Structure:
```
jobs:
  check:         # Run pre-merge-pika on the tagged commit
  build:         # Decrypt keystore, just android-release, upload APK
    needs: check
  publish:       # Create GitHub Release, zapstore publish
    needs: build
```

- Actor allowlist in job-level `if`: only `justinmoon` and `futurepaul`.
- Zapstore publish is a separate job so APK artifacts exist first and failures are independently retryable.

## 6. Release Tag Flow

**Two paths:**

| Recipe | Purpose |
|--------|---------|
| `just release VERSION=x.y.z` | Primary: validate VERSION file matches, check clean tree, create `v$VERSION` tag, push. CI takes over. |
| `just release-local` | Fallback: `android-release` + `zapstore-publish` locally. For testing or when CI is down. |

`just android-release` works identically whether invoked by CI or a human.

## Secrets Summary

| Secret | Local | CI |
|--------|-------|----|
| `AGE_SECRET_KEY` | `.env` | GH secret |
| `PIKA_KEYSTORE_PASSWORD` | `.env` | GH secret |
| `ZAPSTORE_SIGN_WITH` | `.env` | GH secret |

## Files to Create / Modify

| File | Action |
|------|--------|
| `VERSION` | Create — `0.1.0` |
| `scripts/version-read` | Create — parse VERSION → name/code |
| `scripts/decrypt-keystore` | Create — `age -d` → `android/pika-release.jks` |
| `zapstore.yaml` | Create — Zapstore publish config |
| `.github/workflows/release.yml` | Create — tag-triggered release CI |
| `docs/release.md` | Create — documents the whole process |
| `android/app/build.gradle.kts` | Edit — read VERSION, add signingConfigs.release with env-var password |
| `justfile` | Edit — add `android-release`, `release`, `release-local`, `zapstore-publish` |
| `flake.nix` | Edit — add `age` + `zapstore-cli` derivation to default dev shell |
| `.gitignore` | Edit — ignore `*.jks`, keep `*.jks.age` |
