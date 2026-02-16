---
summary: TestFlight CI setup using xcodebuild + altool (no fastlane)
read_when:
  - enabling iOS TestFlight automation in GitHub Actions
  - rotating App Store Connect API credentials
  - debugging CI upload failures
---

# iOS TestFlight CI Setup

This repo uses `.github/workflows/ios-testflight.yml` to build an iOS archive and optionally upload to TestFlight using:

- `xcodebuild archive`
- `xcodebuild -exportArchive`
- `xcrun altool --upload-app`

No fastlane is used.

## GitHub secrets required

Set these **Repository Secrets** (or Environment Secrets if you gate deployments with environments):

- `PIKA_IOS_DEVELOPMENT_TEAM`
  - Your 10-character Apple Team ID (example format: `ABC1234567`)
- `ASC_KEY_ID`
  - App Store Connect API key ID (example format: `ABC1234567`)
- `ASC_ISSUER_ID`
  - App Store Connect issuer ID (UUID format)
- `ASC_KEY_P8`
  - Full contents of the downloaded `.p8` private key file
  - You can paste it as multiline text:
    - `-----BEGIN PRIVATE KEY-----`
    - `...`
    - `-----END PRIVATE KEY-----`

## Where to get each value

## `ASC_KEY_ID`, `ASC_ISSUER_ID`, `ASC_KEY_P8`

1. Open App Store Connect.
2. Go to `Users and Access` -> `Integrations` -> `App Store Connect API`.
3. Create a new API key with sufficient permissions (typically App Manager/Admin for uploads).
4. Save:
   - Key ID -> `ASC_KEY_ID`
   - Issuer ID -> `ASC_ISSUER_ID`
5. Download the `.p8` file immediately (Apple only lets you download it once).
6. Copy the full file contents into `ASC_KEY_P8`.

## `PIKA_IOS_DEVELOPMENT_TEAM`

Get your Team ID from either:

- Apple Developer account membership details, or
- App Store Connect membership/account details.

It is the 10-character Team ID used for code signing.

## Add secrets in GitHub

1. Open your repo on GitHub.
2. Go to `Settings` -> `Secrets and variables` -> `Actions`.
3. Add each secret exactly with the names above.
4. If using environments, add them under `Settings` -> `Environments` -> `<env>` -> `Secrets`.

## How the workflow behaves

- On `push` to `master`: builds + exports IPA artifact, **does not upload**.
- On manual run (`workflow_dispatch`):
  - `upload=false`: build/export only
  - `upload=true`: upload to TestFlight

## Manual run checklist

1. Open `Actions` -> `ios testflight` -> `Run workflow`.
2. Choose branch/ref.
3. Set `upload=true` when you actually want TestFlight delivery.

## Common errors

- `Missing secret ...`
  - One of the required secrets is not configured in GitHub.
- Signing/provisioning failure
  - `PIKA_IOS_DEVELOPMENT_TEAM` is wrong or key lacks permissions.
- `altool` auth failure
  - `ASC_KEY_ID`, `ASC_ISSUER_ID`, or `ASC_KEY_P8` do not match the same key.
- No IPA exported
  - Archive/export step failed earlier; inspect `xcodebuild` logs in that job.
