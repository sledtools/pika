---
summary: How to set up and use the iOS TestFlight CI workflow
read_when:
  - setting up App Store Connect secrets for CI
  - debugging TestFlight upload failures
  - understanding the iOS release pipeline
---

# iOS TestFlight CI

Pushing a `pika/vX.Y.Z` release tag automatically builds and uploads Pika to
TestFlight.

Manual dispatch is still available for non-release or recovery builds from the
Actions tab.

## Setup

You need six GitHub Actions secrets. All six go in **Settings > Secrets and variables > Actions > Repository secrets**.

### 1. `APPLE_TEAM_ID`

Your 10-character Apple Developer Team ID.

**Where to find it:**

1. Go to <https://developer.apple.com/account>
2. Scroll down to **Membership details**
3. Copy the **Team ID** (e.g. `A1B2C3D4E5`)

### 2. `ASC_KEY_ID`, `ASC_ISSUER_ID`, and `ASC_KEY_P8`

These come from an App Store Connect API key.

**How to create the key:**

1. Go to <https://appstoreconnect.apple.com/access/integrations/api>
   - Or: App Store Connect > Users and Access > Integrations > App Store Connect API
2. Click the **+** button to generate a new key
3. Give it a name like `GitHub CI` and select the **Admin** role
4. Click **Generate**

**After generating:**

- **Key ID** — shown in the table next to your key name. Copy it into the `ASC_KEY_ID` secret.
- **Issuer ID** — shown at the top of the page (a UUID like `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`). Copy it into the `ASC_ISSUER_ID` secret.
- **Download the .p8 file** — you can only download this once. Open it in a text editor, copy the entire contents (including the `-----BEGIN PRIVATE KEY-----` and `-----END PRIVATE KEY-----` lines), and paste into the `ASC_KEY_P8` secret.

### 3. `IOS_DIST_CERT_P12` and `IOS_DIST_CERT_PASSWORD`

These are the iOS distribution certificate inputs used by the workflow's
`Install distribution certificate` step.

**What the workflow expects:**

- `IOS_DIST_CERT_P12` — the `.p12` certificate file, base64-encoded onto one line
- `IOS_DIST_CERT_PASSWORD` — the password used when exporting that `.p12`

**How to create them:**

1. Open **Keychain Access** on a Mac that has the iOS distribution certificate installed
2. Find the distribution certificate for the Pika app's Apple team
3. Export it as a `.p12` file and choose an export password
4. Base64-encode the exported file and paste the result into `IOS_DIST_CERT_P12`
5. Paste the export password into `IOS_DIST_CERT_PASSWORD`

Example base64 command:

```sh
base64 -i Certificates.p12 | tr -d '\n'
```

### Adding secrets in GitHub

1. Go to your repository on GitHub
2. **Settings** > **Secrets and variables** > **Actions**
3. Click **New repository secret**
4. Enter the name (e.g. `APPLE_TEAM_ID`) and paste the value
5. Repeat for all six secrets

## How it works

The workflow (`.github/workflows/ios-testflight.yml`) runs on:

- `push.tags: ["pika/v*"]` for release uploads
- `workflow_dispatch` for manual builds from the selected ref

1. Checks out code on a `macos-26` runner
2. On release-tag builds, verifies the pushed tag equals `pika/v$(./scripts/version-read --name)`
3. Installs Nix and enters the dev shell
4. Runs `just ios-xcframework ios-xcodeproj` to build the Rust core for device (`aarch64-apple-ios` only — no simulator slice needed) and generate the Xcode project
5. Stamps a UTC timestamp (`YYYYMMDDHHMMSS`) as the build number
6. Archives with `xcodebuild archive` using automatic signing
7. Exports the IPA using `app-store-connect` method
8. Saves the IPA as a GitHub Actions artifact
9. Uploads to TestFlight via `xcrun altool`

## Manual trigger

1. Go to **Actions** > **iOS TestFlight** in your GitHub repo
2. Click **Run workflow**
3. Select the ref you want to build from
4. Click the green **Run workflow** button

Use manual dispatch for:

- a one-off dev/TestFlight build that should not create a release tag
- rerunning a release upload after Apple-side cleanup
- validating a branch before you cut a real tagged release

## Troubleshooting

### "No signing certificate found"

The API key needs the **Admin** role. Keys with lower roles cannot manage provisioning. Also verify `APPLE_TEAM_ID` matches the team that owns the app's bundle ID (`org.pikachat.pika`).

### "Unable to authenticate"

- Double-check `ASC_KEY_ID` and `ASC_ISSUER_ID` are correct
- Make sure `ASC_KEY_P8` contains the full `.p8` file contents, including the `BEGIN`/`END` lines
- The key must not be revoked in App Store Connect

### "No applicable devices found"

The archive step needs `-destination "generic/platform=iOS"`. This is already set in the workflow. If you see this locally, make sure you're not passing a simulator destination.

### Build number conflicts

CI writes a fresh UTC timestamp (`YYYYMMDDHHMMSS`) to `ios/.build-number` for
every workflow run. That avoids duplicate build numbers on reruns and keeps
release-tag builds aligned with the exact tagged code.

Outside CI, `ios/project.yml` falls back to the same UTC timestamp format when
`ios/.build-number` is missing. That makes local archives much less likely to
collide than the old minute-level fallback.

If you trigger multiple archives in the same second for the same marketing
version, TestFlight can still reject a duplicate. In that case, rerun the
workflow or set a higher `ios/.build-number` before archiving manually.

### "ERROR ITMS-90189: Redundant Binary Upload"

You uploaded an IPA with a build number that already exists for this version.

- For CI uploads: rerun the workflow, which will stamp a fresh UTC timestamp
- For local/manual uploads: set a higher `ios/.build-number` before archiving
- If the marketing version itself needs to change, bump the repo-root `VERSION` file
