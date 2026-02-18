---
summary: How to configure iOS code signing for local development using .env
read_when:
  - setting up iOS development for the first time
  - building on a new Apple Developer team or device
  - debugging code signing or provisioning errors
---

# iOS Signing & Developer Environment

## Quick Start

1. Copy `.env.example` values (or create `.env` if it doesn't exist):

```bash
PIKA_IOS_DEVELOPMENT_TEAM=<your Apple Developer Team ID>
PIKA_IOS_BUNDLE_ID=com.justinmoon.<yourname>
PIKA_IOS_DEVICE=1
PIKA_IOS_CONSOLE=1
```

2. Build and run on device:

```bash
just run-ios --device --udid <your-device-udid>
```

3. List connected devices:

```bash
just run-ios --list-devices
```

## How It Works

Each developer uses their own Apple Developer team and bundle ID. Everything
derives from two `.env` variables:

| Variable | Example | Description |
|----------|---------|-------------|
| `PIKA_IOS_DEVELOPMENT_TEAM` | `W6VF934SEW` | Your Apple Developer Team ID |
| `PIKA_IOS_BUNDLE_ID` | `com.justinmoon.pikatest` | Bundle ID registered on your team |

From these, the build system automatically derives:

- **App group**: `group.<PIKA_IOS_BUNDLE_ID>` (e.g. `group.com.justinmoon.pikatest`)
- **NSE bundle ID**: `<PIKA_IOS_BUNDLE_ID>.notification-service`
- **Keychain access group**: `<TeamPrefix>.<PIKA_IOS_BUNDLE_ID>.shared`

The entitlements files use Xcode build setting variables (`$(PRODUCT_BUNDLE_IDENTIFIER)`,
`$(AppIdentifierPrefix)`, `$(PIKA_APP_BUNDLE_ID)`) so they resolve correctly for any
team/bundle combination.

## Finding Your Team ID

1. Open Xcode → Settings → Accounts → click your Apple ID
2. Click on your team name
3. The Team ID is shown in the team details

Or visit https://developer.apple.com/account → Membership Details.

## First-Time Setup

When using a new bundle ID for the first time, Xcode's automatic signing
(`-allowProvisioningUpdates`) will register:

- The App ID (e.g. `com.justinmoon.pikatest`)
- The NSE App ID (e.g. `com.justinmoon.pikatest.notification-service`)
- The App Group (e.g. `group.com.justinmoon.pikatest`)
- Your device UDID
- Development provisioning profiles

This happens automatically during `just run-ios --device`. If the app group
registration fails, open the Xcode project once (`open ios/Pika.xcodeproj`),
select each target under Signing & Capabilities, and let Xcode register the
app group through its GUI.

## Push Notifications

Push notifications only work with the production bundle ID (`com.justinmoon.pika`)
because the notification server needs to know the App ID. Dev bundle IDs will
build and run fine but won't receive pushes.

## Files Involved

| File | Role |
|------|------|
| `.env` | Per-developer config (gitignored) |
| `ios/project.yml` | Xcodegen project spec; defines `PIKA_APP_BUNDLE_ID` and `PIKA_APP_GROUP` build settings |
| `ios/Pika.entitlements` | Main app entitlements (uses build setting variables) |
| `ios/NotificationService/NotificationService.entitlements` | NSE entitlements |
| `ios/Info.plist` | Exposes `PikaAppGroup` and `PikaKeychainGroup` for Swift runtime |
| `ios/NotificationService/Info.plist` | Same for the NSE |
| `tools/run-ios` | Build script; reads `.env`, passes settings to xcodebuild |

## Switching Between Teams

Just change the two values in `.env` and rebuild. No code changes needed.
The keychain access group and app group are different per team/bundle, so
data (keys, MLS state) won't carry over between different bundle IDs.
