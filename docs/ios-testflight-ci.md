---
summary: How to set up and use the iOS TestFlight CI workflow
read_when:
  - setting up App Store Connect secrets for CI
  - debugging TestFlight upload failures
  - understanding the iOS release pipeline
---

# iOS TestFlight CI

Merging to `master` automatically builds and uploads Pika to TestFlight.
You can also trigger a build manually from the Actions tab.

## Setup

You need four GitHub Actions secrets. All four go in **Settings > Secrets and variables > Actions > Repository secrets**.

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

### Adding secrets in GitHub

1. Go to your repository on GitHub
2. **Settings** > **Secrets and variables** > **Actions**
3. Click **New repository secret**
4. Enter the name (e.g. `APPLE_TEAM_ID`) and paste the value
5. Repeat for all four secrets

## How it works

The workflow (`.github/workflows/ios-testflight.yml`) runs on every push to `master` and on manual dispatch:

1. Checks out code on a `macos-15` runner
2. Installs Nix and enters the dev shell
3. Runs `just ios-xcframework ios-xcodeproj` to build the Rust core for device (`aarch64-apple-ios` only — no simulator slice needed) and generate the Xcode project
4. Stamps `github.run_number` as the build number
5. Archives with `xcodebuild archive` using automatic signing
6. Exports the IPA using `app-store-connect` method
7. Saves the IPA as a GitHub Actions artifact
8. Uploads to TestFlight via `xcrun altool`

## Manual trigger

1. Go to **Actions** > **iOS TestFlight** in your GitHub repo
2. Click **Run workflow**
3. Select the branch (usually `master`)
4. Click the green **Run workflow** button

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

Each CI run uses `github.run_number` as the build number, which auto-increments. If you also upload manually, make sure your manual build number is higher than the latest CI build number, or TestFlight will reject it as a duplicate.

### "ERROR ITMS-90189: Redundant Binary Upload"

You uploaded an IPA with a build number that already exists for this version. Bump the version in `ios/project.yml` (`MARKETING_VERSION`) or wait for the next CI run (which will have a higher `run_number`).
